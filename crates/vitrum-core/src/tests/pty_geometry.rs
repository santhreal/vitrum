//! One PTY, several windows: geometry is the minimum over attached viewers.
//!
//! Windows are independent views of one daemon, so two of them can show the
//! same session at different sizes. A single PTY has one size, so "whoever
//! resized last wins" is not a policy, it is an oscillation: each window lays
//! out, resizes, sees the other's resize, and resizes back, forever. The child
//! reflows the whole time and neither window renders correctly. The minimum is
//! the only fixed point, because it is the only size every attached window can
//! draw without clipping.
//!
//! Every assertion here reads the size out of the KERNEL, not out of
//! `SessionInfo`, because the bug being prevented is a projection that agrees
//! with itself while the child sees something else.

#[cfg(unix)]
use crate::tests::helpers::Collector;
use crate::tests::helpers::shell_spec;
use crate::{SessionManager, ViewerId};

/// Read the size the kernel actually holds for a session's PTY.
fn kernel_size(mgr: &SessionManager, id: vitrum_proto::SessionId) -> (u16, u16) {
    let s = mgr.get(id).expect("session must exist");
    let master = s.master.lock().unwrap_or_else(|e| e.into_inner());
    let size = master.get_size().expect("querying the pty size");
    (size.cols, size.rows)
}

/// Two windows on one session must land on the smallest geometry.
///
/// Verified against a real child asking the kernel, because the point is what
/// the program in the session draws at, not what the daemon recorded. At 200x50
/// the smaller window would be showing a viewport it cannot fit.
#[cfg(unix)]
#[tokio::test]
async fn two_windows_settle_on_the_smaller_geometry() {
    let mgr = SessionManager::new(8192);
    let id = mgr
        .spawn(shell_spec("while read -r x; do stty size; done"))
        .expect("spawn");

    let wide = mgr.new_viewer();
    let narrow = mgr.new_viewer();
    let mut c = Collector::new(mgr.attach(id, wide, 200, 50).expect("attach wide"));
    assert_eq!(kernel_size(&mgr, id), (200, 50), "one window gets its size");

    mgr.attach(id, narrow, 100, 30).expect("attach narrow");
    assert_eq!(
        kernel_size(&mgr, id),
        (100, 30),
        "the pty must fit inside every attached window"
    );

    mgr.write(id, b"\n").expect("write");
    c.until(|b| b.ends_with(b"\r\n") && b.len() > 2).await;
    assert!(
        String::from_utf8_lossy(&c.bytes).contains("30 100"),
        "the child must see the minimum, got {:?}",
        String::from_utf8_lossy(&c.bytes)
    );
    mgr.close(id).expect("close");
}

/// Each axis takes its own minimum.
///
/// A window that is wide and short next to one that is narrow and tall must
/// leave the PTY narrow AND short. Taking the minimum of an area, or of one
/// axis only, would still clip somebody.
#[cfg(unix)]
#[tokio::test]
async fn the_minimum_is_taken_per_axis() {
    let mgr = SessionManager::new(8192);
    let id = mgr
        .spawn(shell_spec("while read -r x; do stty size; done"))
        .expect("spawn");
    let mut c = Collector::new(mgr.attach(id, mgr.new_viewer(), 200, 20).expect("attach"));
    mgr.attach(id, mgr.new_viewer(), 90, 60).expect("attach");
    assert_eq!(kernel_size(&mgr, id), (90, 20));

    mgr.write(id, b"\n").expect("write");
    c.until(|b| b.len() > 2 && b.ends_with(b"\r\n")).await;
    assert!(
        String::from_utf8_lossy(&c.bytes).contains("20 90"),
        "child saw {:?}",
        String::from_utf8_lossy(&c.bytes)
    );
    mgr.close(id).expect("close");
}

