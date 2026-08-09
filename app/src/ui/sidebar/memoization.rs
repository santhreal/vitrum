//! Does an unchanged session row survive a paint?
//!
//! The sidebar redraws whenever anything in the window changes, and at the
//! load this product is built for that is twenty times a second: the daemon
//! pushes one `SessionUpdated` per live session per second, and twenty live
//! sessions is the stated target. Exactly one row's contents change on each of
//! those pushes. The other nineteen are identical to what is already on
//! screen.
//!
//! [`SessionRow`] takes `PartialEq` props precisely so Dioxus can notice that
//! and skip the row: an untouched row should neither run its body nor be
//! diffed against itself. When that memoization works the client re-renders
//! one row per push; when it silently does not, it re-renders twenty, and the
//! measured cost of the difference is the VDOM half of the frame budget.
//!
//! Nothing else in the suite can see this. Every other sidebar guard reads the
//! HTML of a single paint, and the HTML is byte-identical whether the row was
//! rebuilt or skipped. The only observable is whether the body ran, which is
//! what [`render_count`] counts.

use super::*;
use crate::testkit::{HOUR, NOW, project, row};

/// A window holding `n` running sessions in one project.
fn state_with(n: u64) -> UiState {
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "vitrum")];
    st.daemon.sessions = (0..n)
        .map(|i| {
            row(10 + i)
                .project(1)
                .command("claude")
                .title(&format!("session {i}"))
                .waiting(Some(false))
                .created_at_ms(NOW - HOUR + i)
                .last_activity_ms(NOW - HOUR + i)
                .build()
        })
        .collect();
    st
}

thread_local! {
    /// The live clock signal of the harness currently mounted, so a test can
    /// advance time the way the real render loop does: by handing the sidebar
    /// a new reading and letting it repaint.
    static CLOCK: std::cell::Cell<Option<Signal<i64>>> =
        const { std::cell::Cell::new(None) };

    /// The live window state, so a test can push a real `SessionUpdated` and
    /// see what the sidebar emits for it.
    static STATE: std::cell::Cell<Option<Signal<UiState>>> =
        const { std::cell::Cell::new(None) };
}

#[derive(Props, Clone, PartialEq)]
struct HarnessProps {
    initial: UiState,
}

/// The sidebar under a clock that moves, which is the only difference between
/// this harness and `rendered_sidebar`'s.
#[component]
fn Harness(props: HarnessProps) -> Element {
    let state = use_signal(|| props.initial.clone());
    let millis = use_signal(|| NOW as i64);
    // Nothing staged: the restart band is not what this file measures, and
    // an extra band on every paint is one more thing the row count has to
    // be read past.
    let update_standing = use_signal(crate::update::Standing::default);
    CLOCK.with(|c| c.set(Some(millis)));
    STATE.with(|c| c.set(Some(state)));
    let clock = crate::clock::render_clock(millis(), 0);
    rsx! {
        Sidebar {
            state,
            clock,
            home: "/home/u".to_string(),
            server: "127.0.0.1:7717",
            update_standing,
            on_select: move |_: (SessionId, Click)| {},
            on_close_session: move |_: SessionId| {},
            on_toggle_project: move |_: GroupKey| {},
            on_toggle_section: move |_: (GroupKey, Section)| {},
            on_toggle_preview: move |_: GroupKey| {},
            on_toggle_settled_tail: move |_: GroupKey| {},
            on_toggle_sidebar: move |()| {},
            on_retry: move |()| {},
            on_jump: move |()| {},
            on_new_session: move |_: Option<ProjectId>| {},
            on_launch_now: move |()| {},
            on_filter: move |_: String| {},
            on_menu: move |_: (f64, f64, SessionId)| {},
            on_resize_start: move |_: f64| {},
            on_resize_nudge: move |_: f64| {},
            on_settings: move |()| {},
            on_restart: move |()| {},
        }
    }
}

