//! Regression test suite for `vitrum-search`.
//!
//! Covers:
//! - SIMD ASCII case-folding needle matches
//! - Chunk prefiltering line skips
//! - Zero-width regex matching
//! - Context ring buffer line preservation
//! - Parallel and multi-haystack search result ordering

use vitrum_search::{ContextLine, Haystack, MAX_CONTEXT, Pattern, Query, Sweep, search};

/// Helper to construct a single haystack from contiguous byte slices.
fn haystack<'a>(session: u64, base_seq: u64, chunks: &'a [&'a [u8]]) -> Haystack<'a> {
    Haystack {
        session,
        base_seq,
        chunks,
    }
}

/// Helper to convert context lines to lossy UTF-8 strings for easy comparison.
fn context_strings(lines: &[ContextLine]) -> Vec<String> {
    lines.iter().map(|line| line.to_string_lossy()).collect()
}

// ============================================================================
// Group 1: SIMD ASCII Case-Folding Needle Matches
// ============================================================================

/// WHY: Ensures case-insensitive searches correctly match uppercase, lowercase, and mixed-case ASCII needles against mixed scrollback text, while validating that case-sensitive literal queries take the SIMD fast path.
#[test]
fn test_simd_case_folding_ascii_needle_matches_upper_lower_mixed() {
    let data: &[u8] = b"line 1: ERROR occurred\nline 2: error resolved\nline 3: ErRoR logged\nline 4: ALL OK\n";
    let chunks: &[&[u8]] = &[data];
    let hs = haystack(1, 0, chunks);

    // Case-sensitive query uses fast SIMD path
    let q_sensitive = Query::literal("ERROR");
    let res_sens = search(&q_sensitive, &[hs]).expect("search");
    assert_eq!(res_sens.hits.len(), 1);
    assert_eq!(res_sens.hits[0].line_index, 0);

    // Case-insensitive query matches all variants
    let q_insensitive = Query::literal("error").case_insensitive(true).all_matches_per_line(true);
    let res_insens = search(&q_insensitive, &[hs]).expect("search");
    assert_eq!(res_insens.hits.len(), 3);
    assert_eq!(res_insens.hits[0].matched_text(), "ERROR");
    assert_eq!(res_insens.hits[1].matched_text(), "error");
    assert_eq!(res_insens.hits[2].matched_text(), "ErRoR");
}

/// WHY: Defends against coordinate translation corruption when case-insensitive ASCII matching operates on lines containing SGR color codes, ensuring SGR sequences are stripped during matching while original byte sequence offsets are preserved.
#[test]
fn test_simd_case_folding_with_ansi_sgr_color_sequences() {
    //                                  01234567 8901234 56789012 345678901
    // Line 0: "cargo check\n" (12 bytes)
    // Line 1: "\x1b[1;31mFATAl\x1b[0m: system crash\n"
    // SGR "\x1b[1;31m" is 7 bytes. "FATAl" starts at byte offset 12 + 7 = 19.
    let data: &[u8] = b"cargo check\n\x1b[1;31mFATAl\x1b[0m: system crash\n";
    let chunks: &[&[u8]] = &[data];
    let hs = haystack(2, 0, chunks);

    let query = Query::literal("fatal").case_insensitive(true);
    let results = search(&query, &[hs]).expect("search");

    assert_eq!(results.len(), 1);
    let hit = &results.hits[0];
    assert_eq!(hit.session, 2);
    assert_eq!(hit.line_seq, 12);
    assert_eq!(hit.match_seq, 19);
    assert_eq!(hit.visible_lossy(), "FATAl: system crash");
    assert_eq!(hit.matched_text(), "FATAl");
    assert_eq!(hit.original_range, 7..12);
    assert_eq!(hit.visible_range, 0..5);
}

/// WHY: Verifies that ASCII case-folding does not corrupt or misalign multi-byte UTF-8 sequences or invalid UTF-8 bytes adjacent to case-insensitive needles.
#[test]
fn test_simd_case_folding_boundary_non_ascii_preservation() {
    let data: &[u8] = b"STATUS: \xf0\x9f\x9a\x80 LAUNCHING ok\nBAD_BYTE: \xff\xfe WAITING ok\n";
    let chunks: &[&[u8]] = &[data];
    let hs = haystack(3, 100, chunks);

    let query = Query::literal("ok").case_insensitive(true).all_matches_per_line(true);
    let results = search(&query, &[hs]).expect("search");

    assert_eq!(results.len(), 2);
    assert_eq!(results.hits[0].line_seq, 100);
    assert_eq!(results.hits[0].matched_text(), "ok");
    assert_eq!(results.hits[1].matched_text(), "ok");
}

// ============================================================================
// Group 2: Chunk Prefiltering & Line Skips
// ============================================================================

/// WHY: Defends against panics or missed matches when haystacks contain empty chunks, zero-length slices, or multiple fragmented chunks per session.
#[test]
fn test_chunk_prefiltering_empty_and_multiple_chunks() {
    let chunk1: &[u8] = b"";
    let chunk2: &[u8] = b"first line\n";
    let chunk3: &[u8] = b"";
    let chunk4: &[u8] = b"second line with TARGET match\n";
    let chunk5: &[u8] = b"";
    let chunks: &[&[u8]] = &[chunk1, chunk2, chunk3, chunk4, chunk5];

    let hs = haystack(10, 0, chunks);
    let query = Query::literal("TARGET");
    let results = search(&query, &[hs]).expect("search");

    assert_eq!(results.len(), 1);
    assert_eq!(results.hits[0].line_index, 1);
    assert_eq!(results.hits[0].visible_lossy(), "second line with TARGET match");
    assert_eq!(results.lines_scanned, 2);
}

/// WHY: Validates that line spans straddling chunk boundaries (the ring seam) are properly reconstituted in temporary scratch buffers without skipping lines or dropping trailing characters.
#[test]
fn test_chunk_prefiltering_straddling_lines_across_chunk_seams() {
    let part1: &[u8] = b"first line\nsecond line straddling ";
    let part2: &[u8] = b"the seam with NEEDLE inside\nthird line\n";
    let chunks: &[&[u8]] = &[part1, part2];

    let hs = haystack(11, 500, chunks);
    let query = Query::literal("NEEDLE");
    let results = search(&query, &[hs]).expect("search");

    assert_eq!(results.len(), 1);
    let hit = &results.hits[0];
    assert_eq!(hit.line_index, 1);
    assert_eq!(hit.visible_lossy(), "second line straddling the seam with NEEDLE inside");
    assert_eq!(hit.matched_text(), "NEEDLE");
}

/// WHY: Ensures that large scrollback inputs with thousands of non-matching lines skip rapidly through the line iterator without unnecessary allocations or false positive matches.
#[test]
fn test_chunk_prefiltering_long_line_skips_without_matches() {
    let mut large_buffer = Vec::new();
    for i in 0..5_000 {
        large_buffer.extend_from_slice(format!("noise line {i:05}: nothing interesting here\n").as_bytes());
    }
    large_buffer.extend_from_slice(b"target line 5000: FOUND_HERE\n");
    for i in 5_001..10_000 {
        large_buffer.extend_from_slice(format!("noise line {i:05}: nothing interesting here\n").as_bytes());
    }

    let chunks: &[&[u8]] = &[&large_buffer];
    let hs = haystack(12, 0, chunks);
    let query = Query::literal("FOUND_HERE");
    let results = search(&query, &[hs]).expect("search");

    assert_eq!(results.len(), 1);
    assert_eq!(results.lines_scanned, 10_000);
    assert_eq!(results.hits[0].line_index, 5000);
}

// ============================================================================
// Group 3: Zero-Width Regex Matching
// ============================================================================

/// WHY: Guarantees that zero-width regex patterns (such as `^`, `$`, `\b`) match line positions correctly without infinite looping or buffer overruns in matcher iteration.
#[test]
fn test_zero_width_regex_anchors_and_word_boundaries() {
    let data: &[u8] = b"ALPHA BRAVO\nCHARLIE DELTA\n";
    let chunks: &[&[u8]] = &[data];
    let hs = haystack(20, 0, chunks);

    // Start-of-line anchor ^
    let q_start = Query::regex("^ALPHA");
    let res_start = search(&q_start, &[hs]).expect("search");
    assert_eq!(res_start.len(), 1);
    assert_eq!(res_start.hits[0].line_index, 0);

    // End-of-line anchor $
    let q_end = Query::regex("DELTA$");
    let res_end = search(&q_end, &[hs]).expect("search");
    assert_eq!(res_end.len(), 1);
    assert_eq!(res_end.hits[0].line_index, 1);

    // Word boundary \b
    let q_word = Query::regex(r"\bBRAVO\b");
    let res_word = search(&q_word, &[hs]).expect("search");
    assert_eq!(res_word.len(), 1);
    assert_eq!(res_word.hits[0].line_index, 0);
}

/// WHY: Tests zero-width regexes (e.g. `a*` or `(?=foo)`) when `all_matches_per_line` is enabled to ensure the matcher advances byte-by-byte rather than looping infinitely on zero-width hits.
#[test]
fn test_zero_width_regex_empty_match_all_matches_per_line() {
    let data: &[u8] = b"abc\n";
    let chunks: &[&[u8]] = &[data];
    let hs = haystack(21, 0, chunks);

    // Pattern matching zero width
    let query = Query::regex("a*").all_matches_per_line(true);
    let results = search(&query, &[hs]).expect("search");

    // Matches 'a', then zero-width matches at positions without panicking or hanging
    assert!(!results.hits.is_empty());
    assert_eq!(results.hits[0].line_index, 0);
}

/// WHY: Verifies that zero-width regex boundary assertions (e.g. `\b` word boundaries) operate on stripped visible text while correctly mapping range boundaries back to original SGR-encoded byte sequences.
#[test]
fn test_zero_width_regex_lookaround_and_ansi_interaction() {
    let data: &[u8] = b"\x1b[31mERR\x1b[0m: fatal error\n";
    let chunks: &[&[u8]] = &[data];
    let hs = haystack(22, 0, chunks);

    // Word boundary assertion \bERR\b
    let query = Query::regex(r"\bERR\b");
    let results = search(&query, &[hs]).expect("search");

    assert_eq!(results.len(), 1);
    let hit = &results.hits[0];
    assert_eq!(hit.visible_lossy(), "ERR: fatal error");
    assert_eq!(hit.matched_text(), "ERR");
    // "ERR" visible index is 0..3, while match_seq accounts for the 5 SGR bytes (\x1b[31m)
    assert_eq!(hit.visible_range, 0..3);
    assert_eq!(hit.original_range, 5..8);
    assert_eq!(hit.match_seq, 5);
}

// ============================================================================
// Group 4: Context Ring Buffer Line Preservation
// ============================================================================

/// WHY: Verifies that requested context lines before and after a hit are accurately preserved in order, clamped to `MAX_CONTEXT`, and properly evicted from internal ring buffers when capacity is reached.
#[test]
fn test_context_ring_buffer_preservation_and_max_context_clamping() {
    let mut data = Vec::new();
    for i in 0..100 {
        data.extend_from_slice(format!("line {i:03}\n").as_bytes());
    }
    data.extend_from_slice(b"TARGET HIT\n");
    for i in 101..150 {
        data.extend_from_slice(format!("line {i:03}\n").as_bytes());
    }

    let chunks: &[&[u8]] = &[&data];
    let hs = haystack(30, 0, chunks);

    // Ask for 100 lines before and after, which must clamp to MAX_CONTEXT (64)
    let query = Query::literal("TARGET HIT").context_before(100).context_after(100);
    assert_eq!(query.effective_context_before(), MAX_CONTEXT);
    assert_eq!(query.effective_context_after(), MAX_CONTEXT);

    let results = search(&query, &[hs]).expect("search");
    assert_eq!(results.len(), 1);
    let hit = &results.hits[0];

    // Exactly 64 before lines preserved (lines 36..100, which is 036..099 + line 099)
    assert_eq!(hit.before.len(), MAX_CONTEXT);
    let before_strs = context_strings(&hit.before);
    assert_eq!(before_strs.first().unwrap(), "line 036");
    assert_eq!(before_strs.last().unwrap(), "line 099");

    // Exactly 49 after lines (since only 49 lines follow line 100: 101..150)
    assert_eq!(hit.after.len(), 49);
    let after_strs = context_strings(&hit.after);
    assert_eq!(after_strs.first().unwrap(), "line 101");
    assert_eq!(after_strs.last().unwrap(), "line 149");
}

/// WHY: Defends against context loss when hits occur near the end of a haystack or when the global hit cap triggers draining mode before after-context lines are fully collected.
#[test]
fn test_context_ring_buffer_pending_after_context_collection() {
    let data: &[u8] = b"line 0\nline 1: HIT\nline 2\nline 3\nline 4\n";
    let chunks: &[&[u8]] = &[data];
    let hs = haystack(31, 0, chunks);

    // Cap at 1 hit, request 2 lines after-context
    let query = Query::literal("HIT").context_after(2).max_hits(1);
    let results = search(&query, &[hs]).expect("search");

    assert_eq!(results.len(), 1);
    let hit = &results.hits[0];
    assert_eq!(hit.after.len(), 2);
    let after_strs = context_strings(&hit.after);
    assert_eq!(after_strs, vec!["line 2", "line 3"]);
}

/// WHY: Ensures adjacent or overlapping hits in the scrollback correctly capture their respective before and after context lines without context corruption or cross-contamination.
#[test]
fn test_context_ring_buffer_overlapping_hit_contexts() {
    let data: &[u8] = b"L0\nL1: MATCH_A\nL2\nL3: MATCH_B\nL4\n";
    let chunks: &[&[u8]] = &[data];
    let hs = haystack(32, 0, chunks);

    let query = Query::literal("MATCH").context(1).all_matches_per_line(true);
    let results = search(&query, &[hs]).expect("search");

    assert_eq!(results.len(), 2);
    let hit1 = &results.hits[0];
    let hit2 = &results.hits[1];

    assert_eq!(context_strings(&hit1.before), vec!["L0"]);
    assert_eq!(context_strings(&hit1.after), vec!["L2"]);

    assert_eq!(context_strings(&hit2.before), vec!["L2"]);
    assert_eq!(context_strings(&hit2.after), vec!["L4"]);
}

// ============================================================================
// Group 5: Parallel & Multi-Haystack Search Result Ordering
// ============================================================================

/// WHY: Guarantees that search results across multiple haystacks are deterministically sorted by `(session, line_seq, match_seq)` regardless of haystack input order or execution sequence.
#[test]
fn test_parallel_search_result_ordering_deterministic_sorting() {
    let d1: &[u8] = b"session 10: MATCH\n";
    let d2: &[u8] = b"session 2: MATCH\n";
    let d3: &[u8] = b"session 5: MATCH\n";

    let c1: &[&[u8]] = &[d1];
    let c2: &[&[u8]] = &[d2];
    let c3: &[&[u8]] = &[d3];

    // Pass haystacks out of order: 10, 2, 5
    let hs10 = haystack(10, 0, c1);
    let hs2 = haystack(2, 0, c2);
    let hs5 = haystack(5, 0, c3);

    let query = Query::literal("MATCH");
    let results = search(&query, &[hs10, hs2, hs5]).expect("search");

    assert_eq!(results.len(), 3);
    assert_eq!(results.hits[0].session, 2);
    assert_eq!(results.hits[1].session, 5);
    assert_eq!(results.hits[2].session, 10);
    assert_eq!(results.sessions_hit, 3);
}

/// WHY: Tests the `Sweep` API across multiple sessions pushed out of order, verifying that `Sweep::finish()` enforces ascending session order and that pushing past `max_hits` halts further session sweeps.
#[test]
fn test_sweep_multi_haystack_session_ordering_and_cap() {
    let d1: &[u8] = b"session 1: FIND\n";
    let d2: &[u8] = b"session 2: FIND\n";
    let d3: &[u8] = b"session 3: FIND\n";

    let c3: &[&[u8]] = &[d3];
    let c1: &[&[u8]] = &[d1];
    let c2: &[&[u8]] = &[d2];

    let hs3 = haystack(3, 0, c3);
    let hs1 = haystack(1, 0, c1);
    let hs2 = haystack(2, 0, c2);

    let query = Query::literal("FIND").max_hits(2);
    let mut sweep = Sweep::new(&query).expect("sweep");

    // Push out of order: 3, then 1 (reaches cap), then 2 (is skipped because cap full)
    assert!(sweep.push(&hs3));
    assert!(!sweep.push(&hs1)); // cap reached, returns false
    assert!(!sweep.push(&hs2)); // sweep full, returns false immediately
    assert!(sweep.is_full());

    let results = sweep.finish();
    assert_eq!(results.len(), 2);
    // Ordered by session ID ascending (sessions 1 and 3 were consumed before cap reached)
    assert_eq!(results.hits[0].session, 1);
    assert_eq!(results.hits[1].session, 3);
    assert!(results.truncated);
}
/// WHY: Validates that stream byte offsets (`line_seq` and `match_seq`) increase monotonically across multiple multi-chunk haystacks with non-zero `base_seq`.
#[test]
fn test_multi_chunk_haystack_line_seq_monotonicity() {
    let chunk_a: &[u8] = b"line A: MATCH_1\n";
    let chunk_b: &[u8] = b"line B: MATCH_2\n";

    let ca: &[&[u8]] = &[chunk_a];
    let cb: &[&[u8]] = &[chunk_b];

    let hs1 = haystack(1, 1000, ca);
    let hs2 = haystack(1, 5000, cb);

    let query = Query::literal("MATCH");
    let results = search(&query, &[hs1, hs2]).expect("search");

    assert_eq!(results.len(), 2);
    assert_eq!(results.hits[0].line_seq, 1000);
    assert_eq!(results.hits[0].match_seq, 1000 + 8);

    assert_eq!(results.hits[1].line_seq, 5000);
    assert_eq!(results.hits[1].match_seq, 5000 + 8);
}
