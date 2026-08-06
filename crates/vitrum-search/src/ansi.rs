//! Escape-sequence stripping, and the map back to the original bytes.
//!
//! # Why this exists
//!
//! A scrollback ring holds what the program wrote, not what the screen shows.
//! An agent printing an error prints
//!
//! ```text
//! \x1b[1;31merror\x1b[0m: build failed
//! ```
//!
//! and a search for `error` that scans the raw bytes finds it here only by
//! luck — the luck being that the escape happened to land before the word
//! rather than inside it. `\x1b[31me\x1b[0mrror` is equally legal output and a
//! raw scan misses it entirely. Colour is not decoration to a byte matcher; it
//! is noise inserted at arbitrary positions inside words.
//!
//! So matching runs on the *visible* text. But a hit has to report where it is
//! in the session's stream and hand back bytes the client can render with their
//! colour intact, and both of those are properties of the *original* bytes.
//! That is the whole job of this module: produce the visible text, and produce
//! an exact map from any offset in it back to the byte it came from.
//!
//! # The map
//!
//! Stripping removes spans, so the surviving text is a sequence of runs, each
//! contiguous in both coordinate systems:
//!
//! ```text
//! original  \x1b[1;31m e r r o r \x1b[0m : ' ' b u i l d
//!           |________| |_______| |____|
//!            removed     run 0    removed  run 1 ...
//! visible               e r r o r          :  ' ' b u i l d
//! ```
//!
//! [`Map`] stores those runs and binary-searches them. That is a handful of
//! entries for a typical line, versus one `u32` per byte for a dense table, and
//! the lookup happens only for bytes that actually matched.
//!
//! # What counts as invisible
//!
//! Exactly three things, chosen so the fast path and the slow path can never
//! disagree:
//!
//! - escape sequences introduced by `ESC` (0x1B), in every form below;
//! - carriage return (0x0D);
//! - delete (0x7F).
//!
//! Everything else, including TAB, BEL and backspace, is kept verbatim. The
//! test for "does this line need stripping at all" is therefore a single
//! `memchr3` over those three bytes, which is one SIMD pass and lets the
//! overwhelming majority of lines skip stripping entirely and match in place.
//!
//! Carriage return is removed rather than kept for two reasons: a CRLF stream
//! would otherwise put a `\r` between the last word and end-of-line, breaking
//! every `$`-anchored regex, and a progress bar's `\r` would break words apart.
//!
//! # What this deliberately is not
//!
//! Not a terminal emulator. `50%\r100%` becomes `50%100%` here, where a real
//! terminal would show `100%`, so a search can match text that was overwritten
//! and never simultaneously visible. Resolving that means replaying cursor
//! motion into a grid — which is `vitrum-grid`'s job, needs
//! the screen width, and cannot be done on a raw ring in one pass. Stripping is
//! the honest 99% and the failure mode is a rare extra hit, not a missed one.
//!
//! # Sequences recognised
//!
//! | Form | Introducer | Terminator |
//! |---|---|---|
//! | CSI (SGR, cursor motion, erase) | `ESC [` | a byte in `0x40..=0x7E` |
//! | OSC (title, hyperlink, vitrum's own OSC 7373 hints) | `ESC ]` | `BEL`, or `ESC \` |
//! | DCS / SOS / PM / APC | `ESC P` `X` `^` `_` | `BEL`, or `ESC \` |
//! | Escape with intermediates (`ESC ( B`) | `ESC` + `0x20..=0x2F` | a byte in `0x30..=0x7E` |
//! | Two-byte escape (`ESC 7`, `ESC =`, `ESC M`) | `ESC` | the second byte |
//!
//! Any sequence in progress is aborted by `CAN` (0x18) or `SUB` (0x1A), which
//! a terminal consumes along with the sequence, and by a nested `ESC`, which it
//! does not — the `ESC` starts the next sequence and must be re-dispatched
//! rather than swallowed. A producer that emits a partial `ESC [ 3` and then
//! changes its mind and emits a full reset must still get the reset stripped.
//!
//! Single-byte C1 controls (0x80..=0x9F) are **not** treated as introducers.
//! In a UTF-8 stream those bytes are continuation bytes of ordinary characters,
//! and stripping them would corrupt every non-ASCII line.
//!

use std::ops::Range;

use memchr::memchr3;

