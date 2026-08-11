//! The window's own titlebar, and the only band of chrome the window always
//! pays for.
//!
//! Shipping the OS decoration on a tool like this looks unfinished, and it
//! wastes a strip of vertical space on a title nobody reads. The window is
//! frameless on Linux and Windows and this draws the replacement; on macOS the
//! native traffic lights stay, because reimplementing them is how you end up
//! with three buttons that look almost right and do not respond to
//! Mission Control.
//!
//! Platform split, stated plainly:
//!
//! - Linux and Windows: `with_decorations(false)`. This bar draws minimise,
//!   maximise and close, and the empty area drags the window.
//! - macOS: decorations stay on with a transparent titlebar and a fullsize
//!   content view, so the traffic lights sit where macOS puts them. This bar
//!   reserves [`MACOS_TRAFFIC_LIGHT_INSET`] on the left for them and draws no
//!   window controls of its own.
//!
//! # Why four different things live in one 36px strip
//!
//! This bar is 3840px wide on the panel it was designed against and it used to
//! carry two words. Everything folded into it here was previously a full-width
//! horizontal band of its own, stacked above the session list:
//!
//! - **The wordmark.** One place, and this is it. The sidebar used to repeat
//!   it 44px lower down, which is the same word twice in one column.
//! - **The workspace switcher.** A whole band for one chip, in the state every
//!   user is in on day one, where there is nothing to switch between. See
//!   [`crate::ui::workspaces`] for the rule that decides when the strip below
//!   is worth drawing at all.
//! - **The connection state.** A permanent full-width green banner announcing
//!   a healthy socket is a band spent on a non-event. It is a dot here, and it
//!   only says a word when the word is bad news.
//! - **The focused session's context.** Not decoration: at twenty agents the
//!   one thing you lose track of is which session the keyboard is talking to,
//!   and the terminal grid cannot tell you, because it is whatever the agent
//!   printed.
//!
//! Nothing in this file animates and nothing here is measured at runtime.

pub(crate) mod native;

use crate::inbox::Pill;
use crate::state::{ConnState, UiState, status_label};

/// Left inset reserved for the macOS traffic lights, in CSS pixels.
///
/// Three 12px buttons at 20px pitch starting 20px from the edge, plus a gap
/// before our own content begins. Too small and the window title overlaps the
/// close button; too large and the bar looks padded on one side only.
pub const MACOS_TRAFFIC_LIGHT_INSET: f64 = 78.0;

// A zero inset would put the title under the traffic lights, and the value is
// fixed at compile time, so this is a build failure rather than a test.
const _: () = assert!(MACOS_TRAFFIC_LIGHT_INSET > 0.0);

/// Does this build draw its own minimise, maximise and close buttons?
pub const DRAWS_WINDOW_CONTROLS: bool = !cfg!(target_os = "macos");

/// The product name, drawn exactly once in the whole window.
///
/// Taken from `vitrum-os` rather than written here: renaming the product is
/// meant to be one edit to `branding.rs`, and a second copy in the window
/// frame is the one place a stale name would be most visible.
const WORDMARK: &str = vitrum_os::branding::APP_WORDMARK;

/// What the titlebar says about the focused session.
///
/// Returned as data so the exact strings are testable. Every branch is a real
/// state: no session focused at all, a session whose row has not arrived yet,
/// and a focused session with or without a branch.
///
/// `primary` is EMPTY when nothing is focused, and that is deliberate. The
/// previous version put the word "vitrum" here, which is why the window said
/// its own name twice: once in this slot and once in the sidebar header 44px
/// below it. The wordmark is now a separate element that is always present, so
/// this slot is free to say nothing when there is nothing to say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    /// Session title, or empty when no session is focused.
    pub primary: String,
    /// Project, branch and status, already joined; or the session count when
    /// nothing is focused.
    pub secondary: String,
}

