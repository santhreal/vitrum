//! asciicast v2 import and export, byte exact.
//!
//! asciicast is the format `asciinema` records into, and it is the de facto standard:
//! every web player, every embed, every "watch what I did" link speaks it. Exporting
//! to it means a vitrum session can be shared and replayed by things that have never
//! heard of vitrum, and importing it means a recording made anywhere can be scrubbed
//! by the same code path as a live session.
//!
//! # The format
//!
//! Line one is a JSON object. Every line after it is a JSON array of three elements:
//! a time in seconds from the start, a one-character type code, and a string.
//!
//! ```text
//! {"version": 2, "width": 80, "height": 24, "title": "cargo test"}
//! [0.000000, "o", "$ cargo test\r\n"]
//! [0.412000, "o", "\u001b[32m   Compiling\u001b[0m vitrum-replay\r\n"]
//! [3.900000, "m", "approval needed"]
//! ```
//!
//! # Byte exactness
//!
//! Round-tripping is a hard requirement here, not a nice property: a recording you
//! cannot reload is not a recording. The obstacle is that terminal output is bytes
//! and a JSON string is text, and the two are not the same set. See
//! [`mod@jsonstr`] for the whole discussion and the convention that resolves it.
//!
//! ```
//! use vitrum_replay::{Stream, Timeline, asciicast};
//! use vitrum_replay::asciicast::{Header, Utf8Policy};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // A colour escape, a UTF-8 character, and a byte that is not valid UTF-8.
//! let original: &[u8] = b"\x1b[31merror\x1b[0m \xe2\x9c\x93 \xff\xfe";
//! let stream = Stream::new(0, std::slice::from_ref(&original));
//!
//! let text = asciicast::to_string(
//!     &stream,
//!     &Timeline::positional(),
//!     &Header::new(80, 24),
//!     Utf8Policy::SurrogateEscape,
//! )?;
//! let back = asciicast::read(&text)?;
//!
//! assert_eq!(back.bytes(), original);
//! # Ok(())
//! # }
//! ```

pub mod header;
pub mod jsonstr;
pub mod reader;
pub mod writer;

pub use header::Header;
pub use jsonstr::Utf8Policy;
pub use reader::{Recording, Resize, read};
pub use writer::{to_string, write};