/// Mount `n` sessions, then advance the clock by `advance_ms` and report how
/// many row bodies ran on the repaint that followed.
///
/// The first paint necessarily renders every row, so it is measured and
/// discarded; the number under test is the second one.
fn rows_rebuilt_after_advancing(n: u64, advance_ms: i64) -> (usize, usize) {
    let mut dom = VirtualDom::new_with_props(
        Harness,
        HarnessProps {
            initial: state_with(n),
        },
    );
    render_count::take();
    dom.rebuild_in_place();
    let first = render_count::take();

    let clock = CLOCK.with(|c| c.get()).expect("harness published its clock");
    dom.in_runtime(|| {
        let mut clock = clock;
        clock.set(NOW as i64 + advance_ms);
    });
    dom.render_immediate(&mut dioxus_core::NoOpMutations);
    (first, render_count::take())
}

/// The first paint has to build rows at all, or the harness is measuring
/// nothing and every delta below is vacuously zero.
///
/// The count is deliberately not pinned to the session count: the Active band
/// draws a preview rather than every row, so twenty sessions mount fewer than
/// twenty rows and pinning the number here would make this file fail whenever
/// [`inbox::PREVIEW_LIMIT`] moved. Every test below measures against the
/// number this paint actually built.
#[test]
fn the_first_paint_builds_rows() {
    let (first, _) = rows_rebuilt_after_advancing(20, 0);
    assert!(
        first > 0,
        "the harness mounted no rows at all, so it can prove nothing"
    );
}

/// A repaint one millisecond later must not rebuild a single row.
///
/// This is the whole contract. The render loop reads the system clock afresh
/// on every paint, so the reading is different every time even when the
/// sidebar is otherwise completely at rest — a repaint provoked by anything at
/// all, in a window where nothing about any session has changed, used to hand
/// all twenty rows a clock they compared unequal against and rebuild all
/// twenty. Nothing on screen differed by so much as a character.
#[test]
fn a_repaint_a_millisecond_later_rebuilds_no_row() {
    let (_, again) = rows_rebuilt_after_advancing(20, 1);
    assert_eq!(
        again, 0,
        "twenty unchanged rows were rebuilt for a clock that moved 1ms"
    );
}

/// The quantum is a whole second, so a paint anywhere inside the same second
/// as the last one is free no matter where in the second it lands.
///
/// 999ms is the interesting one: it is the largest advance that must still be
/// free, and an implementation that rounded to the nearest second instead of
/// flooring to it would rebuild here.
#[test]
fn every_paint_inside_one_second_is_free() {
    for advance in [1, 2, 17, 500, 998, 999] {
        let (_, again) = rows_rebuilt_after_advancing(20, advance);
        assert_eq!(
            again, 0,
            "a paint {advance}ms into the same second rebuilt {again} rows"
        );
    }
}

/// Crossing a second boundary MUST rebuild the rows.
///
/// The other half of the contract, and the half that stops the fix from being
/// "freeze the clock". Every row carries an age that is measured in seconds,
/// so a row that never rebuilds is a row whose timestamp is wrong forever.
/// A quantised clock buys its skips by being exactly as precise as the
/// coarsest thing drawn from it, not by being less.
#[test]
fn crossing_a_second_rebuilds_the_rows() {
    let (first, again) = rows_rebuilt_after_advancing(20, 1_000);
    assert!(first > 0, "the harness mounted no rows");
    assert_eq!(
        again, first,
        "the clock crossed into a new second and {first} rows did not update their age"
    );
}

/// The saving has to scale with the load, not be an artefact of twenty.
///
/// The product's stated target is twenty sessions, but the failure mode is
/// worse the more sessions there are, and a fix that happened to work at one
/// size would be a coincidence.
#[test]
fn the_skip_holds_at_every_load() {
    for n in [1, 5, 20, 60] {
        let (first, again) = rows_rebuilt_after_advancing(n, 1);
        assert!(first > 0, "mounting {n} sessions built no rows");
        assert_eq!(again, 0, "at {n} sessions a 1ms repaint rebuilt {again} rows");
    }
}

