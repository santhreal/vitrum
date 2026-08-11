//! Shared fixtures.
//!
//! # The captured session
//!
//! [`CAPTURED`] is not synthetic. It is the raw output of a real PTY, recorded with
//! `script(1)`, and it contains what real terminal output contains and what a
//! hand-written fixture always forgets:
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

use vitrum_grid::{Cell, Rgba};

use crate::config::ReplayConfig;
use crate::emulator::Emulator;
use crate::palette::Palette;
use crate::screen::Screen;
use crate::stream::Stream;

/// A real PTY capture. See the module header.
pub const CAPTURED: &[u8] = include_bytes!("../../fixtures/captured-session.raw");

/// The engine's own sixteen named colours, which is what `SGR 30..37` and
/// `SGR 90..97` resolve to now that the engine owns indexed colour.
///
/// This is not the compiled-in table of any other terminal. The engine ships a theme,
/// so `SGR 31` is `cc6666` here. That is the product truth, because the daemon paints
/// the live pane through the same engine; see [`crate::palette`].
///
/// The table exists so an engine version bump that changes the theme turns the suite
/// red in one place and forces somebody to record the decision, rather than silently
/// changing what colour a replayed session was.
pub const GHOSTTY_ANSI: [Rgba; 16] = [
    Rgba::rgb(0x1d, 0x1f, 0x21),
    Rgba::rgb(0xcc, 0x66, 0x66),
    Rgba::rgb(0xb5, 0xbd, 0x68),
    Rgba::rgb(0xf0, 0xc6, 0x74),
    Rgba::rgb(0x81, 0xa2, 0xbe),
    Rgba::rgb(0xb2, 0x94, 0xbb),
    Rgba::rgb(0x8a, 0xbe, 0xb7),
    Rgba::rgb(0xc5, 0xc8, 0xc6),
    Rgba::rgb(0x66, 0x66, 0x66),
    Rgba::rgb(0xd5, 0x4e, 0x53),
    Rgba::rgb(0xb9, 0xca, 0x4a),
    Rgba::rgb(0xe7, 0xc5, 0x47),
    Rgba::rgb(0x7a, 0xa6, 0xda),
    Rgba::rgb(0xc3, 0x97, 0xd8),
    Rgba::rgb(0x70, 0xc0, 0xb1),
    Rgba::rgb(0xea, 0xea, 0xea),
];

/// A screen with `bytes` fed through a single emulator in one call.
///
/// This is the reference implementation every seek is compared against: no
/// keyframes, no restore, just the whole prefix in order.
pub fn linear(cols: u16, rows: u16, bytes: &[u8]) -> Screen {
    let mut emulator = Emulator::new(cols, rows, Palette::DEFAULT).expect("valid geometry");
    emulator.feed(bytes).expect("engine readable");
    emulator.into_screen()
}

/// A screen with the same bytes delivered as several separate feeds.
///
/// A PTY read returns whatever bytes had arrived, so an escape sequence, a UTF-8
/// character, or an OSC string is routinely cut in half by a chunk boundary. This
/// helper exists so a conformance test can assert that the split is invisible.
pub fn split_feed(cols: u16, rows: u16, chunks: &[&[u8]]) -> Screen {
    let mut emulator = Emulator::new(cols, rows, Palette::DEFAULT).expect("valid geometry");
    for chunk in chunks {
        emulator.feed(chunk).expect("engine readable");
    }
    emulator.into_screen()
}

/// Feed `bytes` at a small size, where wrapping and scrolling are easy to assert.
pub fn small(bytes: &[u8]) -> Screen {
    linear(10, 4, bytes)
}

/// A default configuration at `cols` x `rows`.
pub fn config(cols: u16, rows: u16) -> ReplayConfig {
    ReplayConfig::new(cols, rows).expect("valid geometry")
}

/// Every row of the screen, right-trimmed, for a readable assertion failure.
pub fn rows_of(screen: &Screen) -> Vec<String> {
    (0..screen.rows())
        .map(|row| screen.line(row).trim_end().to_string())
        .collect()
}

/// The cell at `col`, `row`, which every geometry in this suite has.
pub fn cell_at(screen: &Screen, col: u16, row: u16) -> Cell {
    screen
        .grid()
        .cell(col, row)
        .unwrap_or_else(|| panic!("cell {col},{row} must be inside a {}x{} screen", screen.cols(), screen.rows()))
}

/// A stream long enough to make a rewind expensive, built by repeating the capture
/// with a changing line each round so no two regions are identical.
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
