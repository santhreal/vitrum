//! What happens to a launch store this build cannot read.
//!
//! WHY THIS EXISTS
//!
//! `launch.json` holds every preset the operator saved, the whole launch
//! history and the last directory used. Reading it went through one function
//! that answered `LaunchStore::default()` for every failure and said nothing:
//! a truncated file, a file from a newer build, a file with one stray byte all
//! became an empty launcher with no message anywhere.
//!
//! Empty was not the cost. The next `save_presets` is a read-modify-write, so
//! the first preset saved after that wrote the defaults over the file, and the
//! presets, the history and the recents were gone for good. One stray byte and
//! a click.
//!
//! WHAT IS PROVED HERE
//!
//! Three things, at the choke point every unreadable file passes through:
//! the file is moved aside rather than left where the next save will land on
//! it, the sentence returned names both paths and what happens next, and a
//! store that is perfectly fine is neither moved nor talked about.
//!
//! WHAT IS NOT
//!
//! Not the flash. `main.rs` puts the sentence on the strip and there is no
//! window here to look at. Not the seeding that follows, which
//! `store_tests.rs` already owns.

use super::*;

/// A scratch directory holding one launch store, removed when it drops.
struct Profile(PathBuf);

impl Profile {
    fn new(name: &str, contents: Option<&str>) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "vitrum-store-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("creating the scratch profile");
        if let Some(text) = contents {
            std::fs::write(dir.join(LAUNCH_STORE_FILE), text).expect("writing the store");
        }
        Self(dir)
    }

    fn store(&self) -> PathBuf {
        self.0.join(LAUNCH_STORE_FILE)
    }

    fn aside(&self) -> PathBuf {
        self.0
            .join(format!("{LAUNCH_STORE_FILE}{QUARANTINE_SUFFIX}"))
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// Every shape this build cannot read is moved aside, and the operator is told
/// where it went.
///
/// Parameterised over the shapes rather than written once for JSON: a
/// truncated write, a file that is valid JSON of the wrong type and a file
/// from a newer build reach the same defaulting, and only one of them was ever
/// exercised.
#[test]
fn an_unreadable_store_is_moved_aside_and_named() {
    // A JSON array is deliberately absent from this list. Serde's derived
    // deserializer reads a struct from a sequence as well as from a map, so
    // `[]` is a readable store with every field at its default rather than a
    // file this build cannot read, and quarantining it would move a file that
    // parsed.
    let cases = [
        ("truncated", "{\"presets\":[{\"id\":1,"),
        ("empty", ""),
        ("null", "null"),
        ("not-json", "presets = []"),
        (
            "from-the-future",
            r#"{"version":99,"presets":[{"id":1,"label":"x","command":"sh"}]}"#,
        ),
    ];
    for (name, contents) in cases {
        let profile = Profile::new(name, Some(contents));
        let said = salvage_launch_store_at(&profile.store())
            .unwrap_or_else(|| panic!("{name}: an unreadable store was accepted in silence"));

        assert!(
            !profile.store().exists(),
            "{name}: the unreadable store is still where the next save will overwrite it"
        );
        assert_eq!(
            std::fs::read_to_string(profile.aside()).expect("the store was not moved aside"),
            contents,
            "{name}: the quarantined copy is not the bytes that were there"
        );
        assert!(
            said.contains(&profile.store().display().to_string()),
            "{name}: the message does not say which file: {said}"
        );
        assert!(
            said.contains(&profile.aside().display().to_string()),
            "{name}: the message does not say where the file went: {said}"
        );
    }
}

/// A store that reads is left exactly alone, and produces no message.
///
/// The other half of the contract. A salvage that fired on a good file would
/// move a working profile aside on every start and put a permanent error on
/// the strip.
#[test]
fn a_readable_store_is_neither_moved_nor_reported() {
    let store = LaunchStore {
        presets: vec![SavedPreset {
            id: 7,
            label: "Claude".to_string(),
            command: "claude".to_string(),
            ..SavedPreset::default()
        }],
        ..LaunchStore::default()
    };
    let encoded = encode_launch_store(&store);
    let profile = Profile::new("good", Some(&encoded));

    assert_eq!(salvage_launch_store_at(&profile.store()), None);
    assert!(!profile.aside().exists(), "a good store was quarantined");
    assert_eq!(
        std::fs::read_to_string(profile.store()).expect("the good store was moved"),
        encoded
    );
}

/// A profile that has never saved anything is not a problem to report.
#[test]
fn a_missing_store_is_silent() {
    let profile = Profile::new("absent", None);
    assert_eq!(salvage_launch_store_at(&profile.store()), None);
    assert!(!profile.aside().exists());
}

/// Reading, not parsing, is what failed: the file stays, and the sentence says
/// so rather than claiming a move that did not happen.
///
/// A rename would need the same directory permission the read just lacked, and
/// a file that is merely unreadable this once still holds the operator's
/// presets.
#[cfg(unix)]
#[test]
fn a_store_that_cannot_be_read_is_left_where_it_is() {
    use std::os::unix::fs::PermissionsExt;

    let profile = Profile::new("unreadable", Some("{}"));
    std::fs::set_permissions(profile.store(), std::fs::Permissions::from_mode(0o000))
        .expect("removing every mode bit");

    // Running as root defeats the mode bits, and the assertion below would
    // then be testing nothing. Skip rather than pass for the wrong reason.
    if std::fs::read_to_string(profile.store()).is_ok() {
        return;
    }

    let said = salvage_launch_store_at(&profile.store())
        .expect("a store that could not be read was accepted in silence");
    assert!(
        profile.store().exists(),
        "an unreadable store was moved aside, which needs the permission the read did not have"
    );
    assert!(!profile.aside().exists());
    assert!(
        said.contains("left alone"),
        "the message claims something other than leaving the file where it is: {said}"
    );

    std::fs::set_permissions(profile.store(), std::fs::Permissions::from_mode(0o600)).ok();
}
