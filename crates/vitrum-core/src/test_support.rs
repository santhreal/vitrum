//! Facts about the host that a test has to establish before it can assert
//! anything, shared by every crate whose tests need them.
//!
//! This is not test scaffolding in the usual sense of fixtures and builders. It
//! answers questions about the machine the suite is running on, and those
//! answers have to be the same sentence in every crate or two suites disagree
//! about what the same kernel is willing to do. It compiles only under `cfg(test)`
//! or the `test-support` feature, so nothing here reaches a shipped binary.

/// Whether this kernel will say what another process is doing.
///
/// The probe reads `/proc/<pid>/syscall`, which the kernel gates behind ptrace
/// attach permission. `kernel.yama.ptrace_scope = 2` refuses it to everything
/// without `CAP_SYS_PTRACE`, including a parent asking about its own child, so
/// on such a host the probe answers "unknown" for every session. That is the
/// documented contract, not a bug: a platform that cannot tell must say so.
///
/// The tests that assert a definite answer are therefore asserting something
/// the kernel has refused to say, and they check this first. The check is
/// empirical rather than a read of the sysctl, because containers, `hidepid`
/// and LSM policy deny the same file for their own reasons.
#[cfg(target_os = "linux")]
pub fn kernel_reports_other_processes() -> bool {
    static ANSWERS: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        // A child of our own, which is the friendliest case there is: anything
        // stricter than this denies the probe every session it will ever see.
        let mut child = match std::process::Command::new("sleep").arg("30").spawn() {
            Ok(child) => child,
            // No `sleep` means this says nothing either way, and claiming the
            // kernel is silent would skip the suite for the wrong reason.
            Err(_) => return true,
        };
        let readable = std::fs::read_to_string(format!("/proc/{}/syscall", child.id())).is_ok();
        let _ = child.kill();
        let _ = child.wait();
        if !readable {
            eprintln!(
                "skipping the probe tests: this kernel denies /proc/<pid>/syscall \
                 (kernel.yama.ptrace_scope = 2 does this), so the probe answers \
                 unknown for every session and there is no definite answer to assert"
            );
        }
        readable
    });
    *ANSWERS
}
