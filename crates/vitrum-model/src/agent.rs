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

/// Whether `title` carries Codex's approval banner, in any blink phase and at
/// any position.
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
///
/// The banner is searched for anywhere in the title rather than only at its
/// start, because Codex composes the title from an ORDERED LIST of items the
/// operator configures. `tui.terminal_title` is an array defaulting to
/// `["spinner", "project"]`, where the `spinner` item is the one documented as
/// "Spinner while working, action-required message while blocked". Under the
/// default the banner does lead the title, which is why matching a prefix
/// worked. Set `tui.terminal_title = ["project", "spinner"]` and the same
/// banner arrives last, so a prefix rule answers `Ready` for the entire time a
/// gate is up, on a session that is doing nothing but waiting for an answer.
/// That is one configuration's worth of the defect the blink phases already
/// cost once.
fn codex_action_required(title: &str) -> bool {
    let mut rest = title;
    // `find` on an ASCII needle, so every index is a character boundary.
    while let Some(open) = rest.find("[ ") {
        let after = &rest[open + "[ ".len()..];
        // `chars` rather than a byte index: the marker is one character of
        // whatever Codex chose, and a multi-byte one must not panic here.
        let mut marker = after.chars();
        // A `]` would mean there is no marker at all, only an empty bracket
        // followed by something else.
        if matches!(marker.next(), Some(c) if c != ']')
            && marker.as_str().starts_with(CODEX_ACTION_REQUIRED)
        {
            return true;
        }
        rest = after;
    }
    false
}

/// The fixed words Gemini CLI puts at the head of its terminal title while it
/// is holding a confirmation, as in `✋  Action Required (vitrum)`.
///
/// Gemini composes the whole title itself: a marker glyph, two spaces, a fixed
/// phrase for the state, then the working directory in parentheses, padded to
/// eighty columns. The four states it publishes are `✋  Action Required`,
/// `⏲  Working…`, `◇  Ready` and, while a turn is streaming, `✦  ` followed by
/// text the MODEL wrote. Only the first is a statement that the next move is
/// the operator's, and it goes up for exactly as long as the confirmation is on
/// screen.
const GEMINI_ACTION_REQUIRED: &str = "Action Required";

/// Gemini's markers for the one branch whose title text is model-authored.
///
/// `✦` leads a streaming turn, where the rest of the title is the model's
/// current thought subject when `ui.showStatusInTitle` is on, and `⏲` leads the
/// silent-working title. A thought subject is arbitrary text: a model reasoning
/// about an approval prompt can write the words "Action Required" into it, and
/// reading that as a declaration would put "Needs approval" on a row that is
/// mid-turn. Naming the two markers that mean "the text after me is not mine"
/// is narrower than naming the one marker that means blocked, which is the
/// glyph most likely to be restyled.
const GEMINI_MODEL_AUTHORED_MARKERS: [char; 2] = ['\u{2726}', '\u{23F2}'];

/// Whether `title` is Gemini announcing that it is holding a confirmation.
///
/// `ui.dynamicWindowTitle` defaults to true, so this is what an unconfigured
/// Gemini session publishes. With it off the title is `Gemini CLI (dir)` for
/// the whole session and no state is ever claimed, which this rule reports
/// honestly by finding no banner.
///
/// The marker glyph is read only for its shape, exactly as Codex's is: one
/// leading non-alphanumeric character, whitespace, then Gemini's fixed words.
/// The glyph itself is not pinned so that restyling it does not silently drop
/// the claim. That it is THERE is pinned, because every branch of Gemini's
/// title builder writes a marker and two spaces in front of its phrase, so a
/// title that is the bare words came from something other than Gemini's own
/// composer. What the words may NOT be followed by is another letter, so
/// `Action Requirements` is not this banner.
fn gemini_action_required(title: &str) -> bool {
    let trimmed = title.trim();
    let mut chars = trimmed.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first.is_alphanumeric() || GEMINI_MODEL_AUTHORED_MARKERS.contains(&first) {
        // Either there is no marker at all, or the marker is one of the two
        // that mean the text after it belongs to the model and not to Gemini.
        return false;
    }
    let rest = chars.as_str();
    // A marker is separated from the phrase; `✋Action` is not Gemini.
    if !rest.starts_with(char::is_whitespace) {
        return false;
    }
    let words = rest.trim_start();
    let Some(tail) = words.strip_prefix(GEMINI_ACTION_REQUIRED) else {
        return false;
    };
    !tail.starts_with(|c: char| c.is_alphanumeric())
}

