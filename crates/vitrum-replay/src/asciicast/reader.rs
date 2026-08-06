//! Reading asciicast v2.
//!
//! # Why the event lines are not parsed by `serde_json`
//!
//! `serde_json` deserialises a JSON string into a Rust `String`, and a Rust `String`
//! is valid UTF-8 by construction. The bytes this crate needs back are not. Worse,
//! `serde_json` rejects the lone surrogate escapes [`crate::asciicast::jsonstr`] uses
//! to spell those bytes, which is correct of it and fatal here.
//!
//! So the header, which is ordinary JSON with no byte-exactness requirement, goes
//! through `serde_json`, and the event lines go through the scanner below. An event
//! line has a fixed shape, `[number, "code", "data"]`, and the only hard part is the
//! string, which [`crate::asciicast::jsonstr::decode`] already does exactly.
//!
//! # What is kept and what is dropped
//!
//! `"o"` events are the session's output and become the byte stream. `"m"` events
//! become [`Marker`]s. `"r"` events become [`Resize`] records, which are surfaced
//! rather than applied: see [`Recording::resizes`].
//!
//! `"i"` events are the keystrokes the user typed, recorded only when asciinema was
//! asked to. They are deliberately not merged into the byte stream: the terminal
//! never received them as output, and feeding them to the emulator would print the
//! user's typing a second time on top of the echo that is already there.
//!
//! Any other event code is skipped rather than rejected, because asciinema has added
//! codes over time and a reader that refused an unfamiliar one would reject valid
//! files from newer recorders.

use crate::asciicast::header::Header;
use crate::asciicast::jsonstr::decode;
use crate::config::ReplayConfig;
use crate::error::{CastError, Result};
use crate::timeline::{ChunkStamp, Marker, Timeline};

/// A terminal resize the recording asked for.
///
/// See [`Recording::resizes`] for why this is reported and not applied.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Resize {
    /// Stream position the resize happened at.
    pub seq: u64,
    /// Microseconds from the start of the recording.
    pub micros: u64,
    /// New width in columns.
    pub cols: u16,
    /// New height in rows.
    pub rows: u16,
}

/// A loaded recording, ready to scrub.
///
/// The bytes come back as one contiguous buffer, so building a [`crate::Stream`] over
/// them uses the same one-chunk form the rest of this workspace uses:
///
/// ```
/// use vitrum_replay::{Replay, Stream, asciicast};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let text = "{\"version\":2,\"width\":20,\"height\":3}\n\
///             [0.000000, \"o\", \"hello\"]\n\
///             [1.500000, \"o\", \"\\r\\nworld\"]\n";
/// let recording = asciicast::read(text)?;
///
/// let bytes = recording.bytes();
/// let stream = Stream::new(0, std::slice::from_ref(&bytes));
/// let mut replay = Replay::build(stream, &recording.config()?)?;
/// replay.set_timeline(recording.timeline());
///
/// // The recording carries its own times, so this scrubs by wall clock.
/// assert!(replay.timeline().has_real_time());
/// replay.seek_micros(1_400_000)?;
/// assert_eq!(replay.screen().line(0).trim_end(), "hello");
/// replay.seek_micros(1_500_000)?;
/// assert_eq!(replay.screen().line(1).trim_end(), "world");
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    /// The header line, with any unmodelled keys preserved in [`Header::extra`].
    pub header: Header,
    bytes: Vec<u8>,
    stamps: Vec<ChunkStamp>,
    markers: Vec<Marker>,
    resizes: Vec<Resize>,
    inputs: usize,
    skipped: usize,
}

impl Recording {
    /// The concatenated output bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The per-event delivery times, one per `"o"` event.
    #[must_use]
    pub fn stamps(&self) -> &[ChunkStamp] {
        &self.stamps
    }

    /// The `"m"` marker events.
    #[must_use]
    pub fn markers(&self) -> &[Marker] {
        &self.markers
    }

