//! WHY: the settings file is the only thing between an operator and losing
//! every workspace, folder, placement and preference they have arranged. Two
//! failure modes have both happened in this codebase's history and both are
//! silent: a document shape this build cannot read being replaced by defaults
//! with no explanation, and a field added without a default making every
//! profile written before the release fail to deserialise.
//!
//! So this suite asserts the two rules that keep either from happening again.
//! A document from another format is refused BY NAME, with the corrective
//! action in the message and the old file kept. A document written before a
//! setting existed still loads, keeps everything in it, and takes the default
//! for what it does not have.
//!
//! What this does NOT catch: a change to a field's meaning rather than its
//! name. `scrollbackLines` reinterpreted as kilobytes reads back fine and is
//! wrong, and nothing here can see it.

use std::path::Path;

use crate::state::{
    Persisted, Settings, UI_STATE_FILE, UI_STATE_VERSION, UiStateLoad, archive_path,
    encode_ui_state, load_ui_state, parse_ui_state,
};

/// A minimal document at `version`.
fn document_at(version: u64) -> String {
    format!(r#"{{"version": {version}, "settings": {{}}, "workspaces": {{}}, "windows": []}}"#)
}

/// A file written by a build that ran an older document format is refused.
///
/// THE BUG this stops: a stale document being read field by field into this
/// build's shape, so whatever happened to line up survives, whatever did not
/// is silently the default, and the result is written back over the original
/// on the next save.
///
/// Red before the fix: `parse_ui_state` had one non-matching arm, which
/// answered `Unsupported { version: 0 }`, and the assertion below reads
/// `left: Unsupported { version: 0 }, right: Stale { version: 0 }`.
#[test]
fn a_document_from_an_older_format_is_refused() {
    assert_eq!(
        parse_ui_state(&document_at(0)),
        UiStateLoad::Stale { version: 0 }
    );
}

/// A file written by a newer build is refused, and is never downgraded.
///
/// THE BUG this stops: an operator who runs a newer vitrum once, goes back to
/// this one, and finds the newer build's profile rewritten in this build's
/// shape. Every setting the newer release added is gone and nothing said so.
#[test]
fn a_document_from_a_newer_build_is_refused_and_not_downgraded() {
    let newer = format!(
        r#"{{"version": {}, "settings": {{"showBranch": false, "somethingNew": 7}},
            "workspaces": {{}}, "windows": []}}"#,
        UI_STATE_VERSION + 1
    );
    let read = parse_ui_state(&newer);
    assert_eq!(
        read,
        UiStateLoad::Unsupported {
            version: UI_STATE_VERSION + 1
        }
    );
    assert!(
        !matches!(read, UiStateLoad::Loaded(_)),
        "a newer document was accepted, so this build will write it back in its own shape"
    );
}

/// The two directions are distinguishable, and say opposite things.
///
/// THE BUG this stops: one variant for both, so the message tells an operator
/// with an old profile to install a newer vitrum, which they already have not
/// got, or tells an operator with a newer profile to run the release that
/// wrote it, which they were just running.
#[test]
fn stale_and_unsupported_are_different_answers() {
    let older = parse_ui_state(&document_at(0));
    let newer = parse_ui_state(&document_at(u64::from(UI_STATE_VERSION) + 1));
    assert_ne!(older, newer);
    assert_ne!(older.to_string(), newer.to_string());
    assert!(
        older.to_string().contains("Run the release that wrote it"),
        "{older}"
    );
    assert!(
        newer.to_string().contains("Install the newer vitrum"),
        "{newer}"
    );
}

/// A version past the range of the type reports the number it claimed.
#[test]
fn an_absurd_version_reports_itself() {
    assert_eq!(
        parse_ui_state(&document_at(4_294_967_297)),
        UiStateLoad::Unsupported { version: u32::MAX }
    );
}

