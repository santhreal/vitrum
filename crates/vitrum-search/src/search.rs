//! The scan itself.
//!
//! # Shape of the loop
//!
//! One pass per session, one line at a time, and per line:
//!
//! 1. **Materialise** — borrowed straight out of the chunk unless the line
//!    straddles the ring join, in which case it is copied into a scratch that
//!    is reused for the whole search.
//! 2. **Feed pending context** — hits found in the last few lines are still
//!    collecting their after-context, and this line may be part of it.
//! 3. **Strip, if needed** — one `memchr3` decides. A line with no escapes is
//!    matched in place with an identity offset map.
//! 4. **Match** — against the visible text.
//! 5. **Remember** — the line span joins a fixed-size ring for before-context.
//!
//! Nothing in that list allocates. Steps 1 and 3 write into buffers that are
//! cleared rather than reallocated, steps 2 and 5 write into containers sized
//! once, and step 4 borrows. The only allocations in a whole search belong to
//! hits and their context, which are bounded by [`Query::max_hits`].
//!
//! # Ordering and the caps
//!
//! Sessions are scanned in ascending session id, and lines within a session in
//! ascending offset, which is exactly the documented result order. That is not
//! a coincidence — it is what lets the global cap early-exit and still promise
//! that the returned hits are a *prefix* of the full answer rather than an
//! arbitrary subset of it. A search capped at 100 hits returns the first 100 in
//! the same order an uncapped search would have produced them.
//!
//! When a cap fires the loop does not stop dead: it keeps walking just far
//! enough to finish the after-context of hits already found, then stops. A
//! truncated result with mutilated context would be worse than one hit fewer.

use std::collections::VecDeque;
use std::sync::LazyLock;

use crate::ansi::{Map, Stripper, needs_stripping};
use crate::chunks::{Chunked, Haystack, LineSpan, Lines};
use crate::error::Result;
use crate::hit::{ContextLine, Hit, SearchResults};
use crate::matcher::Matcher;
use crate::query::Query;

