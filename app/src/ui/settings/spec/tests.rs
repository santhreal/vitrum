//! The catalogue and the surface, differenced at run time.
//!
//! THE BUG this module stops: a setting that ships with no way to change it.
//! It has happened twice in this product, both times because the row list was
//! prose in one file and the struct was a declaration in another, and nothing
//! compared them.
//!
//! The variant space is read from [`crate::state::catalog::SETTINGS`] at run
//! time rather than written down here, so adding a setting turns this suite
//! red instead of leaving a hole a reviewer has to notice.

use std::collections::{BTreeMap, BTreeSet};

use super::{BESPOKE, Control, NOT_A_PREFERENCE, Row, all_rows, settle_options};
use crate::state::catalog::{self, Live};
use crate::state::{Settings, SettingsTab};

/// Every path the surface claims, and how it is claimed.
fn claims() -> BTreeMap<String, &'static str> {
    let mut out = BTreeMap::new();
    for row in all_rows() {
        out.insert(row.path.to_string(), "row");
    }
    for (path, _) in BESPOKE {
        out.insert((*path).to_string(), "bespoke");
    }
    for (path, _) in NOT_A_PREFERENCE {
        out.insert((*path).to_string(), "not a preference");
    }
    out
}

/// Every catalogued setting is reachable from the settings surface.
///
/// THE BUG this stops: a preference that exists in the file, is honoured by
/// the product, and has no control. The operator's only way to change it is a
/// text editor and a manual, which is the state this whole surface exists to
/// end.
///
/// The two escape hatches are lists rather than judgement, so a setting cannot
/// be quietly exempted: it has to be named in [`BESPOKE`] or
/// [`NOT_A_PREFERENCE`] with the surface that owns it.
#[test]
fn every_catalogued_setting_has_a_control() {
    let claimed = claims();
    let missing: Vec<&str> = catalog::SETTINGS
        .iter()
        .map(|s| s.path)
        .filter(|path| !claimed.contains_key(*path))
        .collect();
    assert!(
        missing.is_empty(),
        "these settings are catalogued and the settings surface has no control for them, so \
         the only way to change them is a text editor: {missing:?}. Add a row in \
         `ui::settings::spec`, or name the control that owns it in BESPOKE, or say in \
         NOT_A_PREFERENCE what writes it."
    );
}

/// The surface names no setting the catalogue does not have.
///
/// THE BUG this stops: a row left behind by a rename. `catalog::setting`
/// returns `None`, the timing sentence under the control goes empty, and
/// nothing fails.
#[test]
fn every_control_names_a_catalogued_setting() {
    let catalogued: BTreeSet<&str> = catalog::SETTINGS.iter().map(|s| s.path).collect();
    let unknown: Vec<String> = claims()
        .into_keys()
        .filter(|path| !catalogued.contains(path.as_str()))
        .collect();
    assert!(
        unknown.is_empty(),
        "the settings surface names settings the catalogue does not have: {unknown:?}"
    );
}

/// No setting is claimed by two places at once.
///
/// THE BUG this stops: a row and a bespoke control both writing one field,
/// which is two controls that disagree the moment one of them clamps.
#[test]
fn no_setting_is_claimed_twice() {
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    let mut duplicates = Vec::new();
    let mut record = |path: &'static str, how: &'static str| {
        if let Some(first) = seen.insert(path, how) {
            duplicates.push(format!("{path} is claimed as {first} and as {how}"));
        }
    };
    for row in all_rows() {
        record(row.path, "a row");
    }
    for (path, _) in BESPOKE {
        record(path, "bespoke");
    }
    for (path, _) in NOT_A_PREFERENCE {
        record(path, "not a preference");
    }
    assert!(duplicates.is_empty(), "{duplicates:?}");
}

