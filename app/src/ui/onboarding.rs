//! The first launch.
//!
//! A brand new operator opens vitrum onto an empty sidebar. Nothing on that
//! surface says what a session is, that the chord to start one is
//! `Ctrl+Shift+N`, that the daemon is a second process which keeps the agents
//! alive after the window closes, or that the sidebar is an inbox with bands
//! and a workspace partition behind it.
//!
//! # Why this is paged
//!
//! It began as one screen of three derived rows, on the theory that a new
//! operator will not read a tour. That was the wrong reading of the problem.
//! Three rows can carry the readings this machine produced, and nothing else,
//! so everything that makes this product different from a terminal with tabs
//! - the inbox, the attention jump, the bands, the workspaces - was
//! discoverable only by accident. An operator who never finds those is using
//! a worse tmux.
//!
//! So the surface is a short walkthrough: one chapter of machine readings,
//! then three chapters that name the surfaces those readings are for. It is
//! four pages, it is skippable from every one of them, and it never comes
//! back.
//!
//! # Why the pages are a function
//!
//! Every sentence here is derived from [`Machine`], which is the three
//! readings that decide what is worth saying: which agent binaries this
//! machine really has, whether the daemon is answering, and whether a session
//! already exists. A step is included only when it still has something to
//! tell you, so an operator who somehow reaches this after starting a session
//! is not walked through starting one.
//!
//! That is why [`pages`] takes a value and returns data. The renderer is a
//! thin pass over the result, and the rules are asserted without one.
//!
//! # What this surface refuses to do
//!
//! It does not invent an agent. [`crate::launch::detected_agents`] walks
//! `PATH`, and on a machine with nothing on it the honest answer is a named
//! list of what vitrum looks for plus the fact that any command works, not an
//! empty bullet list under a heading that promises agents.
//!
//! It does not invent a keystroke either. Every chord in the copy is looked
//! up in [`CHORDS`] at render time through [`chord_for`], and a page whose
//! chord is unbound rewrites its own sentence rather than teaching a key that
//! does nothing.
//!
//! It does not animate, and it holds no timer. No page advances on a
//! schedule: the walkthrough moves when the operator moves it, it is on
//! screen until they dismiss it, and the dismissal reports which way it went
//! so the caller can persist it.

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

/// Which reading or surface a step is about.
///
/// Carried so a test can name a step without matching on its prose, and so
/// the renderer can key rows on something stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StepKind {
    /// The daemon that owns the PTYs.
    Daemon,
    /// What this machine can run.
    Agents,
    /// Starting the first session.
    Start,
    /// The five things a row can be doing.
    States,
    /// Jumping to whichever session wants the operator.
    Attention,
    /// Sessions outliving the window.
    Persistence,
    /// Filing sessions into a workspace.
    Filing,
    /// Active, snoozed and settled.
    Bands,
    /// Per-workspace band visibility.
    Visibility,
    /// The shortcut overlay.
    Shortcuts,
    /// Scrollback search across sessions.
    Search,
    /// Where the rest of the product is.
    Settings,
}

/// One row of one page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub kind: StepKind,
    pub state: StepState,
    pub title: String,
    pub body: String,
}

/// One page of the walkthrough.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Chapter {
    /// What this machine has, and what is left to do on it.
    Machine,
    /// The sidebar as an inbox.
    Inbox,
    /// The workspace partition and the bands inside it.
    Workspaces,
    /// The keyboard, the search and the settings.
    Rest,
}

impl Chapter {
    /// Every chapter, in the order the walkthrough visits them.
    ///
    /// A test asserts [`pages`] produces exactly this, so a chapter added to
    /// the enum and left out of the walkthrough fails rather than silently
    /// never rendering.
    pub const ALL: [Chapter; 4] = [
        Chapter::Machine,
        Chapter::Inbox,
        Chapter::Workspaces,
        Chapter::Rest,
    ];
}

/// One page: a heading, a sentence, and the rows under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub chapter: Chapter,
    pub title: String,
    pub blurb: String,
    pub rows: Vec<Step>,
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

/// A sentence that names a chord, or the same sentence without one.
///
/// Both halves are written out by the caller because dropping a chord is not
/// a substring edit: "Press Ctrl+Shift+F to search" with the chord removed is
/// not a sentence, and the surface that teaches the product is the last place
/// to ship a grammatical accident.
fn chord_sentence(action: KeyAction, with: impl FnOnce(&str) -> String, without: &str) -> String {
    match chord_for(action) {
        Some(keys) => with(&keys),
        None => without.to_string(),
    }
}