/// Search every haystack for `query`.
///
/// Haystacks may be supplied in any order; the result order does not depend on
/// it. Compiling the pattern is the only thing that can fail.
///
/// Every haystack must be borrowable at once. A daemon that holds each
/// session's ring behind its own lock cannot do that without blocking output on
/// all of them for the whole sweep, and should use [`Sweep`] instead.
pub fn search(query: &Query, haystacks: &[Haystack<'_>]) -> Result<SearchResults> {
    let matcher = Matcher::compile(query)?;
    search_with(&matcher, query, haystacks)
}

/// Search with an already-compiled matcher.
///
/// Worth having separately: a client typing in a search box reissues the same
/// pattern against a growing set of sessions, and recompiling a regex per
/// keystroke per session is the one avoidable cost in the whole path.
pub fn search_with(
    matcher: &Matcher,
    query: &Query,
    haystacks: &[Haystack<'_>],
) -> Result<SearchResults> {
    if worth_parallel(haystacks) {
        return search_with_parallel(matcher, query, haystacks);
    }
    // Scan in result order so the global cap yields a prefix, not a sample.
    let mut order: Vec<usize> = (0..haystacks.len()).collect();
    order.sort_by_key(|&index| (haystacks[index].session, haystacks[index].base_seq));

    let mut sweep = Sweep::with_matcher(matcher, query);
    for index in order {
        if !sweep.push(&haystacks[index]) {
            break;
        }
    }
    Ok(sweep.finish())
}

/// Below this many bytes, splitting the scan costs more than it saves.
///
/// Measured on this workload: at 389 KiB the threaded scan runs at 0.40x the
/// serial one because spawning dominates, at 1.5 MiB it reaches 1.13x, and by
/// 3 MiB it is 3.14x and still climbing to roughly 6.5x. The threshold sits
/// above the break-even point rather than on it, because a scan that is barely
/// worth splitting is not worth the threads.
const PARALLEL_MIN_BYTES: usize = 2 * 1024 * 1024;

/// Whether a scan is big enough to be worth splitting across threads.
///
/// The deciding quantity is bytes, not haystack count. Four idle sessions
/// holding a screen each are a few hundred kilobytes and lose badly to a
/// single-threaded scan; one session holding a long build log is worth
/// splitting on its own.
fn worth_parallel(haystacks: &[Haystack<'_>]) -> bool {
    if haystacks.len() < 2 {
        return false;
    }
    let mut total = 0usize;
    for haystack in haystacks {
        for chunk in haystack.chunks {
            total = total.saturating_add(chunk.len());
            if total >= PARALLEL_MIN_BYTES {
                return true;
            }
        }
    }
    false
}

/// Search haystacks in parallel across chunked worker threads.
pub fn search_parallel(query: &Query, haystacks: &[Haystack<'_>]) -> Result<SearchResults> {
    Ok(ParallelSearch::new(query)?.search(haystacks))
}

/// Search haystacks in parallel with an existing matcher across chunked worker threads.
pub fn search_with_parallel(
    matcher: &Matcher,
    query: &Query,
    haystacks: &[Haystack<'_>],
) -> Result<SearchResults> {
    Ok(ParallelSearch::with_matcher(matcher, query).search(haystacks))
}

/// Chunked parallel scrollback search iterator across multiple haystacks.
pub struct ParallelSearch<'a> {
    query: &'a Query,
    matcher: Compiled<'a>,
}

impl<'a> ParallelSearch<'a> {
    pub fn new(query: &'a Query) -> Result<Self> {
        let matcher = Matcher::compile(query)?;
        Ok(Self {
            query,
            matcher: Compiled::Owned(matcher),
        })
    }

    pub fn with_matcher(matcher: &'a Matcher, query: &'a Query) -> Self {
        Self {
            query,
            matcher: Compiled::Borrowed(matcher),
        }
    }

    /// Run chunked parallel search across `haystacks`.
    pub fn search(&self, haystacks: &[Haystack<'_>]) -> SearchResults {
        if haystacks.is_empty() {
            return SearchResults::default();
        }
        // Workers are partitioned along session boundaries, never by index
        // count: a session split across two workers would each enforce
        // `max_hits_per_session` independently and a single session could over
        // its allowance. Keeping every haystack of a session in one worker
        // preserves the documented per-session cap exactly.
        let mut ordered_indices: Vec<usize> =
            (0..haystacks.len()).collect();
        ordered_indices.sort_by_key(|&index| (haystacks[index].session, haystacks[index].base_seq));

        // A process does not change how many cores it has; asking per query
        // put a syscall on the keystroke path.
        static THREADS: LazyLock<usize> = LazyLock::new(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .max(1)
        });
        let num_threads = *THREADS;
        if num_threads == 1 || haystacks.len() <= 1 {
            return self.serialize_search(haystacks, &ordered_indices);
        }

        let mut session_bounds = Vec::new(); // index into ordered_indices where a new session starts
        session_bounds.push(0usize);
        for i in 1..ordered_indices.len() {
            if haystacks[ordered_indices[i]].session != haystacks[ordered_indices[i - 1]].session {
                session_bounds.push(i);
            }
        }
        // Split session_groups (slices of ordered_indices) across workers without
        // splitting a session.
        let mut partitions: Vec<(usize, usize)> = Vec::new(); // (start, end) of ordered_indices
        // We gather count of sessions so each worker takes a contiguous run of
        // session groups approximating `groups / num_threads`.
        let groups_per_worker = session_bounds.len().div_ceil(num_threads);
        let mut g = 0usize;
        while g < session_bounds.len() {
            let g_end = (g + groups_per_worker).min(session_bounds.len());
            let part_start = session_bounds[g];
            let part_end = if g_end < session_bounds.len() {
                session_bounds[g_end]
            } else {
                ordered_indices.len()
            };
            partitions.push((part_start, part_end));
            g = g_end;
        }

        let matcher_ref = self.matcher.get();
        let query_ref = self.query;

        let ordered = &ordered_indices;
        let thread_results: Vec<SearchResults> = std::thread::scope(|s| {
            let mut handles = Vec::new();
            for &(part_start, part_end) in &partitions {
                handles.push(s.spawn(move || {
                    let mut sweep = Sweep::with_matcher(matcher_ref, query_ref);
                    for &index in &ordered[part_start..part_end] {
                        if !sweep.push(&haystacks[index]) {
                            break;
                        }
                    }
                    sweep.finish()
                }));
            }
            // A worker panic is the caller's panic; resuming with the original
            // payload keeps the message and location instead of `Any { .. }`.
            handles
                .into_iter()
                .map(|h| match h.join() {
                    Ok(results) => results,
                    Err(payload) => std::panic::resume_unwind(payload),
                })
                .collect()
        });

        let mut total_lines = 0u64;
        let mut total_bytes = 0u64;
        let mut truncated = false;
        let mut all_hits: Vec<Hit> = Vec::new();
        for mut res in thread_results {
            total_lines += res.lines_scanned;
            total_bytes += res.bytes_scanned;
            truncated |= res.truncated;
            all_hits.append(&mut res.hits);
        }

        // Sort everything, then truncate, so a capped result is a true global
        // prefix: the first `max_hits` in the same order an uncapped search
        // would have produced.
        //
        // Today the sort cannot be observed: workers take contiguous ascending
        // ranges of `ordered_indices` and their results come back in the order
        // they were spawned, so the concatenation is already in result order,
        // and deleting this line passes every test. It stays because the cap
        // below is only a prefix if the input to it is ordered, and that would
        // otherwise be a property of how partitions happen to be built rather
        // than of this function. It costs a sort of at most `max_hits` items.
        all_hits.sort_by_key(Hit::order_key);
        truncated |= all_hits.len() > query_ref.max_hits;
        all_hits.truncate(query_ref.max_hits);

        let sessions_hit = all_hits
            .windows(2)
            .filter(|pair| pair[0].session != pair[1].session)
            .count()
            + usize::from(!all_hits.is_empty());

        SearchResults {
            hits: all_hits,
            lines_scanned: total_lines,
            bytes_scanned: total_bytes,
            sessions_hit,
            truncated,
        }
    }

    fn serialize_search(
        &self,
        haystacks: &[Haystack<'_>],
        ordered_indices: &[usize],
    ) -> SearchResults {
        let mut sweep = Sweep::with_matcher(self.matcher.get(), self.query);
        for &index in ordered_indices {
            if !sweep.push(&haystacks[index]) {
                break;
            }
        }
        sweep.finish()
    }
}

/// A sweep that takes its sessions one at a time.
///
/// # Why this exists
///
/// [`search`] needs every haystack borrowable simultaneously. The daemon cannot
/// arrange that: each session's ring lives behind its own lock, and borrowing
/// the bytes zero-copy means holding that lock for as long as the borrow lasts.
/// Twenty locks held across a full sweep is roughly a tenth of a second during
/// which no session can accept output — a stalled PTY pump on every agent, to
/// answer one search.
///
/// Taking one session at a time holds one lock for one session's worth of work,
/// which is about 5 ms for a 10 MiB ring. Staggered across twenty sessions that
/// is invisible, and no session waits on another.
///
/// What a caller must not hand-roll is the part that makes the answer
/// trustworthy: the global cap and the cross-session ordering. Reimplemented in
/// a daemon by hand, "the first 100 hits" quietly becomes "100 hits from
/// whichever sessions were visited first".
///
/// # Ordering
///
/// Push sessions in ascending session id. [`Sweep::finish`] sorts regardless,
/// so the returned order is always correct — but only ascending pushes make a
/// *capped* result a true prefix, because that is the order the cap consumes.
///
/// # Example
///
/// ```
/// use vitrum_search::{Haystack, Query, Sweep};
///
/// let query = Query::literal("OOM").context(0);
/// let mut sweep = Sweep::new(&query)?;
///
/// for (session, bytes) in [(1u64, &b"quiet\n"[..]), (2, &b"OOM here\n"[..])] {
///     // In the daemon this is where the session's ring lock is held, and it is
///     // released again before the next iteration.
///     let chunks = [bytes];
///     if !sweep.push(&Haystack { session, base_seq: 0, chunks: &chunks }) {
///         break; // the cap is full; later sessions would be discarded anyway
///     }
/// }
///
/// let results = sweep.finish();
/// assert_eq!(results.len(), 1);
/// assert_eq!(results.hits[0].session, 2);
/// # Ok::<(), vitrum_search::Error>(())
/// ```
pub struct Sweep<'a> {
    matcher: Compiled<'a>,
    query: &'a Query,
    state: ScanState,
    results: SearchResults,
    /// Set once the global cap is full, so a late `push` is a no-op rather than
    /// a scan whose hits are thrown away.
    full: bool,
}

/// A matcher this sweep either compiled or borrowed.
///
/// Deliberately not `Cow`, which would require `Matcher: Clone` and so make
/// every caller pay for a cloneable regex nobody clones.
enum Compiled<'a> {
    Owned(Matcher),
    Borrowed(&'a Matcher),
}

impl Compiled<'_> {
    #[inline]
    fn get(&self) -> &Matcher {
        match self {
            Compiled::Owned(matcher) => matcher,
            Compiled::Borrowed(matcher) => matcher,
        }
    }
}

impl<'a> Sweep<'a> {
    /// Compile `query` and start a sweep.
    pub fn new(query: &'a Query) -> Result<Self> {
        let matcher = Matcher::compile(query)?;
        Ok(Self::build(Compiled::Owned(matcher), query))
    }

    /// Start a sweep with an already-compiled matcher.
    pub fn with_matcher(matcher: &'a Matcher, query: &'a Query) -> Self {
        Self::build(Compiled::Borrowed(matcher), query)
    }

    fn build(matcher: Compiled<'a>, query: &'a Query) -> Self {
        // A cap is checked after a hit is pushed, which is what makes a capped
        // result a prefix rather than one hit short. That check cannot express
        // "zero", so a zero cap is honoured here instead of returning the single
        // hit the loop would otherwise keep. It is a real request: a client that
        // only wants to know *whether* a pattern occurs sets the cap to zero and
        // reads `truncated`.
        let empty = query.max_hits == 0 || query.max_hits_per_session == 0;
        // Enough to skip the first few doublings, not enough to matter if the
        // search finds nothing. A Hit is 160 bytes, so honouring a caller's
        // max_hits here would reserve 640 KiB for a cap of 4096 whether or not
        // a single line matches, which is a strange price for a daemon that is
        // measured on how little it holds.
        let hit_cap = query.max_hits.min(64);
        Self {
            state: ScanState::new(query),
            matcher,
            query,
            results: SearchResults {
                hits: Vec::with_capacity(hit_cap),
                truncated: empty,
                ..SearchResults::default()
            },
            full: empty,
        }
    }

    /// Search one session.
    ///
    /// Returns `false` once the global cap is full, meaning any further session
    /// would contribute nothing — a caller sweeping under a lock should stop
    /// rather than acquire the next one.
    pub fn push(&mut self, haystack: &Haystack<'_>) -> bool {
        if self.full {
            return false;
        }
        if scan_one(
            haystack,
            self.matcher.get(),
            self.query,
            &mut self.state,
            &mut self.results,
        ) {
            self.full = true;
            return false;
        }
        true
    }

    /// Whether the cap is already full, so nothing more can be contributed.
    ///
    /// [`Sweep::push`] answers the same question, but only after it has been
    /// handed a haystack — and a daemon has to acquire a ring's lock to produce
    /// one. Asking first is what lets it stop before taking a lock it does not
    /// need, and it is also what makes `max_hits == 0` free: such a sweep is
    /// born full, so no lock is taken and no scrollback is touched.
    pub fn is_full(&self) -> bool {
        self.full
    }

    /// Bytes swept so far, for a progress indicator.
    pub fn bytes_scanned(&self) -> u64 {
        self.results.bytes_scanned
    }

    /// Finish the sweep and order the results.
    pub fn finish(mut self) -> SearchResults {
        // Already in order when pushes were ascending; the sort makes the
        // guarantee unconditional, including for a caller that hands over two
        // haystacks for one session with interleaved base_seq values.
        self.results.hits.sort_by_key(Hit::order_key);

        // Hits are grouped by session after the sort, so counting boundaries is
        // the whole answer and there is no need to materialise the session list.
        self.results.sessions_hit = self
            .results
            .hits
            .windows(2)
            .filter(|pair| pair[0].session != pair[1].session)
            .count()
            + usize::from(!self.results.hits.is_empty());
        self.results
    }
}

/// Buffers reused across every session and every line of a single search.
struct ScanState {
    /// Holds a line that straddles a chunk boundary.
    join: Vec<u8>,
    /// Holds the escape-stripped text of the current line.
    stripper: Stripper,
    /// The last `context_before` line spans seen.
    before: VecDeque<LineSpan>,
    /// Indices into `results.hits` of hits still collecting after-context.
    pending: Vec<usize>,
    /// Session the previous haystack belonged to, so the per-session cap counts
    /// a session rather than a haystack.
    session: Option<u64>,
    /// Hits taken from `session` so far, across every haystack it was split into.
    session_hits: usize,
}

impl ScanState {
    fn new(query: &Query) -> Self {
        let pending_cap = query.max_hits.min(64);
        Self {
            join: Vec::with_capacity(2048),
            stripper: Stripper::new(),
            before: VecDeque::with_capacity(query.effective_context_before()),
            pending: Vec::with_capacity(pending_cap),
            session: None,
            session_hits: 0,
        }
    }
}

/// Scan one session. Returns true when the global cap has been reached.
fn scan_one(
    haystack: &Haystack<'_>,
    matcher: &Matcher,
    query: &Query,
    state: &mut ScanState,
    results: &mut SearchResults,
) -> bool {
    let view = Chunked::new(haystack.chunks);
    let context_before = query.effective_context_before();
    let context_after = query.effective_context_after();
    let total = haystack.len();

    state.before.clear();
    state.pending.clear();

    // A session's ring may arrive as several haystacks; the cap is documented
    // per session, so the count only resets when the session does.
    if state.session != Some(haystack.session) {
        state.session = Some(haystack.session);
        state.session_hits = 0;
    }
    if state.session_hits >= query.max_hits_per_session {
        // The session filled its allowance in an earlier haystack. Reading this
        // one could only produce hits that are thrown away.
        results.truncated = true;
        return false;
    }
    let mut scanned_to = 0u64;
    // Set when a cap fires: keep walking only to finish outstanding
    // after-context, then leave.
    let mut draining = false;
    let mut global_cap_reached = false;

    let mut chunk_possible_stack = [true; 16];
    let chunk_possible_vec: Vec<bool>;
    let chunk_possible: &[bool] = if haystack.chunks.len() <= 16 {
        for (i, chunk) in haystack.chunks.iter().enumerate() {
            chunk_possible_stack[i] = matcher.is_possible_match(chunk);
        }
        &chunk_possible_stack[..haystack.chunks.len()]
    } else {
        chunk_possible_vec = haystack
            .chunks
            .iter()
            .map(|chunk| matcher.is_possible_match(chunk))
            .collect();
        &chunk_possible_vec
    };

    for span in Lines::new(haystack.chunks) {
        results.lines_scanned += 1;
        scanned_to = (span.offset + span.len as u64 + 1).min(total);
        // Which chunks this line touches. A match cannot be missed by asking
        // only about them: the prefilter looks for the needle's first byte, and
        // that byte lies in exactly one chunk, which is inside this range even
        // when the match itself straddles a boundary.
        //
        // `locate` returns `None` exactly at end-of-data, which is where an
        // unterminated final line ends. That line still lives in the last
        // chunk, so the end falls back to it rather than to the start chunk,
        // which would collapse the range and could drop a straddling match.
        let (chunk_start, _) = view.locate(span.offset).unwrap_or((0, 0));
        let (chunk_end, _) = view
            .locate(span.offset + span.len as u64)
            .unwrap_or((haystack.chunks.len().saturating_sub(1), 0));
        let is_possible = (chunk_start..=chunk_end)
            .any(|idx| chunk_possible.get(idx).copied().unwrap_or(true));
        if state.pending.is_empty() && !is_possible {
            if context_before > 0 {
                if state.before.len() == context_before {
                    state.before.pop_front();
                }
                state.before.push_back(span);
            }
            continue;
        }

        let bytes = view.materialize(span, &mut state.join);

        // Outstanding after-context first: this line may belong to a hit found
        // one or two lines ago.
        let mut slot = 0;
        while slot < state.pending.len() {
            let hit = state.pending[slot];
            results.hits[hit].after.push(ContextLine {
                seq: haystack.base_seq + span.offset,
                index: span.index,
                bytes: bytes.to_vec(),
            });
            if results.hits[hit].after.len() >= context_after {
                state.pending.swap_remove(slot);
            } else {
                slot += 1;
            }
        }

        if draining {
            if state.pending.is_empty() {
                break;
            }
            continue;
        }

        // A line with no escapes and no stray control bytes is matched exactly
        // as it sits in the ring, with no copy and no coordinate translation.
        let (visible, map) = if needs_stripping(bytes) {
            state.stripper.fill(bytes);
            (state.stripper.text(), state.stripper.map())
        } else {
            (bytes, Map::Identity)
        };

        let mut from = 0usize;
        while let Some(range) = matcher.find_at(visible, from) {
            let original_range = map.range(range.clone());
            let line_seq = haystack.base_seq + span.offset;

            // `collect` from an ExactSizeIterator already reserves exactly
            // once, so spelling the loop out by hand buys nothing.
            let before = state
                .before
                .iter()
                .map(|&context| ContextLine {
                    seq: haystack.base_seq + context.offset,
                    index: context.index,
                    bytes: copy_of(&view, context),
                })
                .collect();

            results.hits.push(Hit {
                session: haystack.session,
                line_seq,
                match_seq: line_seq + original_range.start as u64,
                original_range,
                visible_range: range.clone(),
                line_index: span.index,
                line: bytes.to_vec(),
                visible: visible.to_vec(),
                before,
                after: Vec::with_capacity(context_after),
            });
            state.session_hits += 1;
            if context_after > 0 {
                state.pending.push(results.hits.len() - 1);
            }

            if results.hits.len() >= query.max_hits {
                global_cap_reached = true;
                results.truncated = true;
                draining = true;
                break;
            }
            if state.session_hits >= query.max_hits_per_session {
                results.truncated = true;
                draining = true;
                break;
            }
            if !query.all_matches_per_line {
                break;
            }
            // A zero-width match would otherwise pin `from` in place forever.
            from = if range.end > range.start {
                range.end
            } else {
                range.end + 1
            };
            if from > visible.len() {
                break;
            }
        }

        if draining && state.pending.is_empty() {
            break;
        }

        if context_before > 0 {
            if state.before.len() == context_before {
                state.before.pop_front();
            }
            state.before.push_back(span);
        }
    }

    results.bytes_scanned += scanned_to;
    global_cap_reached
}

/// The bytes of `span` as an owned buffer.
///
/// Deliberately not routed through the scan's straddle scratch: that scratch is
/// mutably borrowed by the current line while a hit is being built, and this
/// allocates a `Vec` the hit is going to keep regardless.
fn copy_of(view: &Chunked<'_>, span: LineSpan) -> Vec<u8> {
    let mut out = Vec::with_capacity(span.len);
    view.copy_into(span, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(session: u64, base_seq: u64, bytes: &[u8]) -> Vec<u8> {
        let _ = (session, base_seq);
        bytes.to_vec()
    }

    /// Locks out the scan reporting fewer bytes than it read, which is what the
    /// benchmark's MB/s figure is computed from.
    #[test]
    fn a_complete_scan_reports_the_whole_haystack() {
        let data = one(1, 0, b"alpha\nbeta\ngamma\n");
        let chunks: &[&[u8]] = &[&data];
        let results = search(
            &Query::literal("nothing-here"),
            &[Haystack {
                session: 1,
                base_seq: 0,
                chunks,
            }],
        )
        .expect("search");
        assert_eq!(results.bytes_scanned, data.len() as u64);
        assert_eq!(results.lines_scanned, 3);
        assert!(results.is_empty());
        assert!(!results.truncated);
        assert_eq!(results.sessions_hit, 0);
    }

    /// Locks out an unterminated final line inflating the byte count past the
    /// end of the buffer, which would report throughput above what was read.
    #[test]
    fn byte_count_never_exceeds_the_haystack() {
        let data = one(1, 0, b"alpha\nno trailing newline");
        let chunks: &[&[u8]] = &[&data];
        let results = search(
            &Query::literal("zzz"),
            &[Haystack {
                session: 1,
                base_seq: 0,
                chunks,
            }],
        )
        .expect("search");
        assert_eq!(results.bytes_scanned, data.len() as u64);
        assert_eq!(results.lines_scanned, 2);
    }

    /// Locks out `sessions_hit` counting haystacks rather than distinct
    /// sessions, which is what "3 of your 20 agents mentioned OOM" reports.
    #[test]
    fn sessions_hit_counts_distinct_sessions_not_matches() {
        let a = one(1, 0, b"OOM\nOOM\nOOM\n");
        let b = one(2, 0, b"quiet\n");
        let c = one(3, 0, b"OOM\n");
        let (ca, cb, cc): (&[u8], &[u8], &[u8]) = (&a, &b, &c);
        let results = search(
            &Query::literal("OOM").all_matches_per_line(true),
            &[
                Haystack {
                    session: 1,
                    base_seq: 0,
                    chunks: std::slice::from_ref(&ca),
                },
                Haystack {
                    session: 2,
                    base_seq: 0,
                    chunks: std::slice::from_ref(&cb),
                },
                Haystack {
                    session: 3,
                    base_seq: 0,
                    chunks: std::slice::from_ref(&cc),
                },
            ],
        )
        .expect("search");
        assert_eq!(results.len(), 4);
        assert_eq!(results.sessions_hit, 2);
    }

    /// Locks out a zero cap returning one hit. The cap is checked after a push,
    /// so without an explicit guard `max_hits: 0` yields exactly one result —
    /// the most confusing possible answer to "give me none".
    #[test]
    fn a_zero_cap_returns_nothing_and_reports_truncation() {
        let data = one(1, 0, b"OOM\nOOM\nOOM\n");
        let chunks: &[&[u8]] = &[&data];
        let haystacks = [Haystack {
            session: 1,
            base_seq: 0,
            chunks,
        }];

        let global = search(&Query::literal("OOM").max_hits(0), &haystacks).expect("search");
        assert!(global.is_empty());
        assert!(global.truncated);
        assert_eq!(global.bytes_scanned, 0);

        let per_session =
            search(&Query::literal("OOM").max_hits_per_session(0), &haystacks).expect("search");
        assert!(per_session.is_empty());
        assert!(per_session.truncated);
    }

    /// Locks out a cap of one silently becoming a cap of two, which is the
    /// off-by-one the zero guard could easily introduce.
    #[test]
    fn a_cap_of_one_returns_exactly_one() {
        let data = one(1, 0, b"OOM\nOOM\nOOM\n");
        let chunks: &[&[u8]] = &[&data];
        let results = search(
            &Query::literal("OOM").context(0).max_hits(1),
            &[Haystack {
                session: 1,
                base_seq: 0,
                chunks,
            }],
        )
        .expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results.hits[0].match_seq, 0);
        assert!(results.truncated);
    }

    /// Bodies shaped so several sessions have hits, at different line offsets,
    /// with colour so the stripping path is crossed too.
    fn sessions() -> Vec<Vec<u8>> {
        vec![
            b"boot\n\x1b[31mOOM\x1b[0m one\nquiet\nOOM two\n".to_vec(),
            b"nothing interesting here\n".to_vec(),
            b"a\nb\nc\nOOM three\nd\n".to_vec(),
            b"OOM four\nOOM five\n".to_vec(),
        ]
    }

    /// Locks out incremental sweeping diverging from the batch path by so much
    /// as one byte. The daemon has no choice but to sweep one session at a time
    /// — it cannot hold twenty ring locks at once — so if these two disagree,
    /// the results a user actually sees are not the ones this crate's whole test
    /// suite covers.
    #[test]
    fn sweeping_one_session_at_a_time_equals_searching_them_together() {
        let bodies = sessions();
        let slices: Vec<&[u8]> = bodies.iter().map(Vec::as_slice).collect();
        let query = Query::literal("OOM").context(1).all_matches_per_line(true);

        let together: Vec<Haystack<'_>> = slices
            .iter()
            .enumerate()
            .map(|(index, body)| Haystack {
                session: index as u64,
                base_seq: index as u64 * 1_000,
                chunks: std::slice::from_ref(body),
            })
            .collect();
        let batch = search(&query, &together).expect("search");

        let mut sweep = Sweep::new(&query).expect("compile");
        for (index, body) in slices.iter().enumerate() {
            // Each iteration borrows one body and drops the borrow again, which
            // is what the daemon does with one lock at a time.
            let chunks = [*body];
            assert!(sweep.push(&Haystack {
                session: index as u64,
                base_seq: index as u64 * 1_000,
                chunks: &chunks,
            }));
        }
        let incremental = sweep.finish();

        assert_eq!(incremental.hits.len(), 5);
        assert_eq!(incremental, batch);
    }

    /// Locks out the sweep losing the equivalence when each session arrives as
    /// two ring halves rather than one contiguous buffer, which is the only
    /// shape the daemon will ever actually hand over.
    #[test]
    fn sweeping_ring_halves_equals_sweeping_whole_buffers() {
        let bodies = sessions();
        let query = Query::literal("OOM").context(1).all_matches_per_line(true);

        let mut whole = Sweep::new(&query).expect("compile");
        for (index, body) in bodies.iter().enumerate() {
            let chunks = [body.as_slice()];
            whole.push(&Haystack {
                session: index as u64,
                base_seq: 0,
                chunks: &chunks,
            });
        }

        let mut halved = Sweep::new(&query).expect("compile");
        for (index, body) in bodies.iter().enumerate() {
            let cut = body.len() / 2;
            let chunks = [&body[..cut], &body[cut..]];
            halved.push(&Haystack {
                session: index as u64,
                base_seq: 0,
                chunks: &chunks,
            });
        }

        assert_eq!(whole.finish(), halved.finish());
    }

    /// Locks out `push` claiming there is room when the global cap is already
    /// full. A daemon uses that answer to decide whether to acquire the next
    /// session's lock at all, so a wrong `true` costs a lock nobody needed.
    #[test]
    fn push_reports_the_cap_so_a_caller_can_stop_acquiring_locks() {
        let bodies = sessions();
        let query = Query::literal("OOM").context(0).max_hits(2);
        let mut sweep = Sweep::new(&query).expect("compile");

        // Session 0 holds two matches, which fills a cap of two exactly.
        let first = [bodies[0].as_slice()];
        assert!(
            !sweep.push(&Haystack {
                session: 0,
                base_seq: 0,
                chunks: &first
            }),
            "a full cap must be reported on the push that fills it"
        );

        // Every later push is refused without scanning, so the byte count does
        // not grow either.
        let scanned = sweep.bytes_scanned();
        let later = [bodies[3].as_slice()];
        assert!(!sweep.push(&Haystack {
            session: 3,
            base_seq: 0,
            chunks: &later
        }));
        assert_eq!(sweep.bytes_scanned(), scanned);

        let results = sweep.finish();
        assert_eq!(results.len(), 2);
        assert!(results.truncated);
        assert!(
            results.hits.iter().all(|hit| hit.session == 0),
            "the refused session must contribute nothing"
        );
    }

    /// Locks out a zero cap scanning anything. A client that only wants to know
    /// whether a pattern occurs anywhere must not cost a full sweep, and must
    /// not make the daemon take a single lock.
    #[test]
    fn a_zero_cap_sweep_refuses_every_session_without_scanning() {
        let bodies = sessions();
        let query = Query::literal("OOM").max_hits(0);
        let mut sweep = Sweep::new(&query).expect("compile");
        let chunks = [bodies[0].as_slice()];
        assert!(!sweep.push(&Haystack {
            session: 0,
            base_seq: 0,
            chunks: &chunks
        }));
        assert_eq!(sweep.bytes_scanned(), 0);
        let results = sweep.finish();
        assert!(results.is_empty());
        assert!(results.truncated);
        assert_eq!(results.lines_scanned, 0);
    }

    /// Locks out `is_full` disagreeing with `push`, which would put the daemon
    /// back to acquiring a ring lock just to be told there was no room. A zero
    /// cap must read as full before the first push, and a filled cap must read
    /// as full after it.
    #[test]
    fn is_full_answers_before_a_lock_is_taken() {
        let bodies = sessions();

        let zero = Query::literal("OOM").max_hits(0);
        let sweep = Sweep::new(&zero).expect("compile");
        assert!(
            sweep.is_full(),
            "a zero cap is full before anything is pushed"
        );

        let two = Query::literal("OOM").context(0).max_hits(2);
        let mut sweep = Sweep::new(&two).expect("compile");
        assert!(!sweep.is_full(), "a fresh cap of two has room");
        let first = [bodies[0].as_slice()];
        sweep.push(&Haystack {
            session: 0,
            base_seq: 0,
            chunks: &first,
        });
        assert!(sweep.is_full(), "two hits filled a cap of two");
    }

    /// Locks out a bad pattern being discovered halfway through a sweep, after
    /// locks have already been taken and released for several sessions.
    #[test]
    fn a_bad_pattern_fails_at_construction_not_on_first_push() {
        let query = Query::regex("(unclosed");
        assert!(matches!(
            Sweep::new(&query),
            Err(crate::error::Error::BadPattern { .. })
        ));
    }

    /// Locks out the per-session cap being applied as a running total across
    /// the sweep, which would let session 0 exhaust the allowance of every
    /// session after it.
    #[test]
    fn the_per_session_cap_resets_for_each_pushed_session() {
        let bodies = sessions();
        let query = Query::literal("OOM")
            .context(0)
            .all_matches_per_line(true)
            .max_hits_per_session(1);
        let mut sweep = Sweep::new(&query).expect("compile");
        for (index, body) in bodies.iter().enumerate() {
            let chunks = [body.as_slice()];
            sweep.push(&Haystack {
                session: index as u64,
                base_seq: 0,
                chunks: &chunks,
            });
        }
        let results = sweep.finish();
        // Sessions 0, 2 and 3 each have matches; each contributes exactly one.
        assert_eq!(
            results.hits.iter().map(|h| h.session).collect::<Vec<_>>(),
            vec![0, 2, 3]
        );
        assert_eq!(results.sessions_hit, 3);
    }

    /// WHY: the per-session cap was counted per haystack, not per session, so a
    /// session handed over in two pieces returned twice its documented
    /// allowance. The daemon splits a wrapped ring exactly that way, so a cap
    /// of 200 quietly became 400 for every session that had wrapped, and the
    /// global budget was consumed by the chattiest sessions after all.
    #[test]
    fn the_per_session_cap_spans_every_haystack_of_one_session() {
        let first = b"OOM one\nOOM two\nOOM three\n";
        let second = b"OOM four\nOOM five\n";
        let query = Query::literal("OOM").context(0).max_hits_per_session(2);

        let mut sweep = Sweep::new(&query).expect("compile");
        for (base_seq, body) in [(0u64, &first[..]), (first.len() as u64, &second[..])] {
            let chunks = [body];
            assert!(sweep.push(&Haystack {
                session: 4,
                base_seq,
                chunks: &chunks,
            }));
        }
        let results = sweep.finish();

        assert_eq!(results.len(), 2, "the cap is two hits for session 4");
        assert_eq!(
            results
                .hits
                .iter()
                .map(|hit| hit.visible_lossy())
                .collect::<Vec<_>>(),
            vec!["OOM one", "OOM two"],
            "the kept hits must be the first two, not an arbitrary pair"
        );
        assert!(results.truncated);
        assert_eq!(results.sessions_hit, 1);
        // The second haystack is never read: its bytes cannot contribute.
        assert_eq!(results.lines_scanned, 2);
    }

    #[test]
    fn chunk_prefilter_skips_non_matching_lines_correctly() {
        let chunk1 = b"line 1: quiet\nline 2: quiet\nline 3: TARGET match\nline 4: quiet\n";
        let chunk2 = b"line 5: no match\nline 6: no match\n";
        let query = Query::literal("TARGET").context(0);

        let mut sweep = Sweep::new(&query).expect("compile");
        assert!(sweep.push(&Haystack {
            session: 1,
            base_seq: 0,
            chunks: &[chunk1, chunk2],
        }));
        let results = sweep.finish();
        assert_eq!(results.len(), 1);
        assert_eq!(results.hits[0].visible_lossy(), "line 3: TARGET match");
    }

    #[test]
    fn chunk_prefilter_handles_ascii_casefold_literals() {
        let chunk1 = b"line 1: quiet\nline 2: target hit\n";
        let chunk2 = b"line 3: still quiet\n";
        let query = Query::literal("TARGET").case_insensitive(true).context(0);
        let matcher = Matcher::compile(&query).expect("compile");
        assert!(matcher.is_ascii_casefold());

        let mut sweep = Sweep::with_matcher(&matcher, &query);
        assert!(sweep.push(&Haystack {
            session: 1,
            base_seq: 0,
            chunks: &[chunk1, chunk2],
        }));
        let results = sweep.finish();
        assert_eq!(results.len(), 1);
        assert_eq!(results.hits[0].visible_lossy(), "line 2: target hit");
    }
    #[test]
    fn chunk_prefilter_does_not_lose_straddling_final_line() {
        // The final line `XYZZZZ` straddles the two chunks: it starts in
        // `chunk1` and ends in `chunk2`, and has no trailing newline. The
        // needle's first byte (`Z`) appears only in `chunk2`, and the first
        // chunk's tail (`XY`) contains no `Z`, so a prefilter that collapsed
        // the span's chunk range to its start chunk would reject the line and
        // silently drop the hit. `locate` returns None exactly at end-of-data,
        // which is where this unterminated line ends — the range must fall
        // back to the last chunk, not the start chunk.
        let chunk1 = b"abc\nXY";
        let chunk2 = b"ZZZZ";
        let query = Query::literal("ZZ").context(0);

        let mut sweep = Sweep::new(&query).expect("compile");
        assert!(sweep.push(&Haystack {
            session: 1,
            base_seq: 0,
            chunks: &[chunk1, chunk2],
        }));
        let results = sweep.finish();
        assert_eq!(results.len(), 1);
        assert_eq!(results.hits[0].visible_lossy(), "XYZZZZ");
        assert_eq!(results.hits[0].original_range, 2..4);
    }

    #[test]
    fn chunk_prefilter_keeps_zero_length_final_line_safe() {
        // A trailing empty line ends exactly at end-of-data; `locate` is None
        // for it too, and the range must not underflow or panic.
        let chunk1 = b"target\n";
        let chunk2 = b"";
        let query = Query::literal("target").context(0);

        let mut sweep = Sweep::new(&query).expect("compile");
        assert!(sweep.push(&Haystack {
            session: 1,
            base_seq: 0,
            chunks: &[chunk1, chunk2],
        }));
        let results = sweep.finish();
        assert_eq!(results.len(), 1);
        assert_eq!(results.hits[0].visible_lossy(), "target");
    }

    #[test]
    fn a_small_scan_is_not_worth_splitting() {
        // Four idle sessions holding a screen each: the threaded scan measured
        // 0.40x the serial one at this size, so this must stay serial.
        let screen = vec![b'x'; 96 * 1024];
        let chunks: Vec<[&[u8]; 1]> = (0..4).map(|_| [screen.as_slice()]).collect();
        let hays: Vec<Haystack<'_>> = chunks
            .iter()
            .enumerate()
            .map(|(i, c)| Haystack { session: i as u64, base_seq: 0, chunks: c })
            .collect();
        assert!(!worth_parallel(&hays));
    }

    #[test]
    fn a_long_log_is_worth_splitting_even_as_one_session() {
        // Bytes decide, not haystack count: two haystacks clearing the
        // threshold qualify where four small ones did not.
        let log = vec![b'x'; PARALLEL_MIN_BYTES];
        let chunks = [log.as_slice()];
        let hays = [
            Haystack { session: 1, base_seq: 0, chunks: &chunks },
            Haystack { session: 1, base_seq: 1, chunks: &chunks },
        ];
        assert!(worth_parallel(&hays));
    }

    #[test]
    fn one_haystack_never_pays_for_threads() {
        // A single haystack goes to one worker no matter how big it is, so
        // splitting could only add cost.
        let log = vec![b'x'; PARALLEL_MIN_BYTES * 4];
        let chunks = [log.as_slice()];
        let hays = [Haystack { session: 1, base_seq: 0, chunks: &chunks }];
        assert!(!worth_parallel(&hays));
    }

    #[test]
    fn parallel_search_yields_correct_ordered_results() {
        let s1 = b"session 1: error A\nsession 1: quiet\n";
        let s2 = b"session 2: quiet\nsession 2: error B\n";
        let s3 = b"session 3: error C\n";
        let s4 = b"session 4: error D\n";

        let c1 = [s1.as_slice()];
        let c2 = [s2.as_slice()];
        let c3 = [s3.as_slice()];
        let c4 = [s4.as_slice()];

        let haystacks = [
            Haystack { session: 3, base_seq: 0, chunks: &c3 },
            Haystack { session: 1, base_seq: 0, chunks: &c1 },
            Haystack { session: 4, base_seq: 0, chunks: &c4 },
            Haystack { session: 2, base_seq: 0, chunks: &c2 },
        ];

        let query = Query::literal("error").context(0);
        let results = search_parallel(&query, &haystacks).expect("search parallel");

        assert_eq!(results.len(), 4);
        assert_eq!(
            results.hits.iter().map(|h| h.session).collect::<Vec<_>>(),
            vec![1, 2, 3, 4],
            "parallel search must return hits ordered by session"
        );
    }

    #[test]
    fn parallel_per_session_cap_spans_split_session() {
        // One session arrives as two haystacks; workers are partitioned along
        // session boundaries, so `max_hits_per_session` must hold across both
        // halves — not be applied independently per worker.
        let half1 = b"err a\nerr b\nerr c\nerr d\n";
        let half2 = b"err e\nerr f\nerr g\n";
        let c1 = [half1.as_slice()];
        let c2 = [half2.as_slice()];
        let c3 = [b"quiet\n".as_slice()];
        let c4 = [b"quiet\n".as_slice()];

        let haystacks = [
            Haystack { session: 1, base_seq: 0, chunks: &c1 },
            Haystack { session: 1, base_seq: 40, chunks: &c2 },
            Haystack { session: 2, base_seq: 0, chunks: &c3 },
            Haystack { session: 3, base_seq: 0, chunks: &c4 },
        ];

        let query = Query::literal("err").context(0).max_hits_per_session(3).max_hits(100);
        let results = search_parallel(&query, &haystacks).expect("search parallel");

        let session1 = results.hits.iter().filter(|h| h.session == 1).count();
        assert_eq!(session1, 3, "session split across two haystacks must still honor its cap");
    }

    #[test]
    fn parallel_global_cap_is_a_true_prefix() {
        // With more hits than `max_hits`, the parallel result must equal the
        // sequential one: the first `max_hits` in result order, not an
        // arbitrary subset shaped by worker assignment.
        let bytes: &[u8] = b"hit\nhit\nhit\nhit\nhit\n";
        let chunks = [bytes];
        let mut hays = Vec::new();
        for session in 0..4u64 {
            hays.push(Haystack { session, base_seq: 0, chunks: &chunks });
        }

        let query = Query::literal("hit").context(0).max_hits(7).max_hits_per_session(200);
        let parallel = search_parallel(&query, &hays).expect("parallel");
        // Driven directly rather than through `search`, which is free to pick
        // the parallel path itself and would leave this comparing it to itself.
        let sequential = {
            let mut order: Vec<usize> = (0..hays.len()).collect();
            order.sort_by_key(|&i| (hays[i].session, hays[i].base_seq));
            let mut sweep = Sweep::new(&query).expect("compile");
            for i in order {
                if !sweep.push(&hays[i]) {
                    break;
                }
            }
            sweep.finish()
        };

        assert_eq!(parallel.hits.len(), 7);
        assert_eq!(parallel.hits.len(), sequential.hits.len());
        for (p, s) in parallel.hits.iter().zip(sequential.hits.iter()) {
            assert_eq!((p.session, p.match_seq), (s.session, s.match_seq));
        }
        assert!(parallel.truncated);
    }
}
