//! `Attention.waiting`, proved against real processes on a real PTY.
//!
//! Every case here spawns an actual program and asks the actual kernel. A test
//! that fed the classifier a syscall number would only prove the classifier
//! agrees with itself; the claim being made is about `bash`, `cat`, `sleep` and
//! `top`, so those are what run.
//!
//! The suite is written for Linux, because Linux is the only platform where the
//! discrimination exists. `waiting_is_unknown_on_platforms_that_cannot_answer`
//! covers the rest, and it is the important one for the honesty contract: a
//! platform that cannot tell must say so rather than guess.

#[cfg(target_os = "linux")]
use std::path::PathBuf;

use crate::SessionManager;
#[cfg(target_os = "linux")]
use crate::SessionSpec;
use vitrum_proto::ProjectId;
#[cfg(target_os = "linux")]
use crate::session::SETTLE_WINDOW;
use crate::tests::helpers::DEADLINE;
#[cfg(target_os = "linux")]
use crate::tests::helpers::collect;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::tests::helpers::waiting_settles_on;
use crate::tests::helpers::{QUIET, probe_now, shell_spec, wait_exit};

/// A spec running `command` with `args` from a directory that exists.
#[cfg(target_os = "linux")]
fn spec(command: &str, args: &[&str]) -> SessionSpec {
    SessionSpec {
        project_id: vitrum_proto::ProjectId(3),
        cwd: std::env::temp_dir(),
        command: command.to_string(),
        args: args.iter().map(|a| (*a).to_string()).collect(),
        env: Vec::new(),
        cols: 100,
        rows: 30,
        title: None,
    }
}

/// First existing path among `candidates`, for tools that are standard but not
/// guaranteed on a stripped-down image.
#[cfg(target_os = "linux")]
fn first_present(candidates: &[&str]) -> Option<PathBuf> {
    candidates.iter().map(PathBuf::from).find(|p| p.is_file())
}

/// A shell sitting at its prompt must read as blocked on the operator.
///
/// This is the single most common state in the product: twenty tabs, nineteen
/// of them a shell waiting for you. Getting it wrong makes every idle session
/// claim to be working and the sidebar stops meaning anything. Measured, the
/// shell parks in `pselect6` with a NULL timeout or in `read` on the tty
/// depending on which shell it is, and both must land on the same answer.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_shell_at_its_prompt_is_blocked_on_the_operator() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(spec("sh", &["-i"])).expect("spawn");
    // The prompt is the shell telling us it has finished starting.
    let mut c = collect(&mgr, id);
    c.until(|b| !b.is_empty()).await;
    waiting_settles_on(&mgr, id, Some(true), "an interactive shell").await;
    mgr.close(id).expect("close");
}

/// A shell blocked in `read` on the terminal must read as blocked.
///
/// The `read -p 'approve? '` case: an agent, or a script, asking a question.
/// The syscall is `read` and the descriptor resolves to this session's tty,
/// which is the strongest evidence the kernel offers.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_child_reading_the_terminal_is_blocked_on_the_operator() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(shell_spec("read -r answer")).expect("spawn");
    waiting_settles_on(&mgr, id, Some(true), "a shell read builtin").await;
    mgr.close(id).expect("close");
}

/// `cat` with no arguments must read as blocked.
///
/// A different program, a different libc path, the same `read(tty)`. Included
/// because the shell cases all go through one implementation and a probe that
/// only worked for shells would be worthless for hosted agents.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_plain_reader_on_the_terminal_is_blocked() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(spec("cat", &[])).expect("spawn");
    waiting_settles_on(&mgr, id, Some(true), "cat on the tty").await;
    mgr.close(id).expect("close");
}

/// A read on something that is NOT the terminal must read as working.
///
/// This is why the probe resolves the descriptor instead of assuming fd 0. A
/// child streaming a file, or parked on a pipe, is doing I/O, not waiting for a
/// human, and calling it Ready would put a "your turn" badge on a busy agent.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_read_on_another_descriptor_is_working() {
    let mgr = SessionManager::new(4096);
    // `exec cat <> fifo` makes cat the foreground leader and parks it in
    // read(2) on a descriptor that is not this session's terminal. Opening
    // read-write is what keeps `open` from blocking first, so the process is
    // provably in the read and the test cannot pass for the wrong reason.
    let dir = crate::tests::helpers::TempDir::new("probe-fifo");
    let fifo = dir.join("pipe");
    let id = mgr
        .spawn(shell_spec(&format!(
            "mkfifo {0} && exec cat <> {0}",
            fifo.display()
        )))
        .expect("spawn");
    waiting_settles_on(&mgr, id, Some(false), "a read on a fifo").await;
    mgr.close(id).expect("close");
}

