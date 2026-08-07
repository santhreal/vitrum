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

use dioxus::prelude::*;

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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    let Some(info) = st.session(id) else {
        // The focused id outlived its row for one paint, between a close and
        // the snapshot that prunes it. Saying so beats a blank bar.
        return Context {
            primary: String::new(),
            secondary: "session closing".to_string(),
        };
    };
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
    parts.push(status_label(&info.status));
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

#[derive(Props, Clone, PartialEq)]
pub struct TitleBarProps {
    pub state: Signal<UiState>,
    /// Daemon URL, for the connection tooltip.
    pub server: &'static str,
    /// The workspace switcher, built by [`crate::ui::workspaces`]. Passed in
    /// rather than constructed here so this file stays about the window frame
    /// and does not grow a second opinion about workspaces.
    pub switcher: Element,
    pub on_drag: EventHandler<()>,
    pub on_toggle_maximize: EventHandler<()>,
    pub on_minimize: EventHandler<()>,
    pub on_close: EventHandler<()>,
    pub on_shortcuts: EventHandler<()>,
    pub on_retry: EventHandler<()>,
    /// Version string for the quiet update chip. `None` means nothing to say.
    ///
    /// A plain value rather than the offer signal so a change in the parent
    /// always reaches this frame: the titlebar's props compare equal across
    /// signal writes that do not change any other field, and a skipped
    /// re-render would leave a ready update invisible.
    pub update_version: Option<String>,
    /// Open Settings → About so Install does not need a second Check.
    pub on_update: EventHandler<()>,
    /// Persist a dismissal for this exact version and hide the chip.
    pub on_dismiss_update: EventHandler<()>,
}

