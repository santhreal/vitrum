//! Process termination: exit codes, signals, and Windows NTSTATUS decoding.

use crate::exit::{Termination, describe, ntstatus_name, signal_name};

/// An ordinary exit renders as `exited <code>`.
#[test]
fn an_ordinary_exit_renders_its_code() {
    assert_eq!(describe(Termination::Exited(0)), "exited 0");
    assert_eq!(describe(Termination::Exited(1)), "exited 1");
    assert_eq!(describe(Termination::Exited(101)), "exited 101");
    assert_eq!(describe(Termination::Exited(255)), "exited 255");
}

/// A shell's `128 + signal` exit code is never decoded back into a signal.
///
/// A POSIX shell reports a killed child as 137, but a program is equally free
/// to call `exit(137)` itself, and the two are different facts. The daemon
/// waits on the child and knows which happened, so guessing here would
/// overwrite a known truth with a confident inference. `killed (SIGKILL)` for a
/// program that chose to exit 137 is a lie a user cannot debug around.
#[test]
fn a_shell_style_exit_code_is_not_decoded_as_a_signal() {
    assert_eq!(describe(Termination::Exited(137)), "exited 137");
    assert_eq!(describe(Termination::Exited(130)), "exited 130");
    assert_eq!(describe(Termination::Exited(143)), "exited 143");
}

/// Only exit code zero counts as success.
///
/// Read by anything that colours a row red. A signalled process is never a
/// success even though it has no exit code at all.
#[test]
fn only_a_zero_exit_is_success() {
    assert!(Termination::Exited(0).is_success());
    assert!(!Termination::Exited(1).is_success());
    assert!(
        !Termination::Signaled {
            signal: 9,
            core_dumped: false
        }
        .is_success()
    );
    assert!(!Termination::Stopped { signal: 5 }.is_success());
}

/// A signalled process renders its signal name, not a number.
///
/// `killed (SIGKILL)` says the OOM killer or a `kill -9` got it;
/// `killed (9)` makes the reader look up a table.
#[test]
fn a_signalled_process_renders_its_signal_name() {
    let killed = Termination::Signaled {
        signal: 9,
        core_dumped: false,
    };
    assert_eq!(describe(killed), "killed (SIGKILL)");
    assert_eq!(
        describe(Termination::Signaled {
            signal: 15,
            core_dumped: false
        }),
        "killed (SIGTERM)"
    );
    assert_eq!(
        describe(Termination::Signaled {
            signal: 2,
            core_dumped: false
        }),
        "killed (SIGINT)"
    );
}

/// A core dump is called out, because it means a file was written.
///
/// The user has a core file on disk they did not ask for and probably want to
/// either use or delete. Hiding it makes the disk usage a mystery.
#[test]
fn a_core_dump_is_reported() {
    assert_eq!(
        describe(Termination::Signaled {
            signal: 11,
            core_dumped: true
        }),
        "killed (SIGSEGV, core dumped)"
    );
    assert_eq!(
        describe(Termination::Signaled {
            signal: 6,
            core_dumped: true
        }),
        "killed (SIGABRT, core dumped)"
    );
}

/// An unrecognised signal renders its number rather than a wrong name.
///
/// Real-time signals start above the named table and their numbers vary. Naming
/// one wrongly is worse than not naming it: a user would chase the wrong cause.
#[test]
fn an_unknown_signal_renders_its_number() {
    assert_eq!(
        describe(Termination::Signaled {
            signal: 77,
            core_dumped: false
        }),
        "killed (signal 77)"
    );
    assert_eq!(
        describe(Termination::Signaled {
            signal: 77,
            core_dumped: true
        }),
        "killed (signal 77, core dumped)"
    );
    assert_eq!(signal_name(77), None);
    assert_eq!(signal_name(0), None);
    assert_eq!(signal_name(-1), None);
}

/// A stopped process is distinguished from a terminated one.
///
/// Job control suspends a process rather than ending it. Reporting `exited 0`
/// for a suspended agent would make a user close a session that is still there
/// and could be resumed.
#[test]
fn a_stopped_process_is_not_a_terminated_one() {
    assert_eq!(describe(Termination::Stopped { signal: 5 }), "stopped (SIGTRAP)");
    assert_eq!(describe(Termination::Stopped { signal: 77 }), "stopped (signal 77)");
}

/// The signals POSIX numbers identically everywhere are named on every target.
///
/// These twelve agree across Linux, macOS, and the BSDs, so they can be
/// answered without consulting the target family. The ones that disagree
/// (7, 10, 12, and 16 upwards) are deliberately excluded from the shared table.
#[test]
fn the_portable_signal_numbers_are_named_everywhere() {
    let portable = [
        (1, "SIGHUP"),
        (2, "SIGINT"),
        (3, "SIGQUIT"),
        (4, "SIGILL"),
        (5, "SIGTRAP"),
        (6, "SIGABRT"),
        (8, "SIGFPE"),
        (9, "SIGKILL"),
        (11, "SIGSEGV"),
        (13, "SIGPIPE"),
        (14, "SIGALRM"),
        (15, "SIGTERM"),
    ];
    for (number, name) in portable {
        assert_eq!(signal_name(number), Some(name), "signal {number}");
    }
}

