//! Asking the operating system whether a session's foreground process is
//! blocked waiting for the operator.
//!
//! This is the signal that lets the sidebar say "this agent stopped and the
//! next move is yours" for ANY hosted process, including agents nobody has
//! integrated. A shell that reads per-harness event streams cannot do that; we
//! hold the PTY master, so we can ask the kernel instead of asking the agent.
//!
//! # Which process
//!
//! The FOREGROUND process group, from `tcgetpgrp()` on the master, never the
//! direct child. A session running `bash` spends its life spawning other
//! programs, and the thing in front of the terminal is the one whose state the
//! operator cares about. `tcgetpgrp` on a Unix98 master answers for the slave's
//! session, which is exactly the question.
//!
//! The group LEADER is probed, not every member. For an interactive shell that
//! is right by construction: job control puts each foreground job in its own
//! process group whose leader is the job's first process. It is wrong only for
//! a non-interactive `sh -c 'a | b'`, where the shell stays the leader and sits
//! in `wait4` while `a` blocks on the terminal; that reads as working, which is
//! the conservative direction.
//!
//! # Platform capability, which differs three ways
//!
//! | Platform | `Some(true)` | `Some(false)` | `None` |
//! |----------|--------------|---------------|--------|
//! | Linux, x86-64 / aarch64 | proven: `read` on this tty, or a poll-family call that can never time out | proven: on CPU, sleeping, reaping a child, or in any syscall that is not a way to block on a terminal | the kernel will not say: a poll with a finite timeout, a restarting syscall, or `/proc` unreadable |
//! | macOS    | never | proven: a thread is on CPU | everything else |
//! | Windows, other Unix, other Linux arch | never | never | always |
//!
//! **macOS genuinely cannot answer the blocking question and must not pretend
//! to.** `proc_pidinfo` exposes a thread's run state — `TH_STATE_RUNNING`,
//! `TH_STATE_WAITING`, and friends — and a tty `read` and a `nanosleep` are
//! both `TH_STATE_WAITING`. The wait channel and the syscall number are not
//! available through any public interface. So macOS proves "working" when a
//! thread is on CPU and reports unknown otherwise. Do not "fix" this by
//! inferring `Some(true)` from a sleeping thread: `None` degrades to the bell
//! and idle inference the sidebar already has, while a confident wrong answer
//! poisons the one state the operator most relies on.
//!
//! # Why a poll with a timeout is unknown rather than working
//!
//! Measured on this machine, on real processes on a real PTY:
//!
//! ```text
//! bash at a prompt   pselect6, timeout pointer NULL          blocked
//! bash `read -p`     read(fd 0) -> /dev/pts/N                blocked
//! cat, python input  read(fd 0) -> /dev/pts/N                blocked
//! less FILE          read(fd 4) -> /dev/pts/N                blocked
//! node, stdin only   epoll_pwait, timeout -1                 blocked
//! sleep 300          clock_nanosleep                         working
//! a shell spin loop  running                                 working
//! `while :; ...`     wait4                                   working
//! node, spinning     running                                 working
//! top -d 0.1         pselect6, timeout pointer SET           ambiguous
//! vi -u NONE, idle   pselect6, timeout pointer SET           ambiguous
//! claude, idle       epoll_pwait2, timeout pointer SET       ambiguous
//! ```
//!
//! The last three are the same syscall with the same shape, and one of them is
//! working while two are waiting for a human. An event-loop program wakes on a
//! timer whether or not it has anything to do, so the timeout proves nothing.
//! Calling that "working" would park a permanent Working badge on every idle
//! TUI agent, which is precisely the state the sidebar exists to distinguish.
//! Reporting unknown hands the row back to the bell and idle inference, which
//! gets both cases approximately right and claims nothing it cannot show.
//!
//! A timeout of exactly zero is different and IS working: a non-blocking poll
//! is not blocked on anything by definition.

