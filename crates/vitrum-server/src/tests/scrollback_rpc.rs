//! The Scrollback request: paging backwards through retained history.

use vitrum_proto::{ClientMsg, ServerMsg, SessionId};

use crate::tests::client::{Harness, create};

/// Pull the fields of the newest scrollback chunk received.
fn latest_chunk(seen: &crate::tests::client::Seen) -> (u64, Vec<u8>, bool) {
    seen.ctl
        .iter()
        .rev()
        .find_map(|m| match m {
            ServerMsg::ScrollbackChunk {
                from_seq,
                data,
                more,
                ..
            } => Some((*from_seq, data.clone(), *more)),
            _ => None,
        })
        .expect("a scrollback chunk must have arrived")
}

/// Ask for the newest page of at most `max_bytes` and return that chunk.
///
/// Requesting and reading as one step keeps two pages from being confused for
/// each other, which matters because a zero-length request is a legitimate way
/// to ask only where the head is.
async fn page(
    c: &mut crate::tests::client::Client,
    session: SessionId,
    max_bytes: u32,
) -> (u64, Vec<u8>, bool) {
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
            ServerMsg::ScrollbackChunk {
                from_seq,
                data,
                more,
                ..
            } => Some((*from_seq, data.clone(), *more)),
            _ => None,
        })
        .expect("the loop above only exits once a chunk has arrived")
}

/// `u64::MAX` must mean "up to the current head".
///
/// The client has no way to know the head before it asks, so this sentinel is the
/// only way to request the newest page. Refusing it, or treating it as an offset,
/// would make the first backfill impossible.
#[tokio::test]
async fn u64_max_means_up_to_the_head() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "echo history")).await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { .. }))
    })
    .await;

    let (from, data, more) = page(&mut c, id, 4096).await;
    assert_eq!(from, 0, "the whole history starts at the first byte");
    // Where the child's line sits is the pseudoconsole's business; that it is
    // there, whole, is the daemon's.
    assert!(
        data.windows(9).any(|w| w == b"history\r\n"),
        "the page must carry the child's line: {data:?}"
    );
    assert!(!more, "the whole history fit in one page");
}

/// Paging backwards must walk to the first byte and then stop.
///
/// `more` is the client's only signal that it has reached the start. If it never
/// clears, the client pages forever; if it clears early, the user cannot scroll to
/// the beginning of the session.
#[cfg(not(windows))]
#[tokio::test]
async fn paging_backwards_reaches_the_first_byte_and_stops() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    let id = c.create(create(
        1,
        "i=0; while [ $i -lt 100 ]; do printf p; i=$((i+1)); done",
    )).await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { .. }))
    })
    .await;

    let mut cursor = u64::MAX;
    let mut collected: Vec<u8> = Vec::new();
    let mut pages = 0;
    loop {
        c.send(ClientMsg::Scrollback {
            session: id,
            before_seq: cursor,
            max_bytes: 40,
        })
        .await;
        let want = pages + 1;
        c.until("the next page", |s| {
            s.ctl
                .iter()
                .filter(|m| matches!(m, ServerMsg::ScrollbackChunk { .. }))
                .count()
                >= want
        })
        .await;
        let (from, data, more) = latest_chunk(&c.seen);
        pages += 1;
        assert!(pages <= 6, "paging did not terminate");
        let mut next = data;
        next.extend_from_slice(&collected);
        collected = next;
        cursor = from;
        if !more {
            assert_eq!(from, 0, "the last page must reach the first byte");
            break;
        }
    }
    assert_eq!(pages, 3, "100 bytes in 40-byte pages");
    assert_eq!(collected, vec![b'p'; 100]);
}

/// A request bounded before the oldest retained byte must answer with an empty
/// final page, so a client that over-pages is told to stop instead of looping.
#[tokio::test]
async fn a_request_older_than_history_is_an_empty_final_page() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "echo edge")).await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { .. }))
    })
    .await;

    c.send(ClientMsg::Scrollback {
        session: id,
        before_seq: 0,
        max_bytes: 4096,
    })
    .await;
    c.until("the chunk", |s| {
        s.has(|m| matches!(m, ServerMsg::ScrollbackChunk { .. }))
    })
    .await;
    let (from, data, more) = latest_chunk(&c.seen);
    assert_eq!(from, 0);
    assert_eq!(data, b"");
    assert!(!more);
}