/// Linux signal numbering is used on Linux.
///
/// `SIGUSR1` is 10 on Linux and 30 on macOS, and `SIGCHLD` is 17 on Linux and
/// 20 on macOS. A single hardcoded table would name half the signals wrong on
/// whichever platform it was not written for.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn linux_signal_numbering_is_used_on_linux() {
    assert_eq!(signal_name(7), Some("SIGBUS"));
    assert_eq!(signal_name(10), Some("SIGUSR1"));
    assert_eq!(signal_name(12), Some("SIGUSR2"));
    assert_eq!(signal_name(17), Some("SIGCHLD"));
    assert_eq!(signal_name(19), Some("SIGSTOP"));
    assert_eq!(signal_name(28), Some("SIGWINCH"));
    assert_eq!(signal_name(31), Some("SIGSYS"));
    assert_eq!(signal_name(32), None, "real-time signals are not named");
}

/// BSD signal numbering is used on macOS and the BSDs.
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
#[test]
fn bsd_signal_numbering_is_used_on_bsd() {
    assert_eq!(signal_name(7), Some("SIGEMT"));
    assert_eq!(signal_name(10), Some("SIGBUS"));
    assert_eq!(signal_name(12), Some("SIGSYS"));
    assert_eq!(signal_name(17), Some("SIGSTOP"));
    assert_eq!(signal_name(20), Some("SIGCHLD"));
    assert_eq!(signal_name(28), Some("SIGWINCH"));
    assert_eq!(signal_name(30), Some("SIGUSR1"));
    assert_eq!(signal_name(32), None);
}

/// A Windows NTSTATUS exit code renders in hex with its meaning.
///
/// Windows reports a crash as an exit code with the high bit set. Rendering
/// `exited -1073741819` is what the naive signed print produces and it tells a
/// user nothing; the hex form is what every Windows tool shows and what a
/// search engine finds. Decoding is driven by the value, not by `cfg`, so it
/// works when a Windows daemon reports to a client on another platform.
#[test]
fn a_windows_ntstatus_code_renders_in_hex_with_its_meaning() {
    assert_eq!(
        describe(Termination::Exited(0xC000_0005u32 as i32)),
        "exited 0xc0000005 (access violation)"
    );
    assert_eq!(
        describe(Termination::Exited(0xC000_013Au32 as i32)),
        "exited 0xc000013a (interrupted)"
    );
    assert_eq!(
        describe(Termination::Exited(0xC000_00FDu32 as i32)),
        "exited 0xc00000fd (stack overflow)"
    );
    assert_eq!(
        describe(Termination::Exited(0x8000_0003u32 as i32)),
        "exited 0x80000003 (breakpoint)"
    );
}

/// An unrecognised high-bit code still renders in hex, without inventing a
/// meaning.
#[test]
fn an_unknown_ntstatus_code_renders_in_hex_alone() {
    assert_eq!(
        describe(Termination::Exited(0xC000_1234u32 as i32)),
        "exited 0xc0001234"
    );
    assert_eq!(describe(Termination::Exited(-1)), "exited 0xffffffff");
    assert_eq!(ntstatus_name(0xC000_1234), None);
    assert_eq!(ntstatus_name(0), None);
}

/// The hex threshold is the high bit and nothing else.
///
/// Ordinary large exit codes stay decimal. A process that exits with
/// `0x7FFFFFFF` chose that number and should see it back as it wrote it.
#[test]
fn only_high_bit_codes_switch_to_hex() {
    assert_eq!(describe(Termination::Exited(0x7FFF_FFFF)), "exited 2147483647");
    assert_eq!(describe(Termination::Exited(i32::MIN)), "exited 0x80000000");
}

/// `Display` and `describe` agree.
///
/// Two ways to render the same value that could drift apart is how one code
/// path ends up formatting differently from another.
#[test]
fn display_matches_describe() {
    for termination in [
        Termination::Exited(0),
        Termination::Exited(101),
        Termination::Signaled {
            signal: 9,
            core_dumped: false,
        },
        Termination::Stopped { signal: 5 },
    ] {
        assert_eq!(termination.to_string(), describe(termination));
    }
}

/// A real child that exits with a code is read back as that code.
///
/// Exercises `ExitStatusExt` against the operating system rather than against a
/// hand-built value, so a wrong branch order (checking `code()` before
/// `signal()`) would show up here.
#[cfg(unix)]
#[test]
fn a_real_child_exit_code_is_read_back() {
    let status = std::process::Command::new("/bin/sh")
        .args(["-c", "exit 3"])
        .status()
        .expect("/bin/sh must be present on a unix host");
    assert_eq!(Termination::from_status(&status), Termination::Exited(3));
    assert_eq!(describe(Termination::from_status(&status)), "exited 3");
}

/// A real child killed by a signal is reported as killed, not as exited.
///
/// `ExitStatus::code()` returns `None` for a signalled process, so anything
/// that reaches for the code first and falls back to a default reports
/// `exited 0` for a process the kernel destroyed. That is the exact bug this
/// locks out, and it would make an OOM-killed agent look like a clean finish.
#[cfg(unix)]
#[test]
fn a_real_child_killed_by_a_signal_is_reported_as_killed() {
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "sleep 30"])
        .spawn()
        .expect("/bin/sh must be present on a unix host");
    child.kill().expect("the child must be killable");
    let status = child.wait().expect("the child must be reapable");

    assert_eq!(
        Termination::from_status(&status),
        Termination::Signaled {
            signal: 9,
            core_dumped: false
        }
    );
    assert_eq!(describe(Termination::from_status(&status)), "killed (SIGKILL)");
    assert!(!Termination::from_status(&status).is_success());
}