/// Build the titlebar's context line.
pub fn context(st: &UiState) -> Context {
    let Some(id) = st.window.focused else {
        // THIS workspace's sessions, not the daemon's.
        //
        // The count renders immediately beside the workspace name, so a
        // daemon-wide total reads as that workspace's. Switching to a
        // freshly created workspace said "1 session" while its sidebar was
        // empty, because the one session belonged to the workspace the
        // operator had just left. A workspace is a separate top-level
        // context or it is not one.
        let here = st
            .daemon
            .sessions
            .iter()
            .filter(|row| st.daemon.workspaces.workspace_of(&row.info) == st.window.workspace)
            .count();
        return Context {
            primary: String::new(),
            secondary: vitrum_fmt::count::count_s(here as u64, "session"),
        };
    };
    let Some(row) = st.row(id) else {
        // The focused id outlived its row for one paint, between a close and
        // the snapshot that prunes it. Saying so beats a blank bar.
        return Context {
            primary: String::new(),
            secondary: "session closing".to_string(),
        };
    };
    let info = &row.info;
    let project = st
        .daemon
        .projects
        .iter()
        .find(|p| p.id == info.project_id)
        .map(|p| p.name.as_str());
    let mut parts: Vec<String> = Vec::with_capacity(3);
    if let Some(p) = project {
        parts.push(p.to_string());
    }
    if let Some(b) = info.git_branch.as_deref() {
        parts.push(b.to_string());
    }
    // ONE VOCABULARY.
    //
    // This slot used to say `status_label`, which is the process word
    // ("running"), while the sidebar row for the same session said the state
    // word ("Working"). Two surfaces, 44px apart, naming one fact twice.
    // `Pill::of` is the one resolution of a row's state and `inbox::status_word`
    // is the one place that state becomes a word, so both surfaces read the
    // same string and a synonym cannot enter one of them alone.
    //
    // The exit description sits BESIDE the word rather than instead of it,
    // because it is a different fact: the word says Ready or Failed, the
    // description says which failure. A live session gets the word only,
    // since "running" is what the word already said.
    parts.push(Pill::of(row).word.to_string());
    if matches!(info.status, vitrum_proto::SessionStatus::Exited { .. }) {
        parts.push(status_label(&info.status));
    }
    Context {
        primary: info.title.clone(),
        secondary: parts.join("  \u{00b7}  "),
    }
}

/// The connection indicator, as data.
///
/// One dot, and a word ONLY when the word is worth a person's attention. A
/// healthy socket is the overwhelmingly common case and it earns a 7px dot
/// with a tooltip, not a full-width green banner; a broken one earns a word,
/// a hue and a button, because sessions keep running while the window cannot
/// see them and the operator has to be able to tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conn {
    /// Modifier for the dot.
    pub class: &'static str,
    /// Text beside the dot, or empty when the dot alone is enough.
    pub word: &'static str,
    /// Full sentence, for the tooltip.
    pub title: String,
    /// Offer the Retry button.
    pub retryable: bool,
}