/// Escape.
pub const ESC: u8 = 0x1b;
/// Bell, which also terminates a string sequence.
const BEL: u8 = 0x07;
/// Cancel: aborts a sequence in progress.
const CAN: u8 = 0x18;
/// Substitute: aborts a sequence in progress.
const SUB: u8 = 0x1a;
/// Carriage return.
const CR: u8 = 0x0d;
/// Delete.
const DEL: u8 = 0x7f;

/// One surviving span, contiguous in both the visible and the original text.
///
/// `visible[visible .. visible + len] == original[original .. original + len]`.
///
/// Offsets are `u32`, which caps a single line at 4 GiB. A scrollback line is
/// bounded by the ring, and a 4 GiB line has already lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    /// Start of the span in the visible text.
    pub visible: u32,
    /// Start of the same span in the original bytes.
    pub original: u32,
    /// Length of the span.
    pub len: u32,
}

/// How to translate a visible offset back into an original one.
#[derive(Debug, Clone, Copy)]
pub enum Map<'a> {
    /// Nothing was removed: the two coordinate systems are the same.
    Identity,
    /// Escapes were removed; translate through these runs.
    Runs(&'a [Run]),
}

impl Map<'_> {
    /// Original offset of the byte at visible offset `visible`.
    ///
    /// A `visible` equal to the length of the visible text maps to one past the
    /// last surviving byte, which keeps [`Map::range`] total.
    pub fn start(&self, visible: usize) -> usize {
        let runs = match self {
            Map::Identity => return visible,
            Map::Runs(runs) => runs,
        };
        let Some(last) = runs.last() else {
            // Nothing survived stripping, so every visible offset is zero-length
            // and the only honest answer is the start of the line.
            return 0;
        };
        if visible >= last.visible as usize + last.len as usize {
            return last.original as usize + last.len as usize;
        }
        let index = match runs.binary_search_by_key(&visible, |run| run.visible as usize) {
            Ok(index) => index,
            // `Err(0)` cannot happen: run 0 always starts at visible offset 0,
            // so any non-negative offset has a run at or before it.
            Err(index) => index.saturating_sub(1),
        };
        let run = runs[index];
        run.original as usize + (visible - run.visible as usize)
    }

    /// Original byte range covering the visible range `range`.
    ///
    /// The end is computed from the *last matched byte* rather than from the
    /// exclusive end offset. Mapping the exclusive end directly would jump over
    /// any escape sequence sitting immediately after the match and report a
    /// range that swallows it — so highlighting `error` in
    /// `error\x1b[0m: failed` would light up the reset sequence too.
    pub fn range(&self, range: Range<usize>) -> Range<usize> {
        let start = self.start(range.start);
        if range.end <= range.start {
            return start..start;
        }
        let end = self.start(range.end - 1) + 1;
        start..end.max(start)
    }
}

/// Would stripping change this line?
///
/// One `memchr3` pass. A `false` answer lets the caller match against the
/// original bytes with [`Map::Identity`], which is the common case and costs
/// nothing beyond this check.
#[inline]
pub fn needs_stripping(line: &[u8]) -> bool {
    memchr3(ESC, CR, DEL, line).is_some()
}

/// Reusable scratch for stripping one line at a time.
///
/// Both buffers are cleared and refilled per line, never reallocated, which is
/// what keeps a full-ring scan at zero allocations per line.
#[derive(Debug, Default)]
pub struct Stripper {
    text: Vec<u8>,
    runs: Vec<Run>,
}

impl Stripper {
    pub fn new() -> Self {
        Self::default()
    }

    /// Strip `line` into the scratch.
    pub fn fill(&mut self, line: &[u8]) {
        self.text.clear();
        self.runs.clear();

        let mut index = 0usize;
        while index < line.len() {
            // Copy everything up to the next removable byte in one go, so a
            // long uncoloured stretch costs one memchr and one memcpy rather
            // than a per-byte loop.
            let next = match memchr3(ESC, CR, DEL, &line[index..]) {
                Some(offset) => index + offset,
                None => line.len(),
            };
            if next > index {
                self.runs.push(Run {
                    visible: self.text.len() as u32,
                    original: index as u32,
                    len: (next - index) as u32,
                });
                self.text.extend_from_slice(&line[index..next]);
                index = next;
            }
            if index >= line.len() {
                break;
            }
            index = if line[index] == ESC {
                skip_escape(line, index)
            } else {
                // CR or DEL: one byte, removed.
                index + 1
            };
        }
    }

