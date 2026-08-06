//! Two windows, one PTY: the size is the minimum over attached clients.
//!
//! Windows are independent views of one daemon, so two of them routinely show
//! the same session at different sizes. A PTY has exactly one size, and
//! "whoever resized last wins" is not a policy but an oscillation: each window
//! lays out, resizes, observes the other's resize, and resizes back. The child
//! reflows forever, both viewports are wrong half the time, and the idle CPU
//! this product is measured on is gone.
//!
//! Every size assertion here comes from the CHILD asking the kernel, because a
//! projection that agrees with itself while the child sees something else is
//! exactly the bug.

use vitrum_proto::ClientMsg;

use crate::tests::client::{Harness, create};

/// A session that prints its terminal size every time it is prodded.
const REPORTER: &str = "while read -r x; do stty size; done";

/// Two windows at different sizes must settle on the smaller one.
///
/// Confirmed by the child, not by `SessionInfo`: at 200x50 the narrow window
/// would be rendering a viewport it cannot fit, and clipping a terminal is
/// silent corruption rather than a visible error.
#[cfg(unix)]
#[tokio::test]
async fn two_windows_settle_on_the_smaller_geometry() {
    let h = Harness::start(64 * 1024).await;
    let mut wide = h.greeted().await;
    let mut narrow = h.greeted().await;
    let id = wide.create(create(1, REPORTER)).await;

    wide.attach(id, 200, 50).await;
    narrow.attach(id, 100, 30).await;

    wide.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    wide.until("the child's report", |s| {
        s.bytes(id).ends_with(b"\r\n") && s.bytes(id).len() > 2
    })
    .await;
    assert!(
        String::from_utf8_lossy(wide.seen.bytes(id)).contains("30 100"),
        "the child must see the minimum, saw {:?}",
        String::from_utf8_lossy(wide.seen.bytes(id))
    );
    let info = h.manager.info(id).expect("info");
    assert_eq!((info.cols, info.rows), (100, 30));
    h.manager.close(id).expect("close");
}

/// When the smaller window detaches, the PTY must grow back and say so.
///
/// The window that stayed did nothing, so nothing in its own event loop tells
/// it the session got bigger; without the push it would keep rendering
/// letterboxed until the user happened to resize.
#[cfg(unix)]
#[tokio::test]
async fn detaching_the_smaller_window_grows_the_pty_and_notifies() {
    let h = Harness::start(64 * 1024).await;
    let mut wide = h.greeted().await;
    let mut narrow = h.greeted().await;
    let id = wide.create(create(1, REPORTER)).await;

    wide.attach(id, 200, 50).await;
    narrow.attach(id, 100, 30).await;
    wide.until("the shrink", |s| {
        s.updates().iter().any(|i| i.id == id && i.cols == 100)
    })
    .await;

    narrow.send(ClientMsg::Detach { session: id }).await;

    wide.until("the pty to grow back", |s| {
        s.updates().iter().any(|i| i.id == id && i.cols == 200)
    })
    .await;
    wide.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    wide.until("the child's report", |s| {
        s.bytes(id).ends_with(b"\r\n") && s.bytes(id).len() > 2
    })
    .await;
    assert!(
        String::from_utf8_lossy(wide.seen.bytes(id)).contains("50 200"),
        "the child must see the remaining window's size, saw {:?}",
        String::from_utf8_lossy(wide.seen.bytes(id))
    );
    h.manager.close(id).expect("close");
}

/// A window that disconnects entirely must stop constraining the size.
///
/// A crashed or quit window never sends `Detach`. If its constraint outlived
/// the socket, one dead 80-column window would hold the session at 80 columns
/// for as long as the daemon runs, with no way to clear it short of a restart.
#[cfg(unix)]
#[tokio::test]
async fn a_disconnecting_window_releases_its_constraint() {
    let h = Harness::start(64 * 1024).await;
    let mut wide = h.greeted().await;
    let id = wide.create(create(1, REPORTER)).await;
    wide.attach(id, 200, 50).await;

    {
        let mut narrow = h.greeted().await;
        narrow.attach(id, 90, 20).await;
        wide.until("the shrink", |s| {
            s.updates().iter().any(|i| i.id == id && i.cols == 90)
        })
        .await;
    }

    // Wait for the record to CHANGE, not for a value the log already holds.
    //
    // This wait used to be `until(|s| s.updates().iter().any(|i| i.cols ==
    // 200))`, and it was vacuous. `Seen::updates()` is "every session
    // projection pushed so far, oldest first" and is never cleared, and
    // `wide.attach(id, 200, 50)` above already pushed a cols == 200 projection
    // before the narrow window existed. So `any()` matched that stale entry,
    // `until` returned without reading a single frame, and the assertion after
    // it ran before the daemon had processed the closed socket. The 0.4s
    // runtime was the tell: a real post-close wait either passes late or burns
    // the deadline, it never returns instantly.
    //
    // `Client::attach` had already solved this one call site away, by counting
    // rather than matching, and its doc comment warns that "waiting for any
    // SessionUpdated would return immediately". Polling the daemon's record is
    // the same idea against its own state: the constraint is released when the
    // record reads 200x50, and it reads 90x20 until then.
    //
    // A (90, 20) here means NOT RELEASED YET. An earlier revision of this
    // comment read it as the record contradicting the wire and left a note
    // inviting someone to investigate a daemon race. There is no such race:
    // that reading rested on the same vacuous wait, and the old child-prodding
    // version passed 21 runs in 40, reporting "50 200" every time it did, so
    // release demonstrably happens and is merely not instantaneous.
    //
    // This is the THIRD predicate on this one test that a precondition already
    // satisfied. First the child's own "20 90", emitted on the narrow window's
    // SIGWINCH, satisfied "ends with CRLF and longer than two bytes". Then the
    // content-tightened version fixed that and left a single un-retryable prod
    // at a reporter that only speaks when spoken to, which hung for the full
    // deadline in 19 runs of 40. Now this. The cure each time is to require a
    // NEW fact rather than a matching one.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let now = h.manager.info(id).expect("still listed");
        if (now.cols, now.rows) == (200, 50) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the narrow window's socket closed, so its 90x20 constraint must be \
             released and the session must return to the only attachment left; \
             the record still reads {:?}",
            (now.cols, now.rows)
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    h.manager.close(id).expect("close");
}