/// What the operating system can say about the foreground process of `master`'s
/// terminal right now.
///
/// `Some(true)` means proven blocked on the terminal, `Some(false)` proven not,
/// and `None` means this platform, this architecture, or this moment cannot
/// answer. `None` is NOT `Some(false)`.
pub(crate) fn waiting(master: &dyn portable_pty::MasterPty) -> Option<bool> {
    platform::waiting(master)
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod platform {
    use std::path::Path;

    /// Syscall numbers for this architecture.
    ///
    /// Numbers, not `wchan` strings: the kernel ABI freezes a syscall's number
    /// forever, while `wchan` names are internal symbols that get renamed.
    /// They are per-architecture, though, which is why an architecture without
    /// a table here reports unknown rather than reading someone else's numbers.
    #[cfg(target_arch = "x86_64")]
    mod nr {
        pub(super) const READ: u64 = 0;
        pub(super) const RESTART_SYSCALL: u64 = 219;
        /// `(number, argument index of the timeout, kind)`.
        pub(super) const POLLING: &[(u64, usize, super::Timeout)] = &[
            (7, 2, super::Timeout::Millis),    // poll
            (23, 4, super::Timeout::Pointer),  // select
            (232, 3, super::Timeout::Millis),  // epoll_wait
            (270, 4, super::Timeout::Pointer), // pselect6
            (271, 2, super::Timeout::Pointer), // ppoll
            (281, 3, super::Timeout::Millis),  // epoll_pwait
            (441, 3, super::Timeout::Pointer), // epoll_pwait2
        ];
    }

    #[cfg(target_arch = "aarch64")]
    mod nr {
        pub(super) const READ: u64 = 63;
        pub(super) const RESTART_SYSCALL: u64 = 128;
        /// The generic ABI has no `poll`, `select` or `epoll_wait`; their
        /// timed-out variants are the only forms.
        pub(super) const POLLING: &[(u64, usize, super::Timeout)] = &[
            (22, 3, super::Timeout::Millis),   // epoll_pwait
            (72, 4, super::Timeout::Pointer),  // pselect6
            (73, 2, super::Timeout::Pointer),  // ppoll
            (441, 3, super::Timeout::Pointer), // epoll_pwait2
        ];
    }

    /// How a poll-family syscall spells its timeout.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub(super) enum Timeout {
        /// A `struct timespec *` or `struct timeval *`. NULL means never.
        Pointer,
        /// A signed 32-bit millisecond count. Negative means never.
        Millis,
    }

    /// Number of argument registers `/proc/<pid>/syscall` reports.
    const ARGS: usize = 6;

    pub(super) fn waiting(master: &dyn portable_pty::MasterPty) -> Option<bool> {
        // Zero or -1 means no foreground process group, i.e. nothing to ask
        // about; portable-pty already folds those into None.
        let pid = master.process_group_leader()?;
        // Without the slave's path a `read` cannot be attributed to this
        // terminal rather than to a file, and guessing fd 0 would call a child
        // reading its input file "blocked on the operator".
        let tty = master.tty_name()?;
        // Unreadable under a hardened `ptrace_scope`, or when the process died
        // between the two calls. Both are honest unknowns.
        let line = std::fs::read_to_string(format!("/proc/{pid}/syscall")).ok()?;
        classify(line.trim_end(), &|fd| fd_is(pid, fd, &tty))
    }

    /// Decide from one `/proc/<pid>/syscall` line.
    ///
    /// The line is either `running`, or `-1 <sp> <pc>` when the task is not in
    /// a syscall, or `<nr> <arg0..arg5> <sp> <pc>` with the number in decimal
    /// and everything else in hex.
    pub(super) fn classify(line: &str, fd_is_tty: &dyn Fn(u64) -> bool) -> Option<bool> {
        let mut fields = line.split_ascii_whitespace();
        let head = fields.next()?;
        if head == "running" {
            // On a CPU. Whatever it is doing, it is not parked in a read.
            return Some(false);
        }
        if head.starts_with('-') {
            // Not inside a syscall: executing user code, or caught on the way
            // in or out of one. Either way it is not blocked on the terminal.
            return Some(false);
        }
        let number: u64 = head.parse().ok()?;

        let mut args = [0u64; ARGS];
        for slot in &mut args {
            let raw = fields.next()?;
            *slot = u64::from_str_radix(raw.strip_prefix("0x").unwrap_or(raw), 16).ok()?;
        }

        if number == nr::READ {
            // The strongest evidence there is: this process is parked in a read
            // on THIS terminal and only the operator can end it. A read on any
            // other descriptor is ordinary I/O, which is work.
            return Some(fd_is_tty(args[0]));
        }
        if number == nr::RESTART_SYSCALL {
            // The kernel is resuming a call interrupted by a signal and does
            // not say which one. A restarted `read` and a restarted `nanosleep`
            // look identical here, so there is nothing honest to report.
            return None;
        }
        let Some(&(_, index, kind)) = nr::POLLING.iter().find(|(n, _, _)| *n == number) else {
            // Some other syscall. None of the remaining ways to block are ways
            // to block on a terminal, so this is a positive "not waiting".
            return Some(false);
        };
        match kind {
            Timeout::Pointer => {
                if args[index] == 0 {
                    // No timeout can ever fire, so this call ends only when a
                    // descriptor becomes ready. In a terminal that is the
                    // operator: this is a shell at its prompt.
                    Some(true)
                } else {
                    // A timed wait. Cannot be separated from an event loop that
                    // wakes periodically while idle; see the module docs.
                    None
                }
            }
            // The register holds a C `int`, so the low 32 bits are the value
            // and -1 arrives as 0xffffffff.
            Timeout::Millis => match args[index] as u32 as i32 {
                milliseconds if milliseconds < 0 => Some(true),
                0 => Some(false),
                _ => None,
            },
        }
    }

    /// Whether descriptor `fd` of `pid` is this session's terminal.
    fn fd_is(pid: i32, fd: u64, tty: &Path) -> bool {
        let Ok(fd) = i32::try_from(fd) else {
            return false;
        };
        std::fs::read_link(format!("/proc/{pid}/fd/{fd}")).is_ok_and(|target| target == tty)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    /// Proven working, or unknown. Never `Some(true)`; see the module docs for
    /// why macOS cannot prove the blocking case.
    pub(super) fn waiting(master: &dyn portable_pty::MasterPty) -> Option<bool> {
        let pid = master.process_group_leader()?;
        let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let want = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        // Anything short of a full struct means the call failed or the kernel
        // filled in a shape this build does not agree with, and reading a
        // partially written struct would invent an answer.
        let filled = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTASKINFO,
                0,
                std::ptr::from_mut(&mut info).cast(),
                want,
            )
        };
        if filled != want {
            return None;
        }
        (info.pti_numrunning > 0).then_some(false)
    }
}

#[cfg(not(any(
    target_os = "macos",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
)))]
mod platform {
    /// Always unknown.
    ///
    /// ConPTY has no console equivalent of `tcgetpgrp` and no per-process
    /// syscall view, so Windows cannot answer at all. The same goes for a Unix
    /// or a Linux architecture with no syscall table here: reporting `None` is
    /// what keeps a port honest instead of quietly wrong.
    pub(super) fn waiting(_master: &dyn portable_pty::MasterPty) -> Option<bool> {
        None
    }
}

#[cfg(all(
    test,
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
/// The classifier, for regression tests over lines captured from real
/// processes. The live path is `waiting`.
pub(crate) fn classify_line(line: &str, fd_is_tty: &dyn Fn(u64) -> bool) -> Option<bool> {
    platform::classify(line, fd_is_tty)
}
