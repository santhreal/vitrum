//! WHY: a setting is four things that can disagree with each other. A field on
//! [`Settings`], a key in the file, a row in the sheet and a line in the
//! table. Every defect this suite exists for is one of them drifting from
//! another: a field nobody persisted, a row describing a default that moved, a
//! key renamed under an operator's profile, a control that needs a restart and
//! does not say so.
//!
//! The list is not written down here. [`declared_paths`] parses the shipped
//! settings source, so adding a field to [`Settings`] and stopping there turns
//! this suite red rather than shipping a preference that resets on every
//! launch.
//!
//! What this does NOT catch: a row whose description is a fluent sentence
//! about the wrong behaviour. Prose against behaviour is asserted in
//! `ui/settings/sheet_copy_is_true.rs`, which reads the source of the code
//! that implements each claim.

use std::collections::BTreeSet;

use super::*;
use crate::state::{
    Persisted, Settings, UI_STATE_VERSION, UiStateLoad, encode_ui_state, parse_ui_state,
};
use serde_json::Value;

/// The value at a dotted path, or `None` when the path is not in the document.
fn value_at<'a>(doc: &'a Value, path: &str) -> Option<&'a Value> {
    let mut here = doc;
    for step in path.split('.') {
        here = here.get(step)?;
    }
    Some(here)
}