/// When output has outgrown the ring, the chunk must start at the oldest retained
/// byte and say nothing older survives.
///
/// This is how a client learns its history was evicted: `from_seq` above zero with
/// `more` false. Reporting zero would make it request bytes the server no longer
/// has, forever.
#[tokio::test]
async fn an_evicted_history_reports_the_oldest_retained_offset() {
    let capacity = 32;
    let h = Harness::start(capacity).await;
    let mut c = h.greeted().await;
    let payload = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJ";
    let id = c.create(create(1, &format!("echo {payload}"))).await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { .. }))
    })
    .await;

    // The retained window is the last `capacity` bytes of whatever the pty
    // wrote, and only the pty knows how much that is: a pseudoconsole adds mode
    // sets and a title of its own, so ask where the head is instead of counting
    // the script's characters. A zero-length request is clamped to the head.
    let (from, data, more) = page(&mut c, id, 4096).await;
    let (head, _, _) = page(&mut c, id, 0).await;
    assert_eq!(from, head - capacity as u64);
    assert_eq!(data.len(), capacity, "the ring keeps exactly its capacity");
    assert!(!more, "nothing older than the oldest retained byte exists");
}

/// A `max_bytes` smaller than the retained history must return the newest slice
/// and report that more remains, so a viewport-sized first page works.
#[tokio::test]
async fn a_small_page_returns_the_newest_slice_and_flags_more() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "echo abcdefghij")).await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { .. }))
    })
    .await;

    let (from, data, more) = page(&mut c, id, 4).await;
    let (head, _, _) = page(&mut c, id, 0).await;
    assert_eq!(from, head - 4, "the newest page ends at the head");
    assert_eq!(data.len(), 4);
    assert!(more);

    // Those four bytes must be the tail of the history, not any other slice.
    let (_, whole, _) = page(&mut c, id, 4096).await;
    assert_eq!(&whole[whole.len() - 4..], &data[..]);
}

/// Scrollback for an unknown session must be an error, not an empty success.
///
/// An empty chunk would look like "no history" and the client would stop paging
/// rather than noticing its id is stale.
#[tokio::test]
async fn scrollback_for_an_unknown_session_errors() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send(ClientMsg::Scrollback {
        session: SessionId(555),
        before_seq: u64::MAX,
        max_bytes: 100,
    })
    .await;
    c.until("the error", |s| !s.errors().is_empty()).await;
    assert!(
        c.seen.errors()[0].contains("555"),
        "unhelpful error: {:?}",
        c.seen.errors()[0]
    );
    assert!(!c.seen.has(|m| matches!(m, ServerMsg::ScrollbackChunk { .. })));
}

/// History must be requestable while the session is still running, since that is
/// when a user scrolls back to re-read what an agent said.
///
/// The child waits for input, speaks, then waits again, so the request provably
/// lands while the process is alive rather than racing its exit.
#[cfg(not(windows))]
#[tokio::test]
async fn history_is_requestable_while_the_session_runs() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "read -r x; echo live; read -r y")).await;

    c.attach(id, 80, 24).await;
    c.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    c.until("the live output", |s| s.bytes(id).ends_with(b"live\r\n"))
        .await;

    c.send(ClientMsg::Scrollback {
        session: id,
        before_seq: u64::MAX,
        max_bytes: 4096,
    })
    .await;
    c.until("the chunk", |s| {
        s.has(|m| matches!(m, ServerMsg::ScrollbackChunk { .. }))
    })
    .await;
    let (from, data, more) = latest_chunk(&c.seen);
    assert_eq!(from, 0);
    assert_eq!(
        data, b"\r\nlive\r\n",
        "history holds the echoed newline and the line, while the child still runs"
    );
    assert!(!more);
    assert!(
        h.manager
            .info(id)
            .expect("still registered")
            .status
            .is_live()
    );
    h.manager.close(id).expect("close");
}
