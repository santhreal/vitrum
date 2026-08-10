//! Staging, applying, and surviving a crash between the two renames.
//!
//! The defect class: an update that swaps binaries under a live client. That
//! shape had two failures in it. The cheap one is that the operator is
//! interrupted by a product that releases many times a day. The expensive one
//! is that `vitrum` and `vitrum-server` speak a versioned protocol, so any
//! interruption between the two renames leaves a client and a daemon that
//! refuse to talk, and nothing on the next start puts that right.
//!
//! What is asserted here is the whole round trip: `install` writes nothing
//! into the install directory, `apply_staged` moves both binaries in one pass,
//! and an apply killed between the two renames finishes correctly on the next
//! start rather than leaving a mismatched pair forever.

use super::*;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vitrum-staged-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// An install directory holding the build that is running now.
fn installed(dir: &Path) {
    fs::write(dir.join("vitrum"), b"old client").unwrap();
    fs::write(dir.join("vitrum-server"), b"old daemon").unwrap();
}

/// Put a verified pair in the staging directory the way `install` does,
/// without a network.
fn stage(dir: &Path, version: &str, client: &[u8], daemon: &[u8]) -> Staged {
    let staging = staging_dir(dir);
    fs::create_dir_all(&staging).unwrap();
    let mut files = Vec::new();
    for (name, body) in [("vitrum", client), ("vitrum-server", daemon)] {
        fs::write(staging.join(name), body).unwrap();
        files.push(StagedFile {
            name: name.to_string(),
            sha256: hex(&Sha256::digest(body)),
        });
    }
    let record = Staged {
        version: version.to_string(),
        tag: format!("v{version}"),
        channel: Channel::Stable,
        files,
    };
    write_record(dir, &record).unwrap();
    record
}

/// Staged is not installed, and the next start installs it.
///
/// WHY: `install` used to rename over the live binaries and print a note. The
/// contract now is that the running pair is untouched until a restart, so both
/// halves are asserted: nothing moved at stage time, everything moved at apply
/// time.
#[test]
fn what_is_staged_is_applied_by_the_next_start() {
    let dir = scratch("roundtrip");
    installed(&dir);
    stage(&dir, "9.9.9", b"new client", b"new daemon");

    // Staging replaced nothing.
    assert_eq!(fs::read(dir.join("vitrum")).unwrap(), b"old client");
    assert_eq!(fs::read(dir.join("vitrum-server")).unwrap(), b"old daemon");
    assert_eq!(
        staged(&dir).and_then(|s| s.version()),
        Some(Version::parse("9.9.9").unwrap()),
        "the record does not name what is waiting"
    );

    let applied = apply_staged(&dir).expect("applied");
    assert_eq!(applied, Some(Version::parse("9.9.9").unwrap()));
    assert_eq!(fs::read(dir.join("vitrum")).unwrap(), b"new client");
    assert_eq!(fs::read(dir.join("vitrum-server")).unwrap(), b"new daemon");
    assert!(
        !staging_dir(&dir).exists(),
        "the staging directory outlived the update it described"
    );

    // A second start has nothing to do and says so.
    assert_eq!(apply_staged(&dir).expect("second start"), None);
    fs::remove_dir_all(&dir).ok();
}

