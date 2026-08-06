//! The workspace switcher: a control in the titlebar, and a strip under it
//! that only exists when there is something to switch between.
//!
//! A workspace is the top-level partition, above projects. Every session
//! belongs to exactly one, which is what makes a new workspace genuinely blank
//! rather than "blank until something you already had drifts into it". The
//! model for all of that is [`crate::state::WorkspaceSet`]; this file is the
//! switcher, and the three view decisions that go with it.
//!
//! # The three decisions this file owns
//!
//! **When the strip is worth a band at all.** [`strip_visible`]. A full-width
//! horizontal band across a 3840px window, holding one chip, in the state
//! every user is in on their first launch and most users are in forever, is
//! not a switcher: it is a band spent on a control with nothing to control.
//! So the strip is drawn when the operator opened it, or when a second
//! workspace exists and there is genuinely something to switch between.
//! Otherwise the whole affordance is [`WorkspaceSwitcher`], which lives in the
//! titlebar and costs no vertical space at all.
//!
//! **What a chip says when there is no room to say much.** Collapsed, a chip
//! is a name and, if the workspace is not resting, a count. It is never a bare
//! badge: a circle with a number in it and no name is a notification, not a
//! place you can go. The count comes from [`badge`], which folds the
//! workspace's sessions through [`vitrum_model::rollup::rollup_rows`] — the
//! same fold a collapsed project header uses, applied to a bigger bucket,
//! which is exactly what [`ProjectRollup`] was built to generalise over. Three
//! tiers collapse to two visual states because a chip cannot carry five, and
//! the collapse is by severity so the loud state is never masked by a quiet
//! one.
//!
//! **Where management lives.** Create is here, because it is one click and
//! wants no name. Rename, delete, reorder and folders are in
//! `Settings > Workspaces`, one click away through the same strip. Inline text
//! editing in a 32px band across the top of the window is worse at every one
//! of those jobs than a sheet is, and the strip is on screen while the sheet
//! is not.
//!
//! # Cost
//!
//! The strip is rendered from one pass over the session list per paint, not
//! one pass per workspace: [`chips`] buckets first and folds second. With
//! twenty sessions and four workspaces that is twenty visits rather than
//! eighty, and more to the point it stays twenty as workspaces are added. The
//! titlebar switcher folds only the ACTIVE workspace's rows, through
//! [`crate::state::DaemonState::workspace_rows`], because that is the one
//! number it draws. Nothing here runs while the window idles.

use dioxus::prelude::*;
use vitrum_model::{Clock, DispositionPolicy, ProjectRollup, SidebarStatus, rollup::rollup_rows};
use vitrum_proto::ProjectId;

use crate::inbox;
use crate::state::{UiState, WorkspaceId};

/// One workspace as the bar draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct Chip {
    pub id: WorkspaceId,
    pub name: String,
    /// The workspace this window is looking at.
    pub active: bool,
    /// Where an unfiled session will land.
    pub intake: bool,
    /// Every session filed here, folded.
    pub rollup: ProjectRollup,
}

/// What a chip shows in the corner, and how loudly.
///
/// `None` is a resting workspace, and it draws NOTHING rather than a zero. A
/// row of chips each wearing a grey `0` is four pieces of furniture that say
/// the same thing as blank space, and it destroys the one property the badge
/// has to have: that a lit chip is visible in peripheral vision while you are
/// looking at a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    pub count: usize,
    /// Something in there is blocked or dead. The chip should be unmissable.
    pub urgent: bool,
    /// The full breakdown, for the chip's tooltip.
    pub title: String,
}

/// Statuses that mean a human is the blocker.
///
/// Approval and Input are the agent asking a question; Failed is a corpse
/// nobody has acknowledged. All three are "you, now", and none of the other two
/// statuses is.
const URGENT: [SidebarStatus; 3] = [
    SidebarStatus::Approval,
    SidebarStatus::Input,
    SidebarStatus::Failed,
];

