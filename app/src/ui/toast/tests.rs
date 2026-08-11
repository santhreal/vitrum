//! The anti-flap rule and the two lifetimes.

use super::*;

fn notice(text: &str) -> Flash {
    Flash::notice(text)
}

fn error(text: &str) -> Flash {
    Flash::error(text)
}

/// **The defect this surface was rebuilt for.** A fan-out that did not change
/// the notice must not touch the notice.
///
/// The state fans out on every daemon message. A toast rebuilt on each one
/// restarts its entrance and its retirement timer several times a second,
/// which is a notice that never finishes arriving and never expires. That is
/// the flapping, and it is not a rendering fault: it is a surface that treated
/// "told again" as "changed".
#[test]
fn the_same_notice_told_twice_is_left_alone() {
    let up = notice("Started an agent.");
    assert_eq!(step(Some(&up), Some(&up)), Step::Hold);
    // Twenty more fan-outs with nothing new to say.
    for _ in 0..20 {
        assert_eq!(step(Some(&up), Some(&up)), Step::Hold);
    }
}

/// Two notices with the same words but different kinds are different notices.
/// A refusal and a confirmation reading the same sentence is unlikely, and
/// treating them as equal would leave an error painted as a confirmation.
#[test]
fn a_notice_and_an_error_with_the_same_words_are_not_the_same_notice() {
    let said = "Nothing happened.";
    assert_eq!(
        step(Some(&error(said)), Some(&notice(said))),
        Step::Raise(error(said))
    );
}

/// A genuinely new notice replaces what is up, with its own life.
#[test]
fn a_new_notice_replaces_the_one_on_screen() {
    assert_eq!(
        step(Some(&notice("second")), Some(&notice("first"))),
        Step::Raise(notice("second"))
    );
    assert_eq!(step(Some(&notice("first")), None), Step::Raise(notice("first")));
}

/// Clearing the state takes the notice down, and a state that was already
/// clear does nothing at all. Retiring an empty toast would hide a widget that
/// is already hidden on every fan-out of a quiet window.
#[test]
fn clearing_retires_and_a_quiet_window_does_nothing() {
    assert_eq!(step(None, Some(&notice("gone"))), Step::Retire);
    assert_eq!(step(None, None), Step::Idle);
}

/// **An error never retires by itself.** A failure that erases itself before
/// it is read is worse than one that stays, whatever the profile says about
/// notice life.
#[test]
fn an_error_outlasts_every_configured_life() {
    for configured in [None, Some(1), Some(1_000), Some(u64::MAX)] {
        assert_eq!(life(&error("Profile not saved"), configured), None);
    }
}

/// A notice takes the configured life, and a profile that configured none
/// keeps its notices until they are dismissed. That is how somebody who reads
/// slowly asks to close their own.
#[test]
fn a_notice_takes_the_configured_life_and_no_other() {
    assert_eq!(life(&notice("Copied"), Some(4_000)), Some(4_000));
    assert_eq!(life(&notice("Copied"), None), None);
}

/// The two kinds are painted by two classes. One class for both would make a
/// spawn failure and a copy confirmation the same colour, which is the whole
/// distinction the kinds carry.
#[test]
fn each_kind_has_its_own_class() {
    assert_ne!(class(FlashKind::Error), class(FlashKind::Notice));
    for kind in [FlashKind::Error, FlashKind::Notice] {
        assert!(
            crate::shell::style::classes().contains(&class(kind)),
            "{} is painted by no rule in the stylesheet",
            class(kind)
        );
    }
}
