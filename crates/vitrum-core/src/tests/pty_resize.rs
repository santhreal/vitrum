//! Resize: the ioctl must reach the kernel and the child, not just the stored
//! `SessionInfo`.

#[cfg(unix)]
use crate::tests::helpers::Collector;
use crate::tests::helpers::shell_spec;
use crate::{SessionManager, ViewerId};

/// Read the size the kernel actually holds for a session's PTY.
///
/// Queried through the crate-internal handle rather than a public accessor,
/// because "did the ioctl happen" is an implementation fact and adding public API
/// to observe it would let the test pass against a cached value.
fn kernel_size(mgr: &SessionManager, id: vitrum_proto::SessionId) -> (u16, u16) {
    let s = mgr.get(id).expect("session exists");
    let size = s
        .master
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_size()
        .expect("kernel winsize");
    (size.cols, size.rows)
}

/// Spawning must set the requested geometry on the PTY itself.
///
/// A full-screen agent asks the terminal for its size at startup; if the spawn
/// geometry never reached the kernel it would draw at the default 80x24 no matter
/// what the client asked for.
#[tokio::test]
async fn spawn_sets_the_initial_kernel_winsize() {
    let mgr = SessionManager::new(1024);
    let mut spec = shell_spec("read -r x");
    spec.cols = 132;
    spec.rows = 43;
    let id = mgr.spawn(spec).expect("spawn");
    assert_eq!(kernel_size(&mgr, id), (132, 43));
    mgr.close(id).expect("close");
}

/// Resize must change the kernel's winsize, not only the stored info.
///
/// This is the exact bug the requirement names: a resize that updates the
/// projection leaves every program in the session drawing at the old size, and
/// the client's own layout looks right, so it is invisible until output wraps.
#[tokio::test]
async fn resize_reaches_the_kernel_winsize() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    assert_eq!(kernel_size(&mgr, id), (80, 24));
    let viewer = mgr.new_viewer();
    mgr.attach(id, viewer, 80, 24).expect("attach");
    mgr.resize(id, viewer, 120, 50).expect("resize");
    assert_eq!(kernel_size(&mgr, id), (120, 50));
    let info = mgr.info(id).expect("info");
    assert_eq!((info.cols, info.rows), (120, 50));
    mgr.close(id).expect("close");
}

/// The child must observe the new size through its own terminal.
///
/// End to end this proves the ioctl landed on the shared PTY and not on a private
/// copy: the child asks the kernel, so nothing the server caches can fake it.
/// The read in the middle sequences the resize before the second query, so the
/// test needs no timing assumptions.
#[cfg(unix)]
#[tokio::test]
async fn the_child_observes_the_new_size() {
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(shell_spec("stty size; read -r x; stty size"))
        .expect("spawn");
    let viewer = mgr.new_viewer();
    let mut c = Collector::new(mgr.attach(id, viewer, 80, 24).expect("attach"));
    c.until(|b| b.ends_with(b"\r\n")).await;
    assert_eq!(c.bytes, b"24 80\r\n", "child sees the spawn geometry");

    mgr.resize(id, viewer, 120, 50).expect("resize");
    mgr.write(id, b"\n").expect("write");
    c.until(|b| b.ends_with(b"50 120\r\n")).await;
    assert_eq!(
        c.bytes, b"24 80\r\n\r\n50 120\r\n",
        "stream is the first size, the echoed newline, then the new size"
    );
}

/// Zero columns or rows must be clamped to one on the PTY as well as in the info.
///
/// A client that has not laid out yet legitimately reports 0, and a zero-sized
/// terminal makes full-screen programs divide by zero.
#[tokio::test]
async fn resize_clamps_zero_to_one() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    let viewer = mgr.new_viewer();
    mgr.attach(id, viewer, 80, 24).expect("attach");
    mgr.resize(id, viewer, 0, 0).expect("resize");
    assert_eq!(kernel_size(&mgr, id), (1, 1));
    let info = mgr.info(id).expect("info");
    assert_eq!((info.cols, info.rows), (1, 1));
    mgr.close(id).expect("close");
}

/// Attaching at zero must be clamped too, since attach is the other way a
/// client's geometry reaches the PTY and a window reports 0 before it lays out.
#[tokio::test]
async fn attach_clamps_zero_to_one() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    mgr.attach(id, mgr.new_viewer(), 0, 0).expect("attach");
    assert_eq!(kernel_size(&mgr, id), (1, 1));
    mgr.close(id).expect("close");
}

/// A zero-sized spawn request must be clamped the same way, so the clamp cannot
/// be bypassed by never calling resize.
#[tokio::test]
async fn spawn_clamps_zero_geometry() {
    let mgr = SessionManager::new(1024);
    let mut spec = shell_spec("read -r x");
    spec.cols = 0;
    spec.rows = 0;
    let id = mgr.spawn(spec).expect("spawn");
    assert_eq!(kernel_size(&mgr, id), (1, 1));
    let info = mgr.info(id).expect("info");
    assert_eq!((info.cols, info.rows), (1, 1));
    mgr.close(id).expect("close");
}

/// Resizing an unknown session must be a named error, since a client racing a
/// close against a window resize is normal and must not panic the daemon.
#[tokio::test]
async fn resize_of_an_unknown_session_errors() {
    let mgr = SessionManager::new(1024);
    let err = mgr
        .resize(vitrum_proto::SessionId(99), ViewerId(1), 80, 24)
        .expect_err("must fail");
    assert!(err.to_string().contains("99"), "unhelpful error: {err}");
}

/// A viewer that never attached must not resize anything.
///
/// This is what makes "only attached clients constrain the size" real: a window
/// that lays out a session it is not drawing, or one that sends a stale resize
/// after switching tabs, would otherwise reflow the child for whoever IS
/// drawing it.
#[tokio::test]
async fn a_detached_viewer_does_not_resize() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    let drawing = mgr.new_viewer();
    mgr.attach(id, drawing, 100, 30).expect("attach");
    assert_eq!(kernel_size(&mgr, id), (100, 30));

    mgr.resize(id, mgr.new_viewer(), 20, 5)
        .expect("a stranger's resize is ignored, not an error");
    assert_eq!(
        kernel_size(&mgr, id),
        (100, 30),
        "a viewer that is not attached must not shrink the pty"
    );
    mgr.close(id).expect("close");
}
