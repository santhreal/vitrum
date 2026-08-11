//! The page-back refusal, and why it is counted rather than merely worded.
//!
//! The gesture behind [`super::page_back`] is arrival at the top of the
//! buffer, not a click. A pane whose grid is reset and repainted arrives at
//! the top again on its own, and the notice strip was itself one of the things
//! that caused a repaint, so a refusal raised per attempt was raised forever:
//! a strip that appeared, retired, and appeared again with the pane jumping
//! under it, and a Dismiss that was undone before the pointer left the button.
//!
//! What is provable here is the pair that decides it: [`super::plan_page_back`]
//! and [`super::record_refusal`]. Between them they hold the whole rule, so a
//! test that drives them drives the shipped decision with only the signal and
//! the socket left out.
//!
//! Not covered: the reflow itself. The strip stays in flow and still takes a
//! line from the pane, because taking it out of flow would hide a row the
//! pane is drawing into. What is fixed here is that the reflow can no longer
//! produce a second refusal, so the cycle has nothing to feed it.

use super::*;
use state::{HistoryWindow, WindowState};

const S: SessionId = SessionId(7);
const LINES: u32 = 10_000;

/// A painted window with nothing older behind it.
fn exhausted(span: u64) -> HistoryWindow {
    HistoryWindow {
        session: Some(S),
        from_seq: 0,
        span,
        more: false,
    }
}

/// A window focused on `S`, painting `history`.
///
/// Assigned rather than built with functional update syntax because
/// `WindowState` has private fields, so `..WindowState::default()` does not
/// compile outside `state`.
#[allow(clippy::field_reassign_with_default)]
fn window(history: HistoryWindow) -> WindowState {
    let mut w = WindowState::default();
    w.focused = Some(S);
    w.history = history;
    w
}

/// One arrival at the top of the buffer, applied to the window it acts on.
///
/// The same two steps `page_back` takes, minus the signal write and the
/// request: plan against what the window already knows, then record.
fn arrive(w: &mut WindowState) -> PageBackPlan {
    let plan = plan_page_back(w.history, w.history_refused, LINES);
    if let PageBackPlan::Refuse(text) = plan {
        record_refusal(w, text);
    }
    plan
}

/// The reported defect: the refusal is stated once, not once per arrival.
///
/// Twenty arrivals is the shape of the loop, not an arbitrary number. Each
/// repaint produced another one, and the operator saw the strip cycle for as
/// long as they left the pane at the top.
#[test]
fn repeated_arrivals_at_the_bottom_of_history_raise_the_notice_once() {
    let mut w = window(exhausted(4096));

    let plans: Vec<PageBackPlan> = (0..20).map(|_| arrive(&mut w)).collect();

    assert_eq!(
        plans[0],
        PageBackPlan::Refuse(NO_OLDER_HISTORY),
        "the first arrival has to answer the gesture"
    );
    assert!(
        plans[1..].iter().all(|p| *p == PageBackPlan::Silent),
        "a refusal already on screen was re-raised: {plans:?}"
    );
    assert_eq!(
        plans
            .iter()
            .filter(|p| matches!(p, PageBackPlan::Refuse(_)))
            .count(),
        1,
        "one refusal per unchanged history window"
    );
    assert_eq!(w.flash, Some(Flash::notice(NO_OLDER_HISTORY)));
}

/// Dismiss ends it. Nothing re-raises it behind the operator's back.
///
/// This is the half the operator could not get: the strip was dismissable and
/// came straight back, which reads as a button that does not work.
#[test]
fn a_dismissed_refusal_stays_dismissed() {
    let mut w = window(exhausted(4096));
    arrive(&mut w);
    assert!(w.flash.is_some());

    // What the strip's Dismiss does.
    w.flash = None;

    for _ in 0..5 {
        assert_eq!(arrive(&mut w), PageBackPlan::Silent);
        assert_eq!(w.flash, None, "a dismissed refusal came back on its own");
    }
}

/// Silence lasts exactly as long as the answer does.
///
/// Suppressing by session, or by a bare flag, would mute a refusal that has
/// become a different fact. The record is the window itself, so a page-back
/// that succeeded and then hit the bottom again is a new question and gets an
/// answer.
#[test]
fn new_history_makes_the_refusal_speak_again() {
    let mut w = window(HistoryWindow {
            session: Some(S),
            from_seq: 200,
            span: 4096,
            more: true,
        });
    assert!(
        matches!(arrive(&mut w), PageBackPlan::Ask(_)),
        "history the daemon still holds is not a refusal"
    );

    // The reply lands: a bigger window, and now there is nothing behind it.
    w.history = exhausted(65_536);
    assert_eq!(arrive(&mut w), PageBackPlan::Refuse(NO_OLDER_HISTORY));
    assert_eq!(arrive(&mut w), PageBackPlan::Silent);

    // Another session's pane is another question.
    w.history = HistoryWindow {
        session: Some(SessionId(8)),
        ..exhausted(65_536)
    };
    assert_eq!(arrive(&mut w), PageBackPlan::Refuse(NO_OLDER_HISTORY));
    assert_eq!(arrive(&mut w), PageBackPlan::Silent);
}

/// The pane's own ceiling is refused on the same terms.
///
/// Both refusals returned without arming the pane and both were reachable on
/// every reflow, so counting one and not the other would leave the identical
/// loop live for anyone who had paged back eight megabytes.
#[test]
fn the_ceiling_refusal_is_counted_too() {
    let mut w = window(HistoryWindow {
            session: Some(S),
            from_seq: 0,
            span: u64::from(wire::PAGE_CEILING_BYTES),
            more: true,
        });

    assert_eq!(arrive(&mut w), PageBackPlan::Refuse(PANE_AT_CEILING));
    for _ in 0..5 {
        assert_eq!(arrive(&mut w), PageBackPlan::Silent);
    }
}

/// A fresh window has refused nothing.
///
/// The record has to start empty, or the first gesture of a session's life
/// would be swallowed and the pane would look inert.
#[test]
fn a_fresh_window_has_refused_nothing() {
    assert_eq!(WindowState::default().history_refused, None);
}
