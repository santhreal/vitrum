//! Writing asciicast v2.
//!
//! The file is a JSON object on the first line, then one JSON array per line after
//! it. Nothing is buffered across lines, so writing a 10 MiB session touches each
//! byte once and holds one event's worth of text at a time.
//!
//! # How the bytes are cut into events
//!
//! One event per [`ChunkStamp`], covering the bytes from the previous stamp's end to
//! this one's. That is the truthful cut: a stamp is one PTY read, so an event is one
//! PTY read, and a player replays the session with the same delivery granularity the
//! session had.
//!
//! A stream with no stamps at all becomes one event at time zero. That is the only
//! honest thing to do with bytes whose delivery times were never recorded, and it is
//! why [`crate::Timeline::synthetic`] exists for a caller who would rather export a
//! plausible pace than a single frame.
//!
//! # Markers
//!
//! An OSC 7373 chapter marker is written as a `"m"` event immediately after the
//! output event that carried it, so a marker never appears before the bytes that
//! produced it and the times stay non-decreasing.

use std::io;

use crate::asciicast::header::Header;
use crate::asciicast::jsonstr::{Utf8Policy, encode};
use crate::stream::Stream;
use crate::timeline::Timeline;

/// Write `stream` as an asciicast v2 recording.
///
/// `header` supplies the geometry and any metadata. When it leaves `duration`
/// unset and the timeline has real times, the duration is filled in, because a
/// player uses it to draw a progress bar before it has parsed the whole file.
///
/// # Errors
///
/// Whatever `out` returns. The encoding itself cannot fail: every byte has a
/// representation under both [`Utf8Policy`] choices.
pub fn write<W: io::Write>(
    out: &mut W,
    stream: &Stream<'_>,
    timeline: &Timeline,
    header: &Header,
    policy: Utf8Policy,
) -> io::Result<()> {
    let mut header = header.clone();
    header.version = Header::VERSION;
    if header.duration.is_none() && timeline.has_real_time() {
        header.duration = Some(timeline.duration_micros() as f64 / 1e6);
    }
    let line = serde_json::to_string(&header).map_err(io::Error::other)?;
    out.write_all(line.as_bytes())?;
    out.write_all(b"\n")?;

    let head = stream.head_seq();
    let markers = timeline.markers();
    let mut next_marker = 0usize;
    let mut at = stream.base_seq();
    let mut text = String::new();
    let mut last_micros = 0u64;

    for stamp in timeline.stamps() {
        let end = stamp.end_seq.clamp(at, head);
        last_micros = stamp.micros;
        at = write_span(
            out,
            &mut text,
            stamp.micros,
            stream,
            at..end,
            markers,
            &mut next_marker,
            policy,
        )?;
        if at >= head {
            break;
        }
    }

    if at < head {
        write_span(
            out,
            &mut text,
            last_micros,
            stream,
            at..head,
            markers,
            &mut next_marker,
            policy,
        )?;
    }
    for marker in &markers[next_marker.min(markers.len())..] {
        write_marker(out, &mut text, last_micros, &marker.label)?;
    }
    Ok(())
}

/// Write `seqs` as output events, cut wherever a marker falls, and write each marker
/// after the bytes that produced it.
///
/// The cut is what makes a marker's position survive a round trip. A reader places an
/// imported marker at however many output bytes preceded it, so a writer that emitted
/// one big event and then all the markers would bring every chapter back at the end of
/// the recording. Splitting costs nothing: the same bytes go out, in the same order,
/// under the same timestamp.
#[expect(clippy::too_many_arguments, reason = "one private seam, all of it required")]
fn write_span<W: io::Write>(
    out: &mut W,
    text: &mut String,
    micros: u64,
    stream: &Stream<'_>,
    seqs: core::ops::Range<u64>,
    markers: &[crate::timeline::Marker],
    next_marker: &mut usize,
    policy: Utf8Policy,
) -> io::Result<u64> {
    let mut at = seqs.start;
    while let Some(marker) = markers.get(*next_marker) {
        if marker.seq > seqs.end {
            break;
        }
        if marker.seq > at {
            write_output(out, text, micros, stream, at..marker.seq, policy)?;
            at = marker.seq;
        }
        write_marker(out, text, micros, &marker.label)?;
        *next_marker += 1;
    }
    if seqs.end > at {
        write_output(out, text, micros, stream, at..seqs.end, policy)?;
        at = seqs.end;
    }
    Ok(at)
}

/// The same recording as a string.
///
/// Convenient for a caller that wants to hand the recording straight to a
/// clipboard, a paste service, or a test assertion.
///
/// # Errors
///
/// Whatever serialising the header returns. The event lines cannot fail: every byte
/// has a representation under both [`Utf8Policy`] choices, and the sink is a
/// `Vec<u8>`.
pub fn to_string(
    stream: &Stream<'_>,
    timeline: &Timeline,
    header: &Header,
    policy: Utf8Policy,
) -> io::Result<String> {
    let mut buffer = Vec::new();
    write(&mut buffer, stream, timeline, header, policy)?;
    String::from_utf8(buffer).map_err(io::Error::other)
}

fn write_output<W: io::Write>(
    out: &mut W,
    text: &mut String,
    micros: u64,
    stream: &Stream<'_>,
    seqs: core::ops::Range<u64>,
    policy: Utf8Policy,
) -> io::Result<()> {
    text.clear();
    text.push('[');
    push_seconds(micros, text);
    text.push_str(", \"o\", \"");
    for slice in stream.slices(seqs) {
        encode(slice, policy, text);
    }
    text.push_str("\"]\n");
    out.write_all(text.as_bytes())
}

fn write_marker<W: io::Write>(
    out: &mut W,
    text: &mut String,
    micros: u64,
    label: &str,
) -> io::Result<()> {
    text.clear();
    text.push('[');
    push_seconds(micros, text);
    text.push_str(", \"m\", \"");
    encode(label.as_bytes(), Utf8Policy::SurrogateEscape, text);
    text.push_str("\"]\n");
    out.write_all(text.as_bytes())
}

/// Microseconds as a plain decimal with six fraction digits.
///
/// Written by hand rather than through a float formatter for one reason: this is
/// exactly reversible. `f64` can hold every microsecond value a session will ever
/// reach, but its *shortest* decimal form is not guaranteed to be six places, and a
/// reader that parsed `1e-06` back would have to go through a float too. Six fixed
/// digits round-trip through integer arithmetic on both sides.
fn push_seconds(micros: u64, out: &mut String) {
    push_u64(micros / 1_000_000, out);
    out.push('.');
    let fraction = micros % 1_000_000;
    let mut divisor = 100_000u64;
    while divisor > 0 {
        out.push((b'0' + (fraction / divisor % 10) as u8) as char);
        divisor /= 10;
    }
}

/// A `u64` as decimal digits, appended in place.
fn push_u64(mut value: u64, out: &mut String) {
    if value == 0 {
        out.push('0');
        return;
    }
    let mut digits = [0u8; 20];
    let mut index = digits.len();
    while value > 0 {
        index -= 1;
        digits[index] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    for &digit in &digits[index..] {
        out.push(digit as char);
    }
}