/// Write a value at a dotted path, creating the objects it passes through.
fn set_at(doc: &mut Value, path: &str, new: Value) {
    let mut here = doc;
    let mut steps = path.split('.').peekable();
    while let Some(step) = steps.next() {
        if steps.peek().is_none() {
            here.as_object_mut()
                .unwrap_or_else(|| panic!("{path} passes through a value that is not an object"))
                .insert(step.to_string(), new);
            return;
        }
        here = here
            .as_object_mut()
            .unwrap_or_else(|| panic!("{path} passes through a value that is not an object"))
            .entry(step.to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
    }
}

/// The document a fresh profile writes.
fn default_document() -> Value {
    serde_json::to_value(Settings::default()).expect("settings serialise")
}

/// Every field the settings source declares has a row here.
///
/// THE BUG this stops: a field added to [`Settings`] with no row, no
/// description and no round-trip case. It persists by accident, it is
/// undocumented, and the first time anybody notices is when it does not.
#[test]
fn the_catalogue_is_the_source() {
    let declared = declared_paths();
    let catalogued = catalogued_paths();
    assert!(
        !declared.is_empty(),
        "the source parser found no settings at all, so this suite is asserting nothing"
    );
    let missing: Vec<&String> = declared.difference(&catalogued).collect();
    assert!(
        missing.is_empty(),
        "these settings exist in the source and have no catalogue row, so they are \
         undocumented and untested: {missing:?}"
    );
    let extra: Vec<&String> = catalogued.difference(&declared).collect();
    assert!(
        extra.is_empty(),
        "these catalogue rows name settings the source does not declare: {extra:?}"
    );
}

/// Every catalogued setting is a key in the file a fresh profile writes.
///
/// THE BUG this stops: a field that is in the struct and not in the document,
/// which is what `#[serde(skip)]` and a mistyped `rename` both produce. The
/// control works for the rest of the session and the value is gone on the next
/// launch, which is indistinguishable from a control that does nothing.
#[test]
fn every_setting_is_written_to_the_file() {
    let doc = default_document();
    for s in SETTINGS {
        assert!(
            value_at(&doc, s.path).is_some(),
            "{} is catalogued and is not a key in the file a fresh profile writes",
            s.path
        );
    }
}

/// Every default in the table is the shipped default.
///
/// THE BUG this stops: a default retuned in one place and documented in
/// another. The table is generated from these strings, so a stale one is a
/// documented default the product does not have.
#[test]
fn every_documented_default_is_the_shipped_default() {
    let doc = default_document();
    for s in SETTINGS {
        let documented: Value = serde_json::from_str(s.default)
            .unwrap_or_else(|e| panic!("{}: the documented default is not JSON: {e}", s.path));
        let shipped = value_at(&doc, s.path).expect("checked by the test above");
        assert_eq!(
            &documented, shipped,
            "{} is documented as {} and ships as {shipped}",
            s.path, s.default
        );
    }
}

/// Every setting survives a write, a read, and the repairs the load path
/// performs on the way in.
///
/// THE BUG this stops: a value that reaches the file and does not come back.
/// The pair used here is the one the product uses, not a plain serde round
/// trip: [`parse_ui_state`] normalises and clamps, so a value that survives
/// `to_string`/`from_str` can still be rewritten by the real load path, and
/// the real load path is what runs at startup.
#[test]
fn every_setting_survives_the_file() {
    let defaults = default_document();
    for s in SETTINGS {
        let alt: Value = serde_json::from_str(s.alt)
            .unwrap_or_else(|e| panic!("{}: the alternate value is not JSON: {e}", s.path));

        let mut doc = defaults.clone();
        set_at(&mut doc, s.path, alt);
        let settings: Settings = serde_json::from_value(doc)
            .unwrap_or_else(|e| panic!("{}: the alternate value is not a legal setting: {e}", s.path));

        // What the alternate becomes once the type has had it. Compared
        // against, rather than the literal, so a type that normalises its own
        // input is not reported as a persistence failure.
        let staged = serde_json::to_value(&settings).expect("settings serialise");
        let want = value_at(&staged, s.path)
            .unwrap_or_else(|| panic!("{} vanished on the way into the type", s.path))
            .clone();
        let shipped = value_at(&defaults, s.path).expect("checked above");
        assert_ne!(
            &want, shipped,
            "{}: the alternate value lands on the default, so the round trip below proves \
             nothing",
            s.path
        );

        let text = encode_ui_state(&Persisted {
            version: UI_STATE_VERSION,
            settings,
            ..Persisted::default()
        });
        let back = match parse_ui_state(&text) {
            UiStateLoad::Loaded(doc) => *doc,
            other => panic!("{}: the file did not read back: {other}", s.path),
        };
        let got = value_at(
            &serde_json::to_value(&back.settings).expect("settings serialise"),
            s.path,
        )
        .cloned();
        assert_eq!(
            got,
            Some(want),
            "{} did not survive the file, so it resets on every launch",
            s.path
        );
    }
}

/// Every row says what it does and when it takes effect.
///
/// THE BUG this stops: a row that needs a restart and does not say so. An
/// operator flips it, watches nothing happen, and concludes the settings are
/// decoration, which is the complaint this whole surface exists to answer.
#[test]
fn every_row_states_what_it_does_and_when() {
    for s in SETTINGS {
        assert!(!s.description.is_empty(), "{} has no description", s.path);
        assert!(
            s.description.ends_with('.'),
            "{}: a description is a sentence: {}",
            s.path,
            s.description
        );
        assert!(
            s.description
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase()),
            "{}: a description starts with a capital: {}",
            s.path,
            s.description
        );
        assert!(!s.kind.is_empty(), "{} has no type", s.path);
        assert!(!s.applies.is_empty(), "{} says nothing about where it applies", s.path);
        assert!(!s.live.note().is_empty(), "{} does not say when it applies", s.path);
    }
}

/// No description explains this product in terms of the interface it no longer
/// has.
///
/// THE BUG this stops: a row inherited from the build that ran a JavaScript
/// terminal inside a webview, still describing a stylesheet, a document or a
/// script to an operator whose terminal is a native surface. The pane is
/// wgpu over a cell grid; a description that says otherwise is false.
#[test]
fn no_description_describes_an_interface_this_product_does_not_have() {
    for s in SETTINGS {
        let text = s.description.to_ascii_lowercase();
        for word in [
            "webview", "javascript", "script", "browser", "dom ", "stylesheet", "css ",
            "web page", "html",
        ] {
            assert!(
                !text.contains(word),
                "{} describes the product through {word:?}: {}",
                s.path,
                s.description
            );
        }
    }
}

/// A path is listed once.
#[test]
fn no_setting_is_catalogued_twice() {
    let mut seen = std::collections::BTreeSet::new();
    for s in SETTINGS {
        assert!(seen.insert(s.path), "{} is catalogued twice", s.path);
    }
}

