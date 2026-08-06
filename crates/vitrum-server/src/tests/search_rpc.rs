//! The Search request, over a real socket against a running daemon.
//!
//! Every test here drives the daemon the way a client does: a WebSocket, real
//! child processes writing through real PTYs, and assertions on the bytes and
//! offsets that come back. Nothing calls a handler directly, because a search
//! that is correct in a unit test and unreachable over the wire is the failure
//! mode this whole file exists to catch.

/// Only the scale measurement times anything, and it needs a POSIX shell.
#[cfg(not(windows))]
use std::sync::Arc;
#[cfg(not(windows))]
use std::time::Instant;

use vitrum_proto::{ClientMsg, SearchHit, ServerMsg, SessionId};

use crate::tests::client::{Client, Harness, create};

/// Scrollback per session for the correctness tests. Small: they assert on
/// exact bytes, and nothing they produce comes close to evicting.
const SMALL_RING: usize = 64 * 1024;

/// A search request, as a struct so a test can override exactly one field.
///
/// `ClientMsg::Search` is an enum variant, which struct-update syntax cannot
/// extend, and spelling all seven fields out per test buries the one that
/// matters.
struct Req {
    sessions: Vec<SessionId>,
    pattern: String,
    regex: bool,
    case_insensitive: bool,
    whole_word: bool,
    context_lines: u16,
    max_hits: u32,
}

fn search(pattern: &str) -> Req {
    Req {
        sessions: Vec::new(),
        pattern: pattern.to_string(),
        regex: false,
        case_insensitive: false,
        whole_word: false,
        context_lines: 0,
        max_hits: 100,
    }
}

impl From<Req> for ClientMsg<'static> {
    fn from(req: Req) -> Self {
        ClientMsg::Search {
            sessions: req.sessions,
            pattern: req.pattern.into(),
            regex: req.regex,
            case_insensitive: req.case_insensitive,
            whole_word: req.whole_word,
            context_lines: req.context_lines,
            max_hits: req.max_hits,
        }
    }
}

/// Send a search and return the results, failing on an error reply.
async fn results_of(c: &mut Client, req: Req) -> (Vec<SearchHit>, bool, u64) {
    let before = c.seen.ctl.len();
    c.send(req.into()).await;
    c.until("the search results", |s| {
        s.ctl[before..]
            .iter()
            .any(|m| matches!(m, ServerMsg::SearchResults { .. } | ServerMsg::Error { .. }))
    })
    .await;
    match c.seen.ctl[before..]
        .iter()
        .find(|m| matches!(m, ServerMsg::SearchResults { .. } | ServerMsg::Error { .. }))
        .expect("the loop above only exits once one of the two has arrived")
    {
        ServerMsg::SearchResults {
            hits,
            truncated,
            bytes_scanned,
            ..
        } => (hits.clone(), *truncated, *bytes_scanned),
        ServerMsg::Error { message, .. } => panic!("search was refused: {message}"),
        _ => unreachable!(),
    }
}

/// The bytes a hit says are highlighted, sliced out of `visible` the way a
/// client renderer would.
fn highlighted(hit: &SearchHit) -> &[u8] {
    &hit.visible[hit.match_start as usize..hit.match_end as usize]
}

/// Run a script and wait for its child to exit.
///
/// The exit is the synchronisation point that matters: the daemon publishes
/// every last byte into scrollback before the session reports `Exited`, so a
/// search issued afterwards sees the whole output and never a partial line.
async fn run_to_exit(c: &mut Client, project: u64, script: &str) -> SessionId {
    let before = c.seen.exits();
    let id = c.create(create(project, script)).await;
    c.until("the script to finish", |s| s.exits() > before)
        .await;
    id
}