    /// The `"r"` resize events, in order.
    ///
    /// These are reported, not applied. A replay is built at one geometry, and
    /// applying a resize mid-stream would need a seq-indexed geometry track for the
    /// keyframes to restore alongside the screen. A caller that cares can rebuild
    /// the replay at the new size at the seq named here.
    #[must_use]
    pub fn resizes(&self) -> &[Resize] {
        &self.resizes
    }

    /// How many `"i"` input events were present and skipped.
    ///
    /// Non-zero means the recording captured keystrokes. Reported so a caller can
    /// say so rather than silently showing a recording with less in it than the file
    /// contains.
    #[must_use]
    pub const fn input_events(&self) -> usize {
        self.inputs
    }

    /// How many events carried a code this reader does not implement.
    #[must_use]
    pub const fn skipped_events(&self) -> usize {
        self.skipped
    }

    /// A timeline over this recording's real times and markers.
    #[must_use]
    pub fn timeline(&self) -> Timeline {
        Timeline::recorded(self.stamps.clone()).with_markers(self.markers.clone())
    }

    /// A replay configuration at the recording's own geometry.
    ///
    /// # Errors
    ///
    /// [`crate::Error::Geometry`] when the header names a size
    /// [`vitrum_grid::CellGrid`] will not build.
    pub fn config(&self) -> Result<ReplayConfig> {
        ReplayConfig::new(self.header.width, self.header.height)
    }
}

/// Parse an asciicast v2 recording.
///
/// # Errors
///
/// [`CastError`], naming the 1-based line that was wrong and what was wrong with it.
pub fn read(text: &str) -> Result<Recording, CastError> {
    let mut lines = text.lines();
    let Some(head) = lines.next() else {
        return Err(CastError::Empty);
    };
    let header: Header = serde_json::from_str(head).map_err(|error| CastError::HeaderSyntax {
        message: error.to_string(),
    })?;
    if header.version != Header::VERSION {
        return Err(CastError::Version {
            found: header.version,
        });
    }
    if header.width == 0 || header.height == 0 {
        return Err(CastError::MissingGeometry);
    }

    let mut recording = Recording {
        header,
        bytes: Vec::new(),
        stamps: Vec::new(),
        markers: Vec::new(),
        resizes: Vec::new(),
        inputs: 0,
        skipped: 0,
    };
    let mut previous_micros = 0u64;

    for (index, line) in lines.enumerate() {
        // Line 1 was the header, so the first event line is line 2.
        let number = index + 2;
        if line.trim().is_empty() {
            continue;
        }
        let event = parse_event(line, number)?;
        if event.micros < previous_micros {
            return Err(CastError::EventTimeOrder {
                line: number,
                micros: event.micros,
                previous: previous_micros,
            });
        }
        previous_micros = event.micros;

        match event.code {
            b'o' => {
                recording.bytes.extend_from_slice(&event.data);
                recording.stamps.push(ChunkStamp {
                    end_seq: recording.bytes.len() as u64,
                    micros: event.micros,
                });
            }
            b'm' => recording.markers.push(Marker {
                seq: recording.bytes.len() as u64,
                label: String::from_utf8_lossy(&event.data).into_owned(),
                hint: None,
            }),
            b'r' => match parse_geometry(&event.data) {
                Some((cols, rows)) => recording.resizes.push(Resize {
                    seq: recording.bytes.len() as u64,
                    micros: event.micros,
                    cols,
                    rows,
                }),
                None => {
                    return Err(CastError::EventData {
                        line: number,
                        reason: "a resize event's data is not \"COLSxROWS\"",
                    });
                }
            },
            b'i' => recording.inputs += 1,
            _ => recording.skipped += 1,
        }
    }

    Ok(recording)
}

/// One parsed event line.
struct RawEvent {
    micros: u64,
    code: u8,
    data: Vec<u8>,
}