/// A sleeping child must read as working.
///
/// `clock_nanosleep` is not a way to block on a terminal. An agent that backs
/// off before a retry is mid-turn, and the operator has nothing to do about it.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_sleeping_child_is_working() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(spec("sleep", &["300"])).expect("spawn");
    waiting_settles_on(&mgr, id, Some(false), "sleep 300").await;
    mgr.close(id).expect("close");
}

/// A child burning CPU must read as working.
///
/// `/proc/<pid>/syscall` says `running` rather than a number, which is a shape
/// the parser has to handle before it reaches any syscall table at all.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_spinning_child_is_working() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(shell_spec("while :; do :; done")).expect("spawn");
    waiting_settles_on(&mgr, id, Some(false), "a spin loop").await;
    mgr.close(id).expect("close");
}

/// A shell waiting on a child must read as working.
///
/// `wait4` means the group leader has delegated the work, not that it has
/// stopped. This is the shape of every `make`, every test run, and every agent
/// invoked from a wrapper script.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_shell_reaping_a_child_is_working() {
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(shell_spec("while :; do sleep 5; done"))
        .expect("spawn");
    waiting_settles_on(&mgr, id, Some(false), "a shell in wait4").await;
    mgr.close(id).expect("close");
}

/// A shell that prints and then waits must flip from working to blocked.
///
/// The two states in one session, driven by a real child, which is what proves
/// the probe re-runs after output rather than answering once and latching.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_child_that_finishes_its_turn_flips_to_blocked() {
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(shell_spec("echo turn; read -r answer; while :; do :; done"))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| b.ends_with(b"turn\r\n")).await;
    waiting_settles_on(&mgr, id, Some(true), "after the turn ended").await;

    // Answering it puts the shell back to work.
    mgr.write(id, b"yes\n").expect("write");
    c.until(|b| b.ends_with(b"yes\r\n")).await;
    waiting_settles_on(&mgr, id, Some(false), "after the operator answered").await;
    mgr.close(id).expect("close");
}

/// An event-loop program that wakes on a timer must read as UNKNOWN.
///
/// Measured on this machine: `top -d 0.1` sits in `pselect6` with a non-NULL
/// timeout, and so does an idle `vi`, and so does a real `claude`. One of those
/// is working and two are waiting for a human, in the same syscall with the
/// same shape. Calling it working would stamp a permanent Working badge on
/// every idle TUI agent, which is the exact state the sidebar exists to
/// distinguish, so the honest answer is that we do not know and the row falls
/// back to bell and idle inference.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_timed_event_loop_is_unknown_rather_than_guessed() {
    let Some(top) = first_present(&["/usr/bin/top", "/bin/top"]) else {
        // Not on this image. `an_idle_full_screen_editor_is_unknown` covers the
        // same branch with a different program, and both are optional for the
        // same reason: neither is in POSIX.
        return;
    };
    let mgr = SessionManager::new(64 * 1024);
    let id = mgr
        .spawn(spec(&top.to_string_lossy(), &["-d", "5"]))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    // top draws its first frame immediately; that is it reaching steady state.
    // A five second refresh, not a fast one, because a program that redraws
    // faster than the settle window is never probed at all, which is correct
    // behaviour and would make this test unable to reach the branch it is for.
    c.until(|b| b.len() > 64).await;
    waiting_settles_on(&mgr, id, None, "top waiting on its refresh timer").await;
    mgr.close(id).expect("close");
}

/// An idle full-screen editor must read as UNKNOWN for the same reason.
///
/// This is the case that rules out "a poll with a timeout means working": an
/// editor with nothing to do is waiting for the operator, and it polls on a
/// timer anyway because of its own cursor-hold and autosave timers.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_idle_full_screen_editor_is_unknown() {
    let Some(vi) = first_present(&["/usr/bin/vim", "/usr/bin/vi", "/bin/vi"]) else {
        return;
    };
    let mgr = SessionManager::new(64 * 1024);
    let id = mgr
        .spawn(spec(&vi.to_string_lossy(), &["-u", "NONE", "-n"]))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| b.len() > 32).await;
    waiting_settles_on(&mgr, id, None, "an idle editor").await;
    mgr.close(id).expect("close");
}