/// The badge for one workspace, or `None` when it is resting.
///
/// Severity order, and the order is the whole design:
///
/// 1. **Urgent** — approvals, input prompts, failures. Anything here outranks
///    everything below it, because these are the states where the agent has
///    stopped and is waiting on a person.
/// 2. **Unseen completions** — finished while you were not looking. Actionable,
///    but nothing is blocked on it.
/// 3. **Working** — the resting state of a busy workspace. Worth a number so
///    the bar distinguishes "four agents running" from "empty", but never
///    urgent: an agent doing its job is not a request.
///
/// Collapsing three tiers onto one `urgent` flag is deliberate. The chip is too
/// small for a five-colour ramp, and the distinction that survives at that size
/// is binary: does this need me. The tooltip carries the full breakdown for
/// anyone who wants it, built by [`inbox::rollup_title`] so the bar and the
/// collapsed project header say the same thing about the same numbers.
#[must_use]
pub fn badge(rollup: &ProjectRollup) -> Option<Badge> {
    let title = inbox::rollup_title(rollup);
    let urgent: usize = URGENT.iter().map(|s| rollup.counts.get(*s)).sum();
    if urgent > 0 {
        return Some(Badge {
            count: urgent,
            urgent: true,
            title,
        });
    }
    if rollup.unseen_completions > 0 {
        return Some(Badge {
            count: rollup.unseen_completions,
            urgent: false,
            title,
        });
    }
    let working = rollup.counts.get(SidebarStatus::Working);
    if working > 0 {
        return Some(Badge {
            count: working,
            urgent: false,
            title,
        });
    }
    None
}

/// Every workspace, in display order, each folded over its own sessions.
///
/// One pass over the session list, not one per workspace. The list is bucketed
/// by [`crate::state::WorkspaceSet::workspace_of`] first and each bucket folded
/// once, so the cost is linear in sessions and independent of how many
/// workspaces exist.
///
/// The `ProjectId` handed to [`rollup_rows`] is the workspace id wearing a
/// different hat. It is only a label on the returned value — the fold does no
/// filtering of its own, which is precisely why `rollup_rows` exists as the
/// unfiltered core under `rollup_project` — and nothing downstream reads it.
#[must_use]
pub fn chips(state: &UiState, clock: Clock, policy: DispositionPolicy) -> Vec<Chip> {
    let workspaces = &state.daemon.workspaces;
    let mut buckets: Vec<(WorkspaceId, Vec<&vitrum_model::SessionView>)> =
        workspaces.iter().map(|w| (w.id, Vec::new())).collect();

    for row in &state.daemon.sessions {
        let home = workspaces.workspace_of(&row.info);
        if let Some((_, rows)) = buckets.iter_mut().find(|(id, _)| *id == home) {
            rows.push(row);
        }
    }

    let active = state.window.workspace;
    let intake = workspaces.intake();
    workspaces
        .iter()
        .zip(buckets)
        .map(|(workspace, (_, rows))| Chip {
            id: workspace.id,
            name: workspace.display_name().to_string(),
            active: workspace.id == active,
            intake: workspace.id == intake,
            rollup: rollup_rows(ProjectId(workspace.id.0), rows, clock, policy),
        })
        .collect()
}

// The name for a one-click workspace comes from
// [`crate::state::WorkspaceSet::suggested_name`], not from here. The `+` button
// takes no name on purpose — naming is a decision, and demanding one before you
// can even see the empty workspace puts a text field in the way of a gesture
// that should be instant — but "what is a free workspace called" is a question
// about the set, and the set owns it. This module had its own copy of that rule
// for a while and two implementations of one naming rule is exactly the drift
// we keep removing from this codebase.

/// Tooltip for one chip.
///
/// Names the workspace and what is in it, because a collapsed chip is a glyph
/// and a number and the operator needs some way to find out which is which
/// without expanding the bar and losing their place.
#[must_use]
pub fn chip_title(chip: &Chip) -> String {
    let mut title = chip.name.clone();
    title.push_str(" \u{2014} ");
    title.push_str(&inbox::rollup_title(&chip.rollup));
    if chip.intake {
        title.push_str(" \u{00b7} new sessions land here");
    }
    title
}

/// Does the strip under the titlebar earn its band right now?
///
/// The rule is one line, and it used to be two:
///
/// **The strip draws when the operator has it open. That is all.**
///
/// It previously read `open || workspaces > 1`, which meant collapsing the
/// bar did nothing at all once a second workspace existed. The operator hit
/// collapse, the band stayed, and the only state where the control worked was
/// the one state where the band was not worth collapsing. That was reported as
/// a defect in exactly those words: collapse should collapse the workspaces so
/// you do not see that bar.
///
/// The reasoning that produced the old second clause was sound about a
/// DIFFERENT question. A full-width band holding one chip, in the state every
/// user is in on day one, is a band spent on a switcher with nothing to switch
/// between, and it was one of the six stacked bands this window used to open
/// with. That is an argument about the DEFAULT, not about what collapse means,
/// and it is answered by `workspace_bar_open` defaulting to false rather than
/// by overriding the operator afterwards.
///
/// Nothing becomes unreachable. [`WorkspaceSwitcher`] sits in the titlebar,
/// names the current workspace, carries the attention badge that made a lit
/// chip worth seeing, and one click opens the strip again.
#[must_use]
pub fn strip_visible(open: bool) -> bool {
    open
}

