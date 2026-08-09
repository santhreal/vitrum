//! End-to-end searches over scrollback shaped like a real one.
//!
//! Every assertion here is an exact value: an exact `seq`, exact line bytes,
//! exact context. A test that only checks "we found something" cannot tell a
//! correct offset from one that is four bytes into an SGR introducer, and that
//! four-byte error is the entire class of bug this crate has to avoid.

use vitrum_search::{ContextLine, Haystack, Pattern, Query, search};

mod corpus;

use corpus::mixed_scrollback;

/// What a haystack's chunk list looks like: a slice of byte slices.
///
/// Named because the bare type is unreadable inline and every multi-session
/// test needs several of them.
type Chunks<'a> = &'a [&'a [u8]];

/// Wrap one contiguous buffer as a haystack.
fn one<'a>(session: u64, base_seq: u64, chunks: &'a [&'a [u8]]) -> Haystack<'a> {
    Haystack {
        session,
        base_seq,
        chunks,
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn context_text(lines: &[ContextLine]) -> Vec<String> {
    lines.iter().map(|line| text(&line.bytes)).collect()
}

/// Locks out a search missing a match because the line is coloured. This is
/// the headline requirement: `error` printed red must be findable, and the
/// returned bytes must still be red so the client renders it unchanged.
#[test]
fn a_match_on_an_sgr_coloured_line_is_found_with_exact_offsets() {
    //                     0..............................
    let data: &[u8] = b"cargo build\n\x1b[1;31merror\x1b[0m: linker killed\nexit 137\n";
    // Line 1 starts at byte 12. `\x1b[1;31m` is 7 bytes, so `error` is at 19.
    assert_eq!(&data[12..19], b"\x1b[1;31m");
    assert_eq!(&data[19..24], b"error");

    let chunks: &[&[u8]] = &[data];
    let results =
        search(&Query::literal("error").context(1), &[one(3, 0, chunks)]).expect("search");

    assert_eq!(results.len(), 1);
    let hit = &results.hits[0];
    assert_eq!(hit.session, 3);
    assert_eq!(hit.line_seq, 12);
    assert_eq!(hit.match_seq, 19);
    assert_eq!(hit.line_index, 1);
    assert_eq!(hit.original_range, 7..12);
    assert_eq!(hit.visible_range, 0..5);
    assert_eq!(hit.visible_lossy(), "error: linker killed");
    assert_eq!(hit.line, b"\x1b[1;31merror\x1b[0m: linker killed");
    assert_eq!(hit.matched_text(), "error");
    assert_eq!(context_text(&hit.before), vec!["cargo build"]);
    assert_eq!(context_text(&hit.after), vec!["exit 137"]);
    assert_eq!(hit.before[0].seq, 0);
    // 12 + 7 (SGR) + 5 (error) + 4 (reset) + 15 (": linker killed") + 1 (LF).
    assert_eq!(hit.after[0].seq, 44);
    assert_eq!(&data[44..52], b"exit 137");
}

/// Locks out the offset map drifting when the match sits after several
/// independent SGR runs, rather than after a single leading one. Main called
/// this out by name: the reported seq must be the ORIGINAL byte.
#[test]
fn a_match_after_several_sgr_runs_reports_the_original_byte() {
    let data: &[u8] =
        b"\x1b[2m[\x1b[0m\x1b[32m ok \x1b[0m\x1b[2m]\x1b[0m allocation failed: OOM killer\n";
    let chunks: &[&[u8]] = &[data];
    let results = search(&Query::literal("OOM"), &[one(1, 0, chunks)]).expect("search");

    assert_eq!(results.len(), 1);
    let hit = &results.hits[0];
    assert_eq!(hit.visible_lossy(), "[ ok ] allocation failed: OOM killer");
    assert_eq!(hit.visible_range, 26..29);

    // Five escape sequences precede the visible text, 4 + 4 + 5 + 4 + 4 + 4 = 25
    // bytes of them, so visible offset 26 is original offset 51.
    assert_eq!(hit.match_seq, 51);
    assert_eq!(&data[51..54], b"OOM");
    assert_eq!(hit.original_range, 51..54);
    assert_eq!(hit.matched_text(), "OOM");
}

/// Locks out a match being missed because a colour change lands in the middle
/// of the word. A raw byte scan cannot find this at all, and it is legal
/// output that any progress-colouring program produces.
#[test]
fn a_match_split_by_an_escape_inside_the_word_is_found() {
    let data: &[u8] = b"status: O\x1b[33mO\x1b[0mM detected\n";
    let chunks: &[&[u8]] = &[data];
    let results = search(&Query::literal("OOM"), &[one(1, 0, chunks)]).expect("search");

    assert_eq!(results.len(), 1);
    let hit = &results.hits[0];
    assert_eq!(hit.visible_lossy(), "status: OOM detected");
    assert_eq!(hit.visible_range, 8..11);
    // The match starts at the `O` before the escape, at original offset 8, and
    // ends at the `M` after it, at original offset 19.
    assert_eq!(hit.match_seq, 8);
    assert_eq!(hit.original_range, 8..20);
    assert_eq!(&data[8..20], b"O\x1b[33mO\x1b[0mM");
}

/// Locks out a match being lost at the ring seam. The two halves each contain
/// part of the word and neither contains the match, so a per-chunk search
/// finds nothing.
#[test]
fn a_match_spanning_a_chunk_boundary_is_found_with_the_right_offset() {
    let whole: &[u8] = b"line one\nallocation failed: OOM killer\nline three\n";
    // Cut between the first and second byte of OOM.
    let cut = 29;
    assert_eq!(&whole[cut - 1..cut + 1], b"OO");
    let head = &whole[..cut];
    let tail = &whole[cut..];
    assert!(!head.windows(3).any(|w| w == b"OOM"));
    assert!(!tail.windows(3).any(|w| w == b"OOM"));

    let chunks: &[&[u8]] = &[head, tail];
    let results = search(&Query::literal("OOM").context(1), &[one(5, 0, chunks)]).expect("search");

    assert_eq!(results.len(), 1);
    let hit = &results.hits[0];
    assert_eq!(hit.line_seq, 9);
    assert_eq!(hit.match_seq, 28);
    assert_eq!(&whole[28..31], b"OOM");
    assert_eq!(hit.line, b"allocation failed: OOM killer");
    assert_eq!(hit.visible_lossy(), "allocation failed: OOM killer");
    assert_eq!(context_text(&hit.before), vec!["line one"]);
    assert_eq!(context_text(&hit.after), vec!["line three"]);
}

/// Locks out an escape sequence cut by the ring seam being mis-stripped, which
/// would leave `[31m` in the visible text and break the match after it.
#[test]
fn an_escape_sequence_split_across_the_boundary_is_still_stripped() {
    let whole: &[u8] = b"before\n\x1b[1;31mfatal error here\x1b[0m\nafter\n";
    // Cut inside `\x1b[1;31m`, which starts at byte 7.
    let cut = 10;
    assert_eq!(&whole[7..14], b"\x1b[1;31m");
    let chunks: &[&[u8]] = &[&whole[..cut], &whole[cut..]];
    let results = search(&Query::literal("fatal error"), &[one(1, 0, chunks)]).expect("search");

    assert_eq!(results.len(), 1);
    let hit = &results.hits[0];
    assert_eq!(hit.visible_lossy(), "fatal error here");
    assert_eq!(hit.line_seq, 7);
    assert_eq!(hit.match_seq, 14);
    assert_eq!(&whole[14..25], b"fatal error");
}

/// Locks out a UTF-8 character split by the ring seam corrupting the line, and
/// out of the mapped offset landing mid-character.
#[test]
fn utf8_split_across_the_boundary_reassembles_and_stays_searchable() {
    let whole = "prefix\n\u{4e2d}\u{6587} OOM \u{1f600} tail\nsuffix\n".as_bytes();
    // First CJK character starts at byte 7 and is three bytes long; cut it.
    let cut = 8;
    assert!(std::str::from_utf8(&whole[..cut]).is_err());
    let chunks: &[&[u8]] = &[&whole[..cut], &whole[cut..]];

    let results = search(&Query::literal("OOM"), &[one(1, 0, chunks)]).expect("search");
    assert_eq!(results.len(), 1);
    let hit = &results.hits[0];
    assert_eq!(hit.visible_lossy(), "\u{4e2d}\u{6587} OOM \u{1f600} tail");
    assert_eq!(hit.line, "\u{4e2d}\u{6587} OOM \u{1f600} tail".as_bytes());
    // Two three-byte characters plus a space precede it.
    assert_eq!(hit.visible_range, 7..10);
    assert_eq!(hit.match_seq, 14);
    assert_eq!(&whole[14..17], b"OOM");
}

/// Locks out a match being found on text that spans a line break. Two lines
/// that happen to end and begin with the halves of a word are not a match, and
/// treating the buffer as one string would say they are.
#[test]
fn a_match_never_spans_a_newline() {
    let data: &[u8] = b"ends with OO\nM starts here\n";
    let chunks: &[&[u8]] = &[data];
    let results = search(&Query::literal("OOM"), &[one(1, 0, chunks)]).expect("search");
    assert!(results.is_empty());
}

/// Locks out `base_seq` being ignored, which would report every hit in a ring
/// that has already wrapped at an offset the client cannot resolve.
#[test]
fn base_seq_offsets_every_reported_position() {
    let data: &[u8] = b"first\nOOM here\nlast\n";
    let chunks: &[&[u8]] = &[data];
    let base = 1_000_000u64;
    let results =
        search(&Query::literal("OOM").context(1), &[one(9, base, chunks)]).expect("search");

    let hit = &results.hits[0];
    assert_eq!(hit.line_seq, base + 6);
    assert_eq!(hit.match_seq, base + 6);
    assert_eq!(hit.before[0].seq, base);
    assert_eq!(hit.after[0].seq, base + 15);
}

/// Locks out context being taken from the wrong side, or the before-context
/// coming back newest-first. Both make the result unreadable next to the
/// scrollback it came from.
#[test]
fn context_is_ordered_oldest_first_on_both_sides() {
    let data: &[u8] = b"l0\nl1\nl2\nTARGET\nl4\nl5\nl6\n";
    let chunks: &[&[u8]] = &[data];
    let results =
        search(&Query::literal("TARGET").context(2), &[one(1, 0, chunks)]).expect("search");

    let hit = &results.hits[0];
    assert_eq!(context_text(&hit.before), vec!["l1", "l2"]);
    assert_eq!(context_text(&hit.after), vec!["l4", "l5"]);
    assert_eq!(
        hit.before.iter().map(|l| l.index).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        hit.after.iter().map(|l| l.index).collect::<Vec<_>>(),
        vec![4, 5]
    );
    assert_eq!(hit.line_index, 3);
}

/// Locks out context running off the ends of the buffer, either by panicking
/// or by padding with empty lines the scrollback does not contain.
#[test]
fn context_at_the_edges_is_short_rather_than_padded() {
    let data: &[u8] = b"TARGET\nonly one after\n";
    let chunks: &[&[u8]] = &[data];
    let results =
        search(&Query::literal("TARGET").context(5), &[one(1, 0, chunks)]).expect("search");

    let hit = &results.hits[0];
    assert!(hit.before.is_empty());
    assert_eq!(context_text(&hit.after), vec!["only one after"]);
}

/// Locks out context lines being stripped of their escapes. The client renders
/// them next to the hit and they must look like the scrollback.
#[test]
fn context_lines_keep_their_original_bytes() {
    let data: &[u8] = b"\x1b[36mbuilding\x1b[0m\nTARGET\n\x1b[32mdone\x1b[0m\n";
    let chunks: &[&[u8]] = &[data];
    let results =
        search(&Query::literal("TARGET").context(1), &[one(1, 0, chunks)]).expect("search");

    let hit = &results.hits[0];
    assert_eq!(hit.before[0].bytes, b"\x1b[36mbuilding\x1b[0m");
    assert_eq!(hit.after[0].bytes, b"\x1b[32mdone\x1b[0m");
}

/// Locks out context being lost when the context line itself straddles the
/// ring seam, which would silently return a truncated half-line.
#[test]
fn context_lines_that_straddle_the_boundary_are_whole() {
    let whole: &[u8] = b"context-before-line\nTARGET\ncontext-after-line\n";
    // Cut inside the before-context line.
    let chunks: &[&[u8]] = &[&whole[..10], &whole[10..30], &whole[30..]];
    let results =
        search(&Query::literal("TARGET").context(1), &[one(1, 0, chunks)]).expect("search");

    let hit = &results.hits[0];
    assert_eq!(context_text(&hit.before), vec!["context-before-line"]);
    assert_eq!(context_text(&hit.after), vec!["context-after-line"]);
}

/// Locks out results ordering depending on the order haystacks were supplied.
/// A client that iterates a hash map of sessions must get a stable answer.
#[test]
fn ordering_is_by_session_then_position_regardless_of_input_order() {
    let a: &[u8] = b"x\nOOM one\nOOM two\n";
    let b: &[u8] = b"OOM three\n";
    let c: &[u8] = b"y\ny\nOOM four\n";
    let ca: Chunks<'_> = &[a];
    let cb: Chunks<'_> = &[b];
    let cc: Chunks<'_> = &[c];

    let forward = search(
        &Query::literal("OOM").context(0),
        &[one(1, 0, ca), one(2, 0, cb), one(3, 0, cc)],
    )
    .expect("search");
    let shuffled = search(
        &Query::literal("OOM").context(0),
        &[one(3, 0, cc), one(1, 0, ca), one(2, 0, cb)],
    )
    .expect("search");

    let key = |r: &vitrum_search::SearchResults| {
        r.hits
            .iter()
            .map(|h| (h.session, h.match_seq))
            .collect::<Vec<_>>()
    };
    assert_eq!(key(&forward), vec![(1, 2), (1, 10), (2, 0), (3, 4)]);
    assert_eq!(key(&forward), key(&shuffled));
    assert_eq!(forward.hits, shuffled.hits);
}

/// Locks out the global cap returning an arbitrary subset. It must be the
/// first N in the documented order, so that "showing 100 of many" means the
/// first hundred and not a hundred at random.
#[test]
fn the_global_cap_returns_a_prefix_and_says_it_truncated() {
    let a: &[u8] = b"OOM\nOOM\nOOM\nOOM\nOOM\n";
    let b: &[u8] = b"OOM\nOOM\n";
    let ca: Chunks<'_> = &[a];
    let cb: Chunks<'_> = &[b];

    let full = search(
        &Query::literal("OOM").context(0),
        &[one(1, 0, ca), one(2, 0, cb)],
    )
    .expect("search");
    assert_eq!(full.len(), 7);
    assert!(!full.truncated);

    let capped = search(
        &Query::literal("OOM").context(0).max_hits(3),
        &[one(2, 0, cb), one(1, 0, ca)],
    )
    .expect("search");
    assert_eq!(capped.len(), 3);
    assert!(capped.truncated);
    assert_eq!(capped.hits, full.hits[..3]);
}

/// Locks out one loud session consuming the whole result budget before the
/// other nineteen are examined, which is the failure that makes a
/// cross-session search useless.
#[test]
fn the_per_session_cap_keeps_a_loud_session_from_starving_the_rest() {
    let loud: &[u8] = b"OOM\nOOM\nOOM\nOOM\nOOM\nOOM\nOOM\nOOM\n";
    let quiet: &[u8] = b"OOM\n";
    let cl: Chunks<'_> = &[loud];
    let cq: Chunks<'_> = &[quiet];

    let results = search(
        &Query::literal("OOM").context(0).max_hits_per_session(2),
        &[one(1, 0, cl), one(2, 0, cq)],
    )
    .expect("search");

    assert_eq!(results.len(), 3);
    assert_eq!(
        results.hits.iter().map(|h| h.session).collect::<Vec<_>>(),
        vec![1, 1, 2]
    );
    assert!(results.truncated);
    assert_eq!(results.sessions_hit, 2);
}

/// Locks out a truncated result also truncating the context of the hits it did
/// return. A hit with its after-context cut off is worse than one hit fewer.
#[test]
fn hits_kept_under_a_cap_still_get_their_full_context() {
    let data: &[u8] = b"before\nOOM\nafter one\nafter two\nOOM again\n";
    let chunks: &[&[u8]] = &[data];
    let results = search(
        &Query::literal("OOM").context(2).max_hits(1),
        &[one(1, 0, chunks)],
    )
    .expect("search");

    assert_eq!(results.len(), 1);
    assert!(results.truncated);
    let hit = &results.hits[0];
    assert_eq!(context_text(&hit.before), vec!["before"]);
    assert_eq!(context_text(&hit.after), vec!["after one", "after two"]);
}

/// Locks out every match on a line being reported when the caller asked for
/// one, which would return six identical context blocks for one log line.
#[test]
fn one_hit_per_line_by_default_and_all_of_them_on_request() {
    let data: &[u8] = b"error error error\n";
    let chunks: &[&[u8]] = &[data];

    let first = search(&Query::literal("error").context(0), &[one(1, 0, chunks)]).expect("search");
    assert_eq!(first.len(), 1);
    assert_eq!(first.hits[0].match_seq, 0);

    let all = search(
        &Query::literal("error")
            .context(0)
            .all_matches_per_line(true),
        &[one(1, 0, chunks)],
    )
    .expect("search");
    assert_eq!(all.len(), 3);
    assert_eq!(
        all.hits.iter().map(|h| h.match_seq).collect::<Vec<_>>(),
        vec![0, 6, 12]
    );
    for hit in &all.hits {
        assert_eq!(hit.line_index, 0);
        assert_eq!(hit.line_seq, 0);
    }
}

/// Locks out per-line match offsets being computed in visible coordinates when
/// the line is coloured, which would put the second and third hits inside the
/// escape sequences between them.
#[test]
fn every_match_on_a_coloured_line_maps_back_correctly() {
    let data: &[u8] = b"\x1b[31merr\x1b[0m and \x1b[31merr\x1b[0m\n";
    let chunks: &[&[u8]] = &[data];
    let results = search(
        &Query::literal("err").context(0).all_matches_per_line(true),
        &[one(1, 0, chunks)],
    )
    .expect("search");

    assert_eq!(results.len(), 2);
    assert_eq!(results.hits[0].visible_lossy(), "err and err");
    assert_eq!(results.hits[0].match_seq, 5);
    assert_eq!(results.hits[1].match_seq, 22);
    assert_eq!(&data[5..8], b"err");
    assert_eq!(&data[22..25], b"err");
}

/// Locks out case folding failing to reach a coloured line, which is the
/// combination that a real "find OOM anywhere" query hits.
#[test]
fn case_insensitive_search_reaches_coloured_text() {
    let data: &[u8] = b"\x1b[33mWarning\x1b[0m: Oom Killer invoked\n";
    let chunks: &[&[u8]] = &[data];
    let results = search(
        &Query::literal("oom killer").case_insensitive(true),
        &[one(1, 0, chunks)],
    )
    .expect("search");

    assert_eq!(results.len(), 1);
    assert_eq!(results.hits[0].matched_text(), "Oom Killer");
    // 5 (SGR) + 7 ("Warning") + 4 (reset) + 2 (": ").
    assert_eq!(results.hits[0].match_seq, 18);
    assert_eq!(&data[18..28], b"Oom Killer");
}

/// Locks out whole-word matching being applied to the raw bytes, where an
/// escape sequence next to the word supplies a false boundary or destroys a
/// real one.
#[test]
fn whole_word_is_evaluated_on_the_visible_text() {
    let joined: &[u8] = b"con\x1b[0mcatenate\n";
    let chunks: &[&[u8]] = &[joined];
    let results = search(
        &Query::literal("con").whole_word(true),
        &[one(1, 0, chunks)],
    )
    .expect("search");
    assert!(
        results.is_empty(),
        "the escape must not create a word boundary inside `concatenate`"
    );

    let separate: &[u8] = b"\x1b[1mcon\x1b[0m fig\n";
    let chunks: &[&[u8]] = &[separate];
    let found = search(
        &Query::literal("con").whole_word(true),
        &[one(1, 0, chunks)],
    )
    .expect("search");
    assert_eq!(found.len(), 1);
    assert_eq!(found.hits[0].match_seq, 4);
}

/// Locks out a regex being escaped into a literal, or a literal being
/// interpreted as a regex. Both make the search box lie about what it does.
#[test]
fn regex_and_literal_patterns_behave_differently() {
    let data: &[u8] = b"version 1.2.3 built\nversion 1x2y3 built\n";
    let chunks: &[&[u8]] = &[data];

    let literal =
        search(&Query::literal("1.2.3").context(0), &[one(1, 0, chunks)]).expect("search");
    assert_eq!(literal.len(), 1);
    assert_eq!(literal.hits[0].line_index, 0);

    let regex = search(&Query::regex(r"1.2.3").context(0), &[one(1, 0, chunks)]).expect("search");
    assert_eq!(regex.len(), 2);

    let anchored = search(
        &Query::regex(r"^version \d+\.\d+\.\d+ built$").context(0),
        &[one(1, 0, chunks)],
    )
    .expect("search");
    assert_eq!(anchored.len(), 1);
    assert_eq!(anchored.hits[0].visible_lossy(), "version 1.2.3 built");
}

/// Locks out `$` failing on CRLF output, which is what a Windows child process
/// writes into the PTY and what a naive stripper leaves a stray `\r` on.
#[test]
fn end_anchors_work_on_crlf_scrollback() {
    let data: &[u8] = b"build failed\r\nnext line\r\n";
    let chunks: &[&[u8]] = &[data];
    let results = search(&Query::regex("failed$"), &[one(1, 0, chunks)]).expect("search");

    assert_eq!(results.len(), 1);
    let hit = &results.hits[0];
    assert_eq!(hit.visible_lossy(), "build failed");
    // The original line still carries its carriage return, for rendering.
    assert_eq!(hit.line, b"build failed\r");
    assert_eq!(hit.match_seq, 6);
}

/// Locks out a bad pattern taking down a whole multi-session search, and out of
/// it being silently swallowed into an empty result.
#[test]
fn a_bad_pattern_fails_before_any_scanning() {
    let data: &[u8] = b"anything\n";
    let chunks: &[&[u8]] = &[data];
    let error = search(&Query::regex("(unclosed"), &[one(1, 0, chunks)]).expect_err("must reject");
    assert!(matches!(error, vitrum_search::Error::BadPattern { .. }));

    let empty = search(
        &Query::new(Pattern::Literal(String::new())),
        &[one(1, 0, chunks)],
    )
    .expect_err("must reject");
    assert_eq!(empty, vitrum_search::Error::EmptyPattern);
}

/// Locks out an empty or all-escape session breaking a multi-session search.
/// A freshly spawned agent has an empty ring and a `clear`ed one holds nothing
/// but control sequences.
#[test]
fn empty_and_escape_only_sessions_do_not_disturb_the_others() {
    let empty: &[u8] = b"";
    let cleared: &[u8] = b"\x1b[2J\x1b[H\n\x1b[?25l\n";
    let real: &[u8] = b"OOM here\n";
    let ce: Chunks<'_> = &[empty];
    let cc: Chunks<'_> = &[cleared];
    let cr: Chunks<'_> = &[real];

    let results = search(
        &Query::literal("OOM"),
        &[one(1, 0, ce), one(2, 0, cc), one(3, 0, cr)],
    )
    .expect("search");

    assert_eq!(results.len(), 1);
    assert_eq!(results.hits[0].session, 3);
    assert_eq!(results.sessions_hit, 1);
    assert_eq!(results.lines_scanned, 3);
}

/// Locks out twenty sessions being searched as anything other than twenty
/// independent streams, with each hit attributed to the right one. This is the
/// whole product claim in one test.
#[test]
fn twenty_sessions_are_searched_and_attributed_correctly() {
    let bodies: Vec<Vec<u8>> = (0..20u64)
        .map(|session| {
            let mut body = format!("session {session} starting\nrunning tests\n").into_bytes();
            if session % 3 == 0 {
                body.extend_from_slice(b"\x1b[31mfatal\x1b[0m: OOM killer took us\n");
            } else {
                body.extend_from_slice(b"all good\n");
            }
            body.extend_from_slice(b"done\n");
            body
        })
        .collect();
    let slices: Vec<&[u8]> = bodies.iter().map(|b| b.as_slice()).collect();
    let haystacks: Vec<Haystack<'_>> = slices
        .iter()
        .enumerate()
        .map(|(index, body)| Haystack {
            session: index as u64,
            base_seq: 0,
            chunks: std::slice::from_ref(body),
        })
        .collect();

    let results = search(&Query::literal("OOM").context(1), &haystacks).expect("search");

    let expected: Vec<u64> = (0..20u64).filter(|s| s % 3 == 0).collect();
    assert_eq!(
        results.hits.iter().map(|h| h.session).collect::<Vec<_>>(),
        expected
    );
    assert_eq!(results.sessions_hit, expected.len());
    assert!(!results.truncated);
    for hit in &results.hits {
        assert_eq!(hit.visible_lossy(), "fatal: OOM killer took us");
        assert_eq!(hit.matched_text(), "OOM");
        assert_eq!(context_text(&hit.before), vec!["running tests"]);
        assert_eq!(context_text(&hit.after), vec!["done"]);
    }
}

/// Locks out an OSC payload being searchable. A window title or a hyperlink
/// URL is not text the user saw, and vitrum's own OSC 7373 hints would
/// otherwise turn every approval prompt into a false hit.
#[test]
fn osc_payloads_are_not_searchable_content() {
    let data: &[u8] =
        b"\x1b]0;OOM in the title\x07visible text\n\x1b]7373;approval;delete OOM logs?\x1b\\prompt\n";
    let chunks: &[&[u8]] = &[data];
    let results = search(&Query::literal("OOM"), &[one(1, 0, chunks)]).expect("search");
    assert!(results.is_empty());

    let visible = search(&Query::literal("visible text"), &[one(1, 0, chunks)]).expect("search");
    assert_eq!(visible.len(), 1);
    assert_eq!(visible.hits[0].visible_lossy(), "visible text");
}

/// Locks out the last, still-being-written line of a ring being unsearchable.
/// It has no trailing newline and it is the line the operator most wants.
#[test]
fn the_unterminated_newest_line_is_searchable() {
    let data: &[u8] = b"done\nOOM right now";
    let chunks: &[&[u8]] = &[data];
    let results = search(&Query::literal("OOM"), &[one(1, 0, chunks)]).expect("search");

    assert_eq!(results.len(), 1);
    assert_eq!(results.hits[0].line, b"OOM right now");
    assert_eq!(results.hits[0].match_seq, 5);
    assert_eq!(results.bytes_scanned, data.len() as u64);
}

/// Locks out a session whose ring holds invalid UTF-8 aborting the search or
/// mangling the reported offsets of the valid text around it.
#[test]
fn invalid_utf8_in_a_session_does_not_break_the_search() {
    let mut data = b"binary junk: ".to_vec();
    data.extend_from_slice(&[0xff, 0xfe, 0x00, 0x80, 0x9f]);
    data.extend_from_slice(b" then OOM\nnext\n");
    let slice: &[u8] = &data;
    let chunks: &[&[u8]] = &[slice];

    let results = search(&Query::literal("OOM"), &[one(1, 0, chunks)]).expect("search");
    assert_eq!(results.len(), 1);
    let hit = &results.hits[0];
    assert_eq!(hit.match_seq, 24);
    assert_eq!(&data[24..27], b"OOM");
    assert_eq!(hit.matched_text(), "OOM");
    // The 0x80 and 0x9f are C1-range bytes; they must survive as replacement
    // characters rather than being eaten as control introducers.
    assert!(hit.visible_lossy().contains('\u{fffd}'));
    assert!(hit.visible_lossy().ends_with(" then OOM"));
    // The exact bytes survive, invalid ones included: the line is everything
    // up to its newline, and nothing in it was dropped or transcoded.
    assert_eq!(hit.visible, &data[..data.len() - "\nnext\n".len()]);
    assert_eq!(hit.visible, hit.line);
}

/// Locks out chunking changing a single reported offset. Where the ring seam
/// falls is an accident of when the session last wrapped, so a result that
/// depends on it is a result the user cannot reproduce or cite.
///
/// The chunk size is chosen to be coprime with nothing in particular, so seams
/// land mid-word, mid-escape-sequence and mid-line throughout.
#[test]
fn chunking_the_same_bytes_changes_nothing_about_the_result() {
    let body = mixed_scrollback(2_000);
    let pieces: Vec<&[u8]> = body.chunks(97).collect();
    assert!(pieces.len() > 1_000, "the seams must be dense to be a test");

    // Both caps lifted: neither side may be truncated at a different place.
    let query = Query::literal("Finished")
        .context(2)
        .max_hits(10_000)
        .max_hits_per_session(10_000);

    let whole: &[u8] = &body;
    let contiguous = search(
        &query,
        &[Haystack {
            session: 1,
            base_seq: 0,
            chunks: std::slice::from_ref(&whole),
        }],
    )
    .expect("search");

    let split = search(
        &query,
        &[Haystack {
            session: 1,
            base_seq: 0,
            chunks: &pieces,
        }],
    )
    .expect("search");

    // One `Finished` line in every eight, over 2000 lines.
    assert_eq!(contiguous.len(), 250);
    assert!(!contiguous.truncated);
    assert!(!split.truncated);
    assert_eq!(contiguous.hits, split.hits);
    assert_eq!(contiguous.lines_scanned, split.lines_scanned);
    assert_eq!(contiguous.bytes_scanned, split.bytes_scanned);
    assert_eq!(contiguous.bytes_scanned, body.len() as u64);
}

/// Locks out a coloured line that straddles a seam being mis-stripped. The
/// stripper sees a reassembled line, so a seam inside `\x1b[1;32m` must not
/// leave `1;32m` in the visible text — which would break the offset map for
/// every match after it on that line.
#[test]
fn every_coloured_line_survives_a_seam_at_every_possible_position() {
    let line: &[u8] = b"\x1b[1;32mstatus\x1b[0m: \x1b[31mOOM\x1b[0m detected here\n";
    let expected_visible = "status: OOM detected here";
    // Byte offset of `OOM` within the line: 7 (SGR) + 6 ("status") + 4 (reset)
    // + 2 (": ") + 5 (SGR).
    let expected_match: usize = 24;
    assert_eq!(&line[expected_match..expected_match + 3], b"OOM");
    let expected_seq = expected_match as u64;

    for cut in 1..line.len() {
        let chunks: &[&[u8]] = &[&line[..cut], &line[cut..]];
        let results = search(
            &Query::literal("OOM").context(0),
            &[Haystack {
                session: 1,
                base_seq: 0,
                chunks,
            }],
        )
        .expect("search");

        assert_eq!(results.len(), 1, "cut at {cut} lost the match");
        let hit = &results.hits[0];
        assert_eq!(hit.visible_lossy(), expected_visible, "cut at {cut}");
        assert_eq!(hit.match_seq, expected_seq, "cut at {cut}");
        assert_eq!(hit.line, &line[..line.len() - 1], "cut at {cut}");
    }
}
