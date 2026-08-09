//! Column widths, checked against the engine that actually lays out a session.
//!
//! `vitrum-grid` classifies characters with the East Asian Width property while
//! `vitrum-vt` hands cells to libghostty, and only one of those two can be
//! right about how many columns a character claims. These tests make libghostty
//! the authority: the sample characters are whatever the engine reports for
//! each width class, so nothing here is a table of codepoints that quietly goes
//! stale when the engine's Unicode data moves on.
//!
//! The dependency direction is the reason this lives in a test rather than in
//! `char_width`: `vitrum-vt` depends on `vitrum-grid`, so the grid can only
//! reach the engine from a dev-dependency. A divergence found here is a defect
//! in [`vitrum_grid::cell::char_width`], not in the test.

use vitrum_vt::{Vt, VtOptions};

use vitrum_grid::cell::{Style, char_width};
use vitrum_grid::grid::CellGrid;

/// Wide enough that nothing under test reaches the wrap point, where the
/// engine holds the cursor on the last column and the advance stops being a
/// width measurement.
const PROBE_COLS: u16 = 40;

/// Highest codepoint the sample scan will look at. Every width class is
/// populated well before this; the bound only keeps a failure to find one from
/// turning into a scan of the whole of Unicode.
const SCAN_END: u32 = 0x3000;

/// Where the scan starts: the combining diacritics block, the first place a
/// single window contains all three width classes.
const SCAN_START: u32 = 0x0300;

fn engine() -> Vt {
    Vt::new(VtOptions {
        cols: PROBE_COLS,
        rows: 1,
        max_scrollback: 0,
    })
    .expect("a one-row probe terminal must be constructible")
}

/// The column the engine's cursor lands on after laying out `text` on a clean
/// screen. This is libghostty's own answer to how wide the text is.
fn engine_advance(vt: &mut Vt, text: &str) -> u16 {
    vt.reset();
    vt.feed(text.as_bytes());
    vt.cursor().expect("the engine must report its cursor").col
}

/// Columns the engine gives `ch`, measured as the extra columns it adds after a
/// narrow base character.
///
/// The base matters. Fed on its own, a combining mark has nothing to attach to
/// and the engine gives it a cell of its own, which measures the probe rather
/// than the character.
fn engine_width(vt: &mut Vt, ch: char) -> u16 {
    let mut probe = String::from("a");
    probe.push(ch);
    engine_advance(vt, &probe).saturating_sub(1)
}

/// The first `per_class` characters of each width the engine reports, indexed
/// by that width: `[zero-width, narrow, wide]`.
fn samples(vt: &mut Vt, per_class: usize) -> [Vec<char>; 3] {
    let mut by_width: [Vec<char>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for cp in SCAN_START..=SCAN_END {
        let Some(ch) = char::from_u32(cp) else {
            continue;
        };
        let width = usize::from(engine_width(vt, ch));
        if let Some(bucket) = by_width.get_mut(width)
            && bucket.len() < per_class
        {
            bucket.push(ch);
        }
        if by_width.iter().all(|bucket| bucket.len() == per_class) {
            break;
        }
    }
    for (width, bucket) in by_width.iter().enumerate() {
        assert_eq!(
            bucket.len(),
            per_class,
            "the engine reported fewer than {per_class} characters of width \
             {width} below U+{SCAN_END:04X}, so the sample set cannot be built"
        );
    }
    by_width
}

/// Every width class the engine reports must get the same column count from
/// [`char_width`].
///
/// This is the classification the grid's cursor arithmetic is built on. A
/// character the engine lays out in two columns and the grid counts as one
/// shifts every later column on the line, and the misalignment survives until
/// the row is repainted, because both sides believe they are right.
///
/// What this does not catch: characters outside the scan window, and control
/// characters, which the engine consumes as actions rather than laying out.
#[test]
fn every_width_class_the_engine_reports_gets_the_same_column_count() {
    let mut vt = engine();
    let by_width = samples(&mut vt, 4);

    for (want, chars) in by_width.iter().enumerate() {
        for &ch in chars {
            let got = usize::from(char_width(ch).columns().unwrap_or(0));
            assert_eq!(
                got,
                want,
                "U+{:04X}: the engine lays it out in {want} column(s), \
                 char_width says {got}",
                u32::from(ch)
            );
        }
    }
}

/// A string of wide, narrow, and combining characters must advance the grid's
/// cursor exactly as far as the engine advances its own, and must produce the
/// same head/tail layout.
///
/// This is the collapse defect: advancing by one character rather than by the
/// character's display width writes every wide character on top of the last
/// one, so three ideographs end up in a single column. Comparing against the
/// engine's synced grid also pins the pairing, because an advance of two with
/// only a head written would still land the cursor in the right place while
/// leaving a hole on screen.
///
/// What this does not catch: rendering. The columns are right here even if the
/// glyph drawn into them is not.
#[test]
fn a_mixed_width_string_advances_exactly_as_far_as_the_engine() {
    let mut vt = engine();
    let by_width = samples(&mut vt, 2);

    // Interleaved rather than grouped: a mark lands on a wide base and a narrow
    // character follows a wide one, which is the arrangement a per-character
    // advance bug survives when every probe character is the same class.
    let mut text = String::new();
    for i in 0..2 {
        text.push(by_width[2][i]);
        text.push(by_width[0][i]);
        text.push(by_width[1][i]);
    }

    let want = engine_advance(&mut vt, &text);
    // Stated independently of the oracle: an engine probe that measured
    // nothing would otherwise agree with a grid that wrote nothing.
    assert_eq!(
        want,
        2 * 2 + 2,
        "two wide characters at two columns each, two narrow at one, and two \
         marks at none: {text:?}"
    );

    let mut grid = CellGrid::new(PROBE_COLS, 1, Style::DEFAULT).expect("probe grid");
    let got = grid
        .write_str(0, 0, &text, Style::DEFAULT)
        .expect("the probe fits the row");
    assert_eq!(
        got, want,
        "write_str ended at column {got}, the engine at {want}: {text:?}"
    );

    let mut engine_grid = CellGrid::new(PROBE_COLS, 1, Style::DEFAULT).expect("probe grid");
    vt.sync(&mut engine_grid)
        .expect("the engine must sync into a grid of its own size");
    for col in 0..want {
        let ours = grid.cell(col, 0).expect("column in bounds");
        let theirs = engine_grid.cell(col, 0).expect("column in bounds");
        assert_eq!(
            (ours.ch, ours.slot),
            (theirs.ch, theirs.slot),
            "column {col} differs from the engine's layout of {text:?}"
        );
    }
}