/// The wire path must reach the daemon on every platform vitrum ships to.
///
/// Every other test in this file needs a POSIX shell — `cmd.exe` has no
/// `printf`, so it cannot emit an escape sequence or a megabyte on demand —
/// and is compiled out on Windows. Leaving Windows with only unit tests would
/// reproduce exactly the failure this file exists to prevent: a feature that
/// is typed and tested everywhere and reachable nowhere. `echo` is the one
/// thing both shells agree on, so this runs there too.
#[tokio::test]
async fn a_search_reaches_the_daemon_on_every_platform() {
    let h = Harness::start(SMALL_RING).await;
    let mut c = h.greeted().await;
    run_to_exit(&mut c, 1, "echo needle here").await;

    let (hits, truncated, bytes_scanned) = results_of(&mut c, search("needle")).await;

    assert!(!truncated);
    assert_eq!(hits.len(), 1, "one line, one match: {hits:?}");
    assert_eq!(hits[0].line_seq, 0, "the only line starts the stream");
    assert_eq!(hits[0].visible, b"needle here");
    assert_eq!(hits[0].match_start, 0);
    assert_eq!(hits[0].match_end, 6);
    assert_eq!(highlighted(&hits[0]), b"needle");
    // Both shells write "needle here\r\n", and the sweep reports what it read.
    assert_eq!(bytes_scanned, 13);
}

/// A search must find lines across several sessions and place them exactly.
///
/// Every field is asserted against bytes computed by hand rather than against
/// whatever the server happened to send, because the point of the message is
/// that a client can scroll to `line_seq` and highlight `match_start..match_end`
/// without guessing.
#[cfg(not(windows))]
#[tokio::test]
async fn a_search_places_hits_exactly_across_several_sessions() {
    let h = Harness::start(SMALL_RING).await;
    let mut c = h.greeted().await;

    let alpha = run_to_exit(
        &mut c,
        1,
        "printf '%s\\n' 'alpha one' 'beta needle two' 'gamma three'",
    )
    .await;
    let quiet = run_to_exit(&mut c, 1, "printf '%s\\n' 'nothing to find here'").await;
    let charlie = run_to_exit(&mut c, 1, "printf '%s\\n' 'delta' 'needle in charlie'").await;

    let (hits, truncated, bytes_scanned) = results_of(&mut c, search("needle")).await;

    assert!(!truncated, "two hits are well inside a cap of a hundred");
    assert_eq!(hits.len(), 2, "one hit per matching session: {hits:?}");
    assert!(
        bytes_scanned >= 60,
        "every session's retained bytes were swept, got {bytes_scanned}"
    );

    // Ascending session id, so a capped answer would be a prefix of this.
    assert_eq!(hits[0].session, alpha);
    assert_eq!(hits[1].session, charlie);
    assert_ne!(hits[0].session, quiet, "the quiet session has no match");

    // "alpha one\r\n" is eleven bytes, so the matching line starts at 11.
    assert_eq!(hits[0].line_seq, 11);
    assert_eq!(hits[0].visible, b"beta needle two");
    assert_eq!(hits[0].match_start, 5);
    assert_eq!(hits[0].match_end, 11);
    assert_eq!(highlighted(&hits[0]), b"needle");

    // "delta\r\n" is seven bytes.
    assert_eq!(hits[1].line_seq, 7);
    assert_eq!(hits[1].visible, b"needle in charlie");
    assert_eq!(hits[1].match_start, 0);
    assert_eq!(hits[1].match_end, 6);
    assert_eq!(highlighted(&hits[1]), b"needle");
}

/// A match on a coloured line must be found, and reported against the text.
///
/// This is the whole reason the search strips escapes: a raw byte scan finds
/// `error` in `\x1b[31merror\x1b[0m` only by luck, and misses
/// `\x1b[31me\x1b[0mrror` entirely. It is also where the two coordinate systems
/// diverge — the match sits at visible byte 0 and at original byte 8 — so an
/// implementation that sent `original_range` would highlight the SGR introducer
/// instead of the word.
#[cfg(not(windows))]
#[tokio::test]
async fn a_match_inside_sgr_colour_codes_is_placed_by_visible_text() {
    let h = Harness::start(SMALL_RING).await;
    let mut c = h.greeted().await;

    // A real ESC byte in the script, so no shell has to agree about `\033`.
    run_to_exit(
        &mut c,
        1,
        "printf '%s\\n' 'plain first line' \
         '\x1b[1;31merror\x1b[0m: linker killed' \
         'plain last line'",
    )
    .await;

    let (hits, _, _) = results_of(&mut c, search("error")).await;
    assert_eq!(hits.len(), 1, "one coloured match: {hits:?}");
    let hit = &hits[0];

    // "plain first line\r\n" is eighteen original bytes.
    assert_eq!(hit.line_seq, 18);
    assert_eq!(
        hit.visible, b"error: linker killed",
        "the colour must be stripped out of the text the operator reads"
    );
    assert!(
        !hit.visible.contains(&0x1b),
        "no escape byte may survive into `visible`"
    );
    assert_eq!(
        hit.match_start, 0,
        "visible offset; the original byte offset is 8"
    );
    assert_eq!(hit.match_end, 5);
    assert_eq!(highlighted(hit), b"error");
}

