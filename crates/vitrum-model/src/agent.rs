//! Which agent is behind a session, and what that agent publishes about itself.
//!
//! Identity lives here rather than in the UI because the sidebar's *status*
//! depends on it. An agent that announces a blocked state in its terminal title
//! can only be read by a rule that belongs to that agent, so
//! [`AgentKind::title_claim`] is the reason this enum is a model fact and not a
//! rendering detail. What the UI adds on top — a provider mark for the tab
//! strip — stays in the UI.
//!
//! [`AgentKind::Unknown`] is a real answer and never a fallback dressed as one.
//! A command this build does not recognise gets no title rule and the unknown
//! mark, not the nearest agent's: guessing produces a confident wrong answer the
//! operator has no way to tell from a right one.

use crate::status::TitleClaim;

/// Which agent is behind a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentKind {
    Claude,
    Codex,
    Gemini,
    Opencode,
    Veyyon,
    /// An interactive shell. Its own identity, not a failure to recognise one:
    /// a `bash` tab is a shell on purpose and the operator knows it.
    Shell,
    /// Nothing this build recognises.
    Unknown,
}

/// Every kind, in declaration order.
///
/// Exported so the tests that must cover every agent — the title rules above
/// all — enumerate the set instead of restating it. A hand-kept list inside one
/// test goes stale in silence, which is the same failure as having no test.
/// [`AgentKind::index`] is the exhaustive match that keeps this list honest.
pub const ALL_AGENT_KINDS: [AgentKind; 7] = [
    AgentKind::Claude,
    AgentKind::Codex,
    AgentKind::Gemini,
    AgentKind::Opencode,
    AgentKind::Veyyon,
    AgentKind::Shell,
    AgentKind::Unknown,
];

/// The agent binaries this build knows, keyed on the command rather than on the
/// label.
///
/// The commands are exactly `launch.rs`'s `AGENTS` table, so a session started
/// from the picker and one typed into the command field resolve to the same
/// mark. The labels are that table's too, and they are what the tooltip says.
const AGENTS: [(&str, AgentKind, &str); 5] = [
    ("claude", AgentKind::Claude, "Claude Code"),
    ("codex", AgentKind::Codex, "Codex"),
    ("gemini", AgentKind::Gemini, "Gemini CLI"),
    ("opencode", AgentKind::Opencode, "opencode"),
    ("veyyon", AgentKind::Veyyon, "veyyon"),
];

/// Interactive shells, which get the prompt mark rather than the unknown one.
///
/// Login shells reach this from `Options::shell` and from history entries like
/// `/bin/bash`, so the common case on a fresh machine is a shell and not an
/// agent. Reporting that as unknown would put a dashed placeholder on the one
/// kind of session every operator has.
const SHELLS: [&str; 15] = [
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "mksh",
    "csh",
    "tcsh",
    "nu",
    "elvish",
    "xonsh",
    "pwsh",
    "powershell",
    "cmd",
];

/// Suffixes `PATHEXT` resolution can leave on a Windows command.
///
/// `launch.rs::on_path` honours `PATHEXT`, so `claude` can resolve to
/// `claude.cmd` and arrive here with the extension attached. Without this a
/// Windows operator would see the unknown mark on every agent tab.
const EXECUTABLE_SUFFIXES: [&str; 5] = [".exe", ".cmd", ".bat", ".com", ".ps1"];

/// The invariant part of the banner Codex writes into the terminal title while
/// it is holding for the operator, as in `[ ! ] Action Required | agent`.
///
/// Codex sets this when it puts up a gate such as "Would you like to run the
/// following command?", and clears it the moment the turn resumes. It is the
/// whole reason [`StatusSource::Title`](crate::status::StatusSource::Title)
/// exists: without it a Codex session sitting on an approval prompt renders as
/// `Ready`, which is the one answer the sidebar must never give while the
/// operator is being waited on.
///
/// The bracket in front of these words is excluded because Codex ANIMATES it.
/// See [`codex_action_required`].
const CODEX_ACTION_REQUIRED: &str = " ] Action Required";

