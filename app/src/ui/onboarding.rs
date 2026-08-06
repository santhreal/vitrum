//! The first launch.
//!
//! A brand new operator opens vitrum onto an empty sidebar. Nothing on that
//! surface says what a session is, that the chord to start one is
//! `Ctrl+Shift+N`, or that the daemon is a second process which keeps the
//! agents alive after the window closes. The three things they need are all
//! knowable from the machine in front of them, so this surface reads them
//! rather than reciting a generic tour.
//!
//! # Why the steps are a function
//!
//! Every sentence here is derived from [`Machine`], which is the three
//! readings that decide what is worth saying: which agent binaries this
//! machine really has, whether the daemon is answering, and whether a session
//! already exists. A step is included only when it still has something to
//! tell you, so an operator who somehow reaches this after starting a session
//! is not walked through starting one.
//!
//! That is why [`steps`] takes a value and returns data. The renderer is a
//! thin pass over the result, and the rules are asserted without one.
//!
//! # What this surface refuses to do
//!
//! It does not invent an agent. [`crate::launch::detected_agents`] walks
//! `PATH`, and on a machine with nothing on it the honest answer is a named
//! list of what vitrum looks for plus the fact that any command works, not an
//! empty bullet list under a heading that promises agents.
//!
//! It does not animate, and it holds no timer. Nothing here appears or leaves
//! on a schedule: it is on screen until the operator dismisses it, and the
//! dismissal reports which way it went so the caller can persist it.

use dioxus::prelude::*;

use crate::keymap::{CHORDS, KeyAction};
use crate::launch::Detected;

/// The agent commands vitrum looks for on `PATH`.
///
/// Named here for the zero-detected case only. The list is a starting point
/// and not a limit, which the copy beside it says, because the command field
/// is free text and anything executable works.
const LOOKS_FOR: &str = "claude, codex, gemini, opencode, veyyon";

/// What onboarding read off this machine.
///
/// Three readings, all resolved by the caller. `agents` costs a `PATH` walk
/// per entry, and the connection and session facts live in `UiState`, so
/// neither belongs behind this module's door.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Machine {
    /// Agent binaries actually resolvable here, from
    /// [`crate::launch::detected_agents`].
    ///
    /// `None` means the PATH walk is still running. The first-launch path
    /// starts that walk beside the daemon connect, so the sheet must not say
    /// "nothing matched" until the walk has actually finished.
    pub agents: Option<Vec<Detected>>,
    /// Is a daemon socket open right now?
    pub connected: bool,
    /// Does this operator already have at least one session?
    pub any_session: bool,
}

/// How a step stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    /// Already true on this machine. Shown so the operator knows it counts.
    Done,
    /// The operator has something to do.
    Todo,
    /// Neither: a fact worth knowing that is not a task.
    Info,
}

/// Which reading a step is about.
///
/// Carried so a test can name a step without matching on its prose, and so
/// the renderer can key rows on something stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// The daemon that owns the PTYs.
    Daemon,
    /// What this machine can run.
    Agents,
    /// Starting the first session.
    Start,
    /// Where the rest of the product is.
    Settings,
}

/// One row of the surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub kind: StepKind,
    pub state: StepState,
    pub title: String,
    pub body: String,
}

/// How the operator left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Read it and pressed the primary control.
    Finished,
    /// Dismissed it. Persisted the same way: it does not come back.
    Skipped,
}

/// The chord that fires `action`, rendered the way the shortcut overlay
/// renders it, or `None` when nothing in the table claims it.
///
/// Read from [`CHORDS`] rather than written into the copy, because a rebind
/// of the built-in table would otherwise leave this surface teaching a
/// keystroke that no longer starts anything. That defect is the whole reason
/// this is a lookup.
pub fn chord_for(action: KeyAction) -> Option<String> {
    CHORDS
        .iter()
        .find(|c| c.action == action)
        .map(|c| c.rendered())
}

/// The sentence naming what this machine can run.
///
/// The zero case is first class. An operator with nothing installed needs the
/// names of the things vitrum looks for and the fact that the launcher takes
/// any command, which is a different sentence from a list, not a shorter one.
pub fn agents_line(agents: &[Detected]) -> String {
    if agents.is_empty() {
        return format!(
            "Nothing on your PATH matched the agents vitrum looks for ({LOOKS_FOR}). \
             That is not a blocker: the launcher takes any command, including your \
             shell, so you can start a session now and install an agent later."
        );
    }
    let names: Vec<&str> = agents.iter().map(|a| a.label).collect();
    format!(
        "Found on this machine: {}. Any other command works too; the launcher takes \
         free text.",
        join_names(&names)
    )
}

