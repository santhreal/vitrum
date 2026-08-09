//! The setting hides the affordance and changes nothing else.
//!
//! The defect class: a cosmetic switch that quietly turns off a mechanism. An
//! operator who hides a restart prompt has said the prompt is noise, not that
//! they want to be left on an old build with a security fix in it. A setting
//! whose name is about drawing and whose effect is about updating is the kind
//! of thing nobody discovers until they are several releases behind and cannot
//! explain why.
//!
//! The split is asserted from both directions: with the affordance hidden,
//! checking still answers, staging still stages, and applying still applies;
//! and with it shown, only the drawing changes.
//!
//! Also here: a `ui.json` written by a build that never had either setting.
//! Adding a required field to that document is how a release wipes every
//! workspace an operator has, and that has happened in this file's history.

use super::*;
use crate::state::{Settings, UiStateLoad, parse_ui_state};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vitrum-toggle-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn stage(dir: &Path, version: &str) {
    let staging = staging_dir(dir);
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
        dir,
        &Staged {
            version: version.to_string(),
            tag: format!("v{version}"),
            channel: Channel::Stable,
            files,
        },
    )
    .unwrap();
}

/// Visible by default.
///
/// WHY: an operator who has never opened Settings must be told that a restart
/// will take a build that is already downloaded. Defaulting to hidden would
/// make the staged update invisible and the whole feature pointless.
#[test]
fn the_affordance_is_visible_on_a_fresh_profile() {
    assert!(Settings::default().show_restart_to_update);
    assert_eq!(Settings::default().update_channel, Channel::Stable);
}

/// Hidden hides the affordance.
#[test]
fn hidden_draws_nothing() {
    let waiting = Standing::Staged {
        version: Version::parse("9.9.9").unwrap(),
    };
    assert_eq!(
        restart_offer(&waiting, true),
        Some(&Version::parse("9.9.9").unwrap())
    );
    assert_eq!(restart_offer(&waiting, false), None);

    // And it is the only standing that ever draws one: an update that has not
    // been downloaded is the titlebar chip's business.
    let merely_available = Standing::Available {
        version: Version::parse("9.9.9").unwrap(),
    };
    assert_eq!(restart_offer(&merely_available, true), None);
    assert_eq!(restart_offer(&Standing::Current, true), None);
}

/// Hidden does not stop checking, staging or applying.
///
/// WHY: this is the split the setting is defined by, and the assertion is
/// deliberately made against the real update path rather than against a claim
/// about it. With the setting off, a staged update is still reported as
/// standing, still applied by the next start, and still leaves the new
/// binaries installed. Only `restart_offer` changes its answer.
#[test]
fn hidden_still_checks_stages_and_applies() {
    let dir = scratch("split");
    fs::write(dir.join("vitrum"), b"old client").unwrap();
    fs::write(dir.join("vitrum-server"), b"old daemon").unwrap();

    let mut settings = Settings::default();
    settings.show_restart_to_update = false;

    // Resolving is not consulted about drawing at all: the same release is
    // offered either way.
    let release = serde_json::json!({
        "tag_name": "v9.9.9",
        "assets": [
            {
                "name": archive_name(&Version::parse("9.9.9").unwrap()),
                "browser_download_url": "https://x/a",
            },
            { "name": "SHA256SUMS", "browser_download_url": "https://x/s" },
        ],
    });
    let status = resolve(
        settings.update_channel,
        Some(&release),
        None,
        &Version::parse("0.0.1").unwrap(),
    )
    .expect("resolved");
    assert!(
        matches!(status, Status::Ready(_)),
        "hiding the affordance changed what a check answers: {status:?}"
    );

    // Staging happens.
    stage(&dir, "9.9.9");
    let waiting = standing(&dir, None);
    assert_eq!(
        waiting,
        Standing::Staged {
            version: Version::parse("9.9.9").unwrap()
        },
        "hiding the affordance hid the staged update from the model too"
    );

    // Drawing is the only thing that changed.
    assert_eq!(restart_offer(&waiting, settings.show_restart_to_update), None);

    // Applying happens.
    let applied = apply_staged(&dir).expect("applied");
    assert_eq!(applied, Some(Version::parse("9.9.9").unwrap()));
    assert_eq!(
        fs::read(dir.join("vitrum")).unwrap(),
        b"new client",
        "hiding the affordance stopped the update from being applied"
    );
    assert_eq!(fs::read(dir.join("vitrum-server")).unwrap(), b"new daemon");
    fs::remove_dir_all(&dir).ok();
}

/// A `ui.json` from a build that never had these settings still loads.
///
/// WHY: adding a field without a default makes serde refuse the whole
/// document, `parse_ui_state` calls that corrupt, the window starts on
/// defaults, and the first save writes those defaults over every workspace,
/// folder and placement the operator had. The cost of getting this wrong is
/// not a missing toggle; it is the profile.
#[test]
fn a_ui_json_written_before_these_settings_still_loads() {
    // A document this build writes, with the two new keys taken back out and
    // some older values put in: what a profile written before these settings
    // existed looks like. Derived from the real encoder rather than hand
    // written, so it stays a full document as other fields are added.
    let mut doc: serde_json::Value =
        serde_json::from_str(&crate::state::encode_ui_state(&crate::state::Persisted::default()))
            .expect("this build's own document parses");
    let settings = doc
        .get_mut("settings")
        .and_then(|s| s.as_object_mut())
        .expect("the document has a settings object");
    assert!(
        settings.remove("showRestartToUpdate").is_some(),
        "the affordance setting is not written under the name the loader reads"
    );
    assert!(
        settings.remove("updateChannel").is_some(),
        "the channel setting is not written under the name the loader reads"
    );
    settings.insert("showBranch".into(), serde_json::json!(false));
    settings.insert("textScalePct".into(), serde_json::json!(120));
    settings.insert("seenVersion".into(), serde_json::json!("0.1.0"));

    let loaded = parse_ui_state(&doc.to_string());
    let doc = match loaded {
        UiStateLoad::Loaded(doc) => doc,
        other => panic!("an older ui.json no longer loads: {other}"),
    };
    assert!(
        doc.settings.show_restart_to_update,
        "the affordance defaulted to hidden for every existing profile"
    );
    assert_eq!(doc.settings.update_channel, Channel::Stable);
    // The values that were in the file are still the values.
    assert!(!doc.settings.show_branch);
    assert_eq!(doc.settings.text_scale_pct, 120);
    assert_eq!(doc.settings.seen_version, "0.1.0");
}

/// Both settings survive a write and a read.
///
/// WHY: a setting that cannot be persisted is a setting that resets on every
/// launch, and the channel resetting to stable would silently move a nightly
/// install back to the stable stream.
#[test]
fn both_settings_round_trip_through_the_document() {
    let mut settings = Settings::default();
    settings.show_restart_to_update = false;
    settings.update_channel = Channel::Nightly;

    let text = serde_json::to_string(&settings).expect("encoded");
    assert!(
        text.contains("\"updateChannel\":\"nightly\""),
        "the channel is not written in the document's casing: {text}"
    );
    let back: Settings = serde_json::from_str(&text).expect("decoded");
    assert_eq!(back, settings);
}
