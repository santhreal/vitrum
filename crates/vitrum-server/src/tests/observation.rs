//! What the daemon observes must reach every window, without anyone polling.
//!
//! `Attention.waiting` and `SessionInfo.hint` change while nothing else does:
//! the child produces no output when it settles at a prompt, and a hint is
//! invisible bytes. Neither has a lifecycle transition behind it, so without a
//! dedicated push a second window's sidebar would show the previous state until
//! something unrelated happened to refresh it, which for a blocked agent is
//! never.

use vitrum_proto::HintState;
#[cfg(target_os = "linux")]
use vitrum_proto::{ClientMsg, ServerMsg};

use crate::tests::client::{Harness, create};

/// The foreground probe's answer must be pushed to a client that only listens.
///
/// No output, no exit, no request: the session goes quiet at its prompt and the
/// sidebar has to learn about it anyway. This is the difference between a row
/// that says "your turn" and a row that spins forever.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_blocked_session_is_pushed_to_a_passive_client() {
    // This kernel will not say what a process is doing, so the daemon has no
    // observation to send and there is nothing to wait for.
    if !vitrum_core::test_support::kernel_reports_other_processes() {
        return;
    }
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "read -r x")).await;

    c.until("the daemon to report it is blocked", |s| {
        s.updates()
            .iter()
            .any(|i| i.id == id && i.attention.waiting == Some(true))
    })
    .await;
    h.manager.close(id).expect("close");
}

/// A second window that never touched the session must get the same push.
///
/// Windows are independent views of one daemon; the observation belongs to the
/// daemon, so every connected client sees it whether or not it created,
/// attached to, or has ever heard of that session.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn every_window_learns_what_the_daemon_observed() {
    // This kernel will not say what a process is doing, so the daemon has no
    // observation to send and there is nothing to wait for.
    if !vitrum_core::test_support::kernel_reports_other_processes() {
        return;
    }
    let h = Harness::start(4096).await;
    let mut creator = h.greeted().await;
    let mut bystander = h.greeted().await;
    let id = creator.create(create(1, "read -r x")).await;

    bystander
        .until("the observation", |s| {
            s.updates()
                .iter()
                .any(|i| i.id == id && i.attention.waiting == Some(true))
        })
        .await;
    h.manager.close(id).expect("close");
}

/// A hint an agent declares must reach every window.
///
/// The opt-in channel is worth nothing if it stops at the daemon. A harness
/// prints six bytes and a window that was not even watching learns there is an
/// approval pending, with the label.
#[cfg(not(windows))]
#[tokio::test]
async fn a_declared_hint_reaches_every_window() {
    let h = Harness::start(8192).await;
    let mut creator = h.greeted().await;
    let mut bystander = h.greeted().await;
    let id = creator
        .create(create(
            1,
            "printf '\\033]7373;approval;force push?\\033\\\\'; read -r x",
        ))
        .await;

    for client in [&mut creator, &mut bystander] {
        client
            .until("the hint", |s| {
                s.updates().iter().any(|i| i.id == id && i.hint.is_some())
            })
            .await;
        let hint = client
            .seen
            .updates()
            .into_iter()
            .filter(|i| i.id == id)
            .find_map(|i| i.hint.clone())
            .expect("a hint must have been pushed");
        assert_eq!(hint.state, HintState::Approval);
        assert_eq!(hint.label.as_deref(), Some("force push?"));
    }
    h.manager.close(id).expect("close");
}

/// A hint terminated by BEL must not also arrive as a bell.
///
/// BEL is a legal terminator, so an agent that used it would otherwise raise
/// the attention indicator on every declaration it ever made.
#[cfg(not(windows))]
#[tokio::test]
async fn a_hint_does_not_arrive_as_a_bell() {
    let h = Harness::start(8192).await;
    let mut c = h.greeted().await;
    let id = c
        .create(create(1, "printf '\\033]7373;ready;done\\007'; read -r x"))
        .await;

    c.until("the hint", |s| {
        s.updates().iter().any(|i| i.id == id && i.hint.is_some())
    })
    .await;
    let latest = c
        .seen
        .updates()
        .into_iter()
        .rfind(|i| i.id == id)
        .expect("an update");
    assert_eq!(
        latest.hint.as_ref().map(|h| h.state),
        Some(HintState::Ready)
    );
    assert!(
        !latest.attention.bell,
        "a hint's own terminator is not a request for the operator"
    );
    h.manager.close(id).expect("close");
}