/// A document with no version is refused rather than assumed to be current.
#[test]
fn a_document_with_no_version_is_refused() {
    assert_eq!(
        parse_ui_state(r#"{"settings": {}, "workspaces": {}, "windows": []}"#),
        UiStateLoad::Corrupt {
            detail: "no version field".to_string()
        }
    );
}

/// Every refusal names what to do about it.
///
/// The variant list is an exhaustive match rather than a written-down set, so
/// a sixth [`UiStateLoad`] variant does not compile until somebody decides
/// whether it is a refusal and what its message tells the operator to do.
#[test]
fn every_refusal_names_a_corrective_action() {
    let variants = [
        UiStateLoad::Missing,
        UiStateLoad::Loaded(Box::new(Persisted::default())),
        UiStateLoad::Corrupt {
            detail: "trailing comma".to_string(),
        },
        UiStateLoad::Stale { version: 0 },
        UiStateLoad::Unsupported { version: 9 },
        UiStateLoad::Unreadable {
            detail: "permission denied".to_string(),
        },
    ];
    for read in &variants {
        // Exhaustive on purpose: a new variant breaks this match, which stops
        // the crate compiling, which is the only way a list Rust cannot
        // enumerate stays complete.
        let refusal = match read {
            UiStateLoad::Missing | UiStateLoad::Loaded(_) => false,
            UiStateLoad::Corrupt { .. }
            | UiStateLoad::Stale { .. }
            | UiStateLoad::Unsupported { .. }
            | UiStateLoad::Unreadable { .. } => true,
        };
        assert_eq!(read.is_refusal(), refusal, "{read:?}");
        if !refusal {
            continue;
        }
        let message = read.to_string();
        assert!(
            message.contains(UI_STATE_FILE),
            "a refusal must name the file: {message}"
        );
        let imperative = [
            "Repair", "Run ", "Install", "Check", "edit", "delete", "rename",
        ]
        .iter()
        .any(|verb| message.contains(verb));
        assert!(
            imperative,
            "a refusal must name what to do about it: {message}"
        );
    }
}

/// A refused file is moved aside under a name that says why.
///
/// THE BUG this stops: writing defaults over the operator's profile. The old
/// behaviour did exactly that, so a single bad byte in the file cost every
/// workspace and every preference with nothing recoverable afterwards.
#[test]
fn a_refused_file_is_moved_aside_under_a_name_that_says_why() {
    let path = Path::new("/src/vitrum/ui.json");
    assert_eq!(archive_path(path, &UiStateLoad::Missing), None);
    assert_eq!(
        archive_path(path, &UiStateLoad::Loaded(Box::new(Persisted::default()))),
        None
    );
    for (read, want) in [
        (UiStateLoad::Stale { version: 0 }, "ui.json.stale.bak"),
        (
            UiStateLoad::Unsupported { version: 2 },
            "ui.json.unsupported.bak",
        ),
        (
            UiStateLoad::Corrupt {
                detail: String::new(),
            },
            "ui.json.corrupt.bak",
        ),
        (
            UiStateLoad::Unreadable {
                detail: String::new(),
            },
            "ui.json.unreadable.bak",
        ),
    ] {
        let to = archive_path(path, &read).expect("a refusal is archived");
        assert_eq!(
            to.file_name().and_then(|f| f.to_str()),
            Some(want),
            "{read:?}"
        );
        assert_eq!(to.parent(), path.parent(), "the archive stays beside it");
    }
}

/// The message a refusal shows names the file the old one went to.
///
/// THE BUG this stops: a window that opens on defaults with a sentence about a
/// version number and no way to find out where the profile went. That reads as
/// the product having lost the operator's work, which is the difference
/// between a recoverable failure and an unstable product.
#[test]
fn a_refusal_says_where_the_old_file_went() {
    for read in [
        UiStateLoad::Stale { version: 0 },
        UiStateLoad::Unsupported { version: 2 },
        UiStateLoad::Corrupt {
            detail: "x".to_string(),
        },
    ] {
        let message = read.to_string();
        let extension = archive_path(Path::new("ui.json"), &read)
            .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
            .expect("a refusal is archived");
        assert!(
            message.contains(&extension),
            "the message does not name {extension}: {message}"
        );
    }
}

/// A profile written before a setting existed still loads, whole.
///
/// THE BUG this stops, and it has shipped here before: a field added without
/// `#[serde(default)]` on its container makes serde refuse the entire
/// document, `parse_ui_state` calls that corrupt, the window comes up on
/// defaults, and the next save writes those defaults over every workspace and
/// folder the operator arranged.
///
/// The document below is deliberately a build behind: it has the settings a
/// release before this one wrote and none of the groups added since.
#[test]
fn a_profile_written_before_these_settings_existed_still_loads() {
    let older = format!(
        r#"{{
          "version": {UI_STATE_VERSION},
          "settings": {{
            "showBranch": false,
            "showTime": false,
            "theme": "dark",
            "textScalePct": 125,
            "terminal": {{ "fontSizePx": 16, "scrollbackLines": 5000 }},
            "appearance": {{ "opacityPct": 90 }},
            "daemonUrl": "ws://10.0.0.4:9000"
          }},
          "workspaces": {{}},
          "windows": []
        }}"#
    );
    let doc = match parse_ui_state(&older) {
        UiStateLoad::Loaded(doc) => *doc,
        other => panic!("an older profile no longer loads: {other}"),
    };
    let fresh = Settings::default();

    // What it said is kept.
    assert!(!doc.settings.show_branch);
    assert!(!doc.settings.show_time);
    assert_eq!(doc.settings.text_scale_pct, 125);
    assert_eq!(doc.settings.terminal.font_size_px, 16);
    assert_eq!(doc.settings.terminal.scrollback_lines, 5_000);
    assert_eq!(doc.settings.appearance.opacity_pct, 90);
    assert_eq!(doc.settings.daemon_url, "ws://10.0.0.4:9000");

    // What it never had is the shipped default, not zero and not empty.
    assert_eq!(doc.settings.show_worktree, fresh.show_worktree);
    assert_eq!(doc.settings.show_status_bar, fresh.show_status_bar);
    assert_eq!(doc.settings.notices, fresh.notices);
    assert_eq!(doc.settings.startup, fresh.startup);
    assert_eq!(doc.settings.terminal.cursor_shape, fresh.terminal.cursor_shape);
    assert_eq!(doc.settings.terminal.cursor_blink, fresh.terminal.cursor_blink);
    assert_eq!(
        doc.settings.terminal.blink_interval_ms,
        fresh.terminal.blink_interval_ms
    );
    assert_eq!(doc.settings.terminal.present_mode, fresh.terminal.present_mode);
    assert_eq!(doc.settings.terminal.line_height_pct, fresh.terminal.line_height_pct);
    assert_eq!(doc.settings.terminal.wheel_lines, fresh.terminal.wheel_lines);
    assert!(!doc.settings.terminal.follow_host_terminal);
}