/// A platform with no way to answer must report `None`, never `Some(false)`.
///
/// `None` and `Some(false)` are different claims: one says "cannot tell", the
/// other says "proven working". Collapsing them would make Windows and every
/// unported architecture render an idle agent as busy forever, with no way for
/// the UI to say the platform cannot tell. This is the contract the whole
/// `Option<bool>` shape exists for.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[tokio::test]
async fn waiting_is_unknown_on_platforms_that_cannot_answer() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(shell_spec("pause")).expect("spawn");
    let info = probe_now(&mgr, id).await;
    assert_eq!(
        info.attention.waiting, None,
        "a platform that cannot answer must not claim to"
    );
    mgr.close(id).expect("close");
}

/// macOS proves working and never claims blocked.
///
/// `proc_pidinfo` reports thread run state, and a tty read and a nanosleep are
/// both `TH_STATE_WAITING`. So a running thread proves `Some(false)` and
/// everything else is `None`. Asserting the negative here is the point: it
/// stops a future change from inferring `Some(true)` from a sleeping thread,
/// which would be a confident wrong answer on the one state that matters most.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_never_claims_blocked() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(shell_spec("read -r answer")).expect("spawn");
    let info = probe_now(&mgr, id).await;
    assert_ne!(
        info.attention.waiting,
        Some(true),
        "macOS cannot prove blocking and must not claim it"
    );
    mgr.close(id).expect("close");
}

/// A spinning child on macOS must be provably working.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_proves_a_spinning_child_is_working() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(shell_spec("while :; do :; done")).expect("spawn");
    waiting_settles_on(&mgr, id, Some(false), "a spin loop on macOS").await;
    mgr.close(id).expect("close");
}

/// The probe must not run on a timer.
///
/// It is armed by output or by input and disarmed by its own answer, so a
/// session nobody is touching must hold its probe count still. A count that
/// climbs on its own is a wakeup per session per tick, which is exactly the
/// idle CPU this product refuses to spend and the thing every other terminal
/// gets wrong.
#[tokio::test]
async fn a_settled_session_is_never_probed_again() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(shell_spec("read -r answer")).expect("spawn");
    let settled = probe_now(&mgr, id).await;
    assert!(settled.status.is_live());
    let after_settling = mgr.probe_count(id).expect("probe count");

    // Long enough to catch any plausible tick: the settle window is 150ms.
    tokio::time::sleep(QUIET * 6).await;
    assert_eq!(
        mgr.probe_count(id).expect("probe count"),
        after_settling,
        "a quiet session must cost zero probes"
    );
    mgr.close(id).expect("close");
}

/// Output re-arms the probe, so a session that starts talking again is
/// re-examined without anything polling it.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn output_re_arms_the_probe() {
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(shell_spec("read -r a; echo second; read -r b"))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    let before = probe_now(&mgr, id).await;
    assert_eq!(before.attention.waiting, Some(true));
    let count = mgr.probe_count(id).expect("probe count");

    mgr.write(id, b"go\n").expect("write");
    c.until(|b| b.ends_with(b"second\r\n")).await;
    let deadline = tokio::time::Instant::now() + DEADLINE;
    while mgr.probe_count(id).expect("probe count") <= count {
        assert!(
            tokio::time::Instant::now() < deadline,
            "output must re-arm the probe"
        );
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    mgr.close(id).expect("close");
}

/// An exited session must report `None`, not its last live answer.
///
/// There is no foreground process to be blocked, so `Some(true)` would be a
/// claim about something that no longer exists, and the row would carry a "your
/// turn" badge with nothing behind it.
#[tokio::test]
async fn an_exited_session_has_no_foreground_answer() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(shell_spec("read -r answer")).expect("spawn");
    let live = probe_now(&mgr, id).await;
    assert!(live.status.is_live(), "the child is still running");

    mgr.write(id, b"done\n").expect("write");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    let dead = mgr.info(id).expect("info");
    assert_eq!(
        dead.attention.waiting, None,
        "a dead session has no foreground process to be waiting"
    );
}

