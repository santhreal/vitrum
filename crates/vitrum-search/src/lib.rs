//! Search every session's scrollback at once.
//!
//! The daemon owns twenty PTYs and twenty ring buffers. "Which of my agents
//! mentioned OOM?" is therefore one server-side query over memory it already
//! holds — not twenty round trips, not a client-side scroll-and-squint, and not
//! something a terminal that hosts one shell per window can offer at all.
//!
//! ```
//! use vitrum_search::{Haystack, Query, search};
//!
//! let session_3: &[u8] = b"cargo build\n\x1b[1;31merror\x1b[0m: linker killed\nexit 137\n";
//! let session_7: &[u8] = b"all tests passed\n";
//!
//! let results = search(
//!     &Query::literal("error").context(1),
//!     &[
//!         Haystack { session: 3, base_seq: 0, chunks: std::slice::from_ref(&session_3) },
//!         Haystack { session: 7, base_seq: 0, chunks: std::slice::from_ref(&session_7) },
//!     ],
//! )
//! .expect("valid pattern");
//!
//! assert_eq!(results.len(), 1);
//! let hit = &results.hits[0];
//! assert_eq!(hit.session, 3);
//! // Matched on the visible text, so the colour did not hide it.
//! assert_eq!(hit.visible_lossy(), "error: linker killed");
//! // Returned with its colour intact, so the client renders it as it was.
//! assert_eq!(hit.line, b"\x1b[1;31merror\x1b[0m: linker killed");
//! // And positioned by the ORIGINAL byte, after the SGR introducer.
//! assert_eq!(hit.match_seq, 19);
//! assert_eq!(hit.before[0].bytes, b"cargo build");
//! assert_eq!(hit.after[0].bytes, b"exit 137");
//! ```
//!
//! # The three hard parts
//!
//! **Colour is noise inserted inside words.** A ring holds what the program
//! wrote, and what an agent writes is `\x1b[1;31merror\x1b[0m`. Worse,
//! `\x1b[31me\x1b[0mrror` is equally legal, so a raw byte scan for `error` does
//! not merely find some matches — it finds an arbitrary subset determined by
//! where the producer happened to change colour. Matching therefore runs on
//! escape-stripped text, while every offset reported and every byte returned is
//! in the original coordinate system. See [`ansi`] for the map that connects
//! them.
//!
//! **A ring has a seam.** Reading a ring gives two contiguous halves and the
//! join lands mid-line, mid-word, mid-character, mid-escape-sequence. Stitching
//! them costs 200 MB of copying across twenty sessions, so nothing is stitched:
//! [`chunks`] walks lines across the halves and copies only the one line per
//! seam that actually straddles it.
//!
//! **Twenty times ten megabytes is a real number.** 200 MB scanned per query,
//! per keystroke if a client is live-searching. The scan therefore allocates
//! nothing per line — see [`mod@search`] — and a plain literal takes a SIMD
//! substring path rather than a regex engine.
//!
//! # What it is not
//!
//! Not a terminal emulator. Cursor motion is not replayed, so text that was
//! overwritten in place is still searchable; `50%\r100%` reads as `50%100%`.
//! Resolving that needs a grid and a screen width, and the failure mode here is
//! an occasional extra hit rather than a missed one.
//!
//! Not an index. Every query is a linear scan. At the measured throughput a
//! full 200 MB sweep is well under a second, and an index over a ring that
//! rewrites itself continuously would cost more to maintain than the scans it
//! saves.
//!
//! # Module map
//!
//! - [`ansi`]: escape stripping, and the offset map back to original bytes.
//! - [`chunks`]: the ring-shaped input, and lines across its seams.
//! - [`query`]: what to look for and how much to keep.
//! - [`matcher`]: literal and regex, compiled once per search.
//! - [`hit`]: what comes back.
//! - [`mod@search`]: the scan.

pub mod ansi;
pub mod chunks;
pub mod error;
pub mod hit;
pub mod matcher;
pub mod query;
pub mod search;

pub use ansi::{Map, Run, Stripper, needs_stripping};
pub use chunks::{Chunked, Haystack, LineSpan, Lines};
pub use error::{Error, Result};
pub use hit::{ContextLine, Hit, SearchResults};
pub use matcher::Matcher;
pub use query::{DEFAULT_MAX_ANSWER_BYTES, MAX_CONTEXT, Pattern, Query};
pub use search::{Sweep, search, search_parallel, search_with, search_with_parallel};
