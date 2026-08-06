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

/// A zero-allocation reference to a parsed event line in an asciicast stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventRef<'a> {
    /// Output event ("o"). `micros` is timestamp, `raw_data` is string body.
    Output {
        /// 1-based line number.
        line: usize,
        /// Timestamp in microseconds.
        micros: u64,
        /// Raw JSON string body (borrowed without allocation).
        raw_data: &'a str,
    },
    /// Marker event ("m").
    Marker {
        /// 1-based line number.
        line: usize,
        /// Timestamp in microseconds.
        micros: u64,
        /// Raw label string body.
        raw_label: &'a str,
    },
    /// Terminal resize event ("r").
    Resize {
        /// Timestamp in microseconds.
        micros: u64,
        /// Columns.
        cols: u16,
        /// Rows.
        rows: u16,
    },
    /// User input event ("i").
    Input {
        /// Timestamp in microseconds.
        micros: u64,
    },
    /// Skipped or custom event code.
    Skipped {
        /// 1-based line number.
        line: usize,
        /// Code byte if single char.
        code: Option<u8>,
    },
}

/// Zero-allocation streaming event reader over an asciicast v2 text recording.
#[derive(Debug, Clone)]
pub struct StreamingReader<'a> {
    lines: core::str::Lines<'a>,
    line_number: usize,
    previous_micros: u64,
    header: Header,
    decode_buf: Vec<u8>,
}

impl<'a> StreamingReader<'a> {
    /// Create a new streaming reader over asciicast recording text.
    pub fn new(text: &'a str) -> Result<Self, CastError> {
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

        Ok(Self {
            lines,
            line_number: 1,
            previous_micros: 0,
            header,
            decode_buf: Vec::new(),
        })
    }

    /// Access the parsed header.
    #[must_use]
    pub const fn header(&self) -> &Header {
        &self.header
    }
}

impl<'a> Iterator for StreamingReader<'a> {
    type Item = Result<EventRef<'a>, CastError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let line = self.lines.next()?;
            self.line_number += 1;
            if line.trim().is_empty() {
                continue;
            }

            let line_number = self.line_number;
            let decode_buf = &mut self.decode_buf;
            let (micros, code_byte, data_body) = match parse_event_ref(line, line_number, decode_buf) {
                Ok(val) => val,
                Err(err) => return Some(Err(err)),
            };

            if micros < self.previous_micros {
                return Some(Err(CastError::EventTimeOrder {
                    line: self.line_number,
                    micros,
                    previous: self.previous_micros,
                }));
            }
            self.previous_micros = micros;

            match code_byte {
                b'o' => return Some(Ok(EventRef::Output { line: self.line_number, micros, raw_data: data_body })),
                b'm' => return Some(Ok(EventRef::Marker { line: self.line_number, micros, raw_label: data_body })),
                b'r' => {
                    if !data_body.contains('\\') {
                        if let Some((cols, rows)) = parse_geometry(data_body.as_bytes()) {
                            return Some(Ok(EventRef::Resize { micros, cols, rows }));
                        }
                    }
                    self.decode_buf.clear();
                    if decode(data_body, self.line_number, &mut self.decode_buf).is_ok() {
                        if let Some((cols, rows)) = parse_geometry(&self.decode_buf) {
                            return Some(Ok(EventRef::Resize { micros, cols, rows }));
                        }
                    }
                    return Some(Err(CastError::EventData {
                        line: self.line_number,
                        reason: "a resize event's data is not \"COLSxROWS\"",
                    }));
                }
                b'i' => return Some(Ok(EventRef::Input { micros })),
                _ => return Some(Ok(EventRef::Skipped {
                    line: self.line_number,
                    code: Some(code_byte),
                })),
            }
        }
    }
}

/// Parse an asciicast v2 recording using zero-allocation streaming event reader.
pub fn read(text: &str) -> Result<Recording, CastError> {
    let reader = StreamingReader::new(text)?;
    let header = reader.header().clone();

    let mut recording = Recording {
        header,
        bytes: Vec::new(),
        stamps: Vec::new(),
        markers: Vec::new(),
        resizes: Vec::new(),
        inputs: 0,
        skipped: 0,
    };

    let mut decode_buf = Vec::new();

    for event in reader {
        let event = event?;
        match event {
            EventRef::Output { line, micros, raw_data } => {
                if !raw_data.contains('\\') {
                    recording.bytes.extend_from_slice(raw_data.as_bytes());
                } else {
                    decode_buf.clear();
                    decode(raw_data, line, &mut decode_buf)?;
                    recording.bytes.extend_from_slice(&decode_buf);
                }
                recording.stamps.push(ChunkStamp {
                    end_seq: recording.bytes.len() as u64,
                    micros,
                });
            }
            EventRef::Marker { line, raw_label, .. } => {
                if !raw_label.contains('\\') {
                    recording.markers.push(Marker {
                        seq: recording.bytes.len() as u64,
                        label: raw_label.to_string(),
                        hint: None,
                    });
                } else {
                    decode_buf.clear();
                    decode(raw_label, line, &mut decode_buf)?;
                    recording.markers.push(Marker {
                        seq: recording.bytes.len() as u64,
                        label: String::from_utf8_lossy(&decode_buf).into_owned(),
                        hint: None,
                    });
                }
            }
            EventRef::Resize { micros, cols, rows } => {
                recording.resizes.push(Resize {
                    seq: recording.bytes.len() as u64,
                    micros,
                    cols,
                    rows,
                });
            }
            EventRef::Input { .. } => recording.inputs += 1,
            EventRef::Skipped { .. } => recording.skipped += 1,
        }
    }

    Ok(recording)
}

fn parse_event_ref<'a>(line: &'a str, number: usize, buf: &mut Vec<u8>) -> Result<(u64, u8, &'a str), CastError> {
    let bytes = line.as_bytes();
    let mut at = 0usize;

    skip_space(bytes, &mut at);
    if bytes.get(at) != Some(&b'[') {
        return Err(CastError::EventShape { line: number });
    }
    at += 1;

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
    let code = if code_body.len() == 1 && !code_body.starts_with('\\') {
        code_body.as_bytes()[0]
    } else {
        buf.clear();
        decode(code_body, number, buf)?;
        let [code] = buf[..] else {
            return Err(CastError::EventCode { line: number });
        };
        code
    };

    skip_space(bytes, &mut at);
    if bytes.get(at) != Some(&b',') {
        return Err(CastError::EventShape { line: number });
    }
    at += 1;

    let data_body = string_body(line, &mut at, number)?;

    skip_space(bytes, &mut at);
    if bytes.get(at) != Some(&b']') {
        return Err(CastError::EventShape { line: number });
    }
    at += 1;
    skip_space(bytes, &mut at);
    if at != bytes.len() {
        return Err(CastError::EventShape { line: number });
    }

    Ok((micros, code, data_body))
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