/// A row that teaches rather than reports.
fn teach(kind: StepKind, title: &str, body: String) -> Step {
    Step {
        kind,
        state: StepState::Info,
        title: title.to_string(),
        body,
    }
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

/// The readings worth showing, in order.
///
/// A step appears only when it still says something. The daemon row is gone
/// once the socket is up, and the start row is gone once a session exists,
/// because a checklist whose every item is already ticked is a surface that
/// wasted the operator's attention on its first launch.
///
/// The agents row always appears: it is the only place the product tells you
/// what it found here.
pub fn steps(machine: &Machine) -> Vec<Step> {
    let mut out = Vec::with_capacity(3);

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
        out.push(Step {
            kind: StepKind::Start,
            state: StepState::Todo,
            title: "Start your first session".to_string(),
            body: chord_sentence(
                KeyAction::NewSession,
                |keys| {
                    format!(
                        "Press {keys}, or use the button at the top of the sidebar. Pick a \
                         row and it starts."
                    )
                },
                "Use the button at the top of the sidebar. Pick a row and it starts.",
            ),
        });
    }

    out
}

/// The walkthrough, in order.
///
/// The first page is this machine. The three after it are the surfaces that
/// make the first page worth anything, and they are constant: an operator
/// learns what the inbox is whether or not their daemon happened to be up.
///
/// Built by walking [`Chapter::ALL`] rather than by listing pages, so the
/// match in [`page`] is exhaustive and a chapter added to the enum stops the
/// build instead of quietly never rendering.
pub fn pages(machine: &Machine) -> Vec<Page> {
    Chapter::ALL
        .iter()
        .map(|chapter| page(*chapter, machine))
        .collect()
}

/// One chapter's page.
fn page(chapter: Chapter, machine: &Machine) -> Page {
    match chapter {
        Chapter::Machine => Page {
            chapter,
            title: "Welcome to vitrum".to_string(),
            blurb: intro(machine),
            rows: steps(machine),
        },
        Chapter::Inbox => inbox_page(),
        Chapter::Workspaces => workspaces_page(),
        Chapter::Rest => rest_page(),
    }
}

/// What the sidebar is for.
fn inbox_page() -> Page {
    Page {
        chapter: Chapter::Inbox,
        title: "The sidebar is an inbox".to_string(),
        blurb: "Running twenty agents is only useful if you can tell, without \
                visiting them, which one stopped."
            .to_string(),
        rows: vec![
            teach(
                StepKind::States,
                "Every row says what its agent is doing",
                "Working while it runs. Approval when it wants permission to do \
                     something. Input when it has asked you a question. Ready when it \
                     finished and is waiting. Failed when it exited badly. The colour \
                     down the left edge of the row is that status, so the list reads \
                     at a glance."
                    .to_string(),
            ),
            teach(
                StepKind::Attention,
                "Jump to whichever one wants you",
                chord_sentence(
                    KeyAction::NextAttention,
                    |keys| {
                        format!(
                            "{keys} moves to the next row that needs you and skips every \
                                 row that does not. This is the loop the product is for: you \
                                 stop going and looking, and you stop missing the one that \
                                 stopped an hour ago."
                        )
                    },
                    "Rows that need you sort to the top of the list, so you stop going \
                         and looking, and stop missing the one that stopped an hour ago.",
                ),
            ),
            teach(
                StepKind::Persistence,
                "The window is not the session",
                "Your sessions belong to the daemon. Close this window and they keep \
                     running, scrollback included; open it again and everything is where \
                     you left it. Quitting the app does not kill your agents."
                    .to_string(),
            ),
        ],
    }
}

/// What a workspace is for.
fn workspaces_page() -> Page {
    Page {
        chapter: Chapter::Workspaces,
        title: "Workspaces keep the list short".to_string(),
        blurb: "A long day leaves a long list. A workspace is how you say \
                \"not this, not now\" without killing anything."
            .to_string(),
        rows: vec![
            teach(
                StepKind::Filing,
                "File a session where it belongs",
                "Right-click any row and use Move to workspace. The switcher above \
                     the sidebar changes which one you are looking at, and the strip \
                     only appears once you have a second workspace, so day one costs \
                     you no chrome."
                    .to_string(),
            ),
            teach(
                StepKind::Bands,
                "Active, snoozed, settled",
                "Inside a workspace, rows sit in three bands. Snooze parks a row \
                     until a time you pick, or until it raises its hand. A session that \
                     wakes early comes back in place, wearing a badge, because the sort \
                     did not move it and the badge is the only thing that can tell you \
                     it returned. Sessions you are finished with drain to settled on \
                     their own."
                    .to_string(),
            ),
            teach(
                StepKind::Visibility,
                "Each workspace shows what you want it to",
                "Settings, Workspaces sets which of the three bands a workspace \
                     shows, so one can be everything and another only the things still \
                     running."
                    .to_string(),
            ),
        ],
    }
}