/// When the smaller window leaves, the PTY must grow back.
///
/// A window that closed a tab has no business holding every other window at its
/// old size. This is also what makes the common case self-healing: quit the
/// small window and the big one immediately renders at full size.
#[cfg(unix)]
#[tokio::test]
async fn detaching_the_smaller_window_grows_the_pty_back() {
    let mgr = SessionManager::new(8192);
    let id = mgr
        .spawn(shell_spec("while read -r x; do stty size; done"))
        .expect("spawn");
    let wide = mgr.new_viewer();
    let narrow = mgr.new_viewer();
    let mut c = Collector::new(mgr.attach(id, wide, 200, 50).expect("attach"));
    mgr.attach(id, narrow, 100, 30).expect("attach");
    assert_eq!(kernel_size(&mgr, id), (100, 30));

    mgr.detach(id, narrow);
    assert_eq!(
        kernel_size(&mgr, id),
        (200, 50),
        "the remaining window must get its own size back"
    );

    mgr.write(id, b"\n").expect("write");
    c.until(|b| b.len() > 2 && b.ends_with(b"\r\n")).await;
    assert!(
        String::from_utf8_lossy(&c.bytes).contains("50 200"),
        "child saw {:?}",
        String::from_utf8_lossy(&c.bytes)
    );
    mgr.close(id).expect("close");
}

/// A grown-back PTY must be announced, so the remaining window redraws.
///
/// The window that stayed did nothing, so nothing in its own event loop tells
/// it the session got bigger. Without a published observation it would keep
/// rendering a letterboxed viewport until the user happened to resize.
#[tokio::test]
async fn a_geometry_change_publishes_an_observation() {
    let mgr = SessionManager::new(8192);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    let wide = mgr.new_viewer();
    let narrow = mgr.new_viewer();
    mgr.attach(id, wide, 200, 50).expect("attach");
    let mut observations = mgr.subscribe_observations(id).expect("observations");

    mgr.attach(id, narrow, 100, 30).expect("attach");
    observations
        .has_changed()
        .expect("channel open")
        .then_some(())
        .expect("shrinking must publish an observation");
    observations.mark_unchanged();
    assert_eq!(mgr.info(id).expect("info").cols, 100);

    mgr.detach(id, narrow);
    assert!(
        observations.has_changed().expect("channel open"),
        "growing back must publish an observation too"
    );
    assert_eq!(mgr.info(id).expect("info").cols, 200);
    mgr.close(id).expect("close");
}

/// Alternating resizes from two windows must converge, not oscillate.
///
/// This is the bug in its natural habitat: two windows each re-asserting their
/// own layout on every render. With "last writer wins" the ioctl count climbs
/// without bound and the child reflows forever. With the minimum, the size is a
/// fixed point and every later resize that does not change the minimum costs
/// nothing at all.
#[tokio::test]
async fn alternating_resizes_converge_without_thrashing() {
    let mgr = SessionManager::new(8192);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    let wide = mgr.new_viewer();
    let narrow = mgr.new_viewer();
    mgr.attach(id, wide, 200, 50).expect("attach");
    mgr.attach(id, narrow, 100, 30).expect("attach");
    assert_eq!(kernel_size(&mgr, id), (100, 30));
    let settled = mgr.resize_count(id).expect("resize count");

    // Both windows re-assert their own layout twenty times over, exactly as
    // two render loops would.
    for _ in 0..20 {
        mgr.resize(id, wide, 200, 50).expect("wide resize");
        mgr.resize(id, narrow, 100, 30).expect("narrow resize");
    }

    assert_eq!(
        kernel_size(&mgr, id),
        (100, 30),
        "the size must be a fixed point, not whoever wrote last"
    );
    assert_eq!(
        mgr.resize_count(id).expect("resize count"),
        settled,
        "re-asserting a geometry that did not change the minimum must cost no ioctl"
    );
    mgr.close(id).expect("close");
}

/// One real change must cost exactly one ioctl.
///
/// The counter has to move when something genuinely changed, or the previous
/// test would pass on a resize path that never works at all.
#[tokio::test]
async fn a_real_change_costs_exactly_one_resize() {
    let mgr = SessionManager::new(8192);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    let viewer = mgr.new_viewer();
    mgr.attach(id, viewer, 80, 24).expect("attach");
    let before = mgr.resize_count(id).expect("resize count");
    mgr.resize(id, viewer, 81, 24).expect("resize");
    assert_eq!(mgr.resize_count(id).expect("resize count"), before + 1);
    assert_eq!(kernel_size(&mgr, id), (81, 24));
    mgr.close(id).expect("close");
}