/// `"a"`, `"a and b"`, `"a, b and c"`.
fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The steps worth showing, in order.
///
/// A step appears only when it still says something. The daemon row is gone
/// once the socket is up, and the start row is gone once a session exists,
/// because a checklist whose every item is already ticked is a surface that
/// wasted the operator's attention on its first launch.
///
/// The agents row and the settings row always appear: the first is the only
/// place the product tells you what it found here, and the second is the only
/// place it says where everything else lives.
pub fn steps(machine: &Machine) -> Vec<Step> {
    let mut out = Vec::with_capacity(4);

    if !machine.connected {
        out.push(Step {
            kind: StepKind::Daemon,
            state: StepState::Todo,
            title: "Wait for the daemon".to_string(),
            body: "vitrum runs a second process that owns the PTYs, and starts it for \
                   you. Nothing is answering yet. If it stays that way, the daemon URL \
                   and the reason are under Settings, Advanced."
                .to_string(),
        });
    }

    out.push(match &machine.agents {
        None => Step {
            kind: StepKind::Agents,
            state: StepState::Info,
            title: "What you can run".to_string(),
            body: "Looking for agent binaries on your PATH…".to_string(),
        },
        Some(agents) => Step {
            kind: StepKind::Agents,
            state: if agents.is_empty() {
                StepState::Todo
            } else {
                StepState::Done
            },
            title: "What you can run".to_string(),
            body: agents_line(agents),
        },
    });

    if !machine.any_session {
        let chord = chord_for(KeyAction::NewSession);
        let body = match chord {
            Some(keys) => format!(
                "Press {keys}, or use the button at the top of the sidebar. Pick a row \
                 and it starts. Your sessions belong to the daemon, so closing this \
                 window leaves them running, scrollback included."
            ),
            None => "Use the button at the top of the sidebar. Pick a row and it \
                     starts. Your sessions belong to the daemon, so closing this window \
                     leaves them running, scrollback included."
                .to_string(),
        };
        out.push(Step {
            kind: StepKind::Start,
            state: StepState::Todo,
            title: "Start your first session".to_string(),
            body,
        });
    }

    out.push(Step {
        kind: StepKind::Settings,
        state: StepState::Info,
        title: "Where the rest is".to_string(),
        body: "The gear at the bottom of the sidebar opens Settings: appearance, \
               grouping, saved commands with their own shortcuts, notifications, and \
               the keyboard table."
            .to_string(),
    });

    out
}

/// Has this operator already done everything the checklist would ask?
///
/// Connected with a session running means both task rows are gone, and the
/// heading says so instead of pretending there is work to do.
pub fn all_clear(machine: &Machine) -> bool {
    machine.connected && machine.any_session
}

/// The line under the title.
pub fn intro(machine: &Machine) -> String {
    if all_clear(machine) {
        "You are already running. Two things worth knowing anyway.".to_string()
    } else {
        "A terminal for running many coding agents at once. Three things and you are \
         going."
            .to_string()
    }
}

/// The word on the primary control.
pub fn finish_label(machine: &Machine) -> &'static str {
    if all_clear(machine) {
        "Close"
    } else {
        "Got it"
    }
}

// ---------------------------------------------------------------------------
// The component
// ---------------------------------------------------------------------------

#[derive(Props, Clone, PartialEq)]
pub struct OnboardingProps {
    /// What the caller read off this machine.
    pub machine: Machine,
    /// How the operator left. Persisting it is the caller's job: this surface
    /// holds no profile handle and writes no file.
    pub on_close: EventHandler<Outcome>,
}