/// The same coloured match must be reachable when the colour is inside the word.
///
/// `\x1b[31me\x1b[0mrror` is legal output that no raw byte scan can find, so
/// this is the case that proves the daemon matches text rather than bytes.
#[cfg(not(windows))]
#[tokio::test]
async fn a_colour_change_in_the_middle_of_a_word_does_not_hide_it() {
    let h = Harness::start(SMALL_RING).await;
    let mut c = h.greeted().await;

    run_to_exit(
        &mut c,
        1,
        "printf '%s\\n' 'build \x1b[31me\x1b[0mrror occurred'",
    )
    .await;

    let (hits, _, _) = results_of(&mut c, search("error")).await;
    assert_eq!(hits.len(), 1, "the split word is still one match: {hits:?}");
    assert_eq!(hits[0].visible, b"build error occurred");
    assert_eq!(hits[0].match_start, 6);
    assert_eq!(hits[0].match_end, 11);
    assert_eq!(highlighted(&hits[0]), b"error");
}

/// Context lines must arrive as text, like the hit line beside them.
///
/// The message carries no original-byte field, so a client renders these
/// directly. Shipping them with escapes intact would print literal `^[[33m`
/// under clean text, and an SGR opened in a context line whose reset lives on
/// the stripped hit line would bleed colour down the rest of the result list.
#[cfg(not(windows))]
#[tokio::test]
async fn context_lines_arrive_stripped_like_the_hit() {
    let h = Harness::start(SMALL_RING).await;
    let mut c = h.greeted().await;

    run_to_exit(
        &mut c,
        1,
        "printf '%s\\n' '\x1b[33mwarning\x1b[0m above' 'plain needle line' \
         '\x1b[32mokay\x1b[0m below'",
    )
    .await;

    let (hits, _, _) = results_of(
        &mut c,
        Req {
            context_lines: 1,
            ..search("needle")
        },
    )
    .await;

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].before, vec![b"warning above".to_vec()]);
    assert_eq!(hits[0].after, vec![b"okay below".to_vec()]);
}

/// An empty session list must mean every session, not no sessions.
///
/// A client's first search has no session filter, so reading the empty list as
/// "search nothing" would answer every unfiltered query with zero hits and look
/// exactly like "none of your agents mentioned it".
#[cfg(not(windows))]
#[tokio::test]
async fn an_empty_session_list_searches_every_session() {
    let h = Harness::start(SMALL_RING).await;
    let mut c = h.greeted().await;

    let first = run_to_exit(&mut c, 1, "printf '%s\\n' 'needle in first'").await;
    let second = run_to_exit(&mut c, 1, "printf '%s\\n' 'needle in second'").await;

    let (all, _, _) = results_of(&mut c, search("needle")).await;
    assert_eq!(all.len(), 2, "both sessions searched: {all:?}");
    assert_eq!(all[0].session, first);
    assert_eq!(all[1].session, second);

    // And a named list must restrict, or the filter is decoration.
    let (one, _, _) = results_of(
        &mut c,
        Req {
            sessions: vec![second],
            ..search("needle")
        },
    )
    .await;
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].session, second);
    assert_eq!(one[0].visible, b"needle in second");
}