    /// The visible text of the last [`Stripper::fill`].
    pub fn text(&self) -> &[u8] {
        &self.text
    }

    /// The map back to the original bytes.
    pub fn map(&self) -> Map<'_> {
        Map::Runs(&self.runs)
    }
}

/// Index just past the escape sequence starting at `start`.
///
/// Always returns at least `start + 1`, so a caller looping on it cannot spin.
/// An unterminated sequence consumes to end of line, which is the right call:
/// the terminal would have swallowed those bytes too.
fn skip_escape(line: &[u8], start: usize) -> usize {
    debug_assert_eq!(line[start], ESC);
    let mut index = start + 1;
    let Some(&introducer) = line.get(index) else {
        // A lone ESC at end of line. Removed; there is nothing to terminate.
        return index;
    };

    match introducer {
        b'[' => {
            index += 1;
            while let Some(&byte) = line.get(index) {
                match byte {
                    0x40..=0x7e => return index + 1,
                    // A nested ESC means the producer abandoned this sequence.
                    // Stop *before* it and let the caller re-dispatch, rather
                    // than swallowing the real sequence that follows.
                    ESC => return index,
                    // CAN and SUB abort the sequence and are themselves
                    // consumed by a terminal, so they are part of what the
                    // user never saw.
                    CAN | SUB => return index + 1,
                    _ => index += 1,
                }
            }
            index
        }
        b']' | b'P' | b'X' | b'^' | b'_' => {
            index += 1;
            while let Some(&byte) = line.get(index) {
                match byte {
                    BEL | CAN | SUB => return index + 1,
                    ESC => {
                        return if line.get(index + 1) == Some(&b'\\') {
                            index + 2
                        } else {
                            index
                        };
                    }
                    _ => index += 1,
                }
            }
            index
        }
        // A stray string terminator with no string.
        b'\\' => index + 1,
        // Intermediates, then a final byte: `ESC ( B`, `ESC # 8`.
        0x20..=0x2f => {
            index += 1;
            while matches!(line.get(index), Some(0x20..=0x2f)) {
                index += 1;
            }
            if matches!(line.get(index), Some(0x30..=0x7e)) {
                index + 1
            } else {
                index
            }
        }
        // Two-byte escapes: `ESC 7`, `ESC =`, `ESC M`.
        0x30..=0x7e => index + 1,
        // ESC followed by a control byte is not a sequence. The ESC is dropped;
        // the control byte is left for the main loop to deal with.
        _ => index,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(line: &[u8]) -> (Vec<u8>, Vec<Run>) {
        let mut stripper = Stripper::new();
        stripper.fill(line);
        let runs = match stripper.map() {
            Map::Runs(runs) => runs.to_vec(),
            Map::Identity => Vec::new(),
        };
        (stripper.text().to_vec(), runs)
    }

    fn visible(line: &[u8]) -> String {
        String::from_utf8(strip(line).0).expect("visible text must stay UTF-8")
    }

    /// Locks out the fast-path predicate disagreeing with the stripper. If
    /// `needs_stripping` says no for a line the stripper would change, that
    /// line is matched against raw bytes with an identity map and every offset
    /// it reports is wrong.
    #[test]
    fn fast_path_predicate_agrees_with_the_stripper() {
        let cases: &[&[u8]] = &[
            b"plain text",
            b"",
            b"\x1b[31mred\x1b[0m",
            b"crlf\r",
            b"mid\rline",
            b"del\x7fete",
            b"tab\there",
            b"bell\x07here",
            "unicode \u{e9}\u{4e2d}".as_bytes(),
        ];
        for case in cases {
            let stripped = strip(case).0;
            assert_eq!(
                needs_stripping(case),
                stripped != *case,
                "predicate disagreed on {:?}",
                String::from_utf8_lossy(case)
            );
        }
    }

    /// Locks out SGR colour codes surviving into the matched text, which is the
    /// entire reason this module exists: a search for `error` must find a line
    /// printed in red.
    #[test]
    fn sgr_colour_is_removed_from_the_visible_text() {
        assert_eq!(
            visible(b"\x1b[1;31merror\x1b[0m: build failed"),
            "error: build failed"
        );
        assert_eq!(visible(b"\x1b[38;2;255;0;0mtruecolour\x1b[m"), "truecolour");
    }

    /// Locks out an escape *inside* a word defeating the match. This is the
    /// case a raw byte scan cannot ever handle and the one that makes stripping
    /// mandatory rather than nice to have.
    #[test]
    fn escape_inside_a_word_is_removed_and_the_word_rejoins() {
        assert_eq!(visible(b"\x1b[31me\x1b[0mrror"), "error");
        assert_eq!(visible(b"O\x1b[33mO\x1b[0mM"), "OOM");
    }

    /// Locks out the offset map drifting after several escapes on one line.
    /// This is the exact failure Main called out: a hit must report the seq of
    /// the ORIGINAL byte, not the stripped one.
    #[test]
    fn offset_map_survives_several_sgr_runs_on_one_line() {
        //             0123456789...
        let line = b"\x1b[1m[\x1b[0m\x1b[32mok\x1b[0m\x1b[1m]\x1b[0m error here";
        let (text, runs) = strip(line);
        assert_eq!(text, b"[ok] error here");

        let map = Map::Runs(&runs);
        // Every visible byte must map to the identical original byte.
        for (index, byte) in text.iter().enumerate() {
            let original = map.start(index);
            assert_eq!(
                line[original], *byte,
                "visible byte {index} ({:?}) mapped to original {original} ({:?})",
                *byte as char, line[original] as char
            );
        }

        // And the specific offset of `error`, computed by hand:
        // \x1b[1m = 4, '[' = 1, \x1b[0m = 4, \x1b[32m = 5, "ok" = 2,
        // \x1b[0m = 4, \x1b[1m = 4, ']' = 1, \x1b[0m = 4, ' ' = 1  -> 30.
        let visible_start = text
            .windows(5)
            .position(|w| w == b"error")
            .expect("error is present");
        assert_eq!(visible_start, 5);
        assert_eq!(map.start(visible_start), 30);
        assert_eq!(&line[30..35], b"error");
    }

    /// Locks out the mapped range swallowing the reset sequence that follows a
    /// match. `error\x1b[0m` must highlight five bytes, not nine.
    #[test]
    fn mapped_range_stops_at_the_last_matched_byte() {
        let line = b"\x1b[31merror\x1b[0m: failed";
        let (text, runs) = strip(line);
        assert_eq!(text, b"error: failed");
        let map = Map::Runs(&runs);
        assert_eq!(map.range(0..5), 5..10);
        assert_eq!(&line[5..10], b"error");
    }

    /// Locks out the identity map being anything other than the identity, which
    /// would corrupt every hit on an uncoloured line — the common case.
    #[test]
    fn identity_map_is_the_identity() {
        let map = Map::Identity;
        assert_eq!(map.start(0), 0);
        assert_eq!(map.start(17), 17);
        assert_eq!(map.range(3..9), 3..9);
        assert_eq!(map.range(4..4), 4..4);
    }

    /// Locks out an empty range mapping to a non-empty one, which would make a
    /// zero-width regex match report bytes it did not match.
    #[test]
    fn empty_visible_range_maps_to_an_empty_original_range() {
        let line = b"\x1b[31mabc";
        let (_, runs) = strip(line);
        let map = Map::Runs(&runs);
        assert_eq!(map.range(1..1), 6..6);
    }

    /// Locks out a line that is nothing but escapes producing a bogus map. It
    /// happens constantly: a clear-screen or a cursor-home on its own line.
    #[test]
    fn a_line_of_pure_escapes_has_empty_visible_text() {
        let (text, runs) = strip(b"\x1b[2J\x1b[H\x1b[?25l");
        assert!(text.is_empty());
        assert!(runs.is_empty());
        let map = Map::Runs(&runs);
        assert_eq!(map.start(0), 0);
        assert_eq!(map.range(0..0), 0..0);
    }

    /// Locks out OSC sequences leaking their payload into the visible text. A
    /// window title or a hyperlink URL would otherwise be searchable content
    /// the user never saw, and vitrum's own OSC 7373 hints would match too.
    #[test]
    fn osc_payloads_are_removed_under_both_terminators() {
        assert_eq!(visible(b"\x1b]0;window title\x07after"), "after");
        assert_eq!(visible(b"\x1b]0;window title\x1b\\after"), "after");
        assert_eq!(
            visible(b"before\x1b]7373;approval;may I force-push?\x1b\\after"),
            "beforeafter"
        );
    }

    /// Locks out an OSC-8 hyperlink's URL being searchable while its label is
    /// not. The label is what the user sees.
    #[test]
    fn osc_hyperlink_keeps_the_label_and_drops_the_url() {
        let line = b"\x1b]8;;https://example.com/oom\x1b\\click here\x1b]8;;\x1b\\";
        assert_eq!(visible(line), "click here");
    }

    /// Locks out DCS, SOS, PM and APC strings being treated as ordinary text,
    /// which would dump terminfo query payloads into search results.
    #[test]
    fn other_string_sequences_are_removed() {
        assert_eq!(visible(b"a\x1bP1$r0m\x1b\\b"), "ab");
        assert_eq!(visible(b"a\x1bXsomething\x1b\\b"), "ab");
        assert_eq!(visible(b"a\x1b^private\x1b\\b"), "ab");
        assert_eq!(visible(b"a\x1b_appprog\x1b\\b"), "ab");
    }

    /// Locks out charset-selection and other intermediate-byte escapes leaving
    /// a stray final character like `B` in the text.
    #[test]
    fn escapes_with_intermediate_bytes_are_fully_consumed() {
        assert_eq!(visible(b"\x1b(Bplain"), "plain");
        assert_eq!(visible(b"\x1b#8filled"), "filled");
        assert_eq!(visible(b"\x1b%Gutf8"), "utf8");
    }

    /// Locks out two-byte escapes leaving their second byte behind. `ESC 7` is
    /// save-cursor and would otherwise leave a `7` in the middle of a line.
    #[test]
    fn two_byte_escapes_leave_nothing_behind() {
        assert_eq!(visible(b"a\x1b7b\x1b8c"), "abc");
        assert_eq!(visible(b"a\x1bMb"), "ab");
        assert_eq!(visible(b"a\x1b=b\x1b>c"), "abc");
    }

    /// Locks out an unterminated CSI at end of line spilling into the next
    /// search or panicking on an out-of-bounds read. A ring boundary can cut a
    /// line anywhere.
    #[test]
    fn unterminated_sequences_consume_to_end_of_line() {
        assert_eq!(visible(b"text\x1b[38;2;255"), "text");
        assert_eq!(visible(b"text\x1b]0;unterminated"), "text");
        assert_eq!(visible(b"text\x1b"), "text");
        assert_eq!(visible(b"text\x1b["), "text");
    }

    /// Locks out an abandoned CSI swallowing the real sequence that follows it.
    /// A producer that emits `ESC [ 3` and then changes its mind and emits a
    /// full reset must still get the reset stripped.
    #[test]
    fn an_abandoned_sequence_does_not_swallow_the_next_one() {
        assert_eq!(visible(b"a\x1b[3\x1b[0mb"), "ab");
        assert_eq!(visible(b"a\x1b[3\x18b"), "ab");
    }

    /// Locks out CR surviving. A CRLF stream would put `\r` before end of line
    /// and break every `$`-anchored regex; a progress bar would split words.
    #[test]
    fn carriage_returns_are_removed() {
        assert_eq!(visible(b"error\r"), "error");
        assert_eq!(visible(b"50%\r100%"), "50%100%");
    }

    /// Locks out DEL surviving into the visible text, where it renders as
    /// nothing but breaks a word for the matcher.
    #[test]
    fn delete_bytes_are_removed() {
        assert_eq!(visible(b"err\x7for"), "error");
    }

    /// Locks out TAB or BEL being stripped. TAB is real layout that a user may
    /// search for, and removing it would change column alignment in the
    /// returned context.
    #[test]
    fn tab_and_bell_are_kept() {
        assert_eq!(visible(b"a\tb"), "a\tb");
        assert_eq!(visible(b"a\x07b"), "a\u{7}b");
    }

    /// Locks out C1 bytes being treated as escape introducers. In UTF-8, every
    /// byte in 0x80..=0x9F is a continuation byte, so stripping them would
    /// mangle every accented or CJK line in the scrollback.
    #[test]
    fn utf8_continuation_bytes_are_never_treated_as_controls() {
        let line = "caf\u{e9} \u{4e2d}\u{6587} \u{1f600}".as_bytes();
        assert!(!needs_stripping(line));
        assert_eq!(visible(line), "caf\u{e9} \u{4e2d}\u{6587} \u{1f600}");

        let coloured = "\x1b[32mcaf\u{e9}\x1b[0m".as_bytes();
        assert_eq!(visible(coloured), "caf\u{e9}");
    }

    /// Locks out the map pointing into the middle of a multi-byte character.
    /// A hit whose seq lands mid-character makes the client render a
    /// replacement glyph and misreport the position.
    #[test]
    fn map_lands_on_character_starts_for_multibyte_text() {
        let line = "\x1b[32mcaf\u{e9} \u{4e2d}\u{6587}\x1b[0m".as_bytes();
        let (text, runs) = strip(line);
        let map = Map::Runs(&runs);
        let visible_text = std::str::from_utf8(&text).expect("utf8");
        for (index, _) in visible_text.char_indices() {
            let original = map.start(index);
            assert!(
                std::str::from_utf8(&line[original..]).is_ok(),
                "offset {original} is not a character boundary"
            );
        }
    }

    /// Locks out runs being emitted for removed spans or being non-maximal.
    /// A wrong run table is a wrong map, and the map is only ever exercised on
    /// bytes that matched, so a subtly wrong one hides until it matters.
    #[test]
    fn runs_describe_exactly_the_surviving_spans() {
        let line = b"\x1b[31mab\x1b[0mcd";
        let (text, runs) = strip(line);
        assert_eq!(text, b"abcd");
        assert_eq!(
            runs,
            vec![
                Run {
                    visible: 0,
                    original: 5,
                    len: 2
                },
                Run {
                    visible: 2,
                    original: 11,
                    len: 2
                },
            ]
        );
        for run in &runs {
            let original = run.original as usize;
            let visible = run.visible as usize;
            let len = run.len as usize;
            assert_eq!(
                &line[original..original + len],
                &text[visible..visible + len]
            );
        }
    }

    /// Locks out the scratch keeping stale bytes between lines. Reusing the
    /// buffer without clearing appends line N+1 to line N and produces matches
    /// that span lines that were never adjacent.
    #[test]
    fn scratch_is_reset_between_lines() {
        let mut stripper = Stripper::new();
        stripper.fill(b"\x1b[31mfirst\x1b[0m");
        assert_eq!(stripper.text(), b"first");
        stripper.fill(b"\x1b[32msecond\x1b[0m");
        assert_eq!(stripper.text(), b"second");
        stripper.fill(b"\x1b[2J");
        assert_eq!(stripper.text(), b"");
        match stripper.map() {
            Map::Runs(runs) => assert!(runs.is_empty()),
            Map::Identity => panic!("stripper always reports a run map"),
        }
    }

    /// Locks out the stripper reallocating on every line, which is the whole
    /// no-allocation-per-line requirement in miniature.
    #[test]
    fn scratch_capacity_is_reused_not_regrown() {
        let mut stripper = Stripper::new();
        let line = b"\x1b[31m0123456789012345678901234567890123456789\x1b[0m";
        stripper.fill(line);
        let text_capacity = stripper.text.capacity();
        let runs_capacity = stripper.runs.capacity();
        assert!(text_capacity > 0);
        for _ in 0..1000 {
            stripper.fill(line);
        }
        assert_eq!(stripper.text.capacity(), text_capacity);
        assert_eq!(stripper.runs.capacity(), runs_capacity);
    }

    /// Locks out `skip_escape` ever failing to advance, which would spin the
    /// stripper forever on a hostile line and hang the daemon.
    #[test]
    fn skip_escape_always_makes_progress() {
        for second in 0u8..=255 {
            let line = [ESC, second, b'x'];
            assert!(
                skip_escape(&line, 0) > 0,
                "no progress on ESC {second:#04x}"
            );
        }
        assert_eq!(skip_escape(&[ESC], 0), 1);
    }

    /// Locks out a hostile stream of escape bytes panicking or looping. Any
    /// program can print any bytes and a coding agent prints other programs'
    /// output verbatim.
    #[test]
    fn arbitrary_byte_soup_terminates_without_panicking() {
        let mut line = Vec::new();
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..20_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let byte = (state & 0xff) as u8;
            line.push(if byte.is_multiple_of(5) { ESC } else { byte });
        }
        let mut stripper = Stripper::new();
        stripper.fill(&line);
        // The visible text can never be longer than the input.
        assert!(stripper.text().len() <= line.len());
        let map = stripper.map();
        for index in 0..stripper.text().len() {
            assert!(map.start(index) < line.len());
        }
    }
}