/// A crash between the two renames is repaired by the next start.
///
/// WHY: this is the failure the whole design exists to prevent. Between the
/// client rename and the daemon rename there is a moment where the pair is a
/// new `vitrum` and an old `vitrum-server`, which refuse to talk across a
/// protocol bump. The interruption is real here, not simulated by a flag: the
/// first rename is performed through the same call `apply_staged` makes, then
/// the process is abandoned mid-apply and a fresh apply is run against the
/// exact bytes on disk that the killed one left.
#[test]
fn a_crash_between_the_two_renames_is_finished_by_the_next_start() {
    let dir = scratch("crash");
    installed(&dir);
    let record = stage(&dir, "9.9.9", b"new client", b"new daemon");

    // The interrupted apply: the first rename lands, then nothing else does.
    let first = &record.files[0];
    swap_in(
        &staging_dir(&dir).join(&first.name),
        &dir.join(&first.name),
    )
    .expect("the first rename lands");

    // Exactly the state a kill -9 leaves behind: one binary new, one old, the
    // record still naming both.
    assert_eq!(fs::read(dir.join("vitrum")).unwrap(), b"new client");
    assert_eq!(fs::read(dir.join("vitrum-server")).unwrap(), b"old daemon");
    assert!(
        staged(&dir).is_some(),
        "the record was gone before the work was done; nothing would ever finish it"
    );

    // The next start.
    let applied = apply_staged(&dir).expect("recovered");
    assert_eq!(applied, Some(Version::parse("9.9.9").unwrap()));
    assert_eq!(
        fs::read(dir.join("vitrum")).unwrap(),
        b"new client",
        "the recovery undid a rename that had already succeeded"
    );
    assert_eq!(
        fs::read(dir.join("vitrum-server")).unwrap(),
        b"new daemon",
        "the pair is still mismatched after a restart"
    );
    assert!(!staging_dir(&dir).exists());
    fs::remove_dir_all(&dir).ok();
}

/// The same crash, interrupted at the other rename.
///
/// WHY: covering only the first file would leave the order-dependent half of
/// the class untested, and the recovery walks the record in order.
#[test]
fn a_crash_after_the_daemon_rename_is_also_finished() {
    let dir = scratch("crash2");
    installed(&dir);
    let record = stage(&dir, "9.9.9", b"new client", b"new daemon");

    let second = &record.files[1];
    swap_in(
        &staging_dir(&dir).join(&second.name),
        &dir.join(&second.name),
    )
    .expect("the daemon rename lands");

    assert_eq!(fs::read(dir.join("vitrum")).unwrap(), b"old client");
    assert_eq!(fs::read(dir.join("vitrum-server")).unwrap(), b"new daemon");

    apply_staged(&dir).expect("recovered");
    assert_eq!(fs::read(dir.join("vitrum")).unwrap(), b"new client");
    assert_eq!(fs::read(dir.join("vitrum-server")).unwrap(), b"new daemon");
    fs::remove_dir_all(&dir).ok();
}

/// An apply that dies inside the rename loop is resumed, not lost.
///
/// WHY: the two tests above stage the interruption from outside. This one is
/// interrupted by `apply_staged` itself, which is the only way to prove the
/// ordering the recovery depends on: the record has to outlive the renames. A
/// build that removed the record first would look identical until the day a
/// rename failed, and then leave a client and a daemon from different builds
/// with nothing on disk saying so.
///
/// The second rename is made to fail the way a real one does — the
/// destination cannot be replaced — by putting a non-empty directory where
/// the daemon binary goes.
///
/// That alone obstructs only Unix, where the swap is the one rename. Windows
/// cannot replace a running image, so it renames the target aside first, and
/// moving a directory to a name nothing holds succeeds: the apply completed
/// there and this test read the success as a defect in the recovery. A
/// non-empty directory at the displaced name obstructs that first rename, so
/// both platforms stop at the same point in the loop, for the reason each one
/// really stops for.
#[test]
fn an_apply_interrupted_inside_the_rename_loop_resumes() {
    let dir = scratch("midapply");
    installed(&dir);
    stage(&dir, "9.9.9", b"new client", b"new daemon");

    // `rename` onto a non-empty directory fails, so the loop stops after the
    // client and before the daemon.
    fs::remove_file(dir.join("vitrum-server")).unwrap();
    fs::create_dir_all(dir.join("vitrum-server/in-the-way")).unwrap();
    let displaced = dir.join("vitrum-server").with_extension("old");
    if cfg!(windows) {
        fs::create_dir_all(displaced.join("in-the-way")).unwrap();
    }

    let e = apply_staged(&dir).unwrap_err();
    assert!(
        format!("{e:#}").contains("vitrum-server"),
        "the failure did not name the binary it could not replace: {e:#}"
    );
    assert_eq!(
        fs::read(dir.join("vitrum")).unwrap(),
        b"new client",
        "the loop did not get as far as the interruption"
    );
    assert!(
        staged(&dir).is_some(),
        "the record was removed while work remained; nothing would ever finish it"
    );

    // The obstruction goes, the machine restarts, and the pair is whole. The
    // displaced name goes with it: sweep_displaced deletes a file, and what
    // stands here is a directory, which is a case only this test creates.
    fs::remove_dir_all(dir.join("vitrum-server")).unwrap();
    let _ = fs::remove_dir_all(&displaced);
    assert_eq!(
        apply_staged(&dir).expect("resumed"),
        Some(Version::parse("9.9.9").unwrap())
    );
    assert_eq!(fs::read(dir.join("vitrum")).unwrap(), b"new client");
    assert_eq!(fs::read(dir.join("vitrum-server")).unwrap(), b"new daemon");
    assert!(!staging_dir(&dir).exists());
    fs::remove_dir_all(&dir).ok();
}