/// The keyboard, the search, and where settings lives.
fn rest_page() -> Page {
    Page {
        chapter: Chapter::Rest,
        title: "The rest of it".to_string(),
        blurb: "Two keys and one gear, and you have seen the whole product.".to_string(),
        rows: vec![
            teach(
                StepKind::Shortcuts,
                "Every shortcut, on demand",
                chord_sentence(
                    KeyAction::ToggleShortcuts,
                    |keys| {
                        format!(
                            "{keys} shows the full keyboard table, including anything you \
                                 have rebound. Nothing here is a keystroke you have to \
                                 remember from this sheet."
                        )
                    },
                    "The keyboard table lives in Settings, and lists anything you have \
                         rebound.",
                ),
            ),
            teach(
                StepKind::Search,
                "Search every session at once",
                chord_sentence(
                    KeyAction::OpenSearch,
                    |keys| {
                        format!(
                            "{keys} searches scrollback across all of your sessions, not \
                                 only the one on screen, which is how you find the run that \
                                 printed the error without remembering which agent it was."
                        )
                    },
                    "Scrollback search runs across all of your sessions, not only the \
                         one on screen.",
                ),
            ),
            teach(
                StepKind::Settings,
                "Where the rest is",
                "The gear at the bottom of the sidebar opens Settings: appearance, \
                     grouping, saved commands with their own shortcuts, notifications, \
                     and the keyboard table."
                    .to_string(),
            ),
        ],
    }
}

/// Has this operator already done everything the checklist would ask?
///
/// Connected with a session running means both task rows are gone, and the
/// heading says so instead of pretending there is work to do.
pub fn all_clear(machine: &Machine) -> bool {
    machine.connected && machine.any_session
}

/// The line under the title on the first page.
pub fn intro(machine: &Machine) -> String {
    if all_clear(machine) {
        "You are already running. Here is what the rest of the window is for.".to_string()
    } else {
        "One interface for every agent TUI you have running. Here is what this \
         machine has."
            .to_string()
    }
}

/// The word on the primary control of the last page.
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