/// The generated table has a row per setting and a header.
///
/// THE BUG this stops: a table generator that silently emits nothing, which
/// documents the whole surface as absent while every other test here passes.
#[test]
fn the_table_has_a_row_per_setting() {
    let table = markdown_table();
    let rows = table.lines().count();
    assert_eq!(
        rows,
        SETTINGS.len() + 2,
        "the table has {rows} lines for {} settings plus two header lines",
        SETTINGS.len()
    );
    for s in SETTINGS {
        assert!(
            table.contains(&format!("| `{}` |", s.path)),
            "{} is not in the generated table",
            s.path
        );
    }
}

/// The source parser finds the groups the document actually has.
///
/// A guard on the guard. [`the_catalogue_is_the_source`] compares two sets, and
/// two empty sets are equal, so a parser that quietly stops finding fields
/// would make the whole suite pass while checking nothing. This pins the shape
/// it must find.
#[test]
fn the_source_parser_finds_the_nested_groups() {
    let declared = declared_paths();
    for path in [
        "showBranch",
        "terminal.fontSizePx",
        "terminal.hostPalette",
        "appearance.opacityPct",
        "notifications.failed",
        "keyboard.overrides",
        "notices.flashSeconds",
        "startup.splashAfterMs",
        "policy",
    ] {
        assert!(
            declared.contains(path),
            "the source parser did not find {path}, so it is not reading the settings source"
        );
    }
    assert!(
        !declared.contains("terminal"),
        "a group that was recursed into must not also be a leaf"
    );
    assert!(
        !declared.contains("terminal.hostPalette.background"),
        "an imported palette is persisted whole and is one row, not seven"
    );
}

/// A setting that applies immediately and one that does not are different
/// rows, and the sheet can tell them apart.
#[test]
fn the_two_kinds_of_row_say_different_things() {
    assert_ne!(Live::Immediately.note(), Live::NextLaunch.note());
    assert_ne!(Live::Immediately.note(), Live::NewWindow.note());
    let restart = setting("startup.splashAfterMs").expect("the splash delay is catalogued");
    assert_eq!(restart.live, Live::NextLaunch);
    let live = setting("terminal.fontSizePx").expect("the font size is catalogued");
    assert_eq!(live.live, Live::Immediately);
}

/// Every key name anywhere in a document, at any depth.
fn collect_keys(value: &Value, out: &mut BTreeSet<String>) {
    if let Value::Object(map) = value {
        for (key, child) in map {
            out.insert(key.clone());
            collect_keys(child, out);
        }
    }
}

/// The backticked key in the first cell of a manual table row, if the line is
/// one.
fn table_key(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("| `")?;
    let (key, after) = rest.split_once('`')?;
    after.trim_start().starts_with('|').then_some(key)
}

/// The manual on disk agrees with the catalogue, in both directions.
///
/// THE BUG this stops: a setting added to the struct, catalogued, shipped and
/// never written into `docs/configuration.md`. That file is the only
/// reference for a hand-edited `ui.json`, so a key missing from it reads as
/// "the file will not accept that" rather than "nobody wrote it down".
///
/// The variant space is [`SETTINGS`] read at run time, and the catalogue is
/// itself held to the parsed settings source by
/// [`the_catalogue_is_the_source`], so a new field has to reach the struct,
/// this list and the manual before the suite goes green again. Nothing here
/// is a written-down list of setting names.
///
/// The reverse direction is checked against the DOCUMENT a fresh profile
/// writes rather than against the catalogue's paths, because a group that is
/// persisted whole is one catalogue row and several documented keys: `policy`
/// is the row, `autoSettleAfterMs` is the key inside it, and both are
/// legitimate things for the manual to name.
///
/// What this does NOT catch: a manual entry whose Effect column describes the
/// wrong behaviour, or a default written into the manual that has since
/// moved. The catalogue's own defaults are pinned to the shipped ones by
/// [`every_documented_default_is_the_shipped_default`]; the manual's copies of
/// them are prose and are read by a person.
#[test]
fn every_setting_is_in_the_manual() {
    const MANUAL: &str = include_str!("../../../../../docs/configuration.md");
    let section = MANUAL
        .split_once("\n## Settings")
        .expect("the manual has a Settings section")
        .1
        .split_once("\n## ")
        .expect("the Settings section ends at the next top-level heading")
        .0;

    for s in SETTINGS {
        let leaf = s.path.rsplit('.').next().unwrap_or(s.path);
        assert!(
            section.contains(&format!("`{leaf}`")),
            "`{}` is catalogued and docs/configuration.md never names `{leaf}`",
            s.path
        );
    }

    let mut written = BTreeSet::new();
    collect_keys(&default_document(), &mut written);
    for line in section.lines() {
        let Some(key) = table_key(line) else {
            continue;
        };
        assert!(
            written.contains(key),
            "docs/configuration.md documents `{key}`, which no fresh profile writes"
        );
    }
}

