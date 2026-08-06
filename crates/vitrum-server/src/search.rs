//! Answering [`vitrum_proto::ClientMsg::Search`] out of the daemon's own rings.
//!
//! Only the daemon can answer this. It holds every session's bytes; a client
//! holds one viewport. "Which of my twenty agents hit an OOM" is therefore one
//! server-side sweep here and twenty round trips anywhere else.
//!
//! # Not on the runtime
//!
//! Measured against the release daemon: twenty live sessions holding a full
//! 10 MiB ring each is 200 MiB per sweep, 84 to 96 ms of CPU, 2200 to 2350
//! MiB/s. That is not a duration an async task may occupy. The PTY coalescers,
//! the output broadcast pumps and every other connection are tasks on the same
//! runtime, so 90 ms inline is 90 ms during which no agent's output reaches any
//! window. The sweep therefore runs on [`tokio::task::spawn_blocking`], and the
//! only thing that waits for it is the connection that asked. Verified from
//! outside the process: while one client's 200 MiB search was running, another
//! client's five sequential PTY echo round trips landed at 7, 15, 22, 29 and
//! 36 ms and the search answered at 84. The daemon idles at 0.0000% CPU with
//! those twenty sessions live, and zero context switches, so nothing here costs
//! anything until someone searches.
//!
//! Locking is the second half of the same problem. The bytes are borrowed from
//! the rings rather than copied — copying 200 MB per keystroke is not a search
//! box — and a borrow means the ring's lock is held. So the sweep takes exactly
//! one ring lock at a time, through
//! [`SessionManager::with_scrollback`](vitrum_core::SessionManager::with_scrollback),
//! and releases it before considering the next session. One session's PTY pump
//! waits about 4.5 ms of that 90; no session ever waits on another's scan.
//!
//! # Three coordinate systems, and the one that gets confused
//!
//! - `line_seq` is the cumulative stream offset of the matched line's first
//!   **original** byte, so clicking a result can scroll the terminal to exactly
//!   that point.
//! - `match_start`/`match_end` are offsets into **`visible`**, the escape-free
//!   text — not into the original bytes, and not into `line_seq`'s space.
//! - `visible` is bytes, never a `String`. A PTY carries whatever the program
//!   wrote, so lossy decoding of one stray `0xFF` would grow the line by two
//!   bytes and slide every offset after it onto the wrong substring.
//!
//! [`vitrum_search::Hit`] carries both ranges, spelled apart precisely because
//! they are so easy to swap. Sending `original_range` would light up the SGR
//! introducer instead of the word on every coloured line, which is most of them.

use std::sync::Arc;

use vitrum_core::SessionManager;
use vitrum_proto::{SearchHit, ServerMsg, SessionId};
use vitrum_search::{Haystack, Hit, Matcher, Query, SearchResults, Stripper, Sweep};

/// Translate the wire request into a query.
///
/// `max_hits_per_session` is left wide open here; the fair share depends on how
/// many sessions are actually being swept, which only [`sweep`] knows.
pub(crate) fn query_from_wire(
    pattern: &str,
    regex: bool,
    case_insensitive: bool,
    whole_word: bool,
    context_lines: u16,
    max_hits: u32,
) -> Query {
    let base = if regex {
        Query::regex(pattern)
    } else {
        Query::literal(pattern)
    };
    base.case_insensitive(case_insensitive)
        .whole_word(whole_word)
        .context(context_lines as usize)
        .max_hits(max_hits as usize)
        .max_hits_per_session(max_hits as usize)
}

/// One session's share of the hit budget.
///
/// Without a per-session share, one chatty agent fills the whole budget before
/// session 4 is looked at, and the answer to "which of my agents mentioned OOM"
/// becomes "session 3, four hundred times". A quarter of the budget, floored at
/// eight so a small cap still returns something usable per session.
///
/// Not applied to a single-session sweep. There is nobody to be fair to, and
/// rationing a search the client scoped to one session itself would return a
/// quarter of what it asked for and call it truncated.
fn fairness_cap(max_hits: usize, sessions: usize) -> usize {
    if sessions <= 1 {
        return max_hits;
    }
    max_hits.div_ceil(4).max(8)
}

