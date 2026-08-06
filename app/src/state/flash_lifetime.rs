//// A flash's LIFETIME, which is the difference between a confirmation and a
//// banner.
////
//// `Flash` shipped with no expiry of any kind. A notice was cleared only by an
//// explicit Dismiss click or by another flash overwriting it, so on a real
//// window "Started bash in tmp. Ctrl+Shift+X stops it." was still occupying a
//// full-width band above the terminal twenty-nine minutes after the session
//// started. Nothing in the type or in any test said a notice was supposed to
//// be temporary, which is why it never was.
////
//// The retirement itself is a one-shot in `main.rs`, because the model has no
//// clock. What is provable here is the part that decides WHICH flashes retire,
//// and that the two kinds are distinguishable at all.

use super::*;

/// A notice is transient and an error is not.
///
/// The whole rule, stated once. An error reports something the operator
/// has to act on, and a failure that erases itself before it is read is
/// worse than a banner that overstays.
#[test]
fn only_a_notice_is_transient() {
    assert_eq!(
        Flash::notice("Started bash in tmp.").kind,
        FlashKind::Notice
    );
    assert_eq!(Flash::error("The daemon went away.").kind, FlashKind::Error);
    assert_ne!(
        FlashKind::Notice,
        FlashKind::Error,
        "the two kinds must be distinguishable or nothing can retire one \
         without retiring the other"
    );
}

/// Two notices with the same words are the same flash.
///
/// The retiring one-shot re-reads the window when it wakes and clears the
/// flash only if it still equals the one it was raised for. That guard is
/// an equality check, so `Flash` must compare by value: if it did not, a
/// notice would never match itself on wake and nothing would ever retire.
#[test]
fn a_flash_compares_by_its_content() {
    let a = Flash::notice("Started bash in tmp.");
    let b = Flash::notice("Started bash in tmp.");
    assert_eq!(a, b, "the retirement guard compares flashes by value");
    assert_ne!(a, Flash::notice("Started claude in vitrum."));
    assert_ne!(
        Flash::notice("same words"),
        Flash::error("same words"),
        "kind is part of identity, or an error could be retired by a \
         notice's timer"
    );
}

/// A window starts with nothing to say.
///
/// The band is absent, not empty: an always-present bar reserving height
/// for a message that is usually not there is the chrome this product is
/// trying not to have.
#[test]
fn a_fresh_window_raises_no_flash() {
    assert_eq!(WindowState::default().flash, None);
}

/// Raising a second flash replaces the first outright.
///
/// One slot, so two things can never be on screen at once and the newest
/// answer is the one shown. This is also why the retirement guard has to
/// check identity: the first notice's timer must not clear the second.
#[test]
fn a_later_flash_replaces_the_one_before_it() {
    let mut w = WindowState::default();
    w.flash = Some(Flash::notice("first"));
    w.flash = Some(Flash::error("second"));
    assert_eq!(w.flash, Some(Flash::error("second")));
}