/// The first-launch surface.
///
/// Every string here comes from [`steps`], [`intro`] and [`finish_label`], so
/// there is nothing to assert against a renderer that is not already asserted
/// against those.
#[component]
pub fn Onboarding(props: OnboardingProps) -> Element {
    let machine = props.machine.clone();
    let rows = steps(&machine);
    let intro_line = intro(&machine);
    let finish = finish_label(&machine);

    rsx! {
        div {
            class: "rg-layer rg-layer--dim",
            onclick: move |_| props.on_close.call(Outcome::Skipped),
            div {
                class: "rg-sheet rg-sheet--onboarding",
                role: "dialog",
                aria_label: "Welcome to vitrum",
                onclick: move |e| e.stop_propagation(),

                div { class: "rg-sheet__head",
                    span { class: "rg-sheet__title", "Welcome to vitrum" }
                    button {
                        class: "rg-btn-inline",
                        r#type: "button",
                        onclick: move |_| props.on_close.call(Outcome::Skipped),
                        "Skip"
                    }
                }

                div { class: "rg-sheet__body",
                    p { class: "rg-onboard__intro", "{intro_line}" }
                    ol { class: "rg-onboard__steps",
                        for step in rows.iter() {
                            li {
                                class: match step.state {
                                    StepState::Done => "rg-onboard__step rg-onboard__step--done",
                                    StepState::Todo => "rg-onboard__step rg-onboard__step--todo",
                                    StepState::Info => "rg-onboard__step rg-onboard__step--info",
                                },
                                key: "{step.title}",
                                span { class: "rg-onboard__step-title", "{step.title}" }
                                span { class: "rg-onboard__step-body", "{step.body}" }
                            }
                        }
                    }
                }

                div { class: "rg-sheet__foot",
                    button {
                        class: "rg-btn rg-btn--primary",
                        r#type: "button",
                        onclick: move |_| props.on_close.call(Outcome::Finished),
                        "{finish}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(label: &'static str, command: &'static str) -> Detected {
        Detected { label, command }
    }

    fn machine(agents: Option<Vec<Detected>>, connected: bool, any_session: bool) -> Machine {
        Machine {
            agents,
            connected,
            any_session,
        }
    }

    fn kinds(machine: &Machine) -> Vec<StepKind> {
        steps(machine).into_iter().map(|s| s.kind).collect()
    }

    fn find(machine: &Machine, kind: StepKind) -> Option<Step> {
        steps(machine).into_iter().find(|s| s.kind == kind)
    }

    /// The chord is read from the shell's table, not written into the copy.
    ///
    /// The defect: a rebind of `KeyAction::NewSession` in `keymap::CHORDS`
    /// leaves a hardcoded sentence teaching a keystroke that no longer starts
    /// anything, on the one surface a new operator trusts completely.
    #[test]
    fn the_start_step_quotes_the_chord_the_shell_actually_claims() {
        let truth = CHORDS
            .iter()
            .find(|c| c.action == KeyAction::NewSession)
            .expect("the shell binds NewSession")
            .rendered();
        assert_eq!(chord_for(KeyAction::NewSession).as_deref(), Some(&*truth));

        let step = find(&machine(Some(vec![]), false, false), StepKind::Start)
            .expect("a machine with no session gets the start step");
        assert!(
            step.body.starts_with(&format!("Press {truth},")),
            "{}",
            step.body
        );
    }

    /// Zero detected agents gets its own sentence, never an empty list.
    ///
    /// The defect: rendering `agents.join(", ")` puts an empty run of text
    /// under a heading that promises what this machine can run, which reads
    /// as a broken surface rather than as "install one, or use your shell".
    #[test]
    fn no_agents_says_what_to_do_instead_of_nothing() {
        let step = find(&machine(Some(vec![]), true, false), StepKind::Agents)
            .expect("the agents step always applies");
        assert_eq!(step.state, StepState::Todo);
        assert_eq!(
            step.body,
            "Nothing on your PATH matched the agents vitrum looks for \
             (claude, codex, gemini, opencode, veyyon). That is not a blocker: the \
             launcher takes any command, including your shell, so you can start a \
             session now and install an agent later."
        );
    }

    /// The agents line names what was really found, in list order.
    ///
    /// The defect: reciting the five names vitrum knows about regardless of
    /// what is installed, which is the exact behaviour the launcher's picker
    /// was changed away from.

    /// A PATH walk still in flight must not claim nothing matched.
    ///
    /// The defect: opening the sheet before `detected_agents` returns, then
    /// rendering the zero-agents sentence, flashes "install an agent" on a
    /// machine that has one until the walk finishes. Looking is its own state.
    #[test]
    fn a_pending_path_walk_does_not_claim_nothing_matched() {
        let step = find(&machine(None, false, false), StepKind::Agents)
            .expect("the agents step always applies");
        assert_eq!(step.state, StepState::Info);
        assert!(
            step.body.contains("Looking for agent binaries"),
            "{}",
            step.body
        );
        assert!(
            !step.body.contains("Nothing on your PATH"),
            "{}",
            step.body
        );
    }

    #[test]
    fn the_agents_line_names_only_detected_binaries() {
        let cases: &[(&[Detected], &str)] = &[
            (&[agent("Claude Code", "claude")], "Claude Code"),
            (
                &[agent("Claude Code", "claude"), agent("Codex", "codex")],
                "Claude Code and Codex",
            ),
            (
                &[
                    agent("Claude Code", "claude"),
                    agent("Codex", "codex"),
                    agent("veyyon", "veyyon"),
                ],
                "Claude Code, Codex and veyyon",
            ),
        ];
        for (agents, expected) in cases {
            let line = agents_line(agents);
            assert_eq!(
                line,
                format!(
                    "Found on this machine: {expected}. Any other command works too; \
                     the launcher takes free text."
                )
            );
            assert!(!line.contains("Gemini"), "{line}");
        }
    }

    /// A step appears only while it still has something to say.
    ///
    /// The defect: a fixed four-row checklist shows "start your first
    /// session" to somebody who has twenty, and "wait for the daemon" to
    /// somebody already connected. Both are ticked boxes taking the operator's
    /// attention on the one launch where attention is scarcest.
    #[test]
    fn only_the_steps_that_still_apply_are_shown() {
        let cases: &[(bool, bool, &[StepKind])] = &[
            (
                false,
                false,
                &[
                    StepKind::Daemon,
                    StepKind::Agents,
                    StepKind::Start,
                    StepKind::Settings,
                ],
            ),
            (
                true,
                false,
                &[StepKind::Agents, StepKind::Start, StepKind::Settings],
            ),
            (
                false,
                true,
                &[StepKind::Daemon, StepKind::Agents, StepKind::Settings],
            ),
            (true, true, &[StepKind::Agents, StepKind::Settings]),
        ];
        for (connected, any_session, expected) in cases {
            let m = machine(Some(vec![agent("Codex", "codex")]), *connected, *any_session);
            assert_eq!(
                kinds(&m),
                *expected,
                "connected={connected} session={any_session}"
            );
            assert_eq!(all_clear(&m), *connected && *any_session);
        }
    }

    /// The heading does not claim there is work when there is none.
    ///
    /// The defect: "three things and you are going" printed above two
    /// informational rows, which promises a checklist the surface does not
    /// have.
    #[test]
    fn the_intro_and_the_button_match_the_rows_below_them() {
        let fresh = machine(Some(vec![]), false, false);
        assert!(intro(&fresh).contains("Three things"), "{}", intro(&fresh));
        assert_eq!(finish_label(&fresh), "Got it");

        let running = machine(Some(vec![agent("Codex", "codex")]), true, true);
        assert_eq!(
            intro(&running),
            "You are already running. Two things worth knowing anyway."
        );
        assert_eq!(finish_label(&running), "Close");
        assert_eq!(steps(&running).len(), 2);
    }

    /// Detection state drives the agents row's mark, both ways.
    ///
    /// The defect: a row permanently marked done, so the one machine where
    /// the operator has to go and install something looks identical to the one
    /// where they do not.
    #[test]
    fn the_agents_row_is_done_only_when_something_was_found() {
        for (agents, expected) in [
            (vec![], StepState::Todo),
            (vec![agent("Codex", "codex")], StepState::Done),
        ] {
            let step = find(&machine(Some(agents), true, false), StepKind::Agents).expect("present");
            assert_eq!(step.state, expected);
        }
    }

    /// The settings row points at the control that exists.
    ///
    /// The defect: teaching a keyboard shortcut for Settings. There is none:
    /// `keymap::CHORDS` binds no settings action, so the gear in the sidebar
    /// footer is the only true answer.
    #[test]
    fn settings_is_described_by_its_real_entry_point() {
        assert!(
            CHORDS.iter().all(|c| c.describes() != "Settings"),
            "a settings chord now exists; this copy must name it"
        );
        let step = find(&machine(Some(vec![]), true, true), StepKind::Settings).expect("present");
        assert_eq!(step.state, StepState::Info);
        assert!(
            step.body
                .starts_with("The gear at the bottom of the sidebar"),
            "{}",
            step.body
        );
    }
}
