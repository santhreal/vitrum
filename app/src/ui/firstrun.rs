//! The first thirty seconds.
//!
//! A new operator opens vitrum onto a window with nothing in it. Until this
//! module existed, what they got was a grey button reading "New session" and,
//! under it, the four characters `Ctrl+Shift+N`. Nothing on that screen said
//! what a session is, that vitrum is for coding agents rather than for
//! terminals, which agents this machine can actually run, or that the one on
//! screen would be started in the directory they launched from. The product's
//! whole argument was behind a keyboard shortcut.
//!
//! So the empty pane is now a first-run surface with three jobs, and it does
//! them in the order an operator needs them:
//!
//! 1. Say what this is. One headline and one sentence, no tour.
//! 2. Name what this machine can run, INCLUDING what it cannot. A roster of
//!    five names with two greyed is a truthful answer to "what is this for";
//!    an empty list under a heading promising agents is not, and neither is
//!    hiding the four the operator has not installed so that the screen
//!    silently disagrees with the README.
//! 3. Offer exactly one action, already aimed. Not a form, not a dialog: a
//!    button that names the agent and the place, and starts it.
//!
//! # Why the decision is a function
//!
//! [`first_run`] takes a [`Machine`] and returns data. Everything impure —
//! the `PATH` walk behind the roster, the profile read behind the memory, the
//! process directory — happens once in [`read_machine`], off the UI thread,
//! and never again. That split is what makes the rules provable: which agent
//! gets promoted, what happens when the remembered one has been uninstalled,
//! what the screen says on a machine with nothing on it at all. None of those
//! may depend on what is installed on the machine running the test, and with
//! a resolver passed in ([`launch::agent_roster`]) none of them does.
//!
//! # What it refuses to do
//!
//! It does not guess silently. The sidebar's `+` deliberately will not fire an
//! agent it merely found on `PATH`, because a bare `+` that launches something
//! is a mystery button. Here the control spells out the whole sentence it is
//! about to execute — the agent by name and the directory by name — so taking
//! it is a decision the operator read, not one the product made for them.
//!
//! It does not offer a shell. Not as a row, not as a fallback on a machine
//! with no agent installed; see `AGENTS.md`, "Demos show agents, not shell
//! output". With nothing detected the honest answer is the list of what vitrum
//! looked for plus the fact that the launcher takes any command.
//!
//! It does not invent a keystroke. The chord in the secondary line is looked
//! up in the live table through [`crate::ui::onboarding::chord_for`], so a
//! rebind cannot leave this surface teaching a key that does nothing.

use crate::launch::{self, AgentAvailability, RecentEntry};
use crate::ui::onboarding;

/// The headline. What this product is, in one line.
///
/// Public, and a constant rather than a field of [`FirstRun`], because the
/// pane paints it before the machine has been read. What vitrum is does not
/// depend on what is installed, and a first-run screen that is blank for as
/// long as a `PATH` walk takes is a first-run screen that looks broken.
pub const HEADLINE: &str = "Run your coding agents here.";

/// The sentence under it: the two facts that distinguish this from a terminal
/// with tabs. Sessions outlive the window, and the sidebar is an inbox.
pub const BLURB: &str = "Every session is one agent working in one project. They \
    run in a daemon of their own, so closing this window does not stop them, \
    and the sidebar tells you which one is waiting on you.";

/// Everything the first-run pane reads off the machine, taken once.
///
/// The only impure part of this module, and it is a value rather than a set
/// of calls so that [`first_run`] can be driven from a table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Machine {
    /// Every agent vitrum knows about, installed or not, in table order.
    pub roster: Vec<AgentAvailability>,
    /// The last agent this operator started and where they started it, or
    /// `None` on a fresh profile. This is the whole of "remember the last
    /// project and agent": `recents` is already keyed on the command, its
    /// arguments and the directory together, and its head is by definition
    /// the last launch.
    pub last: Option<RecentEntry>,
    /// The directory this window was launched from, for a machine with no
    /// projects and no sessions to point at yet.
    pub cwd: String,
    /// This user's home, so a place can be written `~/src/vitrum`.
    pub home: String,
}

