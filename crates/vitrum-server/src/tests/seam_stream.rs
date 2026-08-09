//! Bytes crossing every seam at once: PTY, coalescer, broadcast, wire, client.
//!
//! The per-crate suites each prove their own half. `vitrum-core` proves a burst
//! reaches the broadcast channel; `attach_stream.rs` proves an attached client
//! receives frames. Neither proves that a burst reaches an attached CLIENT,
//! which is the only claim an operator can see, and that gap is exactly where
//! the shipped defect lived: a child wrote a few hundred lines, exited, and the
//! pane stayed empty.
//!
//! Everything here therefore runs a real child in a real PTY behind a real
//! daemon, and asserts on bytes that came off a websocket.
//!
//! Unix only. Every case needs a child that emits an exact, chosen byte
//! sequence and then blocks; `cmd.exe /C` can neither emit an ESC portably nor
//! offer a `read` that holds the session open, so on Windows these would be
//! testing the shell. `vitrum-core`'s `pty_burst_exit.rs` states the same
//! restriction for the same reason.

#![cfg(not(windows))]

use vitrum_proto::{ClientMsg, ServerMsg, SessionId};

use crate::tests::client::{Client, Harness, create};

/// Separates the setup noise from the bytes a case is about.
///
/// A gated child echoes the newline that released it, and the shell may print
/// nothing else, but "may" is not a contract. Anchoring on a marker the payload
/// cannot contain lets every assertion below be an exact comparison of the
/// whole remaining stream rather than a search for a substring inside it.
const MARK: &[u8] = b"<<GO>>";

/// A script that waits for a newline, prints [`MARK`], then runs `then`.
fn gated(then: &str) -> String {
    format!("read -r _; printf '<<GO>>'; {then}")
}

/// The bytes after the LAST occurrence of [`MARK`], or a failure naming what
/// did arrive.
fn after_mark(stream: &[u8]) -> &[u8] {
    let at = stream
        .windows(MARK.len())
        .rposition(|w| w == MARK)
        .unwrap_or_else(|| {
            panic!(
                "the child never reached its marker; stream was {:?}",
                String::from_utf8_lossy(stream)
            )
        });
    &stream[at + MARK.len()..]
}

/// Release a gated child and wait until `c` has the whole of `want` after the
/// marker.
async fn release_and_collect(c: &mut Client, id: SessionId, want: &[u8]) {
    c.input(id, b"\n").await;
    let need = want.len();
    c.until("the gated payload", |s| {
        let bytes = s.bytes(id);
        bytes
            .windows(MARK.len())
            .rposition(|w| w == MARK)
            .is_some_and(|at| bytes.len() - at - MARK.len() >= need)
    })
    .await;
}

/// Walk `session`'s retained history backwards to its first byte.
///
/// Returns the oldest retained offset and every byte from there up to
/// `before_seq`. Paging rather than one huge request because that is what a
/// client does, and because the boundary between two pages is a place a
/// truncated or duplicated byte can hide.
async fn backfill(c: &mut Client, session: SessionId, before_seq: u64) -> (u64, Vec<u8>) {
    let mut cursor = before_seq;
    let mut pages: Vec<(u64, Vec<u8>)> = Vec::new();
    loop {
        let before = c.seen.ctl.len();
        c.send(ClientMsg::Scrollback {
            session,
            before_seq: cursor,
            max_bytes: 4096,
        })
        .await;
        c.until("a scrollback chunk", |s| {
            s.ctl[before..]
                .iter()
                .any(|m| matches!(m, ServerMsg::ScrollbackChunk { .. }))
        })
        .await;
        let (from_seq, data, more) = c.seen.ctl[before..]
            .iter()
            .find_map(|m| match m {
                ServerMsg::ScrollbackChunk {
                    from_seq,
                    data,
                    more,
                    ..
                } => Some((*from_seq, data.clone(), *more)),
                _ => None,
            })
            .expect("the wait above only returns once a chunk has arrived");
        pages.push((from_seq, data));
        if !more {
            break;
        }
        assert!(
            from_seq < cursor,
            "a page that reports more must move the cursor back, \
             or the client pages forever: from_seq {from_seq}, cursor {cursor}"
        );
        cursor = from_seq;
    }

    pages.reverse();
    let oldest = pages.first().map(|(from, _)| *from).unwrap_or(before_seq);
    let mut out = Vec::new();
    let mut at = oldest;
    for (from, data) in pages {
        assert_eq!(
            from, at,
            "backfill pages must abut: expected a page starting at {at}"
        );
        at += data.len() as u64;
        out.extend_from_slice(&data);
    }
    (oldest, out)
}