/// The first-launch walkthrough.
///
/// Every string here comes from [`pages`] and [`finish_label`], so there is
/// nothing to assert against a renderer that is not already asserted against
/// those. The only state it owns is which page is showing.
#[component]
pub fn Onboarding(props: OnboardingProps) -> Element {
    let machine = props.machine.clone();
    let deck = pages(&machine);
    let last = deck.len().saturating_sub(1);
    let finish = finish_label(&machine);

    let mut at = use_signal(|| 0usize);
    let index = (at()).min(last);
    let page = &deck[index];
    let on_last = index == last;

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
                    span { class: "rg-sheet__title", "{page.title}" }
                    button {
                        class: "rg-btn-inline",
                        r#type: "button",
                        onclick: move |_| props.on_close.call(Outcome::Skipped),
                        "Skip"
                    }
                }

                div { class: "rg-sheet__body",
                    p { class: "rg-onboard__intro", "{page.blurb}" }
                    ol { class: "rg-onboard__steps",
                        for step in page.rows.iter() {
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

                div { class: "rg-sheet__foot rg-onboard__foot",
                    // The position indicator is decorative: the count and the
                    // position are already announced by the page heading, and
                    // a screen reader stepping through four dots learns
                    // nothing from them.
                    div {
                        class: "rg-onboard__dots",
                        aria_hidden: "true",
                        for (n, p) in deck.iter().enumerate() {
                            span {
                                key: "{p.chapter:?}",
                                class: if n == index {
                                    "rg-onboard__dot rg-onboard__dot--on"
                                } else {
                                    "rg-onboard__dot"
                                },
                            }
                        }
                    }
                    if index > 0 {
                        button {
                            class: "rg-btn",
                            r#type: "button",
                            onclick: move |_| at.set(index.saturating_sub(1)),
                            "Back"
                        }
                    }
                    button {
                        class: "rg-btn rg-btn--primary",
                        r#type: "button",
                        onclick: move |_| {
                            if on_last {
                                props.on_close.call(Outcome::Finished);
                            } else {
                                at.set(index + 1);
                            }
                        },
                        if on_last { "{finish}" } else { "Next" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

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
        pages(machine)
            .into_iter()
            .flat_map(|p| p.rows)
            .find(|s| s.kind == kind)
    }

    /// Every machine worth testing, so a rule is checked against all of them
    /// rather than against the one the author had in mind.
    fn every_machine() -> Vec<Machine> {
        let mut out = Vec::new();
        for agents in [None, Some(vec![]), Some(vec![agent("Codex", "codex")])] {
            for connected in [false, true] {
                for any_session in [false, true] {
                    out.push(machine(agents.clone(), connected, any_session));
                }
            }
        }
        out
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

    /// WHY: no page may teach a keystroke the shell does not bind.
    ///
    /// This closes the class that `the_start_step_quotes_the_chord_the_shell_actually_claims`
    /// only closes for one row. The walkthrough is the surface a new operator
    /// trusts most and is the surface furthest from the keymap, so a chord
    /// typed into prose here survives every rebind and every removal. Rather
    /// than pin the four sentences that happen to carry a chord today, this
    /// harvests every chord-shaped token out of every rendered page, on every
    /// machine, and demands the keymap actually claim it.
    ///
    /// A page added later with `"Press Ctrl+Alt+K"` written by hand fails
    /// here without anybody remembering to extend a list.
    ///
    /// What it does not catch: a sentence that names the RIGHT chord for the
    /// WRONG action ("Ctrl+Shift+F starts a session"). Nothing short of
    /// reading the prose can catch that, and the per-action assertions above
    /// cover the rows where it would matter most.
    #[test]
    fn no_page_teaches_a_chord_the_keymap_does_not_bind() {
        let bound: BTreeSet<String> = CHORDS.iter().map(|c| c.rendered()).collect();
        assert!(!bound.is_empty(), "the keymap binds nothing at all");

        let mut seen = 0usize;
        for m in every_machine() {
            for page in pages(&m) {
                // The blurb and the row titles are prose too. A chord is at
                // least as likely to be typed into a heading as into a body,
                // and a scanner that only reads bodies is a guard with a hole
                // exactly where the shortest, most quotable copy lives.
                for text in std::iter::once(&page.blurb)
                    .chain(page.rows.iter().flat_map(|r| [&r.title, &r.body]))
                {
                    for token in chord_tokens(text) {
                        seen += 1;
                        assert!(
                            bound.contains(&token),
                            "{text:?} teaches {token:?}, which no chord in the keymap renders"
                        );
                    }
                }
            }
        }
        assert!(
            seen > 0,
            "no chord-shaped text was found at all, so this test proved nothing"
        );
    }

    /// Chord-shaped words in a sentence: `Ctrl+Shift+N`, `F1`.
    ///
    /// Deliberately generous about what it picks up and strict about what it
    /// then demands, because the failure mode worth catching is a chord
    /// nobody registered, and a false positive here is a copy edit away.
    fn chord_tokens(body: &str) -> Vec<String> {
        body.split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '+'))
            .filter(|w| {
                let function_key =
                    w.starts_with('F') && w.len() > 1 && w[1..].chars().all(|c| c.is_ascii_digit());
                let combination = w.contains('+') && !w.starts_with('+') && !w.ends_with('+');
                function_key || combination
            })
            .map(str::to_string)
            .collect()
    }

    /// WHY: every chapter is reachable exactly once, and says its own thing.
    ///
    /// A chapter missing from the walkthrough is now a compile error rather
    /// than a test failure, because [`pages`] walks [`Chapter::ALL`] through
    /// an exhaustive match. What the compiler cannot see is the table itself
    /// being wrong: a chapter listed twice renders the same page twice, and
    /// two chapters that resolve to the same title mean one of them is a
    /// copy-paste of the other and an operator reads a page they have already
    /// read. Both leave the type system perfectly happy.
    ///
    /// The order is asserted with it. Machine readings come first because
    /// that page is the one whose content depends on the machine, and an
    /// operator whose daemon never came up needs it before any teaching.
    #[test]
    fn every_chapter_is_visited_once_and_renders_a_page_of_its_own() {
        assert_eq!(
            Chapter::ALL.iter().collect::<BTreeSet<_>>().len(),
            Chapter::ALL.len(),
            "a chapter is listed twice, so its page renders twice"
        );
        assert_eq!(
            Chapter::ALL.first(),
            Some(&Chapter::Machine),
            "the machine readings must be the first page"
        );

        for m in every_machine() {
            let deck = pages(&m);
            let visited: Vec<Chapter> = deck.iter().map(|p| p.chapter).collect();
            assert_eq!(visited, Chapter::ALL.to_vec());

            let titles: BTreeSet<&str> = deck.iter().map(|p| p.title.as_str()).collect();
            assert_eq!(titles.len(), deck.len(), "two chapters share a title");
        }
    }

    /// WHY: every page must carry something to read.
    ///
    /// A page whose rows are all conditional can empty itself out on some
    /// machine, and an empty page in a four-page walkthrough reads as a
    /// broken product on first launch. The `Machine` page is the live one:
    /// its rows come from readings, so it is the one that can go empty.
    #[test]
    fn no_machine_produces_an_empty_or_untitled_page() {
        for m in every_machine() {
            for page in pages(&m) {
                assert!(!page.title.trim().is_empty(), "{:?}", page.chapter);
                assert!(!page.blurb.trim().is_empty(), "{:?}", page.chapter);
                assert!(
                    !page.rows.is_empty(),
                    "{:?} is empty on {m:?}",
                    page.chapter
                );
                for row in &page.rows {
                    assert!(!row.title.trim().is_empty(), "{:?}", row.kind);
                    assert!(!row.body.trim().is_empty(), "{:?}", row.kind);
                }
            }
        }
    }

    /// WHY: a row must not be shown twice under two headings.
    ///
    /// The teaching pages and the derived checklist grew apart, and both had
    /// a claim on "where settings is". Two rows with the same kind means the
    /// operator reads the same sentence twice in four pages, which is how a
    /// walkthrough earns the skip button.
    #[test]
    fn no_kind_appears_on_two_pages() {
        for m in every_machine() {
            let mut seen: BTreeSet<StepKind> = BTreeSet::new();
            for page in pages(&m) {
                for row in page.rows {
                    assert!(
                        seen.insert(row.kind),
                        "{:?} is rendered more than once on {m:?}",
                        row.kind
                    );
                }
            }
        }
    }

    /// WHY: the teaching pages must not depend on the machine.
    ///
    /// The inbox, the bands and the keyboard are the same on a machine with
    /// no daemon and no agent as on a working one, and an operator whose
    /// first launch went badly is precisely the one who needs to know what
    /// the product is. A conditional slipped into a teaching page would hide
    /// it from them.
    #[test]
    fn only_the_machine_page_changes_with_the_machine() {
        let baseline = pages(&machine(None, false, false));
        for m in every_machine() {
            for (page, first) in pages(&m).into_iter().zip(baseline.iter()) {
                match page.chapter {
                    Chapter::Machine => {}
                    Chapter::Inbox | Chapter::Workspaces | Chapter::Rest => {
                        assert_eq!(&page, first, "{:?} moved with the machine", page.chapter);
                    }
                }
            }
        }
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
        assert!(!step.body.contains("Nothing on your PATH"), "{}", step.body);
    }

    /// A reading appears only while it still has something to say.
    ///
    /// The defect: a fixed checklist shows "start your first session" to
    /// somebody who has twenty, and "wait for the daemon" to somebody already
    /// connected. Both are ticked boxes taking the operator's attention on the
    /// one launch where attention is scarcest.
    #[test]
    fn only_the_readings_that_still_apply_are_shown() {
        let cases: &[(bool, bool, &[StepKind])] = &[
            (
                false,
                false,
                &[StepKind::Daemon, StepKind::Agents, StepKind::Start],
            ),
            (true, false, &[StepKind::Agents, StepKind::Start]),
            (false, true, &[StepKind::Daemon, StepKind::Agents]),
            (true, true, &[StepKind::Agents]),
        ];
        for (connected, any_session, expected) in cases {
            let m = machine(
                Some(vec![agent("Codex", "codex")]),
                *connected,
                *any_session,
            );
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
    /// The defect: a heading that promises a checklist above a page whose
    /// every row is informational.
    #[test]
    fn the_intro_and_the_button_match_the_rows_below_them() {
        let fresh = machine(Some(vec![]), false, false);
        assert!(
            intro(&fresh).contains("what this machine has"),
            "{}",
            intro(&fresh)
        );
        assert_eq!(finish_label(&fresh), "Got it");

        let running = machine(Some(vec![agent("Codex", "codex")]), true, true);
        assert_eq!(
            intro(&running),
            "You are already running. Here is what the rest of the window is for."
        );
        assert_eq!(finish_label(&running), "Close");
        assert_eq!(steps(&running).len(), 1);
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
            let step =
                find(&machine(Some(agents), true, false), StepKind::Agents).expect("present");
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