/// Fold [`ConnState`] into what the titlebar draws.
///
/// Fixture mode is loud on purpose and always has been: fake data that looks
/// like real data is the one failure this program refuses to make quiet.
#[must_use]
pub fn conn(state: &ConnState, url: &str) -> Conn {
    let title = state.banner_text(url);
    match state {
        ConnState::Live { .. } => Conn {
            class: "rg-conn--ok",
            word: "",
            title,
            retryable: false,
        },
        ConnState::Connecting => Conn {
            class: "rg-conn--connecting",
            word: "Connecting",
            title,
            retryable: false,
        },
        ConnState::Failed { .. } => Conn {
            class: "rg-conn--failed",
            word: "Offline",
            title,
            retryable: true,
        },
        ConnState::Fixture => Conn {
            class: "rg-conn--fixture",
            word: "Fixture data",
            title,
            retryable: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::NOW;
    use vitrum_model::SessionView;

    fn to_views(infos: Vec<SessionInfo>) -> Vec<SessionView> {
        infos.into_iter().map(SessionView::new).collect()
    }
    use vitrum_proto::{Attention, ProjectId, ProjectInfo, SessionId, SessionInfo, SessionStatus};

    fn session(id: u64, title: &str, branch: Option<&str>, status: SessionStatus) -> SessionInfo {
        SessionInfo {
            id: SessionId(id),
            project_id: ProjectId(1),
            title: title.to_string(),
            cwd: "/src/vitrum".to_string(),
            command: "claude".to_string(),
            args: Vec::new(),
            status,
            created_at_ms: 0,
            last_activity_ms: 0,
            cols: 80,
            rows: 24,
            git_branch: branch.map(str::to_string),
            worktree: None,
            unread: false,
            attention: Attention::default(),
            hint: None,
            term_title: None,
        }
    }

    fn state() -> UiState {
        let mut st = UiState::default();
        st.daemon.projects = vec![ProjectInfo {
            id: ProjectId(1),
            name: "vitrum".into(),
            root: "/src/vitrum".into(),
        }];
        st
    }

    /// With nothing focused the context slot must be EMPTY, not the product
    /// name. The wordmark is a separate element that is always on screen, and
    /// putting it here as well is exactly how the window came to say "vitrum"
    /// twice in one column.
    #[test]
    fn an_unfocused_window_leaves_the_context_slot_empty() {
        let st = state();
        assert_eq!(
            context(&st),
            Context {
                primary: String::new(),
                secondary: "0 sessions".into(),
            }
        );
    }

    /// The count must be real. A hardcoded or stale number here is the kind of
    /// wrong that survives every screenshot review.
    #[test]
    fn the_session_count_tracks_the_session_list() {
        let mut st = state();
        st.daemon.sessions = to_views(vec![
            session(1, "a", None, SessionStatus::Running),
            session(2, "b", None, SessionStatus::Running),
        ]);
        assert_eq!(context(&st).secondary, "2 sessions");
        st.daemon.sessions.truncate(1);
        assert_eq!(context(&st).secondary, "1 session");
    }

    /// A focused session names itself, its project, its branch and its status,
    /// in that order. This is the line that tells you which of twenty agents
    /// your keystrokes are reaching.
    #[test]
    fn a_focused_session_shows_project_branch_and_status() {
        let mut st = state();
        st.daemon.sessions = to_views(vec![session(
            7,
            "review auth",
            Some("feat/auth"),
            SessionStatus::Running,
        )]);
        st.open(SessionId(7), NOW);
        let ctx = context(&st);
        assert_eq!(ctx.primary, "review auth");
        assert_eq!(
            ctx.secondary,
            "vitrum  \u{00b7}  feat/auth  \u{00b7}  Working"
        );
    }

    /// Defect class: one session state named two ways in one window.
    ///
    /// The titlebar read `state::status_label` while the sidebar pill read
    /// [`crate::inbox::status_word`], so a running agent was "Working" in
    /// the list and "running" in the bar for the same session at the same
    /// instant. The invariant is checked at the surface, not at the helper:
    /// whatever word the pill shows, the bar shows.
    ///
    /// Driven over [`vitrum_model::ALL_STATUSES`] read at run time, and the
    /// coverage assertion at the end fails when a sixth state is added
    /// without a case here, so the class cannot reopen one state at a time.
    #[test]
    fn the_bar_and_the_pill_name_a_state_the_same_way() {
        use std::collections::HashSet;
        use vitrum_model::{ALL_STATUSES, SidebarStatus};
        use vitrum_proto::{AgentHint, HintState};

        let mut covered: HashSet<SidebarStatus> = HashSet::new();
        for declared in HintState::ALL
            .into_iter()
            .map(Some)
            .chain(core::iter::once(None))
        {
            let mut info = session(7, "review auth", None, SessionStatus::Running);
            match declared {
                Some(hint) => {
                    info.hint = Some(AgentHint {
                        state: hint,
                        label: None,
                        received_at_ms: NOW,
                    });
                }
                // The one state no hint can declare: the child died.
                None => info.status = SessionStatus::Exited { code: Some(2) },
            }

            let mut st = state();
            st.daemon.sessions = to_views(vec![info]);
            st.open(SessionId(7), NOW);
            let row = st.row(SessionId(7)).expect("the session is in the list");
            let pill = crate::inbox::Pill::of(row);
            covered.insert(pill.status);

            let secondary = context(&st).secondary;
            assert!(
                secondary
                    .split("  \u{00b7}  ")
                    .any(|part| part == pill.word),
                "the bar says {secondary:?}, the pill says {:?}",
                pill.word
            );
        }

        for status in ALL_STATUSES {
            assert!(
                covered.contains(&status),
                "{status:?} is a state the bar can name freely: no case here produces it"
            );
        }
    }

    /// The status must never be drawn twice in this bar.
    ///
    /// It used to be a coloured dot AND the word beside it, for the same
    /// session, ten pixels apart, while the sidebar row for that session
    /// carried a third copy of the same dot. The word is the one that
    /// survives, because it is the one that needs no colour key.
    #[test]
    fn the_titlebar_states_the_status_once_in_words() {
        let mut st = state();
        st.daemon.sessions = to_views(vec![session(
            7,
            "review auth",
            None,
            SessionStatus::Exited { code: Some(2) },
        )]);
        st.open(SessionId(7), NOW);
        let ctx = context(&st);
        assert!(
            ctx.secondary.ends_with("exited 2"),
            "the status must still be stated: {}",
            ctx.secondary
        );
        assert!(
            !ctx.primary.contains("exited"),
            "the title slot must carry the title and nothing else"
        );
    }

    /// No branch means no empty separator. A line reading
    /// "vitrum  ·    ·  running" is a bug you can see from across the room.
    #[test]
    fn a_session_without_a_branch_has_no_empty_separator() {
        let mut st = state();
        st.daemon.sessions = to_views(vec![session(7, "scratch", None, SessionStatus::Running)]);
        st.open(SessionId(7), NOW);
        assert_eq!(context(&st).secondary, "vitrum  \u{00b7}  Working");
    }

    /// A session in a project the client has not received yet must still show
    /// its own name and status. Dropping the whole line because one lookup
    /// missed would blank the bar during the race between the two snapshots.
    #[test]
    fn a_session_with_an_unknown_project_still_shows_its_status() {
        let mut st = UiState::default();
        st.daemon.sessions = to_views(vec![session(
            7,
            "orphan",
            None,
            SessionStatus::Exited { code: Some(2) },
        )]);
        st.open(SessionId(7), NOW);
        let ctx = context(&st);
        assert_eq!(ctx.primary, "orphan");
        assert_eq!(ctx.secondary, "Failed  \u{00b7}  exited 2");
    }

    /// Focus pointing at a session that no longer exists must produce a real
    /// message, not an empty bar. This happens for one paint between a close
    /// and the snapshot that prunes it.
    #[test]
    fn focus_on_a_vanished_session_says_so() {
        let mut st = state();
        st.window.focused = Some(SessionId(99));
        assert_eq!(
            context(&st),
            Context {
                primary: String::new(),
                secondary: "session closing".into(),
            }
        );
    }

    /// A healthy socket takes a dot and NO word. This is the whole reason the
    /// connection banner could be deleted: the state it spent a permanent
    /// full-width band announcing is the state with nothing to announce.
    #[test]
    fn a_healthy_connection_is_a_dot_and_nothing_else() {
        let link = conn(
            &ConnState::Live {
                server_version: "0.1.0".into(),
            },
            "ws://127.0.0.1:7777",
        );
        assert_eq!(link.word, "", "a working socket must not spend a word");
        assert!(!link.retryable);
        assert!(
            link.title.contains("0.1.0"),
            "the version belongs in the tooltip: {}",
            link.title
        );
    }

    /// Every state that is NOT healthy must say a word. A dot alone is a
    /// colour key the operator has to have learned, and "your sessions are
    /// running somewhere this window cannot see" is not a thing to encode in
    /// seven pixels of red.
    #[test]
    fn every_unhealthy_connection_says_a_word() {
        for state in [
            ConnState::Connecting,
            ConnState::Failed {
                detail: "refused".into(),
            },
            ConnState::Fixture,
        ] {
            let link = conn(&state, "ws://127.0.0.1:7777");
            assert!(!link.word.is_empty(), "{state:?} drew a bare dot");
        }
    }

    /// Only a failure offers Retry, and it must, because the button is the
    /// only pointer-reachable way back from a dropped socket now that the
    /// sidebar banner is gone.
    #[test]
    fn only_a_failure_offers_retry() {
        assert!(
            conn(
                &ConnState::Failed {
                    detail: "refused".into()
                },
                "u"
            )
            .retryable
        );
        for state in [
            ConnState::Connecting,
            ConnState::Fixture,
            ConnState::Live {
                server_version: "0.1.0".into(),
            },
        ] {
            assert!(!conn(&state, "u").retryable, "{state:?} offered Retry");
        }
    }

    /// Fixture data must never be quiet. A window showing invented sessions
    /// that looks exactly like one showing real ones is the single worst thing
    /// this program could do to somebody debugging it.
    #[test]
    fn fixture_mode_is_loud() {
        let link = conn(&ConnState::Fixture, "u");
        assert_eq!(link.word, "Fixture data");
        assert_eq!(link.class, "rg-conn--fixture");
    }

    /// Every surface that names a session state reads the shared vocabulary.
    ///
    /// The sibling test above proves the bar and the pill agree for the
    /// session it renders. It cannot see the way the defect got in: a surface
    /// that never asks [`crate::inbox::status_word`] at all and simply types
    /// the word into its markup. That copy agrees with the pill on the day it
    /// is written and drifts the first time a word is retuned, which is the
    /// same two-names-for-one-state the class is about, arriving through a
    /// door the rendered comparison does not watch.
    ///
    /// So this reads the surfaces as source and forbids the vocabulary as a
    /// literal. `inbox.rs` is not on the list because it OWNS the words and is
    /// the one file allowed to spell them.
    ///
    /// The word list is derived, not transcribed: `EVERY_WORD` is mapped
    /// through `status_word`, and `every_state_word_is_listed` below is an
    /// exhaustive match, so adding a [`StateWord`] fails to compile here until
    /// it is listed and the scan cannot silently stop covering one.
    ///
    /// What it does NOT catch: a surface that computes the word some other
    /// way, or one that reads a second helper returning the same strings.
    /// Comments and test code are skipped, so prose may still say "Working".
    #[test]
    fn no_surface_writes_a_status_word_of_its_own() {
        use crate::inbox::{StateWord, status_word};

        /// Exhaustive by the compiler: see `every_state_word_is_listed`.
        const EVERY_WORD: [StateWord; 7] = [
            StateWord::Approval,
            StateWord::Input,
            StateWord::Working,
            StateWord::Failed,
            StateWord::Ready,
            StateWord::Woke,
            StateWord::Done,
        ];

        /// Every surface that can put a session's state in front of a person.
        const SURFACES: &[(&str, &str)] = &[
            ("ui/titlebar/native.rs", include_str!("titlebar/native.rs")),
            ("ui/sidebar/widgets.rs", include_str!("sidebar/widgets.rs")),
            ("ui/menu/native.rs", include_str!("menu/native.rs")),
            ("ui/search/native.rs", include_str!("search/native.rs")),
            ("ui/dialog/native.rs", include_str!("dialog/native.rs")),
            ("ui/settings/sheet.rs", include_str!("settings/sheet.rs")),
            ("ui/settings/spec.rs", include_str!("settings/spec.rs")),
            ("ui/panebar.rs", include_str!("panebar.rs")),
            ("ui/toast.rs", include_str!("toast.rs")),
            ("ui/mod.rs", include_str!("mod.rs")),
        ];

        for (name, src) in SURFACES {
            // Production markup only. A test may spell a word, because
            // asserting the word is what a test of the word looks like.
            // Anchored on the test module declaration, not the bare
            // attribute: several of these files carry `#[cfg(test)]` on
            // individual items, and cutting at the first one silently
            // shrinks the scan to nothing and passes.
            let code = src
                .split_once("\n#[cfg(test)]\nmod ")
                .map_or(*src, |(a, _)| a);
            assert!(
                code.len() > src.len() / 2,
                "the scan of {name} collapsed to {} of {} bytes; the anchor moved",
                code.len(),
                src.len()
            );
            for (n, line) in code.lines().enumerate() {
                let line = line.trim_start();
                if line.starts_with("//") {
                    continue;
                }
                for word in EVERY_WORD {
                    let text = status_word(word);
                    // A string literal, not the identifier: `StateWord::Ready`
                    // and `SidebarStatus::Ready` are the shared vocabulary
                    // being used correctly, and only a quoted copy is a
                    // second one.
                    for quoted in [format!("\"{text}\""), format!(" {text}\"")] {
                        assert!(
                            !line.contains(&quoted),
                            "{name}:{} writes the state word {text:?} itself; \
                             read crate::inbox::status_word instead so one \
                             state cannot be called two things: {line}",
                            n + 1
                        );
                    }
                }
            }
        }
    }

    /// Adding a [`StateWord`] must break the scan above until it is listed.
    ///
    /// The match is the whole point: a hardcoded word list goes stale in
    /// silence, which is the same failure as having no guard.
    #[test]
    fn every_state_word_is_listed() {
        use crate::inbox::StateWord;
        fn rank(word: StateWord) -> usize {
            match word {
                StateWord::Approval => 0,
                StateWord::Input => 1,
                StateWord::Working => 2,
                StateWord::Failed => 3,
                StateWord::Ready => 4,
                StateWord::Woke => 5,
                StateWord::Done => 6,
            }
        }
        for status in vitrum_model::ALL_STATUSES {
            assert!(rank(StateWord::of(status)) < 7);
        }
        assert_eq!(rank(StateWord::Done), 6, "the vocabulary grew");
    }

}