/// The fields of a projection two clients must agree about, in transition order.
///
/// Deliberately not the whole `SessionInfo`: `last_activity_ms` and `unread`
/// are per-moment and per-viewer, and comparing them would fail on timing
/// rather than on disagreement. What is here is the sidebar's own vocabulary,
/// which is daemon-owned and must be identical in every window.
fn transitions(client: &Client, session: SessionId) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for info in client.seen.ctl.iter().filter_map(|m| match m {
        ServerMsg::SessionCreated(info) | ServerMsg::SessionUpdated(info)
            if info.id == session =>
        {
            Some(info)
        }
        _ => None,
    }) {
        let shape = format!(
            "{:?}|{}|{:?}|{:?}|failed={}",
            info.status,
            info.title,
            info.term_title,
            info.hint.as_ref().map(|h| h.state),
            info.attention.failed,
        );
        if out.last() != Some(&shape) {
            out.push(shape);
        }
    }
    out
}

/// Attach, stream, detach, re-attach: exact bytes, and no history on attach.
///
/// WHY: this is the tab-switch path, and it is three seams deep. The daemon
/// decides what a newly attached connection is owed, the frame header decides
/// where those bytes sit in the session's stream, and the client reassembles
/// them by offset. A replay on attach double-paints a pane for any client that
/// also backfills; a gap on re-attach silently loses whatever an agent said
/// while you were looking at another tab. Both are invisible to a test that
/// only checks the first attach.
///
/// The child is gated on input at every phase boundary, so each phase provably
/// happens on one side of an attach rather than racing it.
///
/// What this does NOT catch: eviction, since the ring here is far larger than
/// the output; the scrollback path, which the backfill cases below own; or
/// Windows, for the reason in this module's header.
#[tokio::test]
async fn attach_detach_and_reattach_carry_exact_bytes_and_no_backlog() {
    let h = Harness::start(1 << 20).await;
    let mut a = h.greeted().await;
    let id = a
        .create(create(
            1,
            "read -r _; printf 'PHASE-ONE\\n'; \
             read -r _; printf 'PHASE-TWO\\n'; \
             read -r _; printf 'PHASE-THREE\\n'; exit 0",
        ))
        .await;
    a.attach(id, 80, 24).await;

    // Phase one, with only A watching.
    a.input(id, b"\n").await;
    a.until("phase one", |s| {
        s.carries(id, b"PHASE-ONE\r\n")
    })
    .await;
    let after_one = a.seen.bytes(id).to_vec();
    assert_eq!(
        a.seen.first_seq(id),
        Some(0),
        "a client attached before any output must start at the first byte"
    );

    // B attaches now. It is owed the live stream from here and nothing older.
    let mut b = h.greeted().await;
    b.attach(id, 80, 24).await;
    assert_eq!(
        b.seen.bytes(id),
        b"",
        "attach must not replay history: {:?}",
        String::from_utf8_lossy(b.seen.bytes(id))
    );

    // Phase two, with both watching. B detaches before phase three.
    a.input(id, b"\n").await;
    for c in [&mut a, &mut b] {
        c.until("phase two", |s| s.carries(id, b"PHASE-TWO\r\n")).await;
    }
    assert_eq!(
        b.seen.first_seq(id),
        Some(after_one.len() as u64),
        "B's first frame must be the byte offset it attached at, not zero"
    );
    let after_two = a.seen.bytes(id).to_vec();
    assert_eq!(
        b.seen.bytes(id),
        &after_two[after_one.len()..],
        "B must have exactly the bytes produced since it attached"
    );

    b.detach(id).await;

    // Phase three, with B detached.
    a.input(id, b"\n").await;
    a.until("phase three", |s| s.carries(id, b"PHASE-THREE\r\n"))
        .await;
    a.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { session, .. } if *session == id))
    })
    .await;
    let whole = a.seen.bytes(id).to_vec();
    // B has to drain the registry frames it is owed before silence means
    // anything: the exit is published to every window, detached or not, and a
    // quiet window that caught it would be reporting the wrong thing.
    b.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { session, .. } if *session == id))
    })
    .await;
    assert_eq!(
        b.seen.bytes(id),
        &after_two[after_one.len()..],
        "a detached client must receive no further output at all"
    );

    // Re-attaching a dead session must still be an attach, and must still not
    // replay. The session is gone as a source of live bytes, so the proof is
    // that the history is only reachable on request.
    b.attach(id, 80, 24).await;
    b.quiet().await;
    assert_eq!(
        b.seen.bytes(id),
        &after_two[after_one.len()..],
        "re-attach must not replay the phase B missed"
    );

    let (oldest, history) = backfill(&mut b, id, u64::MAX).await;
    assert_eq!(oldest, 0, "a 1 MiB ring evicted nothing");
    assert_eq!(
        history, whole,
        "the retained history and the live stream must be the same bytes"
    );
}