#[component]
pub fn TitleBar(props: TitleBarProps) -> Element {
    let (ctx, link) = {
        let st = props.state.read();
        (context(&st), conn(&st.daemon.conn, props.server))
    };
    let inset = if DRAWS_WINDOW_CONTROLS {
        0.0
    } else {
        MACOS_TRAFFIC_LIGHT_INSET
    };
    let update_version = props.update_version.clone();

    rsx! {
        div {
            class: "rg-titlebar",
            style: "padding-left: calc(var(--rg-space-3) + {inset}px)",
            // The drag region is the bar itself minus its buttons. tao's
            // `drag_window` hands the gesture to the window manager, which is
            // what makes tiling, snapping and the shake gesture keep working;
            // moving the window by hand from mousemove deltas does not.
            onmousedown: move |e| {
                use dioxus::html::input_data::MouseButton;
                if e.trigger_button() == Some(MouseButton::Primary) {
                    props.on_drag.call(());
                }
            },
            ondoubleclick: move |_| props.on_toggle_maximize.call(()),

            span { class: "rg-titlebar__brand", "{WORDMARK}" }

            {props.switcher}

            div { class: "rg-titlebar__context",
                // No status dot here. The status is already spelled out in
                // words two elements along ("running", "exited 3"), and the
                // sidebar row for the same session carries its own dot beside
                // its own title. A third marker for one fact is the kind of
                // repetition that makes a bar read as clutter.
                if !ctx.primary.is_empty() {
                    span { class: "rg-titlebar__primary", "{ctx.primary}" }
                }
                span { class: "rg-titlebar__secondary", "{ctx.secondary}" }
            }

            div { class: "rg-titlebar__actions",
                // A quiet chip, not a modal. The About tab owns Install; this
                // only says a newer release exists and gets out of the way
                // when dismissed. Sitting before the connection mark keeps
                // the window-control corner free of a moving target.
                if let Some(version) = update_version {
                    div { class: "rg-update",
                        button {
                            class: "rg-update__open",
                            r#type: "button",
                            title: "Open Settings → About to install vitrum {version}",
                            aria_label: "Update available: vitrum {version}",
                            onmousedown: move |e| e.stop_propagation(),
                            onclick: move |_| props.on_update.call(()),
                            "Update {version}"
                        }
                        button {
                            class: "rg-update__dismiss",
                            r#type: "button",
                            title: "Dismiss until a later release",
                            aria_label: "Dismiss update {version}",
                            onmousedown: move |e| e.stop_propagation(),
                            onclick: move |_| props.on_dismiss_update.call(()),
                            "×"
                        }
                    }
                }
                // The whole connection banner, folded to this. It is a
                // non-interactive marker while the socket is healthy and grows
                // a word and a Retry the moment it is not.
                div {
                    class: "rg-conn {link.class}",
                    title: "{link.title}",
                    span { class: "rg-conn__dot" }
                    if !link.word.is_empty() {
                        span { class: "rg-conn__word", "{link.word}" }
                    }
                    if link.retryable {
                        button {
                            class: "rg-btn-inline",
                            r#type: "button",
                            onmousedown: move |e| e.stop_propagation(),
                            onclick: move |_| props.on_retry.call(()),
                            "Retry"
                        }
                    }
                }
                // No `+` here.
                //
                // The sidebar footer's control does this and says which agent
                // it will start; this one was a bare glyph offering the same
                // action, on a band that already carries a wordmark, a
                // workspace switcher, a context line, a connection state and
                // the window controls. It survives the sidebar's 3rem
                // collapsed rail, so nothing here was reachable that is not
                // reachable there, and Ctrl+Shift+N opens the list from
                // anywhere.
                button {
                    class: "rg-titlebar__action",
                    r#type: "button",
                    title: "Keyboard shortcuts (F1)",
                    aria_label: "Keyboard shortcuts",
                    onmousedown: move |e| e.stop_propagation(),
                    onclick: move |_| props.on_shortcuts.call(()),
                    "?"
                }
            }

            if DRAWS_WINDOW_CONTROLS {
                div { class: "rg-window-controls",
                    button {
                        class: "rg-window-control",
                        r#type: "button",
                        title: "Minimise",
                        aria_label: "Minimise",
                        onmousedown: move |e| e.stop_propagation(),
                        onclick: move |_| props.on_minimize.call(()),
                        span { class: "rg-window-control__glyph", "\u{2500}" }
                    }
                    button {
                        class: "rg-window-control",
                        r#type: "button",
                        title: "Maximise",
                        aria_label: "Maximise",
                        onmousedown: move |e| e.stop_propagation(),
                        onclick: move |_| props.on_toggle_maximize.call(()),
                        span { class: "rg-window-control__glyph", "\u{25a1}" }
                    }
                    button {
                        class: "rg-window-control rg-window-control--close",
                        r#type: "button",
                        title: "Close",
                        aria_label: "Close",
                        onmousedown: move |e| e.stop_propagation(),
                        onclick: move |_| props.on_close.call(()),
                        span { class: "rg-window-control__glyph", "\u{00d7}" }
                    }
                }
            }
        }
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
            unread: false,
            attention: Attention::default(),
            hint: None,
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
            "vitrum  \u{00b7}  feat/auth  \u{00b7}  running"
        );
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
        assert_eq!(context(&st).secondary, "vitrum  \u{00b7}  running");
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
        assert_eq!(ctx.secondary, "exited 2");
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

    /// The quiet update chip must name the version it offers. A bare "Update"
    /// next to the connection mark is a rumour; the version is the fact.
    #[test]
    fn the_update_chip_names_the_version() {
        use dioxus::prelude::*;
        #[derive(Props, Clone, PartialEq)]
        struct HarnessProps {
            version: Option<String>,
        }
        #[component]
        fn Harness(props: HarnessProps) -> Element {
            let state = use_signal(UiState::default);
            rsx! {
                TitleBar {
                    state,
                    server: "ws://127.0.0.1:1",
                    switcher: rsx! { span {} },
                    on_drag: move |()| {},
                    on_toggle_maximize: move |()| {},
                    on_minimize: move |()| {},
                    on_close: move |()| {},
                    on_shortcuts: move |()| {},
                    on_retry: move |()| {},
                    update_version: props.version,
                    on_update: move |()| {},
                    on_dismiss_update: move |()| {},
                }
            }
        }
        let mut dom = VirtualDom::new_with_props(
            Harness,
            HarnessProps {
                version: Some("9.9.9".into()),
            },
        );
        dom.rebuild_in_place();
        let html = dioxus_ssr::render(&dom);
        assert!(
            html.contains("Update 9.9.9"),
            "chip missing from {html}"
        );
        assert!(html.contains("rg-update"), "chip class missing from {html}");
    }

}