/// `[number, "code", "data"]`, scanned by hand. See the module header.
fn parse_event(line: &str, number: usize) -> Result<RawEvent, CastError> {
    let bytes = line.as_bytes();
    let mut at = 0usize;

    skip_space(bytes, &mut at);
    if bytes.get(at) != Some(&b'[') {
        return Err(CastError::EventShape { line: number });
    }
    at += 1;

    // A JSON number contains no comma, so the first comma ends the time field.
    let time_start = at;
    while at < bytes.len() && bytes[at] != b',' {
        at += 1;
    }
    if at >= bytes.len() {
        return Err(CastError::EventShape { line: number });
    }
    let micros =
        parse_micros(line[time_start..at].trim()).ok_or(CastError::EventTime { line: number })?;
    at += 1;

    let code_body = string_body(line, &mut at, number)?;
    let mut code_bytes = Vec::new();
    decode(code_body, number, &mut code_bytes)?;
    let [code] = code_bytes[..] else {
        return Err(CastError::EventCode { line: number });
    };

    skip_space(bytes, &mut at);
    if bytes.get(at) != Some(&b',') {
        return Err(CastError::EventShape { line: number });
    }
    at += 1;

    let data_body = string_body(line, &mut at, number)?;
    let mut data = Vec::new();
    decode(data_body, number, &mut data)?;

    skip_space(bytes, &mut at);
    if bytes.get(at) != Some(&b']') {
        return Err(CastError::EventShape { line: number });
    }
    at += 1;
    skip_space(bytes, &mut at);
    if at != bytes.len() {
        return Err(CastError::EventShape { line: number });
    }

    Ok(RawEvent { micros, code, data })
}

/// The text between the next pair of quotes, escapes left intact.
///
/// Advances `at` past the closing quote. Slicing on byte indices is safe here
/// because the two bytes this stops on, `"` and `\`, are ASCII and every byte of a
/// multi-byte character is `0x80` or above.
fn string_body<'a>(line: &'a str, at: &mut usize, number: usize) -> Result<&'a str, CastError> {
    let bytes = line.as_bytes();
    skip_space(bytes, at);
    if bytes.get(*at) != Some(&b'"') {
        return Err(CastError::EventShape { line: number });
    }
    *at += 1;
    let start = *at;
    while *at < bytes.len() {
        match bytes[*at] {
            b'\\' => *at += 2,
            b'"' => {
                let body = &line[start..*at];
                *at += 1;
                return Ok(body);
            }
            _ => *at += 1,
        }
    }
    Err(CastError::EventShape { line: number })
}

fn skip_space(bytes: &[u8], at: &mut usize) {
    while *at < bytes.len() && (bytes[*at] == b' ' || bytes[*at] == b'\t') {
        *at += 1;
    }
}

/// A non-negative JSON number of seconds, as microseconds.
///
/// Plain decimals are parsed with integer arithmetic so `0.123456` is exactly
/// 123456 microseconds and not whatever the nearest `f64` rounds to. Digits past the
/// sixth are truncated, which is the only choice that cannot make one event overtake
/// the next.
///
/// Exponent notation goes through `f64`, because a decimal parser for `1.5e-3` is a
/// float parser. asciinema does not write that form; some hand-written file might.
fn parse_micros(text: &str) -> Option<u64> {
    if text.is_empty() || text.starts_with('-') || text.starts_with('+') {
        return None;
    }
    if text.contains(['e', 'E']) {
        let value: f64 = text.parse().ok()?;
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        return Some((value * 1e6) as u64);
    }
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let seconds: u64 = whole.parse().ok()?;
    let mut micros = 0u64;
    let mut digits = 0;
    for byte in fraction.bytes().take(6) {
        micros = micros * 10 + u64::from(byte - b'0');
        digits += 1;
    }
    while digits < 6 {
        micros *= 10;
        digits += 1;
    }
    seconds.checked_mul(1_000_000)?.checked_add(micros)
}

/// `"COLSxROWS"`, the data of a `"r"` event.
fn parse_geometry(data: &[u8]) -> Option<(u16, u16)> {
    let text = core::str::from_utf8(data).ok()?;
    let (cols, rows) = text.split_once('x')?;
    Some((cols.parse().ok()?, rows.parse().ok()?))
}