/// Every escape hatch says which control owns the setting.
///
/// THE BUG this stops: an exemption list that grows into a way of not writing
/// a control. A path with an empty reason is an exemption nobody has to
/// justify.
#[test]
fn every_exemption_names_its_owner() {
    for (path, why) in BESPOKE.iter().chain(NOT_A_PREFERENCE) {
        assert!(
            why.len() > 20,
            "{path} is exempt from having a row and does not say what edits it instead"
        );
    }
}

/// Every row prints the catalogue's own timing sentence.
///
/// THE BUG this stops: a control that applies on the next launch and says
/// nothing, which reads as broken and gets toggled back. The sentence comes
/// from the catalogue rather than from the row, so a row cannot claim to be
/// live while the table calls it a restart.
#[test]
fn every_row_prints_when_it_takes_effect() {
    for row in all_rows() {
        let setting = catalog::setting(row.path).expect("checked by the test above");
        assert_eq!(
            super::super::when_note(row.path),
            setting.live.note(),
            "the row for {} prints a timing sentence the catalogue did not write",
            row.path
        );
    }
}

/// Every setting that applies later is reachable from a control that says so.
///
/// The variant space is the catalogue, so a setting added with a delay and no
/// control fails here as well as in the completeness check above.
#[test]
fn every_delayed_setting_is_on_a_surface_that_prints_the_delay() {
    let claimed = claims();
    for setting in catalog::SETTINGS {
        if setting.live == Live::Immediately {
            continue;
        }
        let how = claimed.get(setting.path).copied().unwrap_or("nothing");
        assert_ne!(
            how, "nothing",
            "{} applies later and no control mentions it",
            setting.path
        );
        assert!(
            !super::super::when_note(setting.path).is_empty(),
            "{} applies later and its control prints no timing sentence",
            setting.path
        );
    }
}

/// Every row's label and caption say something.
///
/// THE BUG this stops: a row copied for a new setting whose caption was never
/// written. An unlabelled switch is a setting the operator cannot use even
/// though the completeness check above is satisfied.
#[test]
fn every_row_is_captioned() {
    let settings = Settings::default();
    for row in all_rows() {
        assert!(!row.label.is_empty(), "{} has no label", row.path);
        assert!(
            row.caption(&settings).len() > 20,
            "{} has no caption worth reading",
            row.path
        );
    }
}

/// A row's menu can express the value a fresh profile has.
///
/// THE BUG this stops, and it shipped: a menu whose stored value matches no
/// option shows the FIRST option instead. An install sitting at the shipped
/// seven-day settle window read "Never" while quietly settling rows behind the
/// operator.
#[test]
fn every_menu_can_express_the_shipped_default() {
    let settings = Settings::default();
    for row in all_rows() {
        let Control::Choice { options, get, .. } = &row.control else {
            continue;
        };
        let current = get(&settings);
        let offered = options();
        assert!(
            offered.iter().any(|(value, _)| *value == current),
            "{} defaults to {current:?} and its menu offers {:?}",
            row.path,
            offered.iter().map(|(v, _)| v).collect::<Vec<_>>()
        );
    }
}

/// The settle menu can express the model's own default.
///
/// Stated separately from the check above because the value is owned by
/// `vitrum-model` rather than by this crate's defaults, so the two can drift
/// without any settings file changing.
#[test]
fn the_settle_menu_can_express_the_model_default() {
    let default = vitrum_model::DispositionPolicy::DEFAULT_AUTO_SETTLE_MS.to_string();
    assert!(
        settle_options().iter().any(|(value, _)| *value == default),
        "the model settles after {default} ms and the menu cannot express it"
    );
}

/// No menu offers the same value twice.
///
/// THE BUG this stops: two entries that write one value, so picking either
/// leaves the other looking unselected.
#[test]
fn no_menu_repeats_a_value() {
    for row in all_rows() {
        let Control::Choice { options, .. } = &row.control else {
            continue;
        };
        let offered = options();
        let unique: BTreeSet<&String> = offered.iter().map(|(v, _)| v).collect();
        assert_eq!(
            unique.len(),
            offered.len(),
            "{} offers a value twice: {:?}",
            row.path,
            offered.iter().map(|(v, _)| v).collect::<Vec<_>>()
        );
    }
}