/// A burst followed by an immediate exit must reach an ATTACHED CLIENT whole.
///
/// WHY: this is the shipped defect, at the seam it shipped from. The end of a
/// session's output used to be decided by a stopwatch: once the child was
/// reaped, the coalescer allowed one flush window of silence and then dropped
/// its end of the byte channel, while the reader was still parsing a read it
/// had already taken from the kernel. A child that printed a few hundred lines
/// and exited published a truncated stream, or nothing at all.
///
/// `vitrum-core` guards the publish. This guards the delivery, which is a
/// different claim: the coalescer can publish correctly and the connection can
/// still drop the last frames on the floor when its pump is torn down by the
/// exit. Only bytes that came off a socket are asserted here.
///
/// The sizes straddle the volume at which parsing one read outlasted the flush
/// window, because that was a race rather than a threshold. Equality is exact
/// at every size, so losing one byte or repeating one fails where a length
/// floor would pass.
///
/// What this does NOT catch: a client that attaches after the exit, which the
/// next case owns; eviction; or Windows, where the reader cannot reach end of
/// stream on its own and the exit plus a quiet window is still the only end of
/// output there is.
#[tokio::test]
async fn a_burst_then_an_immediate_exit_reaches_an_attached_client_whole() {
    for n in [50usize, 100, 300, 1000] {
        let h = Harness::start(1 << 20).await;
        let mut c = h.greeted().await;
        let id = c
            .create(create(
                1,
                &gated(&format!(
                    "i=0; while [ $i -lt {n} ]; do printf '\\033[32mburst %s\\033[0m\\n' $i; \
                     i=$((i+1)); done; exit 0"
                )),
            ))
            .await;
        c.attach(id, 80, 24).await;

        let expected: Vec<u8> = (0..n)
            .flat_map(|i| format!("\x1b[32mburst {i}\x1b[0m\r\n").into_bytes())
            .collect();
        release_and_collect(&mut c, id, &expected).await;
        c.until("the exit", |s| {
            s.has(|m| matches!(m, ServerMsg::Exited { session, .. } if *session == id))
        })
        .await;
        // Drain anything still in flight behind the exit, then assert on all of
        // it. A quiet window here is a bound on absence, not a wait.
        c.quiet().await;

        let stream = c.seen.bytes(id).to_vec();
        assert_eq!(
            after_mark(&stream),
            expected.as_slice(),
            "{n} writes: the attached client's stream was not what the child wrote"
        );
    }
}

/// A client that arrives after the exit must recover the whole stream.
///
/// WHY: an agent that finishes while you are looking at another project is the
/// normal case, and the window you open afterwards never had an attachment. It
/// has only the scrollback path, so a truncation there is a session whose work
/// is simply unreadable — the same operator-visible symptom as the burst defect
/// and a completely different code path.
///
/// The reference is the live stream a client that WAS attached received, not a
/// string this test computed, because only a comparison of the two proves the
/// two paths agree.
///
/// What this does NOT catch: eviction, which `scrollback_rpc.rs` owns.
#[tokio::test]
async fn a_client_arriving_after_the_exit_recovers_the_whole_stream() {
    let h = Harness::start(1 << 20).await;
    let mut live = h.greeted().await;
    let id = live
        .create(create(
            1,
            &gated(
                "i=0; while [ $i -lt 400 ]; do printf 'line %s\\n' $i; i=$((i+1)); done; exit 0",
            ),
        ))
        .await;
    live.attach(id, 80, 24).await;

    let expected: Vec<u8> = (0..400)
        .flat_map(|i| format!("line {i}\r\n").into_bytes())
        .collect();
    release_and_collect(&mut live, id, &expected).await;
    live.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { session, .. } if *session == id))
    })
    .await;
    live.quiet().await;
    let whole = live.seen.bytes(id).to_vec();
    assert_eq!(
        after_mark(&whole),
        expected.as_slice(),
        "the live stream itself was short, so the comparison below would be vacuous"
    );

    let mut latecomer = h.greeted().await;
    let (oldest, history) = backfill(&mut latecomer, id, u64::MAX).await;
    assert_eq!(oldest, 0, "the whole history survived a 1 MiB ring");
    assert_eq!(
        history, whole,
        "a client that arrived after the exit did not recover the session's output"
    );
}