/// A capped answer must say so, and must be the first hits rather than a sample.
///
/// Without `truncated` the UI says "2 matches" when there were fifty, which is
/// a wrong answer presented as a complete one. Without the ordering guarantee
/// "the first two" means "two of them", and paging is meaningless.
#[cfg(not(windows))]
#[tokio::test]
async fn a_capped_search_truncates_and_returns_the_first_hits() {
    let h = Harness::start(SMALL_RING).await;
    let mut c = h.greeted().await;

    let early = run_to_exit(&mut c, 1, "printf 'needle %s\\n' one two three four five").await;
    let late = run_to_exit(&mut c, 1, "printf 'needle %s\\n' six seven").await;

    let (hits, truncated, _) = results_of(
        &mut c,
        Req {
            max_hits: 3,
            ..search("needle")
        },
    )
    .await;

    assert!(truncated, "a cap that fired must be reported");
    assert_eq!(hits.len(), 3);
    assert!(
        hits.iter().all(|hit| hit.session == early),
        "the lowest session id is consumed first, so the later one is cut: {hits:?}"
    );
    assert_ne!(early, late);

    // "needle one\r\n" is twelve bytes, "needle two\r\n" another twelve.
    assert_eq!(hits[0].line_seq, 0);
    assert_eq!(hits[1].line_seq, 12);
    assert_eq!(hits[2].line_seq, 24);
    assert_eq!(hits[0].visible, b"needle one");
    assert_eq!(hits[2].visible, b"needle three");

    // The uncapped answer starts with exactly those three, which is what makes
    // the capped one a prefix rather than a sample.
    let (full, full_truncated, _) = results_of(&mut c, search("needle")).await;
    assert!(!full_truncated);
    assert_eq!(full.len(), 7);
    assert_eq!(full[..3], hits[..]);
}

/// A cap of zero must be honoured literally and cost nothing.
///
/// A client that only wants to know whether a pattern occurs sends this. The
/// daemon must not quietly substitute a default: it answers with no hits,
/// `truncated`, and no scrollback read at all.
#[cfg(not(windows))]
#[tokio::test]
async fn a_zero_cap_reads_no_scrollback() {
    let h = Harness::start(SMALL_RING).await;
    let mut c = h.greeted().await;
    run_to_exit(&mut c, 1, "printf '%s\\n' 'needle everywhere'").await;

    let (hits, truncated, bytes_scanned) = results_of(
        &mut c,
        Req {
            max_hits: 0,
            ..search("needle")
        },
    )
    .await;

    assert!(hits.is_empty());
    assert!(truncated, "zero of an unknown number of hits is truncated");
    assert_eq!(bytes_scanned, 0, "not one ring was locked or read");
}

/// A pattern that does not compile is the user's input, not a server fault.
///
/// It must come back named, the connection must survive it, and the next search
/// on the same connection must work — a half-typed regex in a search box is a
/// keystroke, not an incident.
#[cfg(not(windows))]
#[tokio::test]
async fn an_invalid_regex_is_refused_by_name_and_the_connection_survives() {
    let h = Harness::start(SMALL_RING).await;
    let mut c = h.greeted().await;
    run_to_exit(&mut c, 1, "printf '%s\\n' 'needle here'").await;

    c.send(
        Req {
            regex: true,
            ..search("(unclosed")
        }
        .into(),
    )
    .await;
    c.until("the refusal", |s| !s.errors().is_empty()).await;

    let refusal = c.seen.errors().pop().expect("an error must have arrived");
    assert!(
        refusal.contains("(unclosed"),
        "the refusal must name the pattern: {refusal}"
    );
    assert!(
        refusal.contains("cannot compile"),
        "the refusal must name the problem: {refusal}"
    );
    assert!(
        !c.seen.has(|m| matches!(m, ServerMsg::SearchResults { .. })),
        "a refused pattern must not also produce results"
    );

    // The same connection still answers a good pattern, so nothing was torn down.
    let (hits, _, _) = results_of(&mut c, search("needle")).await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].visible, b"needle here");
}