/// Picking every option a menu offers round-trips through the document.
///
/// THE BUG this stops: a setter that parses a value the menu does not emit, or
/// a getter that renders a value the menu cannot select. Either one is a
/// control that snaps back to a different entry the moment the page is
/// redrawn, which is the "settings that do not stick" defect in its purest
/// form.
///
/// Every option of every menu, not one representative, because the failure is
/// per option: a slug renamed in one arm of a `match` leaves the other arms
/// working.
#[test]
fn every_option_a_menu_offers_survives_being_picked() {
    for row in all_rows() {
        let Control::Choice { options, get, set } = &row.control else {
            continue;
        };
        for (value, label) in options() {
            let mut settings = Settings::default();
            set(&mut settings, &value);
            assert_eq!(
                get(&settings),
                value,
                "{}: picking {label:?} stored something the control reads back as {:?}",
                row.path,
                get(&settings)
            );
        }
    }
}

/// Every option a menu offers survives the loader's clamps.
///
/// THE BUG this stops: a control that can produce a value the load path would
/// clamp, so the setting is one thing while the sheet is open and another
/// after a restart. `persistence.rs` owns the clamps; this asserts the menus
/// honour the same bounds rather than restating them.
#[test]
fn no_menu_can_produce_a_value_the_loader_would_clamp() {
    for row in all_rows() {
        let Control::Choice { options, get, set } = &row.control else {
            continue;
        };
        for (value, label) in options() {
            let mut settings = Settings::default();
            set(&mut settings, &value);
            let picked = get(&settings);
            settings.clamp();
            assert_eq!(
                get(&settings),
                picked,
                "{}: {label:?} stores {picked:?} and the loader clamps it to {:?}",
                row.path,
                get(&settings)
            );
        }
    }
}

/// Every switch actually moves the document both ways.
///
/// THE BUG this stops: a getter and a setter aimed at two different fields,
/// which is a switch that renders one setting and writes another.
#[test]
fn every_switch_writes_the_field_it_reads() {
    for row in all_rows() {
        let Control::Switch { get, set } = &row.control else {
            continue;
        };
        let mut settings = Settings::default();
        for want in [true, false, true] {
            set(&mut settings, want);
            assert_eq!(get(&settings), want, "{} does not store what it reads", row.path);
        }
    }
}

/// A row is drawn on exactly one page.
///
/// THE BUG this stops: a row pasted onto a second page, which gives one
/// setting two controls that cannot see each other.
#[test]
fn no_row_appears_on_two_pages() {
    let mut home: BTreeMap<&str, SettingsTab> = BTreeMap::new();
    for tab in SettingsTab::ALL {
        for row in super::rows(tab) {
            if let Some(first) = home.insert(row.path, tab) {
                panic!(
                    "{} is a row on both {} and {}",
                    row.path,
                    first.label(),
                    tab.label()
                );
            }
        }
    }
}

/// A row hidden by a condition can still be reached.
///
/// THE BUG this stops: a setting whose row is only drawn when the setting
/// itself is already at some value, which is a control that cannot be
/// operated. The blink period is drawn when blinking is on; the backdrop
/// controls are drawn when a backdrop is set. Both of those gates are another
/// setting, and both of those settings have their own control.
#[test]
fn every_hidden_row_has_a_visible_way_to_reveal_it() {
    let mut settings = Settings::default();
    settings.terminal.cursor_blink = true;
    settings.startup.show_splash = true;
    "/src/vitrum/wall.png".clone_into(&mut settings.appearance.backdrop);
    let hidden: Vec<&str> = all_rows()
        .filter(|row: &&Row| !row.is_visible(&settings))
        .map(|row| row.path)
        .collect();
    assert!(
        hidden.is_empty(),
        "these rows are hidden even with blinking on, the boot mark on and a backdrop set, so \
         nothing on the surface reveals them: {hidden:?}"
    );
}