/// Backfill and live output must meet exactly at the offset the client attached.
///
/// WHY: this is how a real client paints a session it opened mid-run. It
/// attaches, notes the offset of its first live frame, and asks for everything
/// before it. One byte of overlap repeats a line; one byte of gap corrupts the
/// emulator's parse from there on, because the missing byte is as likely to be
/// inside an escape sequence as inside a word. The seam is between the ring's
/// idea of an offset and the data frame's, and nothing below this level
/// compares the two.
///
/// What this does NOT catch: a ring that evicted the joint, which is a real
/// case and reports itself through `from_seq` above zero rather than silently.
#[tokio::test]
async fn backfill_and_live_meet_exactly_at_the_attach_offset() {
    let h = Harness::start(1 << 20).await;
    let mut watcher = h.greeted().await;
    let id = watcher
        .create(create(
            1,
            "read -r _; i=0; while [ $i -lt 200 ]; do printf 'early %s\\n' $i; i=$((i+1)); done; \
             read -r _; i=0; while [ $i -lt 200 ]; do printf 'late %s\\n' $i; i=$((i+1)); done; \
             exit 0",
        ))
        .await;
    watcher.attach(id, 80, 24).await;

    let early: Vec<u8> = (0..200)
        .flat_map(|i| format!("early {i}\r\n").into_bytes())
        .collect();
    let late: Vec<u8> = (0..200)
        .flat_map(|i| format!("late {i}\r\n").into_bytes())
        .collect();

    watcher.input(id, b"\n").await;
    watcher
        .until("the early half", |s| s.carries(id, b"early 199\r\n"))
        .await;
    let before_join = watcher.seen.bytes(id).to_vec();

    // The latecomer attaches while the child sits on its second gate, so the
    // join is a real boundary and not a race with the output either side.
    let mut joiner = h.greeted().await;
    joiner.attach(id, 80, 24).await;

    watcher.input(id, b"\n").await;
    for c in [&mut watcher, &mut joiner] {
        c.until("the late half", |s| s.carries(id, b"late 199\r\n"))
            .await;
        c.until("the exit", |s| {
            s.has(|m| matches!(m, ServerMsg::Exited { session, .. } if *session == id))
        })
        .await;
        c.quiet().await;
    }

    let whole = watcher.seen.bytes(id).to_vec();
    let join = joiner
        .seen
        .first_seq(id)
        .expect("the joiner received live frames");
    assert_eq!(
        join,
        before_join.len() as u64,
        "the joiner's first live frame must start where the stream already stood"
    );

    let (oldest, history) = backfill(&mut joiner, id, join).await;
    assert_eq!(oldest, 0, "nothing was evicted");
    assert_eq!(
        history.len() as u64,
        join,
        "backfill up to the join must be exactly the join offset in bytes"
    );

    assert_eq!(
        history, before_join,
        "the backfill must be exactly the bytes the client watching from the start \
         already held when the joiner attached"
    );

    let mut rebuilt = history;
    rebuilt.extend_from_slice(joiner.seen.bytes(id));
    assert_eq!(
        rebuilt, whole,
        "backfill plus live must reconstruct the stream a client watching from the start saw"
    );
    assert!(
        whole.windows(early.len()).any(|w| w == early.as_slice()),
        "the early half must be present verbatim"
    );
    assert!(
        whole.windows(late.len()).any(|w| w == late.as_slice()),
        "the late half must be present verbatim"
    );
}