/// Whether `title` opens with Codex's approval banner, in any blink phase.
///
/// Codex blinks the marker inside the bracket for as long as the gate is up,
/// alternating `[ ! ] Action Required` and `[ . ] Action Required` about twice
/// a second. Both phases are the same statement: the turn is stopped and the
/// next move is the operator's. A rule that matched only the `!` phase made
/// the claim appear and vanish at the blink rate, so the sidebar withdrew
/// `Approval` from a row whose prompt was still on screen and put it back a
/// fraction of a second later, twice a second, for as long as the operator
/// took to answer.
///
/// So the marker character is not read, only its shape: an opening bracket, a
/// space, one character of marker, then the fixed words. That survives Codex
/// adding a third animation frame, which a literal list of phases would not,
/// and it is still Codex's own vocabulary rather than a global string match.
fn codex_action_required(title: &str) -> bool {
    let Some(rest) = title.trim_start().strip_prefix("[ ") else {
        return false;
    };
    // `chars` rather than a byte index: the marker is one character of
    // whatever Codex chose, and a multi-byte one must not panic here.
    let mut marker = rest.chars();
    // A `]` would mean there is no marker at all, only an empty bracket
    // followed by something else.
    if !matches!(marker.next(), Some(c) if c != ']') {
        return false;
    }
    marker.as_str().starts_with(CODEX_ACTION_REQUIRED)
}

impl AgentKind {
    /// Resolve the agent behind a command line's program.
    ///
    /// The program's basename is stripped of a Windows executable suffix, then
    /// matched exactly, ignoring ASCII case. There is no prefix match and no
    /// nearest neighbour: `claudex` and `my-claude` are not Claude Code, and
    /// guessing that they are would put the wrong provider on a tab with no way
    /// for the operator to notice.
    ///
    /// Allocation-free, deliberately. Every sidebar row resolves its agent on
    /// every paint — the mark, the tooltip and now the status all ask — so the
    /// obvious `to_ascii_lowercase` here is a `String` per row per frame in a
    /// twenty-row list that repaints on every daemon message.
    pub fn of(command: &str) -> Self {
        // Split on BOTH separators rather than through `std::path`. `Path` uses
        // the host's rules, so on Linux `C:\tools\codex.exe` has no components
        // at all and resolves to unknown, and on Windows a forward-slash path
        // does resolve but only by accident of that platform accepting both.
        // The command string arrives from `SessionInfo` exactly as the operator
        // or the launcher wrote it, and both forms are ordinary.
        let base = command.rsplit(['/', '\\']).next().unwrap_or(command);
        let name = EXECUTABLE_SUFFIXES
            .iter()
            .find_map(|suffix| {
                let cut = base.len().checked_sub(suffix.len())?;
                // `get` rather than slicing: a multi-byte character straddling
                // the cut would panic, and a command line is arbitrary text.
                base.get(cut..)?
                    .eq_ignore_ascii_case(suffix)
                    .then(|| &base[..cut])
            })
            .unwrap_or(base);

        if let Some((_, kind, _)) = AGENTS
            .iter()
            .find(|(cmd, _, _)| cmd.eq_ignore_ascii_case(name))
        {
            return *kind;
        }
        if SHELLS.iter().any(|shell| shell.eq_ignore_ascii_case(name)) {
            return AgentKind::Shell;
        }
        AgentKind::Unknown
    }