/// Regression lock on the exact `/proc/<pid>/syscall` lines real programs
/// produced on a real PTY.
///
/// The live tests above are the proof; these are the record. They exist because
/// the discrimination rests on which argument holds the timeout for each
/// syscall, and getting one index wrong is invisible until the sidebar quietly
/// starts lying about an agent. Every line here was captured, not invented.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod captured {
    use crate::probe::classify_line;

    /// Nothing in this module is a `read`, so no descriptor should ever be
    /// consulted; a classifier that asked anyway is a classifier that would
    /// misread a poll as a terminal read.
    fn never_the_tty(fd: u64) -> bool {
        panic!("the fd of a non-read syscall must never be resolved (asked about {fd})")
    }

    /// Captured from `bash --norc -i` sitting at its prompt. Argument four, the
    /// `struct timespec *`, is NULL: this call can only end when a descriptor
    /// becomes ready, and in a terminal that means the operator.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn a_bash_prompt_reads_as_blocked() {
        let line =
            "270 0x1 0x7ffd34e06750 0x0 0x0 0x0 0x7ffd34e06670 0x7ffd34e06630 0x75ba5272600e";
        assert_eq!(classify_line(line, &never_the_tty), Some(true));
    }

    /// Captured from `top -d 0.1`. Same syscall as the prompt above, but
    /// argument four is a real pointer, so it wakes on its own schedule. That
    /// is indistinguishable from an idle editor doing the same thing, so the
    /// only honest answer is that we do not know.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn a_timed_pselect_reads_as_unknown() {
        let line = "270 0x1 0x7fffacfe46b0 0x0 0x0 0x7fffacfe4610 0x7fffacfe4620 0x7fffacfe45e0 0x7f02fe92600e";
        assert_eq!(classify_line(line, &never_the_tty), None);
    }

    /// Captured from `vi -u NONE` with nothing to do. Proof that the timed-poll
    /// case really does mix waiting and working: this program is waiting for a
    /// keystroke and looks exactly like `top`.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn an_idle_editor_reads_as_unknown() {
        let line = "270 0x1 0x5e1eee1b2300 0x5e1eee1b2280 0x5e1eee1b2200 0x7ffe2372ed10 0x0 0x7ffe2372ecf0 0x7b1646526c6e";
        assert_eq!(classify_line(line, &never_the_tty), None);
    }

    /// Captured from `node -e 'process.stdin.resume()'`. `epoll_pwait` takes an
    /// `int` timeout, so "forever" arrives as 0xffffffff in a 64-bit register
    /// and has to be read back as -1. Truncating to 32 bits and sign-extending
    /// is the whole difference between "blocked on you" and "unknown" for every
    /// node-based agent parked on stdin.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn an_infinite_epoll_reads_as_blocked() {
        let line = "281 0x11 0x7ffed60960f0 0x400 0xffffffff 0x0 0x8 0x7ffed6095380 0x7a689af29f10";
        assert_eq!(classify_line(line, &never_the_tty), Some(true));
    }

    /// Captured from `node -e 'setInterval(()=>{},50); process.stdin.resume()'`:
    /// the same call with a 50 millisecond timeout.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn a_timed_epoll_reads_as_unknown() {
        let line = "281 0x11 0x7ffcd6e1e2e0 0x400 0x32 0x0 0x8 0x7ffcd6e1d570 0x757069d29f10";
        assert_eq!(classify_line(line, &never_the_tty), None);
    }

    /// Captured from a real `claude` process. `epoll_pwait2` puts its timeout
    /// in argument three as a pointer, one slot earlier than `pselect6`, which
    /// is exactly the kind of detail a per-syscall table exists to get right.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn a_real_agent_event_loop_reads_as_unknown() {
        let line = "441 0x4 0x522105000e0 0x400 0x7ffee05c7700 0x7ffee05c75c0 0x8 0x7ffee05c75c0 0x37d492d";
        assert_eq!(classify_line(line, &never_the_tty), None);
    }

    /// A non-blocking poll is not blocked on anything, by definition.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn a_zero_timeout_poll_reads_as_working() {
        let line = "281 0x11 0x7ffcd6e1e2e0 0x400 0x0 0x0 0x8 0x7ffcd6e1d570 0x757069d29f10";
        assert_eq!(classify_line(line, &never_the_tty), Some(false));
    }

    /// Captured from `sleep 300`, and from a shell in `wait4`. Neither is a way
    /// to block on a terminal.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn sleeping_and_reaping_read_as_working() {
        let sleeping =
            "230 0x0 0x0 0x7ffc67ea3320 0x7ffc67ea3310 0x0 0x0 0x7ffc67ea3250 0x7236912eca7a";
        let reaping = "61 0xffffffff 0x7fff4df50590 0x0 0x0 0x8 0x8 0x7fff4df50558 0x71e25ef107d7";
        assert_eq!(classify_line(sleeping, &never_the_tty), Some(false));
        assert_eq!(classify_line(reaping, &never_the_tty), Some(false));
    }

    /// `running` and `-1` are not syscall numbers and must be handled before
    /// any table lookup, or the parser fails on the two commonest shapes.
    #[test]
    fn on_cpu_and_between_syscalls_read_as_working() {
        assert_eq!(classify_line("running", &never_the_tty), Some(false));
        assert_eq!(
            classify_line("-1 0x7ffd34e06630 0x75ba5272600e", &never_the_tty),
            Some(false)
        );
    }

    /// A `read` is only evidence when the descriptor is THIS terminal.
    ///
    /// Captured from `bash -c "read -p ... x"` (fd 0) and from `less FILE`
    /// (fd 4, because less reopens the terminal). Assuming fd 0 would miss the
    /// second and would call a child reading an input file "blocked on you".
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn a_read_is_judged_by_its_descriptor() {
        let on_fd_0 =
            "0 0x0 0x64d67683c3c0 0x1000 0x0 0x79c67f203b20 0x410 0x7ffdbd2189c8 0x79c67f11ba91";
        let on_fd_4 = "0 0x4 0x7ffeb1ab2d57 0x1 0x24 0x5d381c79f478 0x7ffeb1ab27f0 0x7ffeb1ab2c68 0x703e2f71ba91";
        assert_eq!(classify_line(on_fd_0, &|fd| fd == 0), Some(true));
        assert_eq!(classify_line(on_fd_4, &|fd| fd == 4), Some(true));
        assert_eq!(
            classify_line(on_fd_4, &|_| false),
            Some(false),
            "a read on something that is not this terminal is ordinary I/O"
        );
    }

    /// A restarting syscall does not name the call it is restarting, so there
    /// is nothing honest to report.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn a_restarting_syscall_reads_as_unknown() {
        let line = "219 0x0 0x0 0x0 0x0 0x0 0x0 0x7ffd34e06630 0x75ba5272600e";
        assert_eq!(classify_line(line, &never_the_tty), None);
    }

    /// A truncated or unparseable line must be unknown, not a lucky default.
    ///
    /// `/proc` can hand back a short read when a task exits mid-query, and
    /// treating that as "working" would flicker every dying session.
    #[test]
    fn a_malformed_line_reads_as_unknown() {
        for line in [
            "",
            "270 0x1",
            "nonsense 1 2 3 4 5 6 7 8",
            "270 zz 0 0 0 0 0 0 0",
        ] {
            assert_eq!(
                classify_line(line, &never_the_tty),
                None,
                "accepted a malformed line: {line:?}"
            );
        }
    }
}