/// A settled session must stop generating traffic entirely.
///
/// The probe is armed by activity and disarmed by its own answer, so once a
/// session has settled the daemon has nothing more to say. If it kept pushing,
/// twenty idle agents would be twenty streams of identical projections and the
/// idle CPU this product is built around would be gone.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_settled_session_goes_completely_quiet() {
    // This kernel will not say what a process is doing, so the daemon has no
    // observation to send and there is nothing to wait for.
    if !vitrum_core::test_support::kernel_reports_other_processes() {
        return;
    }
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "read -r x")).await;
    c.until("the settled observation", |s| {
        s.updates()
            .iter()
            .any(|i| i.id == id && i.attention.waiting == Some(true))
    })
    .await;

    // Nothing else may arrive: no re-probe, no repeated projection, nothing.
    c.quiet().await;
    assert_eq!(
        h.manager.probe_count(id).expect("probe count"),
        1,
        "a session that settled once must be probed exactly once"
    );
    h.manager.close(id).expect("close");
}

/// Input must re-arm the probe even when the child echoes nothing.
///
/// A password prompt reads with echo off, so answering it produces no output at
/// all. Without treating input as activity, the row would keep claiming the
/// agent is blocked on you long after you answered it.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn input_re_arms_the_probe_with_no_output_at_all() {
    // This kernel will not say what a process is doing, so the daemon has no
    // observation to send and there is nothing to wait for.
    if !vitrum_core::test_support::kernel_reports_other_processes() {
        return;
    }
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    // `stty -echo` means the child prints nothing when the operator types, and
    // the spin loop afterwards is unmistakably working.
    let id = c
        .create(create(1, "stty -echo; read -r x; while :; do :; done"))
        .await;
    c.until("the blocked observation", |s| {
        s.updates()
            .iter()
            .any(|i| i.id == id && i.attention.waiting == Some(true))
    })
    .await;

    c.send(ClientMsg::Input {
        session: id,
        data: b"secret\n".to_vec(),
    })
    .await;
    c.until("the daemon to notice the child went back to work", |s| {
        s.updates()
            .iter()
            .any(|i| i.id == id && i.attention.waiting == Some(false))
    })
    .await;
    assert_eq!(
        c.seen.bytes(id),
        b"",
        "the child echoed nothing, so only the input re-armed the probe"
    );
    h.manager.close(id).expect("close");
}

/// An exit must clear the foreground answer on the wire.
///
/// A dead session is not waiting for you. Shipping `Some(true)` on an exited
/// row would leave a permanent "your turn" badge with nothing behind it.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_exit_clears_the_foreground_answer_on_the_wire() {
    // This kernel will not say what a process is doing, so the daemon has no
    // observation to send and there is nothing to wait for.
    if !vitrum_core::test_support::kernel_reports_other_processes() {
        return;
    }
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "read -r x")).await;
    c.until("the blocked observation", |s| {
        s.updates()
            .iter()
            .any(|i| i.id == id && i.attention.waiting == Some(true))
    })
    .await;

    c.send(ClientMsg::Input {
        session: id,
        data: b"go\n".to_vec(),
    })
    .await;
    c.until("the exit", |s| {
        s.ctl
            .iter()
            .any(|m| matches!(m, ServerMsg::Exited { session, .. } if *session == id))
    })
    .await;

    let final_projection = c
        .seen
        .updates()
        .into_iter()
        .rfind(|i| i.id == id)
        .expect("an update");
    assert!(!final_projection.status.is_live());
    assert_eq!(
        final_projection.attention.waiting, None,
        "a dead session has no foreground process to be waiting"
    );
}