/// One session update, with the clock held still: how many row bodies ran and
/// how many DOM edits crossed into the webview.
///
/// The session mutated is the most recently active one, which is the row the
/// Active band is guaranteed to be drawing. Mutating the oldest session
/// measures nothing: it sits past [`inbox::PREVIEW_LIMIT`] and has no row.
fn cost_of_one_update(n: u64) -> (usize, usize) {
    let mut dom = VirtualDom::new_with_props(
        Harness,
        HarnessProps {
            initial: state_with(n),
        },
    );
    dom.rebuild_in_place();
    let state = STATE.with(|c| c.get()).expect("harness published its state");
    render_count::take();

    let mut edits = 0usize;
    for bump in 1..=REPEATS {
        dom.in_runtime(|| {
            let mut state = state;
            let mut next = state.read().clone();
            if let Some(session) = next.daemon.sessions.last_mut() {
                session.info.title = format!("session moved {bump}");
            }
            state.set(next);
        });
        let mut recorder = dioxus_core::Mutations::default();
        dom.render_immediate(&mut recorder);
        edits += recorder.edits.len();
    }
    (render_count::take(), edits)
}

/// Updates averaged over, so a one-off first-repaint effect cannot dominate.
const REPEATS: usize = 20;

/// The DOM edits one title change is allowed to cost.
///
/// Measured at 3.45 on average: the title text node, and the tooltip that
/// repeats it. The bound is loose enough not to fail on a legitimate extra
/// attribute and tight enough that replacing the row, or the list, blows it.
const EDIT_BUDGET: usize = 8;

/// One session update must cost one row, whatever the load.
///
/// This is the property the whole file exists for, and it is stated as
/// load-independence on purpose. The ways of defeating row memoization — a
/// prop that is freshly allocated each paint, a handler that compares
/// unequal, another live clock reaching the row — turn the cost of one
/// update from one row into every row on screen. That is invisible in a
/// count taken at a single size and obvious the moment the same count is
/// taken at two.
///
/// Two things it does NOT catch, stated because a guard that is trusted for
/// more than it proves is worse than no guard. It cannot see a change that
/// makes every update cost two rows at every load;
/// `one_update_rebuilds_exactly_one_row` is the half that pins the constant.
/// And it does not defend the rows' `key`: removing both keys was measured
/// against this file and changed no count in it, because the bands are not
/// reordered by anything these tests do. Treat the keys as untested here.
#[test]
fn the_cost_of_one_update_does_not_grow_with_the_session_count() {
    let (small_rows, small_edits) = cost_of_one_update(20);
    let (large_rows, large_edits) = cost_of_one_update(60);
    assert_eq!(
        small_rows, large_rows,
        "tripling the sessions changed the rows rebuilt per update          from {small_rows} to {large_rows}, so a row is being rebuilt for          something other than its own contents"
    );
    assert_eq!(
        small_edits, large_edits,
        "tripling the sessions changed the DOM edits per update from          {small_edits} to {large_edits}"
    );
}

/// One update rebuilds exactly one row and emits a handful of edits.
///
/// The constant that load-independence alone cannot pin. Twenty updates to one
/// session's title must run twenty row bodies — one each — and nothing else on
/// screen has changed, so nothing else may be rebuilt or repainted.
#[test]
fn one_update_rebuilds_exactly_one_row() {
    let (rows, edits) = cost_of_one_update(20);
    assert_eq!(
        rows, REPEATS,
        "{REPEATS} updates to one session rebuilt {rows} row bodies, not {REPEATS}"
    );
    assert!(
        edits <= REPEATS * EDIT_BUDGET,
        "{REPEATS} title changes emitted {edits} DOM edits, over the          {EDIT_BUDGET}-per-update budget"
    );
}

/// A window of `n` sessions whose last activity is `age_ms` in the past.
///
/// [`state_with`] pins its rows an hour old, which puts them exactly on the
/// `59m`/`1h` threshold: advancing one second there genuinely changes what
/// every row says. That is the right fixture for the tests above and the wrong
/// one for the tests below, which are about rows with nothing to say.
fn state_aged(n: u64, age_ms: u64) -> UiState {
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "vitrum")];
    st.daemon.sessions = (0..n)
        .map(|i| {
            row(10 + i)
                .project(1)
                .command("claude")
                .title(&format!("session {i}"))
                .waiting(Some(false))
                .created_at_ms(NOW - age_ms)
                .last_activity_ms(NOW - age_ms)
                .build()
        })
        .collect();
    st
}