/// The fixed words Claude Code puts in its terminal title while one or more of
/// its agents is stopped waiting for the operator, as in
/// `3 awaiting input · claude agents`.
///
/// Claude titles its terminal through OSC 0 (set title and icon) from two
/// places. A single interactive session publishes `<frame> <topic>`, where the
/// frame animates between two braille characters while the turn runs and
/// settles on `✳`, and the topic is the conversation's own name run through
/// `Bun.stripANSI`. There is no state vocabulary in that one, which is why
/// Claude carried no rule at all until this shape was read out of the shipped
/// binary.
///
/// The agents screen publishes the other one. It counts the jobs whose band is
/// `blocked` and titles the terminal with that count, falling back to the bare
/// words when the count is zero. `blocked` is the same band that drives
/// Claude's OSC 21337 tab status `status=Waiting`, which this build does not
/// read, so the title is the only channel on which a Claude session says the
/// next move is the operator's.
///
/// Read out of Claude Code 2.1.226: the title builder is a one-line function
/// returning `` `${n} awaiting input · claude agents` `` for a positive count
/// and `claude agents` otherwise, and its result is handed to the same OSC 0
/// setter as the interactive title. `CLAUDE_CODE_DISABLE_TERMINAL_TITLE` turns
/// both off, in which case Claude claims nothing and this rule finds nothing.
const CLAUDE_AWAITING_INPUT: &str = " awaiting input";

/// The fixed tail of that banner, after the separator.
const CLAUDE_AGENTS: &str = "claude agents";