/// A single window must behave exactly as it did before viewers existed.
///
/// The common case by a wide margin, and the one a multi-window feature must
/// not regress: attach, resize, and the child sees precisely what that window
/// asked for, with no minimum getting in the way.
#[cfg(unix)]
#[tokio::test]
async fn a_single_window_still_gets_exactly_what_it_asks_for() {
    let mgr = SessionManager::new(8192);
    // The child reports only when prodded, so the attach is sequenced before
    // the first report rather than racing the shell's startup.
    let id = mgr
        .spawn(shell_spec("while read -r x; do stty size; done"))
        .expect("spawn");
    let viewer = mgr.new_viewer();
    let mut c = Collector::new(mgr.attach(id, viewer, 132, 43).expect("attach"));
    mgr.write(id, b"\n").expect("write");
    c.until(|b| b.ends_with(b"43 132\r\n")).await;

    mgr.resize(id, viewer, 90, 25).expect("resize");
    mgr.write(id, b"\n").expect("write");
    c.until(|b| b.ends_with(b"25 90\r\n")).await;
    assert_eq!(kernel_size(&mgr, id), (90, 25));
    mgr.close(id).expect("close");
}

/// A window that disconnects without detaching must stop constraining.
///
/// Modelled here as a detach, which is what the connection's teardown does. A
/// crashed window that kept its constraint would pin the session at its size
/// for as long as the daemon runs, with no way to clear it short of a restart.
#[tokio::test]
async fn the_last_viewer_leaving_frees_the_geometry() {
    let mgr = SessionManager::new(8192);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    let gone = mgr.new_viewer();
    mgr.attach(id, gone, 40, 10).expect("attach");
    assert_eq!(kernel_size(&mgr, id), (40, 10));

    mgr.detach(id, gone);
    // Nothing attached, so the size is left alone rather than reflowed for
    // nobody. The next window to arrive sets it.
    assert_eq!(kernel_size(&mgr, id), (40, 10));

    mgr.attach(id, mgr.new_viewer(), 180, 45).expect("attach");
    assert_eq!(
        kernel_size(&mgr, id),
        (180, 45),
        "an unconstrained session must take the new window's size"
    );
    mgr.close(id).expect("close");
}

/// Detaching a viewer that is not attached must do nothing, not panic and not
/// resize, because a tab switch legitimately detaches twice.
#[tokio::test]
async fn detaching_twice_is_harmless() {
    let mgr = SessionManager::new(8192);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    let viewer = mgr.new_viewer();
    mgr.attach(id, viewer, 120, 40).expect("attach");
    let resizes = mgr.resize_count(id).expect("resize count");
    mgr.detach(id, viewer);
    mgr.detach(id, viewer);
    mgr.detach(id, ViewerId(9999));
    assert_eq!(mgr.resize_count(id).expect("resize count"), resizes);
    assert_eq!(kernel_size(&mgr, id), (120, 40));
    mgr.close(id).expect("close");
}

/// Detaching from an unknown session must not panic.
///
/// A client tearing down after its session was closed hits this on every
/// disconnect, so it is the normal path rather than an edge case.
#[tokio::test]
async fn detaching_an_unknown_session_is_silent() {
    let mgr = SessionManager::new(1024);
    mgr.detach(vitrum_proto::SessionId(404), ViewerId(1));
}

/// Re-attaching the same viewer must replace its geometry, not add a second
/// constraint.
///
/// A window that re-attaches after a reconnect would otherwise be counted
/// twice, and its old size would keep pinning the session forever.
#[tokio::test]
async fn re_attaching_replaces_that_window_s_constraint() {
    let mgr = SessionManager::new(8192);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    let viewer = mgr.new_viewer();
    mgr.attach(id, viewer, 80, 24).expect("attach");
    mgr.attach(id, viewer, 160, 48).expect("re-attach");
    assert_eq!(
        kernel_size(&mgr, id),
        (160, 48),
        "the same window's older size must not linger as a second viewer"
    );
    mgr.close(id).expect("close");
}

/// Attaching to an unknown session must be a named error.
#[tokio::test]
async fn attaching_to_an_unknown_session_errors() {
    let mgr = SessionManager::new(1024);
    let err = mgr
        .attach(vitrum_proto::SessionId(77), ViewerId(1), 80, 24)
        .expect_err("must fail");
    assert!(err.to_string().contains("77"), "unhelpful error: {err}");
}
