//! An applied update must actually be the build that keeps running.
//!
//! WHY THIS EXISTS
//!
//! `apply_on_start` applies a staged update and then re-executes, so the
//! process the operator is talking to is the build they just installed. It
//! read `current_exe()` *after* `apply_staged` had renamed the new binary over
//! the running one.
//!
//! Renaming over a running image unlinks the inode the process is executing.
//! From that moment Linux answers `/proc/self/exe` with `<path> (deleted)`,
//! and Rust hands that literal string back, so the exec failed with ENOENT.
//! Every successful update printed
//!
//!     updated to <version>, but could not restart into it: ... (os error 2)
//!
//! and carried on as the OLD build. The new binary was on disk and correct, so
//! the next start was fine and the defect looked cosmetic. It was not: for the
//! rest of that run, `vitrum --version` and every code path in the process
//! were the version the operator had just replaced.
//!
//! WHAT THIS CATCHES, AND WHAT IT DOES NOT
//!
//! The staged binary here is a shell script that prints a marker, so reaching
//! it is unambiguous: the marker can only appear if the exec landed on the
//! file that was staged. That also means this asserts the handoff, not that a
//! real vitrum build starts, which the release archives cover.
//!
//! Windows is excluded because the mechanism is: it has no `/proc/self/exe`,
//! it cannot rename over a running image at all, and `sweep_displaced` exists
//! for that reason. A test asserting the Unix failure would assert nothing
//! there.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A scratch directory that removes itself, so the suite adds no dependency
/// for the one thing it needs from one.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "vitrum-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).expect("creating a scratch directory");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Lowercase hex SHA-256, which is the form the staging record stores.
fn sha256_hex(bytes: &[u8]) -> String {
    // The record is verified by the binary under test, so this has to agree
    // with it byte for byte rather than approximately.
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// An install directory holding the binary under test with an update staged
/// beside it, ready to be applied by the next start.
fn install_with_staged_update(root: &Path, marker: &str) -> PathBuf {
    let installed = root.join("vitrum");
    fs::copy(env!("CARGO_BIN_EXE_vitrum"), &installed).expect("copying the binary under test");

    let staging = root.join(".vitrum-staged");
    fs::create_dir_all(&staging).expect("creating the staging directory");

    let replacement = format!("#!/bin/sh\necho {marker}\n");
    let staged_binary = staging.join("vitrum");
    fs::write(&staged_binary, &replacement).expect("writing the staged binary");
    fs::set_permissions(&staged_binary, fs::Permissions::from_mode(0o755))
        .expect("making the staged binary executable");

    // Shape and field names come from `update::Staged`. A record that does not
    // parse is treated as nothing staged, which would make this test pass for
    // the wrong reason, so the assertions below check that an apply happened.
    let record = format!(
        r#"{{"version":"9.9.9","tag":"v9.9.9","channel":"stable",
             "files":[{{"name":"vitrum","sha256":"{}"}}]}}"#,
        sha256_hex(replacement.as_bytes())
    );
    fs::write(staging.join("staged.json"), record).expect("writing the staging record");

    installed
}

#[test]
fn an_applied_update_is_the_build_that_keeps_running() {
    let root = Scratch::new("reexec");
    const MARKER: &str = "REEXEC-REACHED-NEW-IMAGE";
    let installed = install_with_staged_update(root.path(), MARKER);

    let out = Command::new(&installed)
        .arg("--version")
        .output()
        .expect("running the installed binary");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // The marker is the whole point: it can only be printed by the file that
    // was staged, so seeing it proves the exec landed on the new image rather
    // than on a path that no longer names anything.
    assert!(
        stdout.contains(MARKER),
        "the process did not continue as the staged build.\nstdout: {stdout}\nstderr: {stderr}"
    );

    // The precise regression. With the exe path read after the swap this said
    // "could not restart into it: No such file or directory (os error 2)", and
    // because the failure is not fatal the run continued as the old build with
    // nothing but this line to say so.
    assert!(
        !stderr.contains("could not restart into it"),
        "an update was applied and the restart into it failed.\nstderr: {stderr}"
    );

    // An update that was never applied would also print no error, and would
    // leave the staging directory sitting there.
    assert!(
        !root.path().join(".vitrum-staged").exists(),
        "the staged update was not applied, so the assertions above prove nothing"
    );
}