/// The `lo` and `hi` of a catalogued `integer, lo-hi` kind.
fn declared_range(kind: &str) -> Option<(i64, i64)> {
    let span = kind.strip_prefix("integer, ")?;
    let (lo, hi) = span.split_once('-')?;
    Some((lo.trim().parse().ok()?, hi.trim().parse().ok()?))
}

/// Every catalogued range is the range the load path actually enforces.
///
/// THE BUG this stops: a bounded setting whose stated range is prose. The
/// manual and the sheet both quote the catalogue, so a range with no clamp
/// behind it invites an operator to hand-edit a value the product then paints
/// with: a preview cut of zero hides every inbox row, a reconnect ceiling of
/// one millisecond is a busy loop against a socket nothing is listening on.
///
/// The variant space is [`SETTINGS`] read at run time and filtered on the
/// `kind` string, so a new bounded setting is in this test the moment it is
/// catalogued. Adding one with a range and no clamp turns the suite red
/// rather than shipping a number the file accepts and the product cannot
/// survive.
///
/// Three things are asserted per setting: the shipped default is inside the
/// stated range, both ends of the range survive the file unchanged, and a
/// value one past each end comes back as that end rather than as itself or as
/// the default. The last is what distinguishes a clamp from a reset: a load
/// path that replaced an out-of-range document with `Settings::default` would
/// pass a test that only checked the value was legal.
///
/// What this does NOT catch: a range that is enforced and badly chosen. That
/// a ceiling of 200000 scrollback lines is a sensible ceiling is a judgement,
/// and it is argued in the doc comment on the constant.
#[test]
fn every_catalogued_range_is_the_range_the_loader_enforces() {
    let defaults = default_document();
    let mut checked = 0;
    for s in SETTINGS {
        let Some((lo, hi)) = declared_range(s.kind) else {
            continue;
        };
        assert!(lo < hi, "{}: {} is not a range", s.path, s.kind);
        let shipped: i64 = s
            .default
            .parse()
            .unwrap_or_else(|e| panic!("{}: the default is not an integer: {e}", s.path));
        assert!(
            (lo..=hi).contains(&shipped),
            "{}: the shipped default {shipped} is outside the stated range {lo}-{hi}",
            s.path
        );

        // Both ends, and one step past each, through the real load path.
        let mut cases = vec![(lo, lo), (hi, hi), (hi + 1, hi)];
        if lo > 0 {
            cases.push((lo - 1, lo));
        }
        for (wrote, want) in cases {
            let mut doc = defaults.clone();
            set_at(&mut doc, s.path, Value::from(wrote));
            let settings: Settings = serde_json::from_value(doc).unwrap_or_else(|e| {
                panic!("{}: {wrote} is not a value the field can hold: {e}", s.path)
            });
            let text = encode_ui_state(&Persisted {
                version: UI_STATE_VERSION,
                settings,
                ..Persisted::default()
            });
            let back = match parse_ui_state(&text) {
                UiStateLoad::Loaded(doc) => *doc,
                other => panic!("{}: {wrote} made the profile unreadable: {other}", s.path),
            };
            let got = value_at(
                &serde_json::to_value(&back.settings).expect("settings serialise"),
                s.path,
            )
            .and_then(Value::as_i64);
            assert_eq!(
                got,
                Some(want),
                "{}: a document holding {wrote} loaded as {got:?}, not {want}",
                s.path
            );
        }
        checked += 1;
    }
    assert!(
        checked >= 20,
        "only {checked} bounded settings were checked; the kind strings no longer parse"
    );
}