/// Which sessions to sweep, ascending and deduplicated.
///
/// Ascending is not cosmetic. It is the order the global cap consumes, so it is
/// what makes a truncated answer the *first* N hits rather than N hits from
/// whichever sessions happened to be visited first. Deduplicating matters for
/// the same reason: a client that names session 3 twice must not get its hits
/// twice and burn double the budget doing it.
fn targets(manager: &SessionManager, requested: &[SessionId]) -> Vec<SessionId> {
    if requested.is_empty() {
        return manager.list().into_iter().map(|info| info.id).collect();
    }
    let mut ids = requested.to_vec();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Sweep the rings, one lock at a time.
///
/// Blocking and CPU-bound by nature: call it from [`answer`], not from a task.
pub(crate) fn sweep(
    manager: &SessionManager,
    requested: &[SessionId],
    matcher: &Matcher,
    query: &Query,
) -> SearchResults {
    let ids = targets(manager, requested);
    let query = Query {
        max_hits_per_session: fairness_cap(query.max_hits, ids.len()),
        ..query.clone()
    };
    let mut sweep = Sweep::with_matcher(matcher, &query);

    for id in ids {
        // Asked before the lock, not after: a sweep whose cap is already full
        // must not acquire a ring lock only to discard what it reads. This is
        // also what makes `max_hits == 0` touch nothing at all.
        if sweep.is_full() {
            break;
        }
        // The closure runs under that one session's ring lock and does nothing
        // but scan it. `None` means the session was closed between the listing
        // and now, which is routine under load and a reason to skip it rather
        // than to fail a query over nineteen other sessions.
        let _ = manager.with_scrollback(id, |base_seq, first, second| {
            let chunks = [first, second];
            sweep.push(&Haystack {
                session: id.0,
                base_seq,
                chunks: &chunks,
            });
        });
    }

    sweep.finish()
}

/// Project one hit onto the wire.
fn project(hit: &Hit, stripper: &mut Stripper) -> SearchHit {
    SearchHit {
        session: SessionId(hit.session),
        line_seq: hit.line_seq,
        visible: hit.visible.clone(),
        // `visible_range`, never `original_range`. See the module header.
        match_start: hit.visible_range.start as u32,
        match_end: hit.visible_range.end as u32,
        before: hit
            .before
            .iter()
            .map(|line| visible_of(&line.bytes, stripper))
            .collect(),
        after: hit
            .after
            .iter()
            .map(|line| visible_of(&line.bytes, stripper))
            .collect(),
    }
}

/// A context line reduced to the text an operator reads.
///
/// Stripped, like the hit line beside it, because this message carries no
/// original-byte field at all: a result row is text. Sending context with its
/// escapes intact would render literal `^[[31m` next to clean text, and worse,
/// a colour opened in a context line would have its reset stripped out of the
/// hit line and bleed down the rest of the list.
///
/// The scan already answered "does this line contain escapes" for the hit line,
/// but not for its context, so the cheap `memchr3` check is repeated here and
/// the common uncoloured line still costs one copy and no stripping pass.
fn visible_of(line: &[u8], stripper: &mut Stripper) -> Vec<u8> {
    if !vitrum_search::needs_stripping(line) {
        return line.to_vec();
    }
    stripper.fill(line);
    stripper.text().to_vec()
}

/// Answer one search request.
///
/// Always returns a message to send: [`ServerMsg::SearchResults`] on success,
/// [`ServerMsg::Error`] when the pattern cannot be compiled. An unusable
/// pattern is user input, not a server fault, so it is named and reported —
/// never silently downgraded to a literal search, which would answer a
/// different question than the one asked and look like it had succeeded.
pub(crate) async fn answer(
    manager: Arc<SessionManager>,
    sessions: Vec<SessionId>,
    query: Query,
) -> ServerMsg {
    let pattern = query.pattern.text().to_string();

    // Compiled before a thread is borrowed from the blocking pool. A malformed
    // regex is a half-typed keystroke in a search box, and refusing it must
    // cost no more than parsing it.
    let matcher = match Matcher::compile(&query) {
        Ok(matcher) => matcher,
        Err(e) => {
            return ServerMsg::error(None, e.to_string());
        }
    };

    let swept = tokio::task::spawn_blocking(move || {
        let results = sweep(&manager, &sessions, &matcher, &query);
        let mut stripper = Stripper::new();
        let hits: Vec<SearchHit> = results
            .hits
            .iter()
            .map(|hit| project(hit, &mut stripper))
            .collect();
        (hits, results.truncated, results.bytes_scanned)
    })
    .await;

    match swept {
        Ok((hits, truncated, bytes_scanned)) => ServerMsg::SearchResults {
            pattern,
            hits,
            truncated,
            bytes_scanned,
        },
        // A panic in the sweep is a bug in this crate, not in the connection.
        // Reporting it keeps twenty live sessions attached to a client that
        // asked one bad question.
        Err(e) => ServerMsg::error(None, format!("the search task failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sweep a fixed body of bytes as if it were one session's ring.
    fn hits_in(body: &[u8], query: &Query) -> Vec<SearchHit> {
        let matcher = Matcher::compile(query).expect("the test pattern compiles");
        let chunks = [body];
        let mut sweep = Sweep::with_matcher(&matcher, query);
        sweep.push(&Haystack {
            session: 4,
            base_seq: 0,
            chunks: &chunks,
        });
        let results = sweep.finish();
        let mut stripper = Stripper::new();
        results
            .hits
            .iter()
            .map(|hit| project(hit, &mut stripper))
            .collect()
    }

    /// Locks out the single most likely bug in this file: sending
    /// `original_range` as the highlight. On a coloured line the two ranges
    /// differ by the length of every escape before the match, so the client
    /// would highlight the SGR introducer and part of the wrong word.
    #[test]
    fn match_offsets_index_the_visible_text_not_the_original_bytes() {
        // "error" starts at original byte 7 and visible byte 0.
        let body = b"\x1b[1;31merror\x1b[0m: linker killed\n";
        let hits = hits_in(body, &Query::literal("error").context(0));

        assert_eq!(hits.len(), 1);
        let hit = &hits[0];
        assert_eq!(hit.visible, b"error: linker killed");
        assert_eq!(hit.match_start, 0, "visible offset, not the original 7");
        assert_eq!(hit.match_end, 5);
        assert_eq!(
            &hit.visible[hit.match_start as usize..hit.match_end as usize],
            b"error"
        );
    }

    /// Locks out `line_seq` being measured in visible bytes. It addresses the
    /// data plane, whose seqs count every byte the terminal wrote including the
    /// escapes, so a stripped-coordinate `line_seq` would scroll a session to a
    /// point that drifts further off with every colour change above it.
    #[test]
    fn line_seq_counts_original_bytes_including_escapes() {
        // 24 original bytes on line one; the visible text is only 13.
        let body = b"\x1b[32mstart\x1b[0m of output\nsecond line has error here\n";
        assert_eq!(body.iter().position(|&b| b == b'\n'), Some(24));

        let hits = hits_in(body, &Query::literal("error").context(0));
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].line_seq, 25,
            "the second line starts at original byte 25, not at visible byte 14"
        );
    }

    /// Locks out `line_seq` being reported relative to the ring instead of the
    /// session's cumulative stream. A ring that has evicted 10 MB starts at
    /// `base_seq`, and ignoring it would send every hit's offset short by
    /// exactly the amount evicted — pointing at bytes the session wrote long
    /// before the ones that matched.
    #[test]
    fn line_seq_is_offset_by_the_rings_oldest_retained_seq() {
        let body: &[u8] = b"first\nsecond has error\n";
        let query = Query::literal("error").context(0);
        let matcher = Matcher::compile(&query).expect("compile");
        let chunks = [body];
        let mut sweep = Sweep::with_matcher(&matcher, &query);
        sweep.push(&Haystack {
            session: 4,
            base_seq: 10_000_000,
            chunks: &chunks,
        });
        let results = sweep.finish();
        let mut stripper = Stripper::new();
        let hit = project(&results.hits[0], &mut stripper);
        assert_eq!(hit.line_seq, 10_000_006);
    }

    /// Locks out context lines going out with their escapes intact. The hit
    /// line is stripped and this message carries no original-byte field, so
    /// mixed context would render literal `^[[31m` beside clean text and could
    /// leave an SGR open with its reset stripped away.
    #[test]
    fn context_lines_are_stripped_like_the_hit_line() {
        let body = b"\x1b[33mwarning\x1b[0m above\nplain error line\n\x1b[31mbelow\x1b[0m\n";
        let hits = hits_in(body, &Query::literal("error").context(1));

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].before, vec![b"warning above".to_vec()]);
        assert_eq!(hits[0].after, vec![b"below".to_vec()]);
    }

    /// Locks out a lossy `String` round trip in the projection. One invalid
    /// byte would become a three-byte replacement character, growing the line
    /// and sliding `match_start` two bytes past the word it is meant to mark.
    #[test]
    fn invalid_utf8_does_not_shift_the_match_offsets() {
        let body = b"junk \xff\xfe here error there\n";
        let hits = hits_in(body, &Query::literal("error").context(0));

        assert_eq!(hits.len(), 1);
        let hit = &hits[0];
        assert_eq!(hit.visible, b"junk \xff\xfe here error there");
        assert_eq!(hit.match_start, 13);
        assert_eq!(hit.match_end, 18);
        assert_eq!(
            &hit.visible[hit.match_start as usize..hit.match_end as usize],
            b"error"
        );
    }

    /// Locks out a per-session share that rations a search the client already
    /// scoped to one session: it would hand back a quarter of the requested
    /// hits and flag them truncated, for fairness towards nobody.
    #[test]
    fn a_single_session_sweep_keeps_the_whole_budget() {
        assert_eq!(fairness_cap(1_000, 1), 1_000);
        assert_eq!(fairness_cap(1_000, 0), 1_000);
    }

    /// Locks out one chatty session consuming the entire budget before the
    /// others are looked at, which is the exact failure cross-session search
    /// exists to avoid.
    #[test]
    fn a_multi_session_sweep_rations_each_session() {
        assert_eq!(fairness_cap(1_000, 20), 250);
        // Floored, so a small cap still returns something usable per session
        // rather than one hit each.
        assert_eq!(fairness_cap(4, 20), 8);
    }

    /// Locks out a duplicated session id being swept twice, which would return
    /// its hits twice and spend double its share of the budget doing it.
    #[test]
    fn repeated_session_ids_are_swept_once() {
        let manager = SessionManager::new(1024);
        let ids = targets(
            &manager,
            &[SessionId(7), SessionId(3), SessionId(7), SessionId(3)],
        );
        assert_eq!(ids, vec![SessionId(3), SessionId(7)]);
    }

    /// Locks out an unsorted request order reaching the sweep. The global cap
    /// consumes sessions in the order they are pushed, so a client that lists
    /// `[9, 2]` would otherwise get a "first N" answer that starts at session 9.
    #[test]
    fn requested_sessions_are_swept_in_ascending_order() {
        let manager = SessionManager::new(1024);
        let ids = targets(&manager, &[SessionId(9), SessionId(2), SessionId(5)]);
        assert_eq!(ids, vec![SessionId(2), SessionId(5), SessionId(9)]);
    }

    /// Locks out an unusable pattern being silently downgraded to a literal
    /// search, which answers a different question and looks like success.
    #[tokio::test]
    async fn an_uncompilable_regex_is_refused_by_name() {
        let manager = Arc::new(SessionManager::new(1024));
        let query = query_from_wire("(unclosed", true, false, false, 0, 100);
        let msg = answer(manager, Vec::new(), query).await;

        let ServerMsg::Error { session, message, .. } = msg else {
            panic!("a bad regex must be refused, got {msg:?}");
        };
        assert_eq!(session, None, "a bad pattern belongs to no session");
        assert!(
            message.contains("(unclosed"),
            "the message must name the pattern: {message}"
        );
        assert!(
            message.contains("cannot compile"),
            "the message must name the problem: {message}"
        );
    }

    /// Locks out an empty pattern being answered with the entire scrollback of
    /// every session, which is what matching at every position means.
    #[tokio::test]
    async fn an_empty_pattern_is_refused_rather_than_matching_everything() {
        let manager = Arc::new(SessionManager::new(1024));
        let query = query_from_wire("", false, false, false, 0, 100);
        let msg = answer(manager, Vec::new(), query).await;

        let ServerMsg::Error { message, .. } = msg else {
            panic!("an empty pattern must be refused, got {msg:?}");
        };
        assert_eq!(message, "search pattern is empty");
    }

    /// Locks out `regex: false` letting metacharacters through. A user pasting
    /// `a.b` from a log line is searching for `a.b`, and treating the dot as a
    /// wildcard silently returns lines that do not contain what they pasted.
    #[test]
    fn a_literal_request_does_not_compile_metacharacters() {
        let literal = query_from_wire("a.b", false, false, false, 0, 10);
        let matcher = Matcher::compile(&literal).expect("a literal always compiles");
        assert!(matcher.find_at(b"axb", 0).is_none());
        assert!(matcher.find_at(b"a.b", 0).is_some());

        let regex = query_from_wire("a.b", true, false, false, 0, 10);
        let matcher = Matcher::compile(&regex).expect("a valid regex compiles");
        assert!(matcher.find_at(b"axb", 0).is_some());
    }
}
