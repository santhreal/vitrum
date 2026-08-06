//! Display formatting for the vitrum client.
//!
//! Every string a person reads in the sidebar, the tab strip, or a status line
//! is produced here: relative timestamps, working durations, shortened paths,
//! truncated titles, byte sizes, counted nouns, exit statuses, and git heads.
//!
//! # Invariants this crate holds
//!
//! - **Pure.** Nothing reads the system clock, the environment, or the
//!   filesystem. `now`, the UTC offset, and `$HOME` are parameters. The same
//!   inputs always produce the same string, so every boundary is assertable to
//!   the millisecond and to the column, and one render tick produces one
//!   consistent set of labels.
//! - **No panics on hostile input.** Titles arrive from OSC sequences written
//!   by whatever program the user ran, paths arrive from other machines, clocks
//!   run backwards. There is no `unwrap` on any of it; degenerate input yields
//!   a degenerate string, never a crash in a UI thread.
//! - **Columns, not bytes.** Every width is a terminal column count measured
//!   over grapheme clusters, so CJK, emoji, and combining marks lay out
//!   correctly and truncation never splits a glyph. See [`text`].
//! - **No allocation-free theatre.** These functions build short strings and
//!   return them. They run on visible rows only, on the render path, not in a
//!   loop over scrollback.
//!
//! # Modules
//!
//! - [`text`]: width measurement, end and middle truncation, sanitising.
//! - [`time`]: relative timestamps and absolute dates.
//! - [`duration`]: elapsed-time labels for "working for X".
//! - [`path`]: home-relative and component-eliding paths.
//! - [`bytes`]: binary byte sizes.
//! - [`count`]: counted and pluralised nouns.
//! - [`exit`]: process termination descriptions.
//! - [`git`]: branch and detached-head labels.
//!
//! # Example
//!
//! ```
//! use vitrum_fmt::{
//!     TimeFormat, Timestamp,
//!     count, duration, path,
//! };
//! use std::time::Duration;
//!
//! let now = Timestamp::from_secs(1_700_000_000);
//! let clock = TimeFormat::new(now, 0);
//!
//! assert_eq!(clock.relative(Timestamp::from_secs(1_699_999_748)), "4m");
//! assert_eq!(duration::compact(Duration::from_secs(252)), "4m 12s");
//! assert_eq!(count::count_s(2, "session"), "2 sessions");
//! assert_eq!(
//!     path::shorten_home_relative("/home/mk/src/vitrum/crates/vitrum-fmt", "/home/mk", 24),
//!     "~/\u{2026}/crates/vitrum-fmt",
//! );
//! ```

#![deny(missing_docs)]

pub mod bytes;
pub mod color;
pub mod count;
pub mod duration;
pub mod exit;
pub mod git;
pub mod path;
pub mod text;
pub mod time;
#[cfg(test)]
mod tests;

pub use exit::Termination;
pub use git::Head;
pub use text::ELLIPSIS;
pub use time::{TimeFormat, Timestamp};
