//! One concern per module.
//!
//! - [`support`]: shared fixtures, including the captured session the round-trip and
//!   seek suites run against.
//! - [`stream_ranges`]: seq to byte mapping across a ring's seam.
//! - [`screen_print`]: printing, wrapping, wide characters, insert mode.
//! - [`screen_erase`]: ED, EL, ECH and back-colour erase.
//! - [`screen_scroll`]: scroll regions, IL, DL, SU, SD, IND, RI.
//! - [`screen_edit`]: ICH and DCH.
//! - [`screen_cursor`]: cursor addressing, origin mode, tabs, save and restore.
//! - [`screen_sgr`]: rendition, including both extended colour spellings.
//! - [`screen_charset`]: DEC Special Graphics and the shifts.
//! - [`screen_alt`]: the alternate screen and 1049's cursor stash.
//! - [`emulator_ground`]: the ground-state probe keyframes depend on.
//! - [`keyframe_index`]: keyframe placement, lookup, and cost.
//! - [`seek_equivalence`]: every seek against a linear replay.
//! - [`timeline_clock`]: seq to time and back, and the honesty flag.
//! - [`hints_markers`]: OSC 7373 marker positions.
//! - [`asciicast_jsonstr`]: the byte-exact JSON string codec.
//! - [`asciicast_roundtrip`]: bytes to file to bytes.
//! - [`asciicast_reader_errors`]: every way a file can be rejected.
//! - [`asciicast_header`]: header requirements and unknown-key preservation.
//! - [`cost`]: measured seek latency and index memory.

mod asciicast_header;
mod keyframe_delta;
mod asciicast_jsonstr;
mod asciicast_reader_errors;
mod asciicast_roundtrip;
mod cost;
mod emulator_ground;
mod hints_markers;
mod keyframe_index;
mod screen_alt;
mod screen_charset;
mod screen_cursor;
mod screen_edit;
mod screen_erase;
mod screen_print;
mod screen_scroll;
mod screen_sgr;
mod seek_equivalence;
mod stream_ranges;
mod support;
mod timeline_clock;