/// A staged file that rotted between the download and the restart is refused,
/// and refused before anything is renamed.
///
/// WHY: the archive's digest was checked at download time, which may have been
/// days before the restart. Renaming an unverified file over a working binary
/// on the strength of a check that old is how a corrupted staging directory
/// becomes an install that will not launch.
#[test]
fn a_staged_file_that_no_longer_matches_its_digest_applies_nothing() {
    let dir = scratch("rot");
    installed(&dir);
    stage(&dir, "9.9.9", b"new client", b"new daemon");
    fs::write(staging_dir(&dir).join("vitrum-server"), b"corrupted").unwrap();

    let e = apply_staged(&dir).unwrap_err();
    assert!(
        e.to_string().contains("nothing was applied"),
        "{e:#}"
    );
    assert_eq!(
        fs::read(dir.join("vitrum")).unwrap(),
        b"old client",
        "a binary was replaced despite the refusal"
    );
    assert_eq!(fs::read(dir.join("vitrum-server")).unwrap(), b"old daemon");
    assert!(
        !staging_dir(&dir).exists(),
        "the bad staging directory survived to be retried forever"
    );
    fs::remove_dir_all(&dir).ok();
}

/// Staged files with no record are swept, not applied.
///
/// WHY: a stage killed partway through leaves binaries nobody verified as a
/// set. The record is written last precisely so that half-written pair means
/// nothing, and the alternative — applying whatever is lying there — is how an
/// interrupted download becomes the installed build.
#[test]
fn files_without_a_record_are_never_applied() {
    let dir = scratch("norecord");
    installed(&dir);
    fs::create_dir_all(staging_dir(&dir)).unwrap();
    fs::write(staging_dir(&dir).join("vitrum"), b"half a download").unwrap();

    assert_eq!(apply_staged(&dir).expect("swept"), None);
    assert_eq!(fs::read(dir.join("vitrum")).unwrap(), b"old client");
    assert!(!staging_dir(&dir).exists());
    fs::remove_dir_all(&dir).ok();
}

/// What is standing: nothing, available, or staged and waiting.
///
/// WHY: the sidebar has to tell "you could download this" apart from "the
/// bytes are here, restart when you like". They ask for different things and
/// only one of them is free.
#[test]
fn standing_prefers_what_is_already_on_disk() {
    let dir = scratch("standing");
    installed(&dir);

    assert_eq!(standing(&dir, None), Standing::Current);

    let offer = Available {
        version: Version::parse("2.0.0").unwrap(),
        tag: "v2.0.0".into(),
        asset_url: Some("https://example.invalid/a".into()),
        sums_url: Some("https://example.invalid/s".into()),
    };
    assert_eq!(
        standing(&dir, Some(&offer)),
        Standing::Available {
            version: Version::parse("2.0.0").unwrap()
        }
    );

    stage(&dir, "2.0.0", b"new client", b"new daemon");
    assert_eq!(
        standing(&dir, Some(&offer)),
        Standing::Staged {
            version: Version::parse("2.0.0").unwrap()
        },
        "an update already on disk was reported as merely available"
    );
    fs::remove_dir_all(&dir).ok();
}
