//! The search sheet's size, and the two rules that keep it still.

use super::*;
use vitrum_proto::SearchHit;

fn hit(session: u64, line_seq: u64) -> SearchHit {
    SearchHit {
        session: SessionId(session),
        line_seq,
        before: vec![b"context above".to_vec(), b"context above".to_vec()],
        visible: b"error: could not open /src/vitrum/app".to_vec(),
        match_start: 0,
        match_end: 5,
        after: vec![b"context below".to_vec(), b"context below".to_vec()],
    }
}

fn answer(hits: usize) -> Answer {
    Answer {
        pattern: "error".to_string(),
        hits: (0..hits as u64).map(|n| hit(n % 4, n)).collect(),
        truncated: false,
        bytes_scanned: 1_024,
    }
}

/// **The fit rule for this surface.** A sweep that came back at its cap is
/// still reachable inside a window smaller than it is, and it must hold at
/// the largest answer the settings allow: the hit cap and the context budget
/// are both preferences, so the default pair proves nothing about the pair an
/// operator can actually ask for.
#[test]
fn a_full_answer_fits_a_window_smaller_than_it_is() {
    let _bus = crate::state::live::exclusive();
    let mut settings = crate::state::Settings::default();
    settings.search.context_lines = crate::state::CONTEXT_LINES_MAX;
    settings.search.max_hits = crate::state::MAX_HITS_MAX;
    crate::state::live::publish(&settings);
    let full = answer(super::super::max_hits() as usize);
    sheet::assert_fits(sheet::SEARCH, sheet::DOCUMENT, content(Some(&full)));
}

/// The empty sheet is a field and a sentence, and it still fits the frame a
/// workspace switch hands a client. The interesting case is the full one
/// above; this is the other end of the range, where a surface that reserved
/// its cap as a floor would fail instead.
#[test]
fn the_unsearched_sheet_is_short_and_still_fits_the_smallest_frame() {
    let _bus = crate::state::live::exclusive();
    let empty = content(None);
    let full = content(Some(&answer(super::super::max_hits() as usize)));
    assert!(empty.1 < full.1);
    let natural = sheet::natural(sheet::DOCUMENT, empty);
    assert!(sheet::allocated(sheet::SMALLEST.1, natural.1) <= sheet::SMALLEST.1);
}

/// **The flicker rule.** An answer that did not change is not redrawn.
///
/// Without this the sheet rebuilt up to five hundred rows on every daemon
/// message, which is a list that flashes under the pointer several times a
/// second while an agent is producing output.
#[test]
fn an_unchanged_answer_is_not_redrawn() {
    let one = answer(3);
    assert!(!needs_redraw(Some(&one), Some(&one)));
    assert!(!needs_redraw(None, None));
    assert!(needs_redraw(Some(&one), Some(&answer(4))));
    assert!(needs_redraw(None, Some(&one)));
    assert!(needs_redraw(Some(&one), None));
}

/// **The highlight is still cut from the raw bytes.** The surface draws three
/// labels built by [`split_hit`] from the bytes and the daemon's offsets, so a
/// stray byte before the match cannot slide the highlight along the line.
///
/// A single decoded string with a range would land wrong only on lines
/// containing an invalid byte, which are exactly the lines somebody searches
/// for when something has gone wrong.
#[test]
fn the_highlight_is_cut_from_the_bytes_the_daemon_measured() {
    // One invalid byte, then the word. Decoding first would expand it to a
    // three-byte replacement character and push the offsets along by two.
    let mut visible = vec![0xffu8];
    visible.extend_from_slice(b" error here");
    let start = 2u32;
    let end = 7u32;
    let split = split_hit(&visible, start, end);
    assert_eq!(split.matched, "error");
    assert_eq!(split.after, " here");
}

/// The height the sheet asks for follows the hits it holds. A measurement that
/// ignored them would make the fit test above assert nothing.
#[test]
fn the_height_follows_the_number_of_hits() {
    assert!(content(Some(&answer(50))).1 > content(Some(&answer(5))).1);
    assert!(content(Some(&answer(5))).1 > content(None).1);
}