    /// The operator-facing name, for a tooltip.
    ///
    /// The five agent labels come out of the `AGENTS` table rather than a
    /// second match, so the picker that started the session and the tab that
    /// hosts it cannot end up calling it two different things. The two
    /// remaining kinds are not in that table because they are not binaries this
    /// build looks for.
    ///
    /// The `unwrap_or` is total rather than a panic and is never taken while
    /// `AGENTS` names all five agent variants, which
    /// `every_agent_has_its_own_label` proves.
    pub fn label(self) -> &'static str {
        match self {
            AgentKind::Shell => "shell",
            AgentKind::Unknown => "unknown agent",
            agent => AGENTS
                .iter()
                .find(|(_, kind, _)| *kind == agent)
                .map(|(_, _, label)| *label)
                .unwrap_or("agent"),
        }
    }

    /// What this agent is declaring by titling its terminal `title`, if
    /// anything.
    ///
    /// The rule is per agent and there is no global fallback, because the title
    /// is the agent's own vocabulary. `[ ! ] Action Required` means an approval
    /// gate in Codex; in an arbitrary program it means whatever that program
    /// decided, up to and including a filename. An agent with no rule gets
    /// `None` for every title it can possibly set, which is the honest answer
    /// and leaves its row on the observed states.
    ///
    /// The match is exhaustive on purpose. Adding an agent stops this crate
    /// compiling until someone writes down whether it publishes a blocked state
    /// and how, so "no rule" is always a decision and never an omission.
    ///
    /// A title the operator pinned with `SessionManager::rename` still reaches
    /// here. The pin is on the session's name, not on this channel: a session
    /// the operator bothered to name is the one they are watching, and taking
    /// its approval banner away as the price of naming it would remove the
    /// state they renamed it to follow.
    pub fn title_claim(self, title: &str) -> Option<TitleClaim> {
        match self {
            // The banner is a prefix, not the whole title: Codex appends the
            // session's own name after it, as `[ ! ] Action Required | agent`.
            AgentKind::Codex => {
                codex_action_required(title).then_some(TitleClaim::Approval)
            }
            // No rule, deliberately. Claude declares through the hint channel
            // instead; Gemini, opencode and veyyon publish nothing recognisable
            // in their titles; a shell's title is its command line and an
            // unknown command's title is anybody's guess.
            AgentKind::Claude
            | AgentKind::Gemini
            | AgentKind::Opencode
            | AgentKind::Veyyon
            | AgentKind::Shell
            | AgentKind::Unknown => None,
        }
    }

    /// Whether this kind's terminal title is a name for the session.
    ///
    /// A shell titles its terminal with the command it is running or the
    /// directory it is in, which is the best name that session will ever have,
    /// and taking it is how a `vim` tab comes to say `vim`. An agent TUI does
    /// the opposite: it treats the title bar as a status line and rewrites it
    /// every turn. Gemini writes `Ready (kernel-notes)` and Codex writes
    /// `[ ! ] Action Required`, so honouring those as names produced a sidebar
    /// row reading `Ready (kernel-n…` beside a pill already saying Ready, and
    /// a row whose name changed every time the agent changed what it was doing.
    ///
    /// Unknown says yes. A command this build does not recognise is far more
    /// likely to be an ordinary program that titles itself sensibly than an
    /// agent that does not, and the failure mode of guessing wrong here is a
    /// worse name rather than a wrong status.
    ///
    /// Exhaustive for the same reason [`AgentKind::title_claim`] is: a new
    /// agent must not silently inherit either answer.
    pub const fn title_is_a_name(self) -> bool {
        match self {
            AgentKind::Claude
            | AgentKind::Codex
            | AgentKind::Gemini
            | AgentKind::Opencode
            | AgentKind::Veyyon => false,
            AgentKind::Shell | AgentKind::Unknown => true,
        }
    }

    /// This kind's position in [`ALL_AGENT_KINDS`], as an EXHAUSTIVE match.
    ///
    /// Rust cannot enumerate an enum's variants, so the list has to be written
    /// out, and a hand-kept list of the thing the guards iterate is a hole.
    /// Adding a variant breaks this match and stops the crate compiling; giving
    /// it an index without adding it to the list fails
    /// `the_all_list_names_every_variant_exactly_once`.
    pub const fn index(self) -> usize {
        match self {
            AgentKind::Claude => 0,
            AgentKind::Codex => 1,
            AgentKind::Gemini => 2,
            AgentKind::Opencode => 3,
            AgentKind::Veyyon => 4,
            AgentKind::Shell => 5,
            AgentKind::Unknown => 6,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`ALL_AGENT_KINDS`] must name every variant exactly once.
    ///
    /// The guard that makes the list enforced rather than trusted: every test
    /// below iterates it, so a variant missing from it would be checked by none
    /// of them while shipping an agent whose title rule nobody had decided.
    #[test]
    fn the_all_list_names_every_variant_exactly_once() {
        let mut seen = [0usize; ALL_AGENT_KINDS.len()];
        for kind in ALL_AGENT_KINDS {
            seen[kind.index()] += 1;
        }
        assert_eq!(
            seen,
            [1; ALL_AGENT_KINDS.len()],
            "ALL_AGENT_KINDS must name each variant once; counted {seen:?}"
        );
    }

    /// The five commands `launch.rs` offers must each reach their own kind.
    ///
    /// The bug: the picker starts `gemini` and the tab draws Claude's burst,
    /// because the table here drifted from `launch.rs`'s. The operator then has
    /// no way at all to tell which agent a tab is, since the mark is the only
    /// channel that says so on a renamed session.
    #[test]
    fn the_launcher_commands_each_resolve_to_their_own_agent() {
        assert_eq!(AgentKind::of("claude"), AgentKind::Claude);
        assert_eq!(AgentKind::of("codex"), AgentKind::Codex);
        assert_eq!(AgentKind::of("gemini"), AgentKind::Gemini);
        assert_eq!(AgentKind::of("opencode"), AgentKind::Opencode);
        assert_eq!(AgentKind::of("veyyon"), AgentKind::Veyyon);
    }

    /// A full path, an uppercased name and a `PATHEXT` suffix must all still
    /// resolve.
    ///
    /// The bug: `SessionInfo::command` holds whatever was typed or resolved, so
    /// `/home/mk/.local/bin/claude` and `claude.cmd` are both ordinary. Matching
    /// the raw string puts the unknown placeholder on every agent tab on
    /// Windows, and on any machine where the operator typed a path.
    #[test]
    fn a_path_a_case_and_a_windows_suffix_still_resolve() {
        assert_eq!(AgentKind::of("/home/mk/.local/bin/claude"), AgentKind::Claude);
        assert_eq!(AgentKind::of("CLAUDE"), AgentKind::Claude);
        assert_eq!(AgentKind::of("claude.cmd"), AgentKind::Claude);
        assert_eq!(AgentKind::of(r"C:\tools\codex.exe"), AgentKind::Codex);
        assert_eq!(AgentKind::of("/usr/bin/bash"), AgentKind::Shell);
        assert_eq!(AgentKind::of("PowerShell.EXE"), AgentKind::Shell);
    }

    /// An unrecognised command must report unknown, never the nearest agent.
    ///
    /// The bug: a prefix or substring match. `claudex`, `my-claude` and
    /// `claude-wrapper` all contain `claude`, and a tab that draws Anthropic's
    /// burst for someone else's binary is a confident wrong answer the operator
    /// cannot detect. `env` is the live case: `/usr/bin/env sh -c ...` is a
    /// perfectly ordinary command line whose program identifies nothing.
    #[test]
    fn an_unrecognised_command_is_unknown_and_not_the_nearest_agent() {
        for command in [
            "",
            "env",
            "/usr/bin/env",
            "claudex",
            "my-claude",
            "claude-wrapper",
            "gemini2",
            "shell",
            "make",
        ] {
            assert_eq!(
                AgentKind::of(command),
                AgentKind::Unknown,
                "{command:?} must be unknown rather than guessed"
            );
        }
    }

    /// Seven kinds, seven distinct labels, and the five agent labels come from
    /// the launcher's own table.
    #[test]
    fn every_agent_has_its_own_label() {
        let labels: Vec<&str> = ALL_AGENT_KINDS.iter().map(|kind| kind.label()).collect();
        assert_eq!(
            labels,
            vec![
                "Claude Code",
                "Codex",
                "Gemini CLI",
                "opencode",
                "veyyon",
                "shell",
                "unknown agent",
            ]
        );
        for (i, a) in labels.iter().enumerate() {
            for (j, b) in labels.iter().enumerate().skip(i + 1) {
                assert_ne!(a, b, "{:?} and {:?} share a label", ALL_AGENT_KINDS[i], ALL_AGENT_KINDS[j]);
            }
        }
    }

    /// Codex's live case, verbatim: a session parked on "Would you like to run
    /// the following command?" sets this title, and before the rule existed the
    /// row read `Ready` while the pane held an approval gate.
    ///
    /// Both blink phases are here because Codex animates the marker while the
    /// gate is up. Recognising only `[ ! ]` resolved every other title write to
    /// no claim at all, which is the flap `codex_action_required` closes.
    #[test]
    fn codex_action_required_is_an_approval_claim() {
        for title in [
            "[ ! ] Action Required",
            "[ . ] Action Required",
            "[ ! ] Action Required - codex",
            "[ . ] Action Required - codex",
            "[ ! ] Action Required | agent",
            "[ . ] Action Required | agent",
            "[ ! ] Action Required — vitrum",
            "  [ ! ] Action Required",
            "  [ . ] Action Required",
            // A frame Codex does not currently draw. The rule reads the
            // position of the marker and not its identity, so a third
            // animation frame does not silently drop the claim.
            "[ * ] Action Required",
            "[ ⠴ ] Action Required",
        ] {
            assert_eq!(
                AgentKind::Codex.title_claim(title),
                Some(TitleClaim::Approval),
                "{title:?} is Codex announcing it is blocked"
            );
        }
    }

    /// The claim must clear the instant the banner does, or a row sticks on
    /// "Needs approval" after the agent has moved on — a worse failure than
    /// never showing it, because it trains the operator to ignore the pill.
    #[test]
    fn a_codex_title_without_the_banner_claims_nothing() {
        for title in [
            "codex",
            "~/src/vitrum",
            "Action Required",
            "[!] Action Required",
            "codex [ ! ] Action Required",
            // Codex's working and quiet titles, which is how the claim ends.
            "⠴ agent",
            "agent",
            // An empty bracket is not a marker, and neither is a bracket
            // followed by something other than the fixed words.
            "[ ] Action Required",
            "[ ! ] Actions Required",
            "[ ! ]Action Required",
            "[ ",
            "[ !",
            "",
        ] {
            assert_eq!(
                AgentKind::Codex.title_claim(title),
                None,
                "{title:?} must not be read as a declaration"
            );
        }
    }

    /// Both phases of the banner Codex animates while a gate is up, as whole
    /// titles. `CODEX_ACTION_REQUIRED` is only the invariant tail, so a test
    /// that wants a title the agent actually writes builds it here.
    const CODEX_APPROVAL_TITLES: [&str; 2] =
        ["[ ! ] Action Required", "[ . ] Action Required"];

    /// THE CLASS. Every kind must have a decided title rule, and the decision
    /// is asserted here per kind rather than inferred from behaviour. A new
    /// agent breaks the exhaustive match in `title_claim` at compile time and
    /// then fails this table until someone records what it publishes.
    #[test]
    fn every_agent_kind_has_a_decided_title_rule() {
        // `true` means "this kind reads a blocked state out of some title".
        let decided = |kind: AgentKind| match kind {
            AgentKind::Codex => true,
            AgentKind::Claude
            | AgentKind::Gemini
            | AgentKind::Opencode
            | AgentKind::Veyyon
            | AgentKind::Shell
            | AgentKind::Unknown => false,
        };

        // Every title any rule in this build recognises. Handed to every kind,
        // so a rule silently gaining a second owner fails here too.
        let known_titles = [
            CODEX_APPROVAL_TITLES[0],
            CODEX_APPROVAL_TITLES[1],
            "[ ! ] Action Required - codex",
        ];

        for kind in ALL_AGENT_KINDS {
            let claims: Vec<Option<TitleClaim>> = known_titles
                .iter()
                .map(|title| kind.title_claim(title))
                .collect();
            let any = claims.iter().any(Option::is_some);
            assert_eq!(
                any,
                decided(kind),
                "{kind:?} claims {claims:?} for {known_titles:?}, which is not the \
                 rule recorded for it. Adding an agent means deciding here whether \
                 it publishes a blocked state in its title."
            );
        }
    }

    /// An agent with no rule must stay silent on the exact title that drives
    /// another agent's rule. This is the "unknown command gets no rule at all"
    /// half of the contract, and it is what stops the Codex banner from
    /// becoming a global string match.
    #[test]
    fn an_agent_without_a_rule_ignores_another_agents_banner() {
        for kind in ALL_AGENT_KINDS {
            if kind == AgentKind::Codex {
                continue;
            }
            for title in CODEX_APPROVAL_TITLES {
                assert_eq!(
                    kind.title_claim(title),
                    None,
                    "{kind:?} read Codex's banner {title:?} as its own"
                );
            }
        }
        for title in CODEX_APPROVAL_TITLES {
            assert_eq!(AgentKind::of("some-unknown-tool").title_claim(title), None);
        }
    }
}