/// Read the machine. Blocking: one `PATH` walk per known agent and one small
/// profile read, so callers run it off the UI thread and once.
pub fn read_machine() -> Machine {
    Machine {
        roster: launch::agent_roster_now(),
        last: launch::load_launch_store().recents.first().cloned(),
        cwd: std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        home: launch::user_home(),
    }
}

/// One named agent on the offer list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// The name a person recognises: "Claude Code".
    pub label: &'static str,
    /// The binary it runs.
    pub command: &'static str,
    /// Whether the row is an action or a statement.
    pub installed: bool,
    /// Whether the primary control already fires this one.
    ///
    /// A marked row rather than a second button. The empty pane's own rule is
    /// that an action appears exactly once per state, and a roster row reading
    /// "Claude Code" beside a control reading "Start Claude Code in
    /// src/vitrum" is the same action twice under two affordances, which is
    /// what made the previous version of this screen say one thing four times.
    pub primary: bool,
    /// The caption on the right. The command when it is here, and why it is
    /// not a row you can take when it is not.
    pub note: &'static str,
}

/// The single action the pane offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    /// The command line to launch, exactly as it will be split and spawned.
    pub line: String,
    /// The absolute directory to launch it in.
    pub cwd: String,
    /// `cwd` written the way a person names it. Empty when there is no
    /// directory to name, which is the only case the label omits it.
    pub place: String,
    /// The agent's own name, for the control.
    pub word: String,
    /// The whole sentence the control wears.
    pub label: String,
    /// True when this is the pair the operator last used rather than the
    /// first thing found on this machine. Drives the caption, so a second
    /// launch reads as "where you left off" instead of as a fresh guess.
    pub remembered: bool,
}

/// What the empty pane draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstRun {
    pub headline: &'static str,
    pub blurb: &'static str,
    /// The primary control, or `None` when there is honestly nothing to aim
    /// it at.
    pub start: Option<Start>,
    /// The caption under the control: why it is offering this.
    pub caption: Option<String>,
    /// Every known agent, in table order, missing ones included.
    pub offers: Vec<Offer>,
    /// Said only when this machine has none of them and this profile has no
    /// history. Replaces the control rather than sitting beside it.
    pub nothing: Option<String>,
}

/// The whole surface, decided. PURE.
///
/// `here` is the directory a session would start in: the project the window is
/// pointing at, or the directory vitrum itself was launched from. `place`
/// renders a directory the way a person names it, and is a parameter because
/// doing it properly needs the daemon's project list, which is not this
/// module's business.
///
/// The order of preference is the point:
///
/// - The remembered pair wins, so the second launch is the first thing under
///   the pointer and under the keyboard. It loses only when the agent it names
///   is one vitrum knows and this machine no longer has, because offering a
///   binary that was uninstalled is a spawn failure three seconds later.
/// - Otherwise the first installed agent in table order, in `here`. Named on
///   the control, so it is offered rather than guessed.
/// - Otherwise nothing, and the screen says what it looked for.
pub fn first_run(machine: &Machine, here: &str, place: impl Fn(&str) -> String) -> FirstRun {
    let here = here.trim();
    let remembered = machine.last.as_ref().filter(|e| runnable(machine, e));
    let start = match remembered {
        Some(e) => {
            let cwd = pick_dir(&e.cwd, here);
            Some(build(
                machine,
                launch::join_command(&e.command, &e.args),
                cwd,
                true,
                &place,
            ))
        }
        None => machine.roster.iter().find(|a| a.installed).map(|a| {
            build(
                machine,
                a.command.to_string(),
                here.to_string(),
                false,
                &place,
            )
        }),
    };

    // The agent the control is already aimed at, when it is one vitrum knows.
    // A remembered command from outside the table promotes nothing, so every
    // installed row stays takeable.
    let promoted = start
        .as_ref()
        .map(|s| basename(s.line.split_whitespace().next().unwrap_or(&s.line)));

    let offers: Vec<Offer> = machine
        .roster
        .iter()
        .map(|a| Offer {
            label: a.label,
            command: a.command,
            installed: a.installed,
            primary: a.installed && promoted == Some(a.command),
            note: if a.installed {
                a.command
            } else {
                "not installed"
            },
        })
        .collect();

    let caption = start.as_ref().map(|s| {
        if s.remembered {
            "Where you left off.".to_string()
        } else {
            format!(
                "{} is on this machine. Anything else is one key away.",
                s.word
            )
        }
    });

    let nothing = start.is_none().then(|| nothing_line(&offers));

    FirstRun {
        headline: HEADLINE,
        blurb: BLURB,
        start,
        caption,
        offers,
        nothing,
    }
}