/// Two windows re-asserting their own layouts must converge and stop.
///
/// The reflow storm, reproduced: each render loop sends its own geometry, over
/// and over. With last-writer-wins the ioctl count climbs without bound and the
/// child reflows on every one of them. Counting the actual resizes is what
/// makes "converged" a fact rather than an impression.
#[cfg(unix)]
#[tokio::test]
async fn alternating_resizes_converge_without_thrashing() {
    let h = Harness::start(64 * 1024).await;
    let mut wide = h.greeted().await;
    let mut narrow = h.greeted().await;
    let id = wide.create(create(1, REPORTER)).await;

    wide.attach(id, 200, 50).await;
    narrow.attach(id, 100, 30).await;
    narrow
        .until("the agreed size", |s| {
            s.updates().iter().any(|i| i.id == id && i.cols == 100)
        })
        .await;
    let settled = h.manager.resize_count(id).expect("resize count");

    for _ in 0..15 {
        wide.send(ClientMsg::Resize {
            session: id,
            cols: 200,
            rows: 50,
        })
        .await;
        narrow
            .send(ClientMsg::Resize {
                session: id,
                cols: 100,
                rows: 30,
            })
            .await;
    }

    // Round-trip both connections so every one of those resizes has been
    // handled before the count is read.
    for client in [&mut wide, &mut narrow] {
        client.send(ClientMsg::List).await;
        client.until("a snapshot", |s| s.sessions().is_some()).await;
    }

    assert_eq!(
        h.manager.resize_count(id).expect("resize count"),
        settled,
        "thirty redundant resizes must cost zero ioctls"
    );
    let info = h.manager.info(id).expect("info");
    assert_eq!(
        (info.cols, info.rows),
        (100, 30),
        "the size must be a fixed point, not whoever wrote last"
    );

    wide.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    wide.until("the child's report", |s| {
        s.bytes(id).ends_with(b"\r\n") && s.bytes(id).len() > 2
    })
    .await;
    assert!(
        String::from_utf8_lossy(wide.seen.bytes(id)).contains("30 100"),
        "the child must still be at the converged size, saw {:?}",
        String::from_utf8_lossy(wide.seen.bytes(id))
    );
    h.manager.close(id).expect("close");
}

/// One window must behave exactly as it did before viewers existed.
///
/// By far the common case, and the one a multi-window feature has no licence to
/// regress: attach, resize, and the child sees precisely what that window asked
/// for, with no minimum in the way.
#[cfg(unix)]
#[tokio::test]
async fn a_single_window_still_gets_exactly_what_it_asks_for() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    // The child reports only when prodded, so the attach is sequenced before
    // the first report rather than racing the shell's startup.
    let id = c.create(create(1, REPORTER)).await;

    c.attach(id, 132, 43).await;
    c.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    c.until("the attach geometry", |s| {
        s.bytes(id).ends_with(b"43 132\r\n")
    })
    .await;

    c.send(ClientMsg::Resize {
        session: id,
        cols: 90,
        rows: 25,
    })
    .await;
    c.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    c.until("the resized geometry", |s| {
        s.bytes(id).ends_with(b"25 90\r\n")
    })
    .await;
    let info = h.manager.info(id).expect("info");
    assert_eq!((info.cols, info.rows), (90, 25));
    h.manager.close(id).expect("close");
}

/// Re-attaching must replace that window's constraint, not add another.
///
/// A window that reconnects would otherwise be counted twice, and its stale
/// size would pin the session forever with no viewer behind it.
#[cfg(unix)]
#[tokio::test]
async fn re_attaching_replaces_that_window_s_constraint() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, REPORTER)).await;

    c.attach(id, 80, 24).await;
    c.attach(id, 160, 48).await;
    c.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    c.until("the child's report", |s| {
        s.bytes(id).ends_with(b"\r\n") && s.bytes(id).len() > 2
    })
    .await;
    assert!(
        String::from_utf8_lossy(c.seen.bytes(id)).contains("48 160"),
        "the same window's older size must not linger, saw {:?}",
        String::from_utf8_lossy(c.seen.bytes(id))
    );
    h.manager.close(id).expect("close");
}