/// Whether `title` is Claude reporting agents parked on a prompt.
///
/// The whole title has to be the banner. An interactive Claude session titles
/// itself `<frame> <topic>` with a topic the conversation named itself, so a
/// conversation about this very feature can put these words on screen; the
/// animation frame in front of them is what tells the two apart, and requiring
/// the count to lead the title is what reads it.
///
/// The separator between the count and the fixed tail is read for its shape
/// rather than its identity, exactly as Codex's marker is: Claude currently
/// writes a middle dot, and pinning that character would drop the claim if it
/// were restyled. A count of zero is refused because it is the phrasing Claude
/// uses for "nothing is waiting", and reading it as a declaration would leave
/// the row on `Needs input` for the whole session.
fn claude_awaiting_input(title: &str) -> bool {
    let trimmed = title.trim();
    let Some((count, rest)) = trimmed.split_once(CLAUDE_AWAITING_INPUT) else {
        return false;
    };
    if count.is_empty() || !count.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if count.bytes().all(|b| b == b'0') {
        return false;
    }
    // The separator is whatever punctuation and space Claude puts between the
    // count and the words; `trim_start_matches` on "not alphanumeric" walks it
    // without naming it.
    rest.trim_start_matches(|c: char| !c.is_alphanumeric()) == CLAUDE_AGENTS
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
    ///
    /// # What each agent puts in its title
    ///
    /// Every answer below is read out of the agent that ships, at the version
    /// named, and not assumed:
    ///
    /// - **Codex** 0.147.0 writes `[ ! ] Action Required` and
    ///   `[ . ] Action Required`, both present verbatim in the released binary
    ///   beside its own `failed to set terminal title` message. The title is
    ///   assembled from the ordered `tui.terminal_title` item list, whose
    ///   items that binary documents as the app name, the project name, the
    ///   working directory, the spinner ("Spinner while working,
    ///   action-required message while blocked"), the run state, the thread
    ///   title, the branch, context and usage percentages, the version and
    ///   token counts. The banner is the spinner item, so its position depends
    ///   on configuration. See [`codex_action_required`].
    /// - **Gemini** 0.54.4 writes `✋  Action Required (dir)` while a
    ///   confirmation is up, `◇  Ready (dir)` when idle, `⏲  Working… (dir)`
    ///   while working silently and `✦  <thought>` mid-turn, composed in one
    ///   function that pads every result to eighty columns and strips control
    ///   characters, and enabled by default through `ui.dynamicWindowTitle`.
    ///   With that setting off the title is `Gemini CLI (dir)` for the whole
    ///   session. See [`gemini_action_required`].
    /// - **Claude** 2.1.226 writes two titles through OSC 0. A single session
    ///   writes `<frame> <topic>`, where the frame animates between `⠂` and
    ///   `⠐` about once a second and settles on `✳`, and the topic is the
    ///   conversation's own name: no state vocabulary. The agents screen
    ///   writes `<count> awaiting input · claude agents`, and `claude agents`
    ///   once nothing is waiting. That count is the only Claude title that
    ///   declares a state; Claude's other blocked channel is an OSC 21337 tab
    ///   status carrying `status=Waiting`, which this build does not read. See
    ///   [`claude_awaiting_input`].
    /// - **opencode** 1.18.16 sets no terminal title. Its TUI package and the
    ///   CLI package behind it contain no OSC 0 or OSC 2 write and no title
    ///   setter call, so a session running it keeps whatever title the shell
    ///   that started it left behind.
    /// - **veyyon** sets no terminal title either. Its source writes no OSC 0
    ///   or OSC 2 sequence, and the shipped binary carries none.
    /// - **A shell** writes whatever `PS1` tells it to, usually the command or
    ///   the directory, and an **unknown** command writes anything at all.
    pub fn title_claim(self, title: &str) -> Option<TitleClaim> {
        match self {
            // The banner is one item of a configurable title, so it can lead,
            // trail, or sit between the project name and the branch.
            AgentKind::Codex => {
                codex_action_required(title).then_some(TitleClaim::Approval)
            }
            // Gemini's confirmation banner. Approval rather than input: it goes
            // up for a tool call the operator has to allow, which is the same
            // gate Codex's banner announces.
            AgentKind::Gemini => {
                gemini_action_required(title).then_some(TitleClaim::Approval)
            }
            // Claude's agents screen counting the jobs parked on a prompt.
            // Input rather than approval: the count covers every reason a job
            // stopped for the operator, a permission gate and a question
            // alike, and naming it approval would put the stronger word on a
            // row that is only being asked something.
            AgentKind::Claude => {
                claude_awaiting_input(title).then_some(TitleClaim::Input)
            }
            // No rule, deliberately. opencode and veyyon set no title at all;
            // a shell's title is its command line and an unknown command's
            // title is anybody's guess.
            AgentKind::Opencode
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
            // `codex [ ! ] Action Required` is NOT here. It was, back when the
            // rule matched a prefix. Codex composes its title from the ordered
            // `tui.terminal_title` list, so an operator who put the project
            // ahead of the spinner gets exactly this string while a gate is
            // up, and refusing it answered `Ready` for the whole time the
            // prompt was on screen. It is asserted as a claim in the evidence
            // table instead.
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

    /// What one agent kind does with its terminal title, as read out of the
    /// agent rather than as an intention.
    ///
    /// The three arms are the only three answers there are, and each carries
    /// the evidence that answer rests on. A kind cannot be added without
    /// picking one, and no arm can be filled in with nothing: `Silent` is
    /// checked against every other agent's banner, and the other two carry
    /// titles that are asserted, so an empty entry fails rather than passes.
    enum TitleRule {
        /// The agent writes no terminal title at all. Whatever is in the title
        /// came from somewhere else and means nothing about this agent.
        Silent,
        /// The agent titles itself, and the title is a name with no state
        /// vocabulary in it.
        NameOnly {
            /// Titles this agent is known to write. Never empty: a kind that
            /// writes nothing is `Silent`.
            writes: &'static [&'static str],
        },
        /// The agent publishes a blocked state in its title.
        Declares {
            /// Titles it writes while blocked, with the claim each must make.
            blocked: &'static [(&'static str, TitleClaim)],
            /// Titles it writes at any other time, which must claim nothing.
            /// This is the half that keeps a row from sticking on a state the
            /// agent has already left.
            otherwise: &'static [&'static str],
        },
    }

    /// Where one kind's rule was read from, and what it is.
    struct Evidence {
        /// The shipped artifact the shapes below were read out of, at the
        /// version that was read. A rule verified against one release is a
        /// rule that can be re-verified against the next one.
        source: &'static str,
        rule: TitleRule,
    }

    /// EXHAUSTIVE, and the reason a new agent cannot ship without a decided
    /// rule: adding a variant stops this match compiling, and the entry it
    /// forces has to name an artifact and carry titles the tests below run.
    fn evidence(kind: AgentKind) -> Evidence {
        match kind {
            AgentKind::Claude => Evidence {
                source: "Claude Code 2.1.226, OSC 0 title builders",
                rule: TitleRule::Declares {
                    blocked: &[
                        ("1 awaiting input · claude agents", TitleClaim::Input),
                        ("3 awaiting input · claude agents", TitleClaim::Input),
                        ("12 awaiting input · claude agents", TitleClaim::Input),
                    ],
                    otherwise: &[
                        // The agents screen once nothing is waiting.
                        "claude agents",
                        "0 awaiting input · claude agents",
                        // A single session: an animation frame, then the
                        // conversation's own name.
                        "⠂ resolve the status flap",
                        "⠐ resolve the status flap",
                        "✳ resolve the status flap",
                        // The resume screen.
                        "claude · resume",
                        // A conversation that named itself after this very
                        // banner. The frame in front of it is what says the
                        // text is a name and not a count.
                        "✳ 3 awaiting input · claude agents",
                        "awaiting input · claude agents",
                    ],
                },
            },
            AgentKind::Codex => Evidence {
                source: "Codex 0.147.0, tui.terminal_title spinner item",
                rule: TitleRule::Declares {
                    blocked: &[
                        (CODEX_APPROVAL_TITLES[0], TitleClaim::Approval),
                        (CODEX_APPROVAL_TITLES[1], TitleClaim::Approval),
                        // The banner is one item of an ordered list, so it can
                        // arrive after the project or the branch.
                        ("vitrum [ ! ] Action Required", TitleClaim::Approval),
                        ("[ . ] Action Required — main", TitleClaim::Approval),
                    ],
                    otherwise: &[
                        "codex",
                        "vitrum — main — 62%",
                        "⠴ vitrum",
                        "Ready",
                        "Working",
                        "Thinking",
                    ],
                },
            },
            AgentKind::Gemini => Evidence {
                source: "Gemini CLI 0.54.4, computeTerminalTitle",
                rule: TitleRule::Declares {
                    blocked: &[
                        ("✋  Action Required (vitrum)", TitleClaim::Approval),
                        // Every title is padded to eighty columns.
                        (
                            "✋  Action Required (vitrum)                                                    ",
                            TitleClaim::Approval,
                        ),
                    ],
                    otherwise: &[
                        "◇  Ready (vitrum)",
                        "⏲  Working… (vitrum)",
                        "✦  Working… (vitrum)",
                        // ui.dynamicWindowTitle off.
                        "Gemini CLI (vitrum)",
                        // A model thought subject, which is arbitrary text and
                        // is the one branch Gemini does not author.
                        "✦  Action Required (vitrum)",
                        "⏲  Action Required (vitrum)",
                        // Not the fixed words.
                        "✋  Action Requirements (vitrum)",
                        // The words with no marker at all are somebody else's.
                        "Action Required",
                    ],
                },
            },
            AgentKind::Opencode => Evidence {
                source: "opencode 1.18.16, no OSC 0 or OSC 2 write in its TUI \
                         or CLI packages",
                rule: TitleRule::Silent,
            },
            AgentKind::Veyyon => Evidence {
                source: "veyyon source and shipped binary, no OSC 0 or OSC 2 \
                         write",
                rule: TitleRule::Silent,
            },
            AgentKind::Shell => Evidence {
                source: "PS1, which is the operator's",
                rule: TitleRule::NameOnly {
                    writes: &["~/src/vitrum", "mk@host:~/src/vitrum", "vim README.md"],
                },
            },
            AgentKind::Unknown => Evidence {
                source: "an unrecognised command, which titles itself anything",
                rule: TitleRule::NameOnly {
                    writes: &["make -j32", "htop", ""],
                },
            },
        }
    }

    /// Every blocked title any agent in this build writes, paired with its
    /// owner, derived from [`evidence`] at run time.
    ///
    /// Built rather than listed so that adding an agent's banner immediately
    /// tests every other agent against it. A hand-kept corpus would leave the
    /// new banner unchecked against the six kinds that must ignore it, which
    /// is exactly the hole a global string match falls into.
    fn every_banner() -> Vec<(AgentKind, &'static str, TitleClaim)> {
        ALL_AGENT_KINDS
            .iter()
            .flat_map(|kind| match evidence(*kind).rule {
                TitleRule::Declares { blocked, .. } => blocked
                    .iter()
                    .map(|(title, claim)| (*kind, *title, *claim))
                    .collect::<Vec<_>>(),
                TitleRule::Silent | TitleRule::NameOnly { .. } => Vec::new(),
            })
            .collect()
    }

    /// THE CLASS. Every kind's rule is asserted against the titles the agent
    /// was observed writing, and the table is exhaustive, so a new agent stops
    /// the crate compiling until someone reads its titles and records them.
    ///
    /// What this does not catch: an agent changing its title in a release
    /// nobody re-read. `source` names what to re-read.
    #[test]
    fn every_agent_kind_has_a_decided_title_rule() {
        for kind in ALL_AGENT_KINDS {
            let Evidence { source, rule } = evidence(kind);
            assert!(!source.trim().is_empty(), "{kind:?} records no evidence");
            match rule {
                TitleRule::Silent => {}
                TitleRule::NameOnly { writes } => {
                    assert!(
                        !writes.is_empty(),
                        "{kind:?} is recorded as titling itself but names no \
                         title it writes; a kind that writes none is Silent"
                    );
                    for title in writes {
                        assert_eq!(
                            kind.title_claim(title),
                            None,
                            "{kind:?} read its own name {title:?} as a state"
                        );
                    }
                }
                TitleRule::Declares { blocked, otherwise } => {
                    assert!(
                        !blocked.is_empty(),
                        "{kind:?} is recorded as declaring a state but names no \
                         title that declares one"
                    );
                    assert!(
                        !otherwise.is_empty(),
                        "{kind:?} names no title that clears its claim, so \
                         nothing proves the claim ever ends"
                    );
                    for (title, claim) in blocked {
                        assert_eq!(
                            kind.title_claim(title),
                            Some(*claim),
                            "{kind:?} must read {title:?} as {claim:?} ({source})"
                        );
                    }
                    for title in otherwise {
                        assert_eq!(
                            kind.title_claim(title),
                            None,
                            "{kind:?} read {title:?} as a declaration ({source})"
                        );
                    }
                }
            }
        }
    }

    /// No agent may read another agent's banner.
    ///
    /// The corpus is derived, so this covers every banner in the build against
    /// every kind in the build. It is what stops any of these rules from
    /// degenerating into a global string match, which would put `Approval` on
    /// a shell whose directory happens to be named after one.
    #[test]
    fn an_agent_never_reads_another_agents_banner() {
        let banners = every_banner();
        assert!(
            banners.len() >= ALL_AGENT_KINDS.len(),
            "the derived corpus is too small to be testing anything: {banners:?}"
        );
        for (owner, title, _) in &banners {
            for kind in ALL_AGENT_KINDS {
                if kind == *owner {
                    continue;
                }
                assert_eq!(
                    kind.title_claim(title),
                    None,
                    "{kind:?} read {owner:?}'s banner {title:?} as its own"
                );
            }
        }
    }

    /// Claude's agents screen counts the jobs parked on a prompt, and the
    /// count is what makes it a declaration.
    ///
    /// The bug this closes: Claude carried no title rule at all, so a session
    /// on the agents screen with three jobs waiting for an answer rendered as
    /// `Ready`. The zero case and the bare words are here because they are how
    /// the claim has to end.
    #[test]
    fn claudes_agent_count_is_an_input_claim_and_clears_at_zero() {
        assert_eq!(
            AgentKind::Claude.title_claim("7 awaiting input · claude agents"),
            Some(TitleClaim::Input)
        );
        for title in [
            "0 awaiting input · claude agents",
            "claude agents",
            " awaiting input · claude agents",
            "many awaiting input · claude agents",
            "3 awaiting input · claude",
            "3 awaiting input",
            "3 awaiting inputs · claude agents",
        ] {
            assert_eq!(
                AgentKind::Claude.title_claim(title),
                None,
                "{title:?} is not Claude counting blocked jobs"
            );
        }
    }

    /// Gemini's banner survives a restyled marker and the eighty-column pad,
    /// and stays out of the one branch whose text the model wrote.
    ///
    /// The bug this closes: reading the words alone would put `Approval` on a
    /// mid-turn session whose model was reasoning aloud about an approval
    /// prompt, since that thought subject goes straight into the title.
    #[test]
    fn geminis_banner_reads_its_shape_and_not_its_glyph() {
        for title in [
            "✋  Action Required (vitrum)",
            "⚠  Action Required (vitrum)",
            "✋ Action Required (vitrum)",
            "  ✋  Action Required (vitrum)  ",
        ] {
            assert_eq!(
                AgentKind::Gemini.title_claim(title),
                Some(TitleClaim::Approval),
                "{title:?} is Gemini holding a confirmation"
            );
        }
        for title in [
            "✦  Action Required (vitrum)",
            "⏲  Action Required (vitrum)",
            "✋Action Required (vitrum)",
            "Action Required",
            "◇  Ready (vitrum)",
            "",
        ] {
            assert_eq!(
                AgentKind::Gemini.title_claim(title),
                None,
                "{title:?} is not Gemini declaring a gate"
            );
        }
    }
}
