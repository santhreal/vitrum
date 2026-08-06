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
#[test]
fn the_completion_message_names_both_halves() {
    assert!(
        AFTER_INSTALL.contains("restart vitrum"),
        "the client half is missing: {AFTER_INSTALL}"
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