/// Two windows on one session must see the same bytes and the same transitions.
///
/// WHY: sessions are daemon-owned, so a second window is a view rather than a
/// takeover. The bytes travel per attachment and the projections travel on a
/// shared bus, which are two different fan-outs with two different ways to
/// diverge: a per-connection pump can drop frames one client received, and a
/// projection built per subscriber can carry a different name or a different
/// hint. An operator with two windows open sees the disagreement immediately
/// and has no way to tell which one is lying.
///
/// What this does NOT catch: divergence that both windows share, which is what
/// every other case here is for; and per-viewer fields such as `unread`, which
/// are supposed to differ and are excluded from the comparison on purpose.
#[tokio::test]
async fn two_clients_see_the_same_bytes_and_the_same_transitions() {
    let h = Harness::start(1 << 20).await;
    let mut one = h.greeted().await;
    let mut two = h.greeted().await;

    let id = one
        .create(create(
            1,
            "read -r _; printf '\\033]2;renamed-by-program\\007'; \
             printf '\\033]7373;approval;may i?\\033\\\\'; \
             printf 'shared output\\n'; exit 3",
        ))
        .await;
    for c in [&mut one, &mut two] {
        c.attach(id, 80, 24).await;
    }

    one.input(id, b"\n").await;
    for c in [&mut one, &mut two] {
        c.until("the exit", |s| {
            s.has(|m| matches!(m, ServerMsg::Exited { session, code } if *session == id && *code == Some(3)))
        })
        .await;
        c.until_projection("the exited projection", id, |i| !i.status.is_live())
            .await;
        c.quiet().await;
    }

    assert_eq!(
        one.seen.bytes(id),
        two.seen.bytes(id),
        "two attached windows received different bytes"
    );
    assert!(
        one.seen.carries(id, b"shared output\r\n"),
        "the comparison above would be vacuous on two empty streams"
    );
    assert_eq!(
        transitions(&one, id),
        transitions(&two, id),
        "two windows disagreed about what the session did"
    );
}

/// Invalid UTF-8 must cross the socket as the bytes the child wrote.
///
/// WHY: the data plane is binary precisely so PTY output is never text, but a
/// client's own reassembly and any future framing change can still round-trip
/// through a lossy decode. One 0xFF turned into U+FFFD is three bytes where
/// there was one, which shifts every offset after it: search hits point at the
/// wrong column and scrollback pages stop abutting.
///
/// What this does NOT catch: what the terminal engine in the daemon makes of
/// these bytes, which is `vitrum-vt`'s question and deliberately not the
/// transport's.
#[tokio::test]
async fn invalid_utf8_crosses_the_socket_verbatim() {
    let h = Harness::start(1 << 16).await;
    let mut c = h.greeted().await;
    let id = c
        .create(create(1, &gated("printf '\\377\\376\\375\\200'; exit 0")))
        .await;
    c.attach(id, 80, 24).await;

    let expected = b"\xff\xfe\xfd\x80";
    release_and_collect(&mut c, id, expected).await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { session, .. } if *session == id))
    })
    .await;
    c.quiet().await;

    let stream = c.seen.bytes(id).to_vec();
    assert_eq!(
        after_mark(&stream),
        expected,
        "an undecodable byte was rewritten somewhere on the way to the client"
    );
}

/// A lone ESC with nothing after it must arrive, and must not stall the stream.
///
/// WHY: an incomplete escape is what a parser waits on, and a parser that waits
/// is a pane that stops updating. The daemon parses every byte itself, so a
/// half-finished sequence at the end of a child's output is a place where the
/// engine could hold the last chunk back forever. Termination is asserted here
/// as hard as the bytes are: the exit has to arrive, and the deadline in the
/// harness turns a stall into a readable failure rather than a hung suite.
///
/// What this does NOT catch: a stall shorter than the harness deadline, which
/// is a latency question and belongs in the bench crate.
#[tokio::test]
async fn a_lone_escape_arrives_and_does_not_stall_the_stream() {
    let h = Harness::start(1 << 16).await;
    let mut c = h.greeted().await;
    let id = c
        .create(create(1, &gated("printf '\\033'; exit 0")))
        .await;
    c.attach(id, 80, 24).await;

    release_and_collect(&mut c, id, b"\x1b").await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { session, code } if *session == id && *code == Some(0)))
    })
    .await;
    c.quiet().await;

    let stream = c.seen.bytes(id).to_vec();
    assert_eq!(
        after_mark(&stream),
        b"\x1b",
        "a trailing ESC must reach the client alone and unaltered"
    );
}