#[derive(Props, Clone, PartialEq)]
pub struct WorkspaceSwitcherProps {
    pub state: Signal<UiState>,
    pub clock: Clock,
}

/// The workspace control that lives in the titlebar.
///
/// Always present, costs no band, and names the workspace you are in. It folds
/// only the ACTIVE workspace's rows rather than every workspace's, because the
/// one number it draws is the one for the workspace it names; the strip below
/// is what pays for the rest, and only when it is on screen.
/// Does the titlebar's workspace control print the workspace's name?
///
/// Only when there is a choice to name. One workspace is not one the operator
/// picked, it is the one that had to exist, so its name states a decision
/// nobody made. Measured, "Default" was the highest-contrast text in the whole
/// window at 14.43:1 against 4.22:1 for the product's own name: the loudest
/// thing on screen was the least meaningful thing on it.
///
/// The control itself always draws, because it is also how the strip is opened
/// to create a second workspace. Only the word goes.
#[must_use]
pub fn names_the_workspace(total: usize) -> bool {
    total > 1
}

#[component]
pub fn WorkspaceSwitcher(props: WorkspaceSwitcherProps) -> Element {
    let mut state = props.state;
    let (open, name, mark, total) = {
        let read = state.read();
        let id = read.window.workspace;
        let name = read
            .daemon
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .map_or_else(|| "Workspace".to_string(), |w| w.display_name().to_string());
        let rollup = rollup_rows(
            ProjectId(id.0),
            read.daemon.workspace_rows(id),
            props.clock,
            read.daemon.settings.policy,
        );
        (
            read.window.workspace_bar_open,
            name,
            badge(&rollup),
            read.daemon.workspaces.len(),
        )
    };
    let hint = match (open, total) {
        (true, _) => "Hide the workspace strip",
        (false, 1) => "Workspaces \u{2014} show the strip to add another",
        (false, _) => "Show the workspace strip",
    };
    let title = match &mark {
        Some(mark) => format!("{name} \u{2014} {}\n{hint}", mark.title),
        None => format!("{name} \u{2014} nothing running\n{hint}"),
    };

    rsx! {
        button {
            class: if open { "rg-wsw rg-wsw--open" } else { "rg-wsw" },
            r#type: "button",
            title: "{title}",
            aria_expanded: if open { "true" } else { "false" },
            aria_label: "Workspace: {name}",
            // The titlebar is a drag region. Without this a press on the
            // switcher hands the gesture to the window manager and the button
            // never fires.
            onmousedown: move |e| e.stop_propagation(),
            onclick: move |_| {
                {
                    let mut write = state.write();
                    write.window.workspace_bar_open = !write.window.workspace_bar_open;
                }
                crate::ui::settings::commit(&state.peek());
            },
            // The markup owns the glyph, as it does for the project chevron:
            // disclosure state must not depend on a transform landing.
            span { class: "rg-wsw__chevron", if open { "\u{25BE}" } else { "\u{25B8}" } }
            // The NAME only when there is a choice to name.
            //
            // One workspace is not a workspace the operator picked, it is the
            // one that had to exist, and printing "Default" beside the
            // product name states a fact nobody chose and nobody can act on.
            // Measured, it was the loudest text in the window: 14.43:1
            // against 4.22:1 for the product's own name. The control stays,
            // because it is also how the strip is opened to make a second
            // one; only the word goes.
            if names_the_workspace(total) {
                span { class: "rg-wsw__name", "{name}" }
            }
            // And the COUNT only when there is a choice, for the same reason.
            //
            // With one workspace this badge and the sidebar's own attention
            // chip are the same number about the same sessions, on screen at
            // once: "1" here and "1 waiting" ten pixels below it. The sidebar's
            // is the one worth keeping, because it is clickable, it says what
            // the number means, and it sits beside the rows it counts. With
            // two or more workspaces this one starts answering a different
            // question -- how much is waiting in the one you are LOOKING at --
            // and earns its place again.
            if names_the_workspace(total)
                && let Some(mark) = mark
            {
                span {
                    class: if mark.urgent { "rg-wsw__count rg-wsw__count--urgent" } else { "rg-wsw__count" },
                    "{mark.count}"
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct WorkspaceBarProps {
    pub state: Signal<UiState>,
    pub clock: Clock,
    /// Open `Settings > Workspaces`, where rename, delete, reorder and folders
    /// live.
    pub on_manage: EventHandler<()>,
}

#[component]
pub fn WorkspaceBar(props: WorkspaceBarProps) -> Element {
    let mut state = props.state;
    let (open, policy) = {
        let read = state.read();
        (read.window.workspace_bar_open, read.daemon.settings.policy)
    };
    let entries = chips(&state.read(), props.clock, policy);
    if !strip_visible(open) {
        return rsx! {};
    }

    rsx! {
        div {
            class: if open { "rg-wsbar" } else { "rg-wsbar rg-wsbar--collapsed" },
            role: "tablist",
            aria_label: "Workspaces",

            for chip in entries.iter() {
                {
                    let id = chip.id;
                    let mark = badge(&chip.rollup);
                    rsx! {
                        button {
                            class: if chip.active { "rg-wsbar__item rg-wsbar__item--active" } else { "rg-wsbar__item" },
                            key: "{id.0}",
                            r#type: "button",
                            role: "tab",
                            aria_selected: if chip.active { "true" } else { "false" },
                            title: chip_title(chip),
                            onclick: move |_| {
                                let now = crate::tick().now_ms;
                                let outcome = state.write().set_workspace(id, now);
                                match outcome {
                                    Ok(()) => crate::ui::settings::commit(&state.peek()),
                                    Err(why) => {
                                        state.write().window.flash = Some(crate::state::Flash::error(why.to_string()));
                                    }
                                }
                            },
                            // The name is never dropped, in either state. A
                            // chip that is only a badge is a notification the
                            // operator cannot read, which is precisely what
                            // the collapsed bar used to be.
                            span { class: "rg-wsbar__name", "{chip.name}" }
                            if let Some(mark) = mark {
                                span {
                                    class: if mark.urgent { "rg-wsbar__count rg-wsbar__count--urgent" } else { "rg-wsbar__count" },
                                    "{mark.count}"
                                }
                            }
                        }
                    }
                }
            }

            // Grouped with the chips, not thrown to the far edge. On a 3840px
            // window an actions cluster pinned right is separated from the
            // thing it acts on by two thousand pixels of nothing.
            if open {
                span { class: "rg-wsbar__rule" }
                button {
                    class: "rg-wsbar__add",
                    r#type: "button",
                    title: "Create a workspace. It starts empty; new sessions land in whichever workspace you are looking at.",
                    onclick: move |_| {
                        let name = state.peek().daemon.workspaces.suggested_name();
                        let created = state.write().create_workspace(&name);
                        match created {
                            Ok(id) => {
                                // Switch to it immediately. A workspace you
                                // created and cannot see is indistinguishable
                                // from a button that did nothing, and the
                                // blank sidebar IS the confirmation.
                                let now = crate::tick().now_ms;
                                let _ = state.write().set_workspace(id, now);
                                crate::ui::settings::commit(&state.peek());
                            }
                            Err(why) => {
                                state.write().window.flash = Some(crate::state::Flash::error(why.to_string()));
                            }
                        }
                    },
                    span { class: "rg-wsbar__add-glyph", "+" }
                    "New workspace"
                }
                button {
                    class: "rg-wsbar__manage",
                    r#type: "button",
                    title: "Rename, delete, reorder and folders",
                    onclick: move |_| props.on_manage.call(()),
                    "Manage"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::UiState;
    use vitrum_model::{SessionView, StatusCounts};
    use vitrum_proto::{Attention, SessionInfo, SessionStatus};

    fn clock() -> Clock {
        Clock::utc(1_000_000)
    }

    fn info(id: u64, cwd: &str) -> SessionInfo {
        SessionInfo {
            id: vitrum_proto::SessionId(id),
            project_id: ProjectId(1),
            title: format!("session {id}"),
            cwd: cwd.to_string(),
            command: "claude".to_string(),
            args: Vec::new(),
            status: SessionStatus::Running,
            created_at_ms: 1_000 + id,
            last_activity_ms: 999_000,
            cols: 80,
            rows: 24,
            git_branch: None,
            unread: false,
            attention: Attention::default(),
            hint: None,
        }
    }

    fn rollup_of(counts: StatusCounts, unseen: usize) -> ProjectRollup {
        ProjectRollup {
            project_id: ProjectId(1),
            indicator: counts.most_urgent(),
            counts,
            settled: 0,
            snoozed: 0,
            woke: 0,
            unseen_completions: unseen,
            total: counts.total(),
        }
    }

    fn counts(of: &[(SidebarStatus, usize)]) -> StatusCounts {
        let mut out = StatusCounts::default();
        for (status, n) in of {
            for _ in 0..*n {
                out.add(*status);
            }
        }
        out
    }

    /// A resting workspace draws no badge at all. A row of chips each wearing
    /// a grey zero is four pieces of furniture saying the same thing as blank
    /// space, and it destroys the property the badge exists for: that a lit
    /// chip is visible while you are looking somewhere else.
    #[test]
    fn a_resting_workspace_has_no_badge() {
        assert_eq!(badge(&ProjectRollup::empty(ProjectId(1))), None);
        assert_eq!(badge(&rollup_of(counts(&[]), 0)), None);
    }

    /// Working agents get a plain count, never the urgent treatment. An agent
    /// doing its job is not a request, and lighting the chip for it would mean
    /// the bar is lit permanently and therefore says nothing.
    #[test]
    fn a_busy_workspace_is_counted_but_not_urgent() {
        let mark = badge(&rollup_of(counts(&[(SidebarStatus::Working, 4)]), 0))
            .expect("four running agents are worth a number");
        assert_eq!(mark.count, 4);
        assert!(!mark.urgent, "a working agent is not a request");
    }

    /// Every state that means a human is the blocker must light the chip.
    /// Missing one of the three would hide exactly the workspace the operator
    /// needed to switch to.
    #[test]
    fn every_blocking_state_lights_the_chip() {
        for status in URGENT {
            let mark = badge(&rollup_of(counts(&[(status, 1)]), 0))
                .unwrap_or_else(|| panic!("{status:?} produced no badge"));
            assert!(mark.urgent, "{status:?} did not light the chip");
            assert_eq!(mark.count, 1);
        }
    }

    /// Urgency counts every blocking session, not just the most urgent one.
    /// Showing `1` for a workspace with two approvals and a failure would send
    /// the operator in believing there is one thing to do.
    #[test]
    fn the_urgent_count_is_every_blocked_session() {
        let mark = badge(&rollup_of(
            counts(&[
                (SidebarStatus::Approval, 2),
                (SidebarStatus::Failed, 1),
                (SidebarStatus::Working, 5),
            ]),
            0,
        ))
        .expect("three blocked sessions");
        assert_eq!(
            mark.count, 3,
            "working sessions leaked into the urgent count"
        );
        assert!(mark.urgent);
    }

    /// Urgent outranks quieter tiers. A workspace with one approval and nine
    /// working agents must read as "one thing needs you", not "nine things are
    /// fine".
    #[test]
    fn urgency_is_never_masked_by_a_quieter_tier() {
        let mark = badge(&rollup_of(
            counts(&[(SidebarStatus::Approval, 1), (SidebarStatus::Working, 9)]),
            7,
        ))
        .expect("an approval is always worth a badge");
        assert_eq!(mark.count, 1);
        assert!(mark.urgent);
    }

    /// An unseen completion outranks working. Something that finished while
    /// you were away is actionable; something still running is not.
    #[test]
    fn an_unseen_completion_outranks_a_working_agent() {
        let mark = badge(&rollup_of(counts(&[(SidebarStatus::Working, 3)]), 2))
            .expect("two unseen completions");
        assert_eq!(mark.count, 2);
        assert!(!mark.urgent, "a completion is not a block");
    }

    /// The tooltip must name the states, not just the total. "3" on a chip is
    /// unactionable; "2 working, 1 failed" tells you whether to switch.
    #[test]
    fn the_badge_tooltip_breaks_the_number_down() {
        let mark = badge(&rollup_of(
            counts(&[(SidebarStatus::Working, 2), (SidebarStatus::Failed, 1)]),
            0,
        ))
        .expect("a failure is worth a badge");
        assert!(mark.title.contains("failed"), "{}", mark.title);
        assert!(mark.title.contains("working"), "{}", mark.title);
    }

    /// A generated name must not collide with one already taken, or the bar
    /// grows two chips the operator cannot tell apart, and it must reuse a
    /// hole rather than counting past it: deleting "Workspace 2" and adding
    /// one must give "Workspace 2" again, not "Workspace 4".
    ///
    /// Asserted here as well as in `state.rs` deliberately. The rule belongs to
    /// the set, but the `+` button in this bar is the only caller that ever
    /// exercises it in anger, and this test is what says the bar still depends
    /// on it holding.
    #[test]
    fn a_generated_name_never_collides_and_reuses_gaps() {
        let mut st = UiState::default();
        assert_eq!(st.daemon.workspaces.suggested_name(), "Workspace 2");

        let second = st.create_workspace("Workspace 2").expect("a valid name");
        let third = st.create_workspace("Workspace 3").expect("a valid name");
        assert_eq!(st.daemon.workspaces.suggested_name(), "Workspace 4");

        st.delete_workspace(second, 1_000).expect("it is empty");
        assert_eq!(
            st.daemon.workspaces.suggested_name(),
            "Workspace 2",
            "the freed number was skipped, so the bar would number its own button presses"
        );
        assert!(st.daemon.workspaces.contains(third));
    }

    /// A brand-new workspace must be genuinely empty: no sessions, no badge,
    /// nothing inherited from the workspace it was created beside. This is the
    /// property the whole feature rests on.
    #[test]
    fn a_new_workspace_starts_blank() {
        let mut st = UiState::default();
        st.daemon.sessions = vec![
            SessionView::new(info(1, "/home/mk/a")),
            SessionView::new(info(2, "/home/mk/b")),
        ];
        st.daemon
            .workspaces
            .adopt(st.daemon.sessions.iter().map(|row| &row.info));

        let created = st.create_workspace("Review").expect("a valid name");
        st.set_workspace(created, 1_000_000).expect("it exists");

        let bar = chips(&st, clock(), DispositionPolicy::default());
        let fresh = bar
            .iter()
            .find(|chip| chip.id == created)
            .expect("the new workspace is on the bar");
        assert_eq!(fresh.rollup.total, 0, "sessions followed the operator in");
        assert_eq!(badge(&fresh.rollup), None, "a blank workspace was lit");
        assert!(fresh.active, "the bar did not follow the switch");

        let old = bar
            .iter()
            .find(|chip| !chip.active)
            .expect("the original workspace is still there");
        assert_eq!(old.rollup.total, 2, "the original workspace lost its rows");
    }

    /// Every session must be counted exactly once across the whole bar. A
    /// session counted twice inflates two badges; a session counted nowhere is
    /// a row the operator cannot find from the bar at all.
    #[test]
    fn every_session_is_counted_in_exactly_one_workspace() {
        let mut st = UiState::default();
        st.daemon.sessions = (1..=6)
            .map(|id| SessionView::new(info(id, "/home/mk/src")))
            .collect();
        st.daemon
            .workspaces
            .adopt(st.daemon.sessions.iter().map(|row| &row.info));

        let second = st.create_workspace("Second").expect("a valid name");
        let moved = st
            .move_to_workspace(
                &[vitrum_proto::SessionId(2), vitrum_proto::SessionId(4)],
                second,
                1_000_000,
            )
            .expect("both sessions exist");
        assert_eq!(moved, 2);

        let bar = chips(&st, clock(), DispositionPolicy::default());
        let counted: usize = bar.iter().map(|chip| chip.rollup.total).sum();
        assert_eq!(
            counted,
            st.daemon.sessions.len(),
            "sessions were double-counted or dropped across {bar:?}"
        );
        assert_eq!(
            bar.iter()
                .find(|chip| chip.id == second)
                .expect("the second workspace is on the bar")
                .rollup
                .total,
            2
        );
    }

    /// The bar must list workspaces in the operator's order, so reordering in
    /// the sheet is visible in the strip.
    #[test]
    fn the_bar_follows_the_operators_order() {
        let mut st = UiState::default();
        let second = st.create_workspace("Second").expect("a valid name");
        let third = st.create_workspace("Third").expect("a valid name");

        let before: Vec<WorkspaceId> = chips(&st, clock(), DispositionPolicy::default())
            .iter()
            .map(|chip| chip.id)
            .collect();
        assert_eq!(before, vec![before[0], second, third]);

        st.daemon
            .workspaces
            .move_to(third, 0)
            .expect("the index is in range");
        let after: Vec<WorkspaceId> = chips(&st, clock(), DispositionPolicy::default())
            .iter()
            .map(|chip| chip.id)
            .collect();
        assert_eq!(after[0], third, "the bar ignored the reorder");
    }

    /// Exactly one chip is ever active, and it is the one this window is
    /// looking at. Two active chips, or none, means the strip has stopped
    /// being a switcher.
    #[test]
    fn exactly_one_chip_is_active() {
        let mut st = UiState::default();
        let second = st.create_workspace("Second").expect("a valid name");
        st.set_workspace(second, 1_000_000).expect("it exists");

        let bar = chips(&st, clock(), DispositionPolicy::default());
        assert_eq!(bar.iter().filter(|chip| chip.active).count(), 1);
        assert!(
            bar.iter()
                .find(|chip| chip.active)
                .is_some_and(|chip| chip.id == second)
        );
    }

    /// The bar must say where a new session will land. Without it, "I created
    /// an agent and it went somewhere else" has no explanation on screen.
    #[test]
    fn the_bar_marks_where_new_sessions_land() {
        let mut st = UiState::default();
        let second = st.create_workspace("Second").expect("a valid name");
        st.set_workspace(second, 1_000_000).expect("it exists");

        let bar = chips(&st, clock(), DispositionPolicy::default());
        let intake = bar
            .iter()
            .find(|chip| chip.intake)
            .expect("something must be the intake");
        assert_eq!(
            intake.id, second,
            "switching workspace did not move the intake, so a new agent would land out of sight"
        );
        assert!(chip_title(intake).contains("new sessions land here"));
    }

    /// The band rule, and it is the reason this window opens with two rows of
    /// chrome instead of six.
    ///
    /// One workspace and the strip closed is the state every operator is in on
    /// their first launch and most are in permanently. A full-width band
    /// across a 3840px window holding a single chip is a band spent on a
    /// switcher with nothing to switch between; the affordance for that case
    /// is `WorkspaceSwitcher`, in the titlebar, which costs no height at all.
    #[test]
    fn one_workspace_and_a_closed_strip_draws_no_band() {
        assert!(
            !strip_visible(false),
            "collapse must collapse: the band has to go away"
        );
        assert!(
            strip_visible(true),
            "the open strip is where New workspace lives, so it must draw"
        );
        // The regression this locks out, in the exact words it was reported
        // in: collapse should collapse the workspaces so you do not see that
        // bar. The old rule was `open || workspaces > 1`, so the control did
        // nothing in every state where it mattered, and worked only in the
        // one state where the band was already worth keeping.
        for workspaces in [1usize, 2, 4, 40] {
            assert!(
                !strip_visible(false),
                "collapsed with {workspaces} workspaces still drew the band"
            );
        }
    }
}

/// What the titlebar's workspace control SAYS, which is nothing until there is
/// a choice.
///
/// The question was why a workspace spawns in with
/// defaults. A first-run window has exactly one workspace because one had
/// to exist, and printing its name beside the product name announces a
/// decision nobody made. Measured, "Default" was the highest-contrast text in
/// the whole window at 14.43:1, against 4.22:1 for the product's own name: the
/// loudest thing on screen was the least meaningful.
#[cfg(test)]
mod the_workspace_control_says_nothing_until_it_must {
    use super::*;

    /// One workspace draws no name.
    #[test]
    fn a_single_workspace_is_not_announced() {
        assert!(
            !names_the_workspace(1),
            "a window with one workspace still prints its name"
        );
    }

    /// Two or more draws it, because now it is telling you which.
    #[test]
    fn a_second_workspace_makes_the_name_worth_printing() {
        for total in [2usize, 3, 40] {
            assert!(
                names_the_workspace(total),
                "with {total} workspaces the operator cannot tell which they are in"
            );
        }
    }

    /// Zero is not a state the product reaches, and must not draw either.
    ///
    /// `WorkspaceSet` always holds at least one, but a count that reached the
    /// UI as zero would mean the set was empty or unreadable, and inventing a
    /// name for it would be a confident wrong answer.
    #[test]
    fn an_empty_set_draws_no_name_rather_than_a_placeholder() {
        assert!(!names_the_workspace(0));
    }
}
