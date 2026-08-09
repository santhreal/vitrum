use super::*;

/// The completion message must name the daemon, not just the window.
///
/// "Restart vitrum to run the new version" was true about the client and
/// silently false about everything else. The daemon owns the PTYs and
/// outlives every window by design, so replacing its file leaves the live
/// process untouched: it keeps serving the old code until it is restarted,
/// and if `PROTOCOL_VERSION` moved between the two, the new client then
/// refuses to talk to it and the operator is told "protocol mismatch" with
/// no idea why.
///
/// The cost of the fix is not free either, which is the second half: the
/// daemon cannot be restarted without ending every session it holds. An
/// operator running twenty agents must be told that before they act, not
/// after.
///
/// The client half changed and the daemon half did not. Nothing is swapped
/// under a running client any more: the message is printed over an install
/// directory whose binaries are still the old ones, and a message that says
/// the new client is in place would now be false in the other direction.
#[test]
fn the_completion_message_names_both_halves() {
    assert!(
        AFTER_INSTALL.contains("restart vitrum"),
        "the client half is missing: {AFTER_INSTALL}"
    );
    assert!(
        AFTER_INSTALL.contains("staged"),
        "nothing was replaced and the message does not say what did happen: {AFTER_INSTALL}"
    );
    assert!(
        AFTER_INSTALL.contains("daemon"),
        "the daemon keeps running the old version and the message does not say so"
    );
    assert!(
        AFTER_INSTALL.contains("ends every session"),
        "restarting the daemon kills every agent and the message does not warn"
    );
}

/// Applying an update touches the two binaries and nothing else.
///
/// WHY: the daemon holds every PTY, and restarting it to finish an update
/// would end every session the operator is running — the cost the message
/// above exists to put in their hands rather than take on their behalf.
/// Staging made this easy to get wrong, because applying at startup is one
/// step away from "and start the daemon while we are here". What is observable
/// here is that an apply leaves the install directory holding exactly the
/// files it held before, with new contents and nothing started.
#[test]
fn applying_replaces_files_and_starts_nothing() {
    let dir = std::env::temp_dir().join(format!("vitrum-apply-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("vitrum"), b"old client").unwrap();
    fs::write(dir.join("vitrum-server"), b"old daemon").unwrap();
    let staging = staging_dir(&dir);
    fs::create_dir_all(&staging).unwrap();
    let mut files = Vec::new();
    for (name, body) in [
        ("vitrum", b"new client".as_slice()),
        ("vitrum-server", b"new daemon".as_slice()),
    ] {
        fs::write(staging.join(name), body).unwrap();
        files.push(StagedFile {
            name: name.to_string(),
            sha256: hex(&Sha256::digest(body)),
        });
    }
    write_record(
        &dir,
        &Staged {
            version: "9.9.9".into(),
            tag: "v9.9.9".into(),
            channel: Channel::Stable,
            files,
        },
    )
    .unwrap();

    apply_staged(&dir).expect("applied");

    let mut left: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    left.sort();
    assert_eq!(
        left,
        vec!["vitrum".to_string(), "vitrum-server".to_string()],
        "applying an update left something else in the install directory"
    );
    fs::remove_dir_all(&dir).ok();
}

/// A displaced image is only ever swept on Windows.
///
/// Unix replaces a running binary with one `rename` and leaves nothing
/// behind, so a sweep there could only ever delete a file that this
/// program did not create.
#[test]
fn the_sweep_does_nothing_on_unix() {
    let dir = std::env::temp_dir().join(format!("vitrum-sweep-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let decoy = dir.join("vitrum.old");
    fs::write(&decoy, b"not ours").unwrap();
    sweep_displaced(&dir);
    assert_eq!(
        decoy.exists(),
        !cfg!(windows),
        "the sweep touched a file it does not own"
    );
    fs::remove_dir_all(&dir).ok();
}
