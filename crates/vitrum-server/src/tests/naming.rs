//! A connected client hearing about a name the program chose for itself.
//!
//! The daemon reads the title out of a session's own output on the PTY reader
//! thread, which is nowhere near a websocket. This is the proof that the change
//! actually crosses that distance: a client sitting on the socket is told,
//! without asking and without a refresh timer.
//!
//! Unix only, for the reason every escape test here is: `cmd.exe /C` has no
//! portable way to put an ESC byte into its output.

use vitrum_proto::ServerMsg;

use crate::tests::client::{Harness, create};

/// The projection carrying the program's own title reaches a live client.
///
/// Everything downstream of this is a sidebar row. A title the daemon knows and
/// never mentions is a feature nobody can see.
#[tokio::test]
async fn a_client_is_told_when_a_program_names_itself() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send(create(1, "printf '\\033]2;deploy\\007'; read -r x"))
        .await;

    c.until("the renamed projection", |s| {
        s.ctl.iter().any(|m| {
            matches!(m, ServerMsg::SessionUpdated(info) if info.title == "deploy")
        })
    })
    .await;
}

/// The same for a directory the shell reports.
///
/// The sidebar shows where a session is, and the branch beside it is resolved
/// from that directory, so a move nobody is told about leaves both wrong.
#[tokio::test]
async fn a_client_is_told_when_a_shell_changes_directory() {
    let dir = std::env::temp_dir().join("vitrum-osc7-wire");
    std::fs::create_dir_all(&dir).expect("create the directory");
    let want = dir.to_string_lossy().into_owned();

    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send(create(
        1,
        &format!("printf '\\033]7;file://host{}\\007'; read -r x", dir.display()),
    ))
    .await;

    c.until("the moved projection", |s| {
        s.ctl
            .iter()
            .any(|m| matches!(m, ServerMsg::SessionUpdated(info) if info.cwd == want))
    })
    .await;
}