/// A very long line with no newline must arrive whole.
///
/// WHY: line-oriented thinking is how a byte stream gets corrupted. Nothing on
/// this path is allowed to wait for a terminator — not the coalescer's flush,
/// not the ring's indexing, not the frame writer — and an agent rendering a
/// progress bar or a wide table emits exactly this: kilobytes between one
/// newline and the next.
///
/// The length is chosen to exceed the 80-column viewport many times over and
/// to straddle several coalesced frames, so reassembly is really exercised.
///
/// What this does NOT catch: a line longer than the retained ring, which is
/// eviction and reports itself.
#[tokio::test]
async fn a_long_line_with_no_newline_arrives_whole() {
    let h = Harness::start(1 << 20).await;
    let mut c = h.greeted().await;
    // 16 * 2^9 = 8192 bytes, built by doubling so the script stays short.
    let id = c
        .create(create(
            1,
            &gated(
                "s=0123456789abcdef; i=0; while [ $i -lt 9 ]; do s=\"$s$s\"; i=$((i+1)); done; \
                 printf '%s' \"$s\"; exit 0",
            ),
        ))
        .await;
    c.attach(id, 80, 24).await;

    let expected: Vec<u8> = "0123456789abcdef".repeat(512).into_bytes();
    assert_eq!(expected.len(), 8192, "the script and the reference must agree");
    release_and_collect(&mut c, id, &expected).await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { session, .. } if *session == id))
    })
    .await;
    c.quiet().await;

    let stream = c.seen.bytes(id).to_vec();
    assert_eq!(
        after_mark(&stream),
        expected.as_slice(),
        "an unterminated line was not delivered byte for byte"
    );
}

/// A title escape split across two PTY reads must still be understood.
///
/// WHY: the daemon's terminal engine is fed one 32 KiB read at a time, and a
/// child under no obligation to write atomically can put `ESC ] 2 ;` in one
/// write and the name plus its terminator in the next. An engine reset per read
/// would silently lose every title that straddled a boundary, and the failure
/// is invisible: the row keeps its old name and nothing errors.
///
/// The two halves are separated by a gate rather than by a sleep, so they are
/// provably two reads.
///
/// What this does NOT catch: a split inside a UTF-8 character in the title,
/// which is `vitrum-vt`'s parser to answer.
#[tokio::test]
async fn a_title_escape_split_across_two_reads_still_names_the_session() {
    let h = Harness::start(1 << 16).await;
    let mut c = h.greeted().await;
    let id = c
        .create(create(
            1,
            "printf '\\033]2;split'; read -r _; printf 'name\\007'; read -r _; exit 0",
        ))
        .await;
    c.attach(id, 80, 24).await;

    // Nothing may be claimed from the first half alone: an engine that guessed
    // a terminator would name the row `split`.
    c.input(id, b"\n").await;
    let info = c
        .until_projection("the reassembled title", id, |i| i.term_title.is_some())
        .await;
    assert_eq!(
        info.term_title.as_deref(),
        Some("splitname"),
        "the two halves of one escape must be parsed as one sequence"
    );

    c.input(id, b"\n").await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { session, .. } if *session == id))
    })
    .await;
}

/// A single 1 MiB write must reach the client whole and terminate.
///
/// WHY: this is the firehose the product exists for, and the two failure modes
/// it hides are opposite. A coalescer that never flushes early buffers the
/// whole megabyte and the pane freezes; one that flushes wrongly duplicates or
/// drops at the frame boundary, and at this volume there are many boundaries.
/// Exact equality catches both, and the harness deadline catches the case where
/// nothing arrives at all.
///
/// "Single write" is the child's view: one `printf` of one 1 MiB argument. The
/// kernel is free to split it, which is the point — the client must not be able
/// to tell.
///
/// What this does NOT catch: throughput. This asserts the bytes are all there,
/// never how fast they got there; that measurement lives in the bench crate.
#[tokio::test]
async fn a_one_mib_single_write_arrives_whole() {
    let h = Harness::start(4 << 20).await;
    let mut c = h.greeted().await;
    // 16 * 2^16 = 1_048_576 bytes.
    let id = c
        .create(create(
            1,
            &gated(
                "s=0123456789abcdef; i=0; while [ $i -lt 16 ]; do s=\"$s$s\"; i=$((i+1)); done; \
                 printf '%s' \"$s\"; exit 0",
            ),
        ))
        .await;
    c.attach(id, 80, 24).await;

    let expected: Vec<u8> = "0123456789abcdef".repeat(65536).into_bytes();
    assert_eq!(
        expected.len(),
        1 << 20,
        "the script and the reference must agree"
    );
    release_and_collect(&mut c, id, &expected).await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { session, .. } if *session == id))
    })
    .await;
    c.quiet().await;

    let stream = c.seen.bytes(id).to_vec();
    assert_eq!(
        after_mark(&stream).len(),
        expected.len(),
        "a megabyte arrived at the wrong length"
    );
    assert_eq!(
        after_mark(&stream),
        expected.as_slice(),
        "a megabyte arrived corrupted"
    );
    assert!(
        c.seen.data_frames > 1,
        "a megabyte that arrived in one frame means the coalescer never flushed early, \
         which is the freeze this case exists to catch"
    );
}