/// Mount `initial`, then advance the clock in `steps` one-second ticks, and
/// report how many row bodies ran across all of them.
fn rows_rebuilt_over(initial: UiState, steps: i64) -> usize {
    let mut dom = VirtualDom::new_with_props(Harness, HarnessProps { initial });
    render_count::take();
    dom.rebuild_in_place();
    render_count::take();

    let clock = CLOCK.with(|c| c.get()).expect("harness published its clock");
    let mut total = 0;
    for step in 1..=steps {
        dom.in_runtime(|| {
            let mut clock = clock;
            clock.set(NOW as i64 + step * 1_000);
        });
        dom.render_immediate(&mut dioxus_core::NoOpMutations);
        total += render_count::take();
    }
    total
}

/// WHY: the second-quantised clock stopped rows rebuilding WITHIN a second and
/// left them rebuilding on every second boundary, forever, whether or not they
/// had anything new to say. A row reading `5h ago` repeats that answer 3600
/// times before one character of it changes, and at twenty sessions that is
/// twenty row bodies a second for nothing.
///
/// The row clock is floored per row now, so this asserts the consequence: a
/// minute of paints over rows with no pending anything must rebuild NOTHING.
/// Sixty boundaries, twenty rows, zero rebuilds.
///
/// The class is "time passing costs work in a row where nothing time-driven
/// is happening". It is stated as a total over many boundaries rather than
/// one, because a single-boundary check cannot tell a row that is genuinely
/// stable from one that happens to straddle a threshold.
#[test]
fn a_minute_of_paints_over_settled_rows_rebuilds_nothing() {
    // 5h07m: comfortably inside the hour bucket, and deliberately not on a
    // boundary, so nothing these rows draw changes for another 53 minutes.
    let aged = state_aged(20, 5 * HOUR as u64 + 7 * 60_000);
    assert_eq!(
        rows_rebuilt_over(aged, 60),
        0,
        "sixty second-boundaries rebuilt rows that had nothing new to say"
    );
}

/// The other half, and the half that stops the change from being "freeze old
/// rows": when an aged row's own label finally turns over, it MUST rebuild.
///
/// Without this, the test above is satisfied by a clock that never moves.
#[test]
fn an_aged_row_rebuilds_when_its_own_hour_turns_over() {
    // One second short of six hours, so a single tick crosses `5h` into `6h`.
    let aged = state_aged(20, 6 * HOUR as u64 - 1_000);
    let (first, again) = rebuilt_after(aged, 1_000);
    assert!(first > 0, "the harness mounted no rows");
    assert_eq!(
        again, first,
        "an aged row crossed its own label boundary and {first} rows did not update"
    );
}

/// A row with a live timer keeps its per-second clock.
///
/// The floor is allowed to coarsen only what cannot be told apart. A working
/// row draws an elapsed counter, so every second is a real change and skipping
/// it would stop the timer on screen.
#[test]
fn a_working_row_still_rebuilds_every_second() {
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "vitrum")];
    st.daemon.sessions = (0..4)
        .map(|i| {
            row(10 + i)
                .project(1)
                .command("claude")
                .title(&format!("session {i}"))
                .hint(vitrum_proto::HintState::Working, None, NOW - 30_000)
                .created_at_ms(NOW - 5 * HOUR as u64)
                .last_activity_ms(NOW - 5 * HOUR as u64)
                .build()
        })
        .collect();
    let (first, again) = rebuilt_after(st, 1_000);
    assert!(first > 0, "the harness mounted no rows");
    assert_eq!(
        again, first,
        "a working row's elapsed timer stopped: {first} mounted, {again} rebuilt"
    );
}

/// [`rows_rebuilt_after_advancing`] against an arbitrary window rather than
/// the hour-old default.
fn rebuilt_after(initial: UiState, advance_ms: i64) -> (usize, usize) {
    let mut dom = VirtualDom::new_with_props(Harness, HarnessProps { initial });
    render_count::take();
    dom.rebuild_in_place();
    let first = render_count::take();

    let clock = CLOCK.with(|c| c.get()).expect("harness published its clock");
    dom.in_runtime(|| {
        let mut clock = clock;
        clock.set(NOW as i64 + advance_ms);
    });
    dom.render_immediate(&mut dioxus_core::NoOpMutations);
    (first, render_count::take())
}