/// A valid regex must actually run as a regex, not as a literal.
///
/// Silently falling back would answer a different question than the one asked
/// and report success doing it.
#[cfg(not(windows))]
#[tokio::test]
async fn a_regex_search_matches_by_pattern_not_by_text() {
    let h = Harness::start(SMALL_RING).await;
    let mut c = h.greeted().await;
    run_to_exit(
        &mut c,
        1,
        "printf '%s\\n' 'exit code 137' 'exit code seven'",
    )
    .await;

    let (hits, _, _) = results_of(
        &mut c,
        Req {
            regex: true,
            ..search("code [0-9]+")
        },
    )
    .await;

    assert_eq!(hits.len(), 1, "only the numeric line matches: {hits:?}");
    assert_eq!(hits[0].visible, b"exit code 137");
    assert_eq!(highlighted(&hits[0]), b"code 137");
}

/// Ring capacity for the tests that need a ring to have wrapped.
#[cfg(not(windows))]
const TINY_RING: usize = 4 * 1024;

/// Ring capacity for the tests that need a sweep to take real time.
#[cfg(not(windows))]
const BIG_RING: usize = 8 * 1024 * 1024;

/// Roughly two megabytes of uninteresting output, then one findable line.
///
/// `head -c` cuts mid-line, so the marker opens with its own newline rather
/// than being glued onto the truncated one.
#[cfg(not(windows))]
const FILL_SCRIPT: &str = "yes 'lorem ipsum dolor sit amet consectetur adipiscing elit' \
                           | head -c 2000000; printf '\\n%s\\n' 'unique needle here'";

/// Sessions used for the scale measurement.
#[cfg(not(windows))]
const SCALE_SESSIONS: usize = 20;

/// Distinct payloads for the interleaving proof, one per sequential round trip.
#[cfg(not(windows))]
const ROUND_TRIPS: [&str; 5] = ["ping-one", "ping-two", "ping-3", "ping-4", "ping-5"];

/// Longest the interleaving proof will pin a ring lock.
///
/// A bound on a broken build, not a wait: the correct path releases the lock
/// from the test body after about 40 ms of round trips. It matches the
/// harness's own per-wait deadline so a genuinely starved client reports itself
/// through `until` rather than through this.
#[cfg(not(windows))]
const HOLD_CAP: std::time::Duration = std::time::Duration::from_secs(10);

/// Fill `count` sessions with [`FILL_SCRIPT`] and wait for all of them.
#[cfg(not(windows))]
async fn fill_sessions(c: &mut Client, project: u64, count: usize) -> Vec<SessionId> {
    let before = c.seen.exits();
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        ids.push(c.create(create(project, FILL_SCRIPT)).await);
    }
    c.until("every filler session to finish", |s| {
        s.exits() >= before + count
    })
    .await;
    ids
}

/// The seq one past the newest retained byte, straight from the daemon.
///
/// A zero-length scrollback request is clamped to the head, so its `from_seq`
/// is the head. Asking beats predicting it: the PTY turns every `\n` into
/// `\r\n`, so the byte count a script produces is not the byte count a test
/// could compute from the script.
#[cfg(not(windows))]
async fn head_seq(c: &mut Client, session: SessionId) -> u64 {
    chunk_start(c, session, 0).await
}

/// The seq of the oldest retained byte, straight from the daemon.
#[cfg(not(windows))]
async fn oldest_seq(c: &mut Client, session: SessionId) -> u64 {
    chunk_start(c, session, u32::MAX).await
}

#[cfg(not(windows))]
async fn chunk_start(c: &mut Client, session: SessionId, max_bytes: u32) -> u64 {
    let before = c.seen.ctl.len();
    c.send(ClientMsg::Scrollback {
        session,
        before_seq: u64::MAX,
        max_bytes,
    })
    .await;
    c.until("a scrollback chunk", |s| {
        s.ctl[before..]
            .iter()
            .any(|m| matches!(m, ServerMsg::ScrollbackChunk { .. }))
    })
    .await;
    c.seen.ctl[before..]
        .iter()
        .find_map(|m| match m {
            ServerMsg::ScrollbackChunk { from_seq, .. } => Some(*from_seq),
            _ => None,
        })
        .expect("the loop above only exits once a chunk has arrived")
}

