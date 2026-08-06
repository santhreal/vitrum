//! Shared fixtures.
//!
//! # The captured session
//!
//! [`CAPTURED`] is not synthetic. It is the raw output of a real PTY, recorded with
//! `script(1)` on this machine, and it contains what real terminal output contains
//! and what a hand-written fixture always forgets:
//!
//! - `git log --color=always` with its actual SGR sequences, including the bare
//!   `ESC [ m` reset that has no parameter at all;
//! - a progress line that redraws itself with `CR` and `EL`;
//! - UTF-8 that is not Latin script: Japanese, an accented Latin-1 character encoded
//!   as UTF-8, box-drawing glyphs;
//! - bytes that are not valid UTF-8 at all (`0xff 0xfe`, and a lone `0x80`);
//! - a box drawn through DEC Special Graphics with `ESC ( 0` and `ESC ( B`;
//! - 24-bit and 256-colour SGR in the semicolon spelling;
//! - `CSI s` and `CSI u`;
//! - an alternate-screen excursion with `CSI ? 1049 h` and `l`;
//! - an OSC 0 title, and three OSC 7373 agent hints, one terminated by `BEL` and two
//!   by `ESC \`.
//!
//! Every one of those is a case that has broken a terminal emulator in the wild, and
//! a fixture written by hand would have contained the ones its author remembered.

use crate::config::ReplayConfig;
use crate::emulator::Emulator;
use crate::palette::Palette;
use crate::screen::Screen;
use crate::stream::Stream;

/// A real PTY capture. See the module header.
pub const CAPTURED: &[u8] = include_bytes!("../../fixtures/captured-session.raw");

/// A screen with `bytes` fed through a single emulator in one call.
///
/// This is the reference implementation every seek is compared against: no
/// keyframes, no restore, just the whole prefix in order.
pub fn linear(cols: u16, rows: u16, bytes: &[u8]) -> Screen {
    let mut emulator = Emulator::new(cols, rows, Palette::XTERM).expect("valid geometry");
    emulator.feed(bytes);
    emulator.into_screen()
}

/// A default configuration at `cols` x `rows`.
pub fn config(cols: u16, rows: u16) -> ReplayConfig {
    ReplayConfig::new(cols, rows).expect("valid geometry")
}

/// A one-chunk stream over `bytes` starting at seq zero.
///
/// Written as a macro because a [`Stream`] borrows the slice-of-slices it walks, and
/// that array has to live in the caller's frame.
#[macro_export]
macro_rules! stream_over {
    ($bytes:expr) => {{ $crate::stream::Stream::new(0, ::core::slice::from_ref(&$bytes)) }};
    ($base:expr, $bytes:expr) => {{
        $crate::stream::Stream::new($base, ::core::slice::from_ref(&$bytes))
    }};
}

/// Every row of the screen, right-trimmed, for a readable assertion failure.
pub fn rows_of(screen: &Screen) -> Vec<String> {
    (0..screen.rows())
        .map(|row| screen.line(row).trim_end().to_string())
        .collect()
}

/// A stream long enough to need keyframes, built by repeating the capture with a
/// changing line each round so no two regions are identical.
///
/// Identical regions would let a broken seek land in the wrong place and still
/// produce the right screen, which is the one way this whole suite could pass while
/// being wrong.
pub fn grown(target: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(target + CAPTURED.len());
    let mut round = 0u32;
    while out.len() < target {
        out.extend_from_slice(CAPTURED);
        out.extend_from_slice(
            format!("\r\n\x1b[36mround {round} at {} bytes\x1b[0m\r\n", out.len()).as_bytes(),
        );
        round += 1;
    }
    out
}

/// Feed `bytes` through a fresh emulator and return the screen, at 80x24.
pub fn screen80(bytes: &[u8]) -> Screen {
    linear(80, 24, bytes)
}

/// Feed `bytes` at a small size, where wrapping and scrolling are easy to assert.
pub fn small(bytes: &[u8]) -> Screen {
    linear(10, 4, bytes)
}

/// Build a stream, replay it, and hand both to `check`.
///
/// Exists because the borrow of the chunk array has to outlive the replay, which
/// means a helper cannot return the replay.
pub fn with_replay<F>(bytes: &[u8], config: &ReplayConfig, check: F)
where
    F: FnOnce(&mut crate::replay::Replay<'_>),
{
    let chunks = [bytes];
    let stream = Stream::new(0, &chunks);
    let mut replay = crate::replay::Replay::build(stream, config).expect("build");
    check(&mut replay);
}