/// A hand-edited profile with values outside every range still opens a window
/// somebody can use.
///
/// THE BUG this stops: a zero font size, which is a zero-width cell box and a
/// blank pane; a text scale of 4000, which puts the settings control off the
/// bottom of the screen; a zero blink period, which strobes the cursor at the
/// frame rate. None of these can come out of a control and all of them can
/// come out of a text editor, so the load path is where they are caught.
#[test]
fn a_hand_edited_profile_is_clamped_on_the_way_in() {
    let hostile = format!(
        r#"{{
          "version": {UI_STATE_VERSION},
          "settings": {{
            "textScalePct": 4000,
            "terminal": {{
              "fontSizePx": 0,
              "lineHeightPct": 5000,
              "cellWidthPct": 1,
              "blinkIntervalMs": 0,
              "wheelLines": 250,
              "scrollbackLines": 4000000000
            }},
            "appearance": {{ "opacityPct": 0, "backdropDimPct": 250 }},
            "notices": {{ "flashSeconds": 250, "noticeSeconds": 200 }},
            "startup": {{ "splashAfterMs": 60000 }}
          }},
          "workspaces": {{}},
          "windows": []
        }}"#
    );
    let doc = match parse_ui_state(&hostile) {
        UiStateLoad::Loaded(doc) => *doc,
        other => panic!("a hand-edited profile must load, repaired: {other}"),
    };
    let s = &doc.settings;
    assert_eq!(s.text_scale_pct, crate::state::TEXT_SCALE_MAX_PCT);
    assert_eq!(s.terminal.font_size_px, crate::state::TERM_FONT_MIN_PX);
    assert_eq!(s.terminal.line_height_pct, crate::state::LINE_HEIGHT_MAX_PCT);
    assert_eq!(s.terminal.cell_width_pct, crate::state::CELL_WIDTH_MIN_PCT);
    assert_eq!(s.terminal.blink_interval_ms, crate::state::BLINK_MIN_MS);
    assert_eq!(s.terminal.wheel_lines, crate::state::WHEEL_LINES_MAX);
    assert_eq!(s.terminal.scrollback_lines, crate::state::SCROLLBACK_MAX_LINES);
    assert_eq!(s.appearance.opacity_pct, crate::state::OPACITY_MIN_PCT);
    assert_eq!(s.appearance.backdrop_dim_pct, 100);
    assert_eq!(s.notices.flash_seconds, crate::state::NOTICE_SECONDS_MAX);
    assert_eq!(s.notices.notice_seconds, crate::state::NOTICE_SECONDS_MAX);
    assert_eq!(s.startup.splash_after_ms, crate::state::SPLASH_AFTER_MAX_MS);
}

/// The document a fresh profile writes reads back as itself.
///
/// The base case, and the one that fails first when a field is added with a
/// serde attribute that does not round-trip.
#[test]
fn a_fresh_profile_reads_back_as_itself() {
    let doc = Persisted::default();
    let text = encode_ui_state(&doc);
    match parse_ui_state(&text) {
        UiStateLoad::Loaded(back) => assert_eq!(*back, doc),
        other => panic!("a fresh profile did not read back: {other}"),
    }
}

/// A file that is not there is a first launch, not a failure.
#[test]
fn a_missing_file_is_a_first_launch() {
    let nowhere = std::env::temp_dir()
        .join(format!("vitrum-settings-{}", std::process::id()))
        .join(UI_STATE_FILE);
    assert_eq!(load_ui_state(&nowhere), UiStateLoad::Missing);
    assert_eq!(load_ui_state(&nowhere).or_default().1, None);
}