/// A hit in a ring that has already evicted must be placed in stream
/// coordinates, not in ring coordinates.
///
/// This is the failure that corrupts every offset silently: a ring holding the
/// last 4 KiB of a 20 KiB session starts at index 0 in its own storage, and
/// reporting that as the seq points a client at bytes the agent wrote long
/// before the ones that matched. It gets worse the longer the session runs,
/// which is exactly when search is worth having.
#[cfg(not(windows))]
#[tokio::test]
async fn a_hit_in_a_wrapped_ring_is_placed_in_stream_coordinates() {
    let h = Harness::start(TINY_RING).await;
    let mut c = h.greeted().await;
    let id = run_to_exit(&mut c, 1, FILL_SCRIPT).await;

    let head = head_seq(&mut c, id).await;
    let oldest = oldest_seq(&mut c, id).await;
    assert!(
        oldest > 0,
        "the ring must have evicted for this test to mean anything"
    );
    assert_eq!(
        head - oldest,
        TINY_RING as u64,
        "a wrapped ring retains exactly its capacity"
    );

    let (hits, _, _) = results_of(&mut c, search("unique needle")).await;
    assert_eq!(hits.len(), 1, "one marker line: {hits:?}");
    let hit = &hits[0];

    // The marker is the last line: "unique needle here\r\n" is twenty bytes.
    assert_eq!(hit.visible, b"unique needle here");
    assert_eq!(hit.line_seq, head - 20);
    assert!(
        hit.line_seq > TINY_RING as u64,
        "a ring-relative offset would be under {TINY_RING}, got {}",
        hit.line_seq
    );
    assert!(hit.line_seq >= oldest, "the hit is inside what is retained");
    assert_eq!(highlighted(hit), b"unique needle");
}