/// A session that never stops talking must never be probed.
///
/// **This is the regression guard for the single largest client cost in the
/// product. Do not relax it.** A probe that fires only changes `waiting` when
/// the answer differs, and every such change publishes one `SessionUpdated` to
/// every connected window. That projection was measured at 16 to 20 ms of
/// CLIENT CPU each, almost all of it WebKit style, layout and paint, and almost
/// independent of session count. A probe on the hot path would arm once per
/// coalesced chunk, which is up to 166 per second per session, so twenty
/// streaming agents would be asking twenty clients to spend 16 ms thousands of
/// times a second. The daemon's silence while output flows is what makes that
/// number irrelevant instead of fatal.
///
/// Output re-arms the settle timer, so a child producing bytes faster than the
/// window never reaches the point of being asked about at all, and the probe
/// runs exactly once for the whole burst when it finally stops. The daemon-side
/// saving, a `tcgetpgrp` and a `/proc` read per burst per session, is the small
/// half of this.
/// A child that emits 30 ticks 10ms apart WITHOUT forking once.
///
/// The obvious script is `printf tick; sleep 0.01` in a loop, and it was the
/// reason this test was flaky: that forks and execs `sleep` thirty times, and
/// on a machine already building something else a single one of those forks
/// takes longer than the 150ms settle window. The stall then looks exactly
/// like the bug under test, because the gap between two writes really did
/// exceed the window.
///
/// `read -t` is a bash builtin and stdin here is the PTY, which stays silent
/// until the test writes to it. So the delay costs no process at all, and the
/// only fork in the whole run is the shell itself. Bash by name rather than
/// `sh`, because a fractional `-t` is not POSIX; safe here because this test
/// is Linux-only for unrelated reasons.
fn ticking_spec() -> SessionSpec {
    SessionSpec {
        project_id: ProjectId(7),
        cwd: std::env::temp_dir(),
        command: "bash".to_string(),
        args: vec![
            "-c".to_string(),
            "i=0; while [ $i -lt 30 ]; do printf tick; read -t 0.01 x; i=$((i+1)); done; \
             read -r x"
                .to_string(),
        ],
        env: Vec::new(),
        cols: 80,
        rows: 24,
        title: None,
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_streaming_session_is_never_probed() {
    // A tick every 10ms against a 150ms settle window, for 30 ticks.
    //
    // The interval used to be 50ms, which left only a 3x margin: three missed
    // ticks and the timer settles, the probe runs, and the assertions below
    // fail. 10ms gives 15 ticks of headroom instead of 3.
    //
    // Headroom alone is still not enough, because this box also builds other
    // workspaces: measured at load 145 this failed two runs in five, and the
    // panic message had to talk the reader out of believing it. THE WHOLE
    // EXPERIMENT is therefore repeatable. Its premise is that the ticks were
    // continuous, and a run where the burst took far longer than the settle
    // window did not have continuous ticks, so whatever it observed is not
    // evidence about the re-arm. Such a run is discarded and repeated.
    //
    // Nothing is ignored. A run whose burst WAS continuous is judged on the
    // spot, and running out of attempts fails with the timings.
    const ATTEMPTS: usize = 6;
    // Ten ticks is about 100ms of wall time plus a fork and exec each. Past
    // twice the settle window the ticks were no longer continuous.
    // 30 ticks of 10ms, ignoring the fork and exec each one also costs.
    const IDEAL_BURST: std::time::Duration = std::time::Duration::from_millis(300);

    let mut last = String::new();
    for attempt in 1..=ATTEMPTS {
        let mgr = SessionManager::new(64 * 1024);
        let id = mgr.spawn(ticking_spec()).expect("spawn");
        let mut c = collect(&mgr, id);

        // Ten ticks of continuous output must never reach the probe.
        let started = std::time::Instant::now();
        c.until(|b| b.len() >= 40).await;
        let burst = started.elapsed();
        let during = mgr.probe_count(id).expect("probe count");

        // Once it stops, exactly one probe answers for the whole run.
        c.until(|b| b.len() >= 120).await;
        let total = started.elapsed();
        let deadline = tokio::time::Instant::now() + DEADLINE;
        while mgr.probe_count(id).expect("probe count") == 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the probe must run once the stream finally goes quiet"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        // The script reaches `read` a moment after its last `printf`, and
        // under load that gap widens: a probe firing inside it correctly
        // reports "not waiting", which is a true observation of the wrong
        // instant. Waited for rather than sampled, because the contract is
        // that a quiet session ends up marked waiting, not that it is marked
        // within one scheduler tick.
        let deadline = tokio::time::Instant::now() + DEADLINE;
        while mgr.info(id).expect("info").attention.waiting != Some(true) {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let waiting = mgr.info(id).expect("info").attention.waiting;
        let after = mgr.probe_count(id).expect("probe count");
        mgr.close(id).expect("close");

        if during == 0 && after == 1 && waiting == Some(true) {
            return;
        }

        last = format!(
            "attempt {attempt}: {during} probe(s) during the first ten ticks \
             ({burst:?}), {after} in total over the whole {total:?} burst, \
             waiting={waiting:?}, against a {SETTLE_WINDOW:?} settle window"
        );

        // A probe fired mid-burst, which means SOME gap between two `printf`s
        // exceeded the settle window. Either the timer stopped re-arming, or
        // this machine did not schedule the child for that long. The whole
        // burst is 30 ticks of 10ms plus a fork and exec each; a run that took
        // anywhere near the ideal was continuous, so a probe during it is the
        // bug. A run far past it was starved and proves nothing.
        assert!(
            total > IDEAL_BURST + SETTLE_WINDOW,
            "output stopped re-arming the settle timer. {last}. The burst ran \
             close to its ideal {IDEAL_BURST:?}, so the ticks were continuous \
             and this is the bug this test exists for, not a stalled machine"
        );
    }

    panic!(
        "this machine never gave the experiment a continuous burst in {ATTEMPTS} \
         tries, so the re-arm could not be observed either way. {last}"
    );
}

