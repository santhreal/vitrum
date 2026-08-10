//! Closing a session must leave nothing running that the session started.
//!
//! One sentence, two implementations. On Unix the child is a process group
//! leader on the pty and the escalation reaches the group; on Windows the child
//! and everything below it are in a job object that is closed when the session
//! is. The tests below assert the same observable property on both, so a
//! platform that stops honouring it fails here rather than in an operator's
//! session that will not let go of a directory.
//!
//! The process under test is a grandchild: the session's child spawns it and
//! then keeps running, which is what an agent that starts a language server or
//! a build does. Killing the child alone leaves it behind.

use std::path::Path;
use std::time::Duration;

use crate::SessionManager;
use crate::tests::helpers::{DEADLINE, TempDir};

/// How long between two looks at a pid that should be going away.
const POLL: Duration = Duration::from_millis(20);

/// Read the pid the grandchild wrote for itself, waiting for it to appear.
///
/// The file is written by the grandchild and by nothing else, so the number in
/// it names one process rather than describing a population. That is what makes
/// the assertion an identity check: a test that counted processes would pass
/// while the wrong one died.
async fn recorded_pid(path: &Path) -> u32 {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(pid) = text.trim().parse::<u32>() {
                if pid != 0 {
                    return pid;
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the grandchild never recorded its pid in {}",
            path.display()
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Wait for `pid` to stop existing, and say what it was still doing if it does
/// not.
async fn wait_gone(pid: u32, what: &str) {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    while alive(pid) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} (pid {pid}) was still running {DEADLINE:?} after the session closed"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Whether `pid` still names a running process.
///
/// A pid can be reused, so a true answer here is "something with that number
/// exists", not "the grandchild exists". The test only ever waits for this to
/// go false, and reuse can only delay that, never fake it.
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 performs the existence and permission checks and
    // delivers nothing.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Whether `pid` still names a running process.
///
/// `OpenProcess` succeeds for a process that has exited while a handle to it is
/// still open somewhere, so the exit code is checked as well: a pid object that
/// outlives its process is not a process that is still running.
#[cfg(windows)]
fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: no pointers in, and the returned handle is closed below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let mut code: u32 = 0;
    // SAFETY: `handle` is open and `code` is a live local.
    let ok = unsafe { GetExitCodeProcess(handle, &mut code) };
    // SAFETY: closing a handle this function opened and does not use again.
    unsafe { CloseHandle(handle) };
    ok != 0 && code == STILL_ACTIVE as u32
}

/// A session's grandchild must not outlive the session on Unix.
///
/// The child is `sh`, the grandchild is a second `sh` it backgrounds, and the
/// child then keeps running so that the close finds a live session rather than
/// one that has already exited on its own. Nothing here depends on the
/// grandchild being killed directly: it dies because the pty hangup reaches the
/// process group the child put it in, not because anything sends it a signal by
/// pid, and the contract is the outcome rather than the route.
///
/// What it does NOT catch: a grandchild that leaves the process group. Running
/// the same script through `setsid` was measured surviving the close for the
/// full thirty second bound, so this asserts nothing about a process that has
/// deliberately detached itself. Closing that would need the child's whole
/// descendant set tracked, which is the guarantee a Windows job object gives
/// and Unix has no equivalent of here.
#[cfg(unix)]
#[tokio::test]
async fn a_grandchild_does_not_outlive_a_closed_session() {
    let dir = TempDir::new("close-tree");
    let pid_file = dir.join("grandchild.pid");
    let script = format!(
        "sh -c 'echo $$ > {pid}; exec sleep 600' & sleep 600",
        pid = pid_file.display()
    );
    let spec = crate::SessionSpec {
        project_id: vitrum_proto::ProjectId(7),
        cwd: dir.path.clone(),
        command: "sh".to_string(),
        args: vec!["-c".to_string(), script],
        env: Vec::new(),
        cols: 80,
        rows: 24,
        title: None,
    };

    let mgr = SessionManager::new(64 * 1024);
    let id = mgr.spawn(spec).expect("spawning a session");
    let grandchild = recorded_pid(&pid_file).await;
    assert!(
        alive(grandchild),
        "the grandchild (pid {grandchild}) was already gone before the session was closed, so \
         this run proves nothing"
    );

    mgr.close(id).expect("closing the session");
    wait_gone(grandchild, "the session's grandchild").await;
}

/// A session's grandchild must not outlive the session on Windows.
///
/// This has never been executed on the machine that wrote it. There is no
/// Windows host here; it runs on the Windows CI leg and nowhere else, and the
/// only evidence behind it locally is that it compiles for
/// `x86_64-pc-windows-gnu`.
///
/// What it does NOT catch, and what would make it a false pass:
///
/// - The grandchild dying for a reason other than the job. If PowerShell's
///   `Start-Process` were to make the grandchild a console-attached descendant
///   that the pseudoconsole tears down on its own, this passes with the job
///   object removed. The assertion before the close only proves the grandchild
///   was alive then, not that the job is what killed it.
/// - Pid reuse. `alive` asks about a number; a pid retired and reissued to
///   something short-lived can only delay the pass, but a pid still held open
///   by an unrelated handle owner is reported as running and would fail the
///   test rather than pass it.
/// - A job that was never armed. `JobObject::containing` returns `None` on any
///   failure and only logs, so a run where the job could not be created reaches
///   the close with no tree guarantee at all. That shows up as a failure here,
///   which is the right direction, but the failure names the survivor rather
///   than the setup call that gave up.
#[cfg(windows)]
#[tokio::test]
async fn a_grandchild_does_not_outlive_a_closed_session() {
    let dir = TempDir::new("close-tree");
    let pid_file = dir.join("grandchild.pid");
    // `Start-Process` rather than a background job, because a PowerShell job
    // runs in a child that the runspace tears down with itself, which would
    // pass this test without a job object ever existing.
    let script = format!(
        "Start-Process -FilePath powershell -WindowStyle Hidden -ArgumentList \
         '-NoProfile','-Command',\"`$PID | Set-Content -Encoding ascii '{pid}'; \
         Start-Sleep -Seconds 600\"; Start-Sleep -Seconds 600",
        pid = pid_file.display()
    );
    let spec = crate::SessionSpec {
        project_id: vitrum_proto::ProjectId(7),
        cwd: dir.path.clone(),
        command: "powershell".to_string(),
        args: vec![
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-Command".to_string(),
            script,
        ],
        env: Vec::new(),
        cols: 80,
        rows: 24,
        title: None,
    };

    let mgr = SessionManager::new(64 * 1024);
    let id = mgr.spawn(spec).expect("spawning a session");
    let grandchild = recorded_pid(&pid_file).await;
    assert!(
        alive(grandchild),
        "the grandchild (pid {grandchild}) was already gone before the session was closed, so \
         this run proves nothing"
    );

    mgr.close(id).expect("closing the session");
    wait_gone(grandchild, "the session's grandchild").await;
}