/// Output must keep reaching an attached client while a search is in progress.
///
/// A full sweep is about 95 ms of pure CPU, measured. Spent on the runtime it
/// would stop every PTY coalescer and every output pump in the daemon, so
/// twenty agents would go silent to answer one search — the worst possible
/// trade for a terminal.
///
/// # Why this holds a lock instead of timing a big sweep
///
/// The obvious test — fill sessions, search, and check that echoes came back
/// sooner than the results — compares two wall-clock durations, and the margin
/// depends entirely on how loaded the machine is. It passed here at 37 ms of
/// round trips against a 538 ms sweep and failed on a busier box, because the
/// round trips inflate under load far more than the sweep does: five round
/// trips are about thirty scheduling hops across three processes, the sweep is
/// one CPU-bound thread. A test whose verdict depends on spare CPU is not a
/// test of concurrency.
///
/// So the search is stopped where the test can hold it: this thread takes one
/// session's ring lock through the same accessor the sweep uses, which parks
/// the sweep on its very first session until the lock is released. The search
/// is then provably in flight, and everything asserted below is a fact rather
/// than a race.
///
/// Falsified by inverting the implementation: sweeping inline instead of on a
/// blocking thread freezes the runtime the moment the daemon reads the request,
/// so the echoes never come back at all and this fails on the first `until`
/// rather than on a comparison. That failure does not depend on machine speed
/// either.
#[cfg(not(windows))]
#[tokio::test]
async fn output_keeps_flowing_to_an_attached_client_during_a_search() {
    let h = Harness::start(SMALL_RING).await;

    // Created first, so it holds the lowest session id and the sweep, which
    // walks sessions in ascending order, reaches it before anything else.
    let mut owner = h.greeted().await;
    let blocked = run_to_exit(&mut owner, 1, "echo needle in the parked session").await;

    // A live session on its own connection, attached and waiting for input.
    let mut worker = h.greeted().await;
    let echo = worker.create(create(2, "cat")).await;
    worker.attach(echo, 80, 24).await;

    // Take the parked session's ring lock from a real thread. A genuine sweep
    // holds each ring for about 4.8 ms; this holds one for as long as the
    // assertions need, which is the whole point.
    //
    // The hold is capped, and the cap is a safety valve rather than a wait. A
    // sweep that occupied the runtime would freeze this test too — the runtime
    // it needs to drive its own sockets AND its own timeouts is the one that is
    // blocked — so releasing only from the test body would deadlock and the
    // failure would surface as a hung suite instead of an assertion. Releasing
    // from this thread regardless lets the daemon finish, whereupon the
    // `is_finished` check below fails with the reason. The correct path never
    // reaches the cap: the round trips take about 40 ms.
    let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let manager = Arc::clone(&h.manager);
    let holder = std::thread::spawn(move || {
        manager
            .with_scrollback(blocked, |_, _, _| {
                held_tx.send(()).expect("the test is waiting for this");
                release_rx.recv_timeout(HOLD_CAP).is_ok()
            })
            .expect("the parked session exists")
    });
    held_rx.recv().expect("the holder thread took the lock");

    let mut searcher = h.greeted().await;
    searcher.send(search("needle").into()).await;
    let waiting = tokio::spawn(async move {
        searcher
            .until("the search results", |s| {
                s.has(|m| matches!(m, ServerMsg::SearchResults { .. }))
            })
            .await;
        searcher
    });

    // Sequential on purpose: each `until` returns only once that echo has come
    // all the way back through the PTY, the coalescer, the output pump and the
    // socket, so five of them are five full traversals of everything a sweep
    // could have stalled. None of them can complete if the runtime is blocked.
    for token in ROUND_TRIPS {
        worker
            .send(ClientMsg::Input {
                session: echo,
                data: format!("{token}\n").into_bytes().into(),
            })
            .await;
        worker
            .until("the echoed input", |s| {
                s.bytes(echo)
                    .windows(token.len())
                    .any(|w| w == token.as_bytes())
            })
            .await;
    }

    // The daemon has been servicing another connection throughout, so it has
    // certainly read this request by now — and it is still parked on the lock,
    // which is what makes the echoes above concurrent with the search rather
    // than merely before it.
    assert!(
        !waiting.is_finished(),
        "the search answered while its first ring was still locked, so the \
         sweep was not running concurrently with the echoes above: either it \
         never took that lock, or it occupied the runtime until the hold's \
         {HOLD_CAP:?} safety valve released it"
    );

    // A failed send means the valve already fired, which the assertion above
    // has ruled out.
    release_tx.send(()).expect("the holder thread is waiting");
    let released_by_test = holder.join().expect("the holder thread must not panic");
    assert!(
        released_by_test,
        "the hold was released by its own safety valve rather than by the test"
    );

    // And the search that was parked still returns the right answer.
    let searcher = waiting.await.expect("the searcher task must not panic");
    let ServerMsg::SearchResults { hits, .. } = searcher
        .seen
        .find(|m| matches!(m, ServerMsg::SearchResults { .. }))
        .expect("the task above only returns once the results have arrived")
    else {
        unreachable!("matched on the line above")
    };
    assert_eq!(hits.len(), 1, "only the parked session holds a needle");
    assert_eq!(hits[0].session, blocked);
    assert_eq!(hits[0].visible, b"needle in the parked session");
    assert_eq!(hits[0].match_start, 0);
    assert_eq!(hits[0].match_end, 6);
}

/// A search across twenty live sessions must reach every one of them.
///
/// Twenty agents is the shape vitrum is for, and this is the query the product
/// is sold on. It also reports what the round trip costs, which is the number
/// that decides whether a client can search on every keystroke.
#[cfg(not(windows))]
#[tokio::test]
async fn a_search_across_twenty_live_sessions_reaches_every_one() {
    let h = Harness::start(BIG_RING).await;
    let mut c = h.greeted().await;
    let ids = fill_sessions(&mut c, 1, SCALE_SESSIONS).await;

    let start = Instant::now();
    let (hits, truncated, swept) = results_of(&mut c, search("unique needle")).await;
    let elapsed = start.elapsed();

    eprintln!(
        "{SCALE_SESSIONS} sessions, {:.1} MiB swept, {} hits, round trip {elapsed:?}",
        swept as f64 / (1024.0 * 1024.0),
        hits.len()
    );

    assert!(!truncated, "twenty hits fit in a cap of a hundred");
    assert_eq!(hits.len(), SCALE_SESSIONS, "one marker per session");
    let found: Vec<SessionId> = hits.iter().map(|hit| hit.session).collect();
    let mut expected = ids;
    expected.sort_unstable();
    assert_eq!(found, expected, "every session, in ascending id order");
    assert!(
        hits.iter().all(|hit| hit.visible == b"unique needle here"),
        "every hit is the marker line"
    );
}
