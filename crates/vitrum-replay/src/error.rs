//! How this crate fails.
//!
//! Every variant names the value that was wrong and the range it had to be in,
//! because the two questions a caller actually has are "which seq did I ask for"
//! and "which seqs does this stream still hold". A ring that has evicted 9 MiB
//! answers the second question differently every second, so an error that only
//! said "out of range" would send the caller back to the daemon to find out what
//! the range even was.

use core::fmt;

/// Result alias for this crate.
pub type Result<T, E = Error> = core::result::Result<T, E>;

/// Why a replay operation was refused.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Error {
    /// A seek named a seq the stream does not hold.
    ///
    /// `oldest` is the seq of the first retained byte and `head` is the seq one
    /// past the last, so `head` itself is a legal seek target meaning "the end
    /// of everything written so far".
    SeqOutOfRange {
        /// The seq asked for.
        seq: u64,
        /// First seq the stream still holds.
        oldest: u64,
        /// One past the last seq the stream holds.
        head: u64,
    },
    /// The requested screen size is not a grid [`vitrum_grid::CellGrid`] accepts.
    Geometry {
        /// Requested columns.
        cols: u16,
        /// Requested rows.
        rows: u16,
    },
    /// A keyframe stride of zero was requested, which would ask for one snapshot
    /// per byte.
    ZeroStride,
    /// An asciicast recording could not be read.
    Cast(CastError),
    /// Stream compression or archive decoding error.
    StreamCompression(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SeqOutOfRange { seq, oldest, head } => write!(
                f,
                "seq {seq} is outside the retained stream {oldest}..={head}; \
                 the ring has evicted everything before {oldest}"
            ),
            Self::Geometry { cols, rows } => write!(
                f,
                "{cols}x{rows} is not a usable screen size; both sides must be \
                 non-zero and within vitrum-grid's MAX_COLS/MAX_ROWS/MAX_CELLS"
            ),
            Self::ZeroStride => write!(
                f,
                "keyframe stride must be at least 1 byte; zero would snapshot \
                 the whole screen once per byte"
            ),
            Self::Cast(inner) => write!(f, "asciicast: {inner}"),
            Self::StreamCompression(msg) => write!(f, "stream compression error: {msg}"),
        }
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Cast(inner) => Some(inner),
            _ => None,
        }
    }
}

impl From<CastError> for Error {
    fn from(inner: CastError) -> Self {
        Self::Cast(inner)
    }
}

/// Why an asciicast recording could not be read.
///
/// Line numbers are 1-based and count every line of the file including the
/// header, so they match what an editor shows.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CastError {
    /// The file had no header line.
    Empty,
    /// The header line was not a JSON object.
    HeaderSyntax {
        /// What `serde_json` said.
        message: String,
    },
    /// The header declared a version this crate does not implement.
    Version {
        /// The version found.
        found: u64,
    },
    /// The header was missing `width` or `height`, which v2 requires.
    MissingGeometry,
    /// An event line was not a JSON array of three elements.
    EventShape {
        /// 1-based line number.
        line: usize,
    },
    /// An event's time field was not a finite, non-negative decimal number.
    EventTime {
        /// 1-based line number.
        line: usize,
    },
    /// Event times went backwards. asciicast v2 times are absolute and
    /// monotonic, and a scrubber built on non-monotonic times seeks to the wrong
    /// place rather than failing visibly.
    EventTimeOrder {
        /// 1-based line number.
        line: usize,
        /// The time on this line, in microseconds.
        micros: u64,
        /// The time on the previous event, in microseconds.
        previous: u64,
    },
    /// An event's type code was not a one-character JSON string.
    EventCode {
        /// 1-based line number.
        line: usize,
    },
    /// An event's data string contained something no JSON string may contain.
    EventData {
        /// 1-based line number.
        line: usize,
        /// What was wrong.
        reason: &'static str,
    },
}

impl fmt::Display for CastError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "the file is empty; v2 requires a header line"),
            Self::HeaderSyntax { message } => {
                write!(f, "line 1 is not a JSON object: {message}")
            }
            Self::Version { found } => write!(
                f,
                "header says version {found}; this reader implements version 2"
            ),
            Self::MissingGeometry => write!(
                f,
                "header is missing width or height, which version 2 requires \
                 because a recording cannot be replayed without a screen size"
            ),
            Self::EventShape { line } => write!(
                f,
                "line {line} is not a 3-element JSON array [time, code, data]"
            ),
            Self::EventTime { line } => write!(
                f,
                "line {line} has a time that is not a finite non-negative number"
            ),
            Self::EventTimeOrder {
                line,
                micros,
                previous,
            } => write!(
                f,
                "line {line} is at {micros}us but the previous event was at \
                 {previous}us; v2 times are absolute and must not go backwards"
            ),
            Self::EventCode { line } => {
                write!(f, "line {line} has a type code that is not one character")
            }
            Self::EventData { line, reason } => {
                write!(f, "line {line} has an unreadable data string: {reason}")
            }
        }
    }
}

impl core::error::Error for CastError {}