/// Is a remembered launch still worth offering?
///
/// A command vitrum does not know is taken at face value: it is something this
/// operator really ran, the roster has no opinion on it, and the launch path
/// validates it on the way out anyway. A command vitrum DOES know has to still
/// be installed, because that is a question this module has already answered.
fn runnable(machine: &Machine, entry: &RecentEntry) -> bool {
    if entry.command.trim().is_empty() {
        return false;
    }
    let name = basename(&entry.command);
    match machine.roster.iter().find(|a| a.command == name) {
        Some(a) => a.installed,
        None => true,
    }
}

/// The remembered directory, or `here` when it left none.
fn pick_dir(remembered: &str, here: &str) -> String {
    let remembered = remembered.trim();
    if remembered.is_empty() {
        here.to_string()
    } else {
        remembered.to_string()
    }
}

/// Assemble one [`Start`], including the sentence on the control.
fn build(
    machine: &Machine,
    line: String,
    cwd: String,
    remembered: bool,
    place: impl Fn(&str) -> String,
) -> Start {
    let word = word_for(machine, &line);
    let place = if cwd.trim().is_empty() {
        String::new()
    } else {
        place(&cwd)
    };
    let label = if place.is_empty() {
        format!("Start {word}")
    } else {
        format!("Start {word} in {place}")
    };
    Start {
        line,
        cwd,
        place,
        word,
        label,
        remembered,
    }
}

/// The name to put on the control: the agent's own, when vitrum knows it.
///
/// `claude --permission-mode plan` is "Claude Code", not the whole line. The
/// argument is not what distinguishes one launch from another and it does not
/// fit a control, and a first-run operator reading a flag they did not write
/// is being shown the product's plumbing.
fn word_for(machine: &Machine, line: &str) -> String {
    let program = line.split_whitespace().next().unwrap_or(line);
    let name = basename(program);
    machine
        .roster
        .iter()
        .find(|a| a.command == name)
        .map(|a| a.label.to_string())
        .unwrap_or_else(|| name.to_string())
}

/// The program a command line names, without its directory.
fn basename(command: &str) -> &str {
    command.rsplit(['/', '\\']).next().unwrap_or(command)
}

/// The sentence for a machine with no agent on it and no history.
///
/// Names every agent vitrum looked for, because "no agents found" is a dead
/// end and a list is something the operator can act on. The list is built from
/// the roster rather than written out, so an agent added to the table appears
/// here without anyone remembering to.
fn nothing_line(offers: &[Offer]) -> String {
    let names: Vec<&str> = offers.iter().map(|o| o.command).collect();
    let looked = onboarding::join_names(&names);
    match onboarding::chord_for(crate::keymap::KeyAction::NewSession) {
        Some(chord) => format!(
            "vitrum looked for {looked} and found none of them here. Install one, \
             or press {chord} and run any command you like."
        ),
        None => format!(
            "vitrum looked for {looked} and found none of them here. Install one, \
             or open the launcher and run any command you like."
        ),
    }
}

/// The line under the control that names the other way in.
///
/// Read from the live chord table rather than written into the copy, for the
/// same reason the onboarding sheet does it: a rebind must not leave a surface
/// teaching a key that no longer opens anything.
pub fn other_way() -> Option<String> {
    onboarding::chord_for(crate::keymap::KeyAction::NewSession)
}

#[cfg(test)]
mod tests;
