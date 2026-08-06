//! Native notifications for the three moments an agent needs a human.
//!
//! The payload is built by pure code shared across every backend, and each
//! backend's wire form ([`Notification::dbus_args`], [`Notification::toast_xml`],
//! [`Notification::mac_plan`]) is a value rather than a side effect. That is
//! what makes the Windows toast XML and the macOS request assertable from a
//! Linux test run: the part that is easy to get subtly wrong is the part that
//! is testable everywhere, and only the four lines that hand the value to the
//! OS are platform-gated.
//!
//! Body text comes from PTY output, so it is sanitised before it goes anywhere:
//! an agent that prints a colour escape or a stray `<b>` must not be able to
//! corrupt a notification, and on GNOME an unescaped `&` makes the daemon drop
//! the body entirely.
//!
//! Activation carries the session. Every backend routes a click through a
//! `vitrum://session/<id>` URL, which means the click path and the deep-link
//! path are the same code and the same parser.

use core::fmt;
use std::sync::Arc;

use vitrum_proto::SessionId;

use crate::branding::{APP_DISPLAY_NAME, APP_NAME, BUNDLE_ID, ICON_NAME};
use crate::capability::{Support, Unavailable};
use crate::deeplink::DeepLink;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::DbusNotifier;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Longest notification title. Past this every desktop truncates anyway, and
/// truncating here means the ellipsis lands where we chose it to.
pub(crate) const MAX_TITLE_CHARS: usize = 72;
/// Longest notification body.
pub(crate) const MAX_BODY_CHARS: usize = 240;

/// Which of the three moments this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NotificationKind {
    /// The agent's turn ended and the operator has not looked.
    Finished,
    /// The agent is waiting on a yes or no. This is the one that must not be
    /// missed, so it is the one that never auto-dismisses.
    NeedsApproval,
    /// The child exited nonzero or was signalled.
    Failed,
}

impl NotificationKind {
    /// Suffix appended to the session label to form the title.
    pub const fn title_suffix(self) -> &'static str {
        match self {
            Self::Finished => "finished",
            Self::NeedsApproval => "needs approval",
            Self::Failed => "failed",
        }
    }

    /// How loudly the desktop should present it.
    pub const fn urgency(self) -> Urgency {
        match self {
            Self::Finished => Urgency::Normal,
            Self::NeedsApproval | Self::Failed => Urgency::Critical,
        }
    }

    /// Vendor-namespaced freedesktop category, per the notification spec's
    /// `x-vendor.class.specific` form.
    pub fn dbus_category(self) -> String {
        let leaf = match self {
            Self::Finished => "finished",
            Self::NeedsApproval => "approval",
            Self::Failed => "failed",
        };
        format!("x-{APP_NAME}.session.{leaf}")
    }

    /// Stable machine token, used in reports and tests. Hyphenated, unlike
    /// the prose label a user sees.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Finished => "finished",
            Self::NeedsApproval => "needs-approval",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for NotificationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad`, not `write_str`: a report column uses `{:<16}` and
        // `write_str` silently discards the width.
        f.pad(self.as_str())
    }
}

/// freedesktop urgency levels, reused verbatim by the other two backends
/// because the concept is identical and the mapping is one match arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Urgency {
    /// Informational. Some servers show it without a popup at all.
    Low,
    /// A popup that expires on its own.
    Normal,
    /// Never expires on a spec-compliant server and is exempt from
    /// do-not-disturb. Reserved for state that blocks the agent, because a
    /// desktop that learns to ignore these has lost the one signal that
    /// mattered.
    Critical,
}

impl Urgency {
    /// The byte the `urgency` hint carries.
    pub const fn dbus_byte(self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Normal => 1,
            Self::Critical => 2,
        }
    }
}

/// A notification, fully resolved and safe to hand to any backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// Which session this is about. Carried through activation, so a click
    /// arrives at the handler as this id and nothing else has to be looked up.
    pub session: SessionId,
    /// What happened. Fixes the urgency, the category and how long the popup
    /// stays up, so no backend decides any of that for itself.
    pub kind: NotificationKind,
    /// Already sanitised and length-limited.
    pub title: String,
    /// Already sanitised and length-limited. May be empty when there was
    /// nothing worth saying beyond the title.
    pub body: String,
}

impl Notification {
    /// Build from a raw session title and a raw detail line, both of which may
    /// be arbitrary PTY bytes.
    pub fn new(
        kind: NotificationKind,
        session: SessionId,
        session_title: &str,
        detail: &str,
    ) -> Self {
        let label = session_label(session, session_title);
        Self {
            session,
            kind,
            title: truncate_chars(&format!("{label} {}", kind.title_suffix()), MAX_TITLE_CHARS),
            body: truncate_chars(&sanitize_pty_text(detail), MAX_BODY_CHARS),
        }
    }

    /// "the agent finished".
    pub fn finished(session: SessionId, session_title: &str, detail: &str) -> Self {
        Self::new(NotificationKind::Finished, session, session_title, detail)
    }

    /// "the agent needs approval".
    pub fn needs_approval(session: SessionId, session_title: &str, detail: &str) -> Self {
        Self::new(NotificationKind::NeedsApproval, session, session_title, detail)
    }

    /// "the agent failed".
    pub fn failed(session: SessionId, session_title: &str, detail: &str) -> Self {
        Self::new(NotificationKind::Failed, session, session_title, detail)
    }

    /// Shorthand for the urgency implied by `kind`.
    pub fn urgency(&self) -> Urgency {
        self.kind.urgency()
    }

    /// The URL a click resolves to. Identical to what a browser-issued deep
    /// link would carry, on purpose: one activation path, one parser.
    pub fn activation_url(&self) -> String {
        DeepLink::Session(self.session).to_url()
    }

    /// Arguments for `org.freedesktop.Notifications.Notify`.
    ///
    /// `replaces_id` of 0 means "new notification"; passing back an id from a
    /// previous call updates it in place, which is how repeated updates about
    /// one session avoid stacking up twelve deep.
    pub fn dbus_args(&self, replaces_id: u32) -> DbusNotifyArgs {
        DbusNotifyArgs {
            app_name: APP_DISPLAY_NAME.to_string(),
            replaces_id,
            app_icon: ICON_NAME.to_string(),
            summary: escape_body_markup(&self.title),
            body: escape_body_markup(&self.body),
            // "default" is the spec's key for "the body itself was clicked".
            // Offered here because a payload is a pure value and cannot know
            // whether anything is listening; the D-Bus backend strips both keys
            // when no click can be routed, so an unrouted notification never
            // renders a button.
            actions: vec!["default".to_string(), "Show".to_string()],
            hints: vec![
                ("urgency".to_string(), HintValue::Byte(self.urgency().dbus_byte())),
                ("desktop-entry".to_string(), HintValue::Str(APP_NAME.to_string())),
                ("category".to_string(), HintValue::Str(self.kind.dbus_category())),
                (
                    format!("x-{APP_NAME}-session"),
                    HintValue::Str(self.session.0.to_string()),
                ),
            ],
            // A critical notification that auto-dismisses is a missed approval.
            expire_timeout: match self.urgency() {
                Urgency::Critical => 0,
                _ => -1,
            },
        }
    }

    /// The WinRT toast XML document.
    ///
    /// `launch` carries the deep link, which is what
    /// `ToastNotificationManager` hands back in the activation event, so the
    /// Windows click path reuses the same parser as everything else.
    pub fn toast_xml(&self) -> String {
        let scenario = match self.urgency() {
            Urgency::Critical => " scenario=\"urgent\"",
            _ => "",
        };
        let mut xml = String::with_capacity(256);
        xml.push_str("<toast launch=\"");
        xml.push_str(&escape_xml(&self.activation_url()));
        xml.push_str("\" activationType=\"foreground\"");
        xml.push_str(scenario);
        xml.push_str("><visual><binding template=\"ToastGeneric\"><text>");
        xml.push_str(&escape_xml(&self.title));
        xml.push_str("</text>");
        if !self.body.is_empty() {
            xml.push_str("<text>");
            xml.push_str(&escape_xml(&self.body));
            xml.push_str("</text>");
        }
        xml.push_str("</binding></visual></toast>");
        xml
    }

    /// The `UNMutableNotificationContent` fields and request identifier.
    pub fn mac_plan(&self) -> MacNotificationPlan {
        MacNotificationPlan {
            // One identifier per session per kind: a second "finished" for the
            // same session replaces the first instead of stacking.
            identifier: format!("{BUNDLE_ID}.{}.{}", self.kind.as_str(), self.session.0),
            title: self.title.clone(),
            body: self.body.clone(),
            // Groups every notification from this app into one thread in
            // Notification Center.
            thread_identifier: BUNDLE_ID.to_string(),
            user_info: vec![
                ("url".to_string(), self.activation_url()),
                ("session".to_string(), self.session.0.to_string()),
            ],
            interruption_level: match self.urgency() {
                Urgency::Critical => MacInterruptionLevel::TimeSensitive,
                _ => MacInterruptionLevel::Active,
            },
        }
    }
}

/// A value in the `a{sv}` hints dictionary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HintValue {
    /// D-Bus `y`, the type the `urgency` hint is required to carry.
    Byte(u8),
    /// D-Bus `b`, for flags such as `resident` and `transient`.
    Bool(bool),
    /// D-Bus `i`, for the `x` and `y` positioning hints.
    Int32(i32),
    /// D-Bus `s`, for `category`, `desktop-entry` and `sound-name`.
    Str(String),
}

/// Exactly the eight arguments of `org.freedesktop.Notifications.Notify`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbusNotifyArgs {
    /// Sender name. Must match the desktop entry's name or the server will
    /// not resolve the application icon.
    pub app_name: String,
    /// Server id of a notification to update in place, or `0` to post a new
    /// one. Updating is how a session's progress avoids stacking popups.
    pub replaces_id: u32,
    /// A themed icon name or a file URI. Empty leaves the choice to the
    /// server.
    pub app_icon: String,
    /// The single line of the popup, already sanitised.
    pub summary: String,
    /// Detail below the summary, already sanitised. Servers advertising the
    /// `body-markup` capability parse this as markup, which is why the text
    /// reaching here must not carry raw `<`.
    pub body: String,
    /// Flat `[key, label, key, label, ...]`, as the wire format requires.
    pub actions: Vec<String>,
    /// The `a{sv}` dictionary, in the order it should be serialised.
    pub hints: Vec<(String, HintValue)>,
    /// Milliseconds; `0` never expires, `-1` lets the server decide.
    pub expire_timeout: i32,
}

impl DbusNotifyArgs {
    /// Look a hint up by key.
    pub fn hint(&self, key: &str) -> Option<&HintValue> {
        self.hints.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

/// macOS `UNNotificationInterruptionLevel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacInterruptionLevel {
    /// Delivered normally; suppressed by Focus.
    Active,
    /// Breaks through Focus, which is what an approval prompt is for.
    TimeSensitive,
}

impl MacInterruptionLevel {
    /// The raw value of the `UNNotificationInterruptionLevel` enum.
    pub const fn raw(self) -> isize {
        match self {
            Self::Active => 1,
            Self::TimeSensitive => 2,
        }
    }
}

/// Everything the macOS backend needs, computed without touching AppKit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacNotificationPlan {
    /// `UNNotificationRequest` identifier. Reusing one replaces the delivered
    /// notification, the macOS equivalent of a D-Bus `replaces_id`.
    pub identifier: String,
    /// The bold first line, already sanitised.
    pub title: String,
    /// Detail, already sanitised. May be empty.
    pub body: String,
    /// Groups one session's notifications into a single stack in Notification
    /// Center instead of a run of separate rows.
    pub thread_identifier: String,
    /// Handed back verbatim on activation. How a click finds its session
    /// without the process keeping a side table alive.
    pub user_info: Vec<(String, String)>,
    /// Whether this is allowed to break through Focus.
    pub interruption_level: MacInterruptionLevel,
}

/// A human-readable name for a session, or a stable fallback.
///
/// An untitled session is normal early in its life, and "` finished`" with a
/// leading space is the kind of detail that makes a product feel unfinished.
pub fn session_label(session: SessionId, title: &str) -> String {
    let clean = sanitize_pty_text(title);
    if clean.is_empty() {
        format!("Session {}", session.0)
    } else {
        clean
    }
}

/// Strip terminal control sequences and collapse whitespace.
///
/// Notification text is PTY output. Without this a body reads
/// `\u{1b}[32mdone\u{1b}[0m`, a title can contain a newline that the daemon
/// renders as a box glyph, and a NUL byte can truncate the string inside the
/// daemon rather than inside us.
///
/// Handles CSI (`ESC [ ... final`), OSC and the other string-terminated
/// sequences (`ESC ] / P / X / ^ / _ ... BEL | ESC \`), and two-character
/// designators (`ESC ( B`). Anything else after `ESC` drops just the `ESC`.
pub(crate) fn sanitize_pty_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(if c.is_control() { ' ' } else { c });
            continue;
        }
        match chars.peek().copied() {
            Some('[') => {
                chars.next();
                // Parameter and intermediate bytes, then one final byte.
                for f in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&f) {
                        break;
                    }
                }
            }
            Some(']' | 'P' | 'X' | '^' | '_') => {
                chars.next();
                // Runs until BEL or ST (ESC \).
                while let Some(f) = chars.next() {
                    if f == '\u{7}' {
                        break;
                    }
                    if f == '\u{1b}' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            Some('(' | ')' | '*' | '+' | '#' | '%') => {
                chars.next();
                chars.next();
            }
            _ => {}
        }
        out.push(' ');
    }

    let mut collapsed = String::with_capacity(out.len());
    let mut pending_space = false;
    for c in out.chars() {
        if c == ' ' {
            pending_space = !collapsed.is_empty();
            continue;
        }
        if pending_space {
            collapsed.push(' ');
            pending_space = false;
        }
        collapsed.push(c);
    }
    collapsed
}

/// Escape the three characters that break the notification spec's markup
/// subset.
///
/// Only `&`, `<` and `>`: quotes are significant only inside a tag attribute
/// and our bodies never contain a tag, so escaping them would show a literal
/// `&quot;` to the user for no gain.
pub(crate) fn escape_body_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape for an XML text node or a double-quoted attribute value.
///
/// The toast `launch` attribute is quoted, so quotes must go too.
pub(crate) fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Truncate to a character count, appending a single-character ellipsis.
///
/// Character-based, not byte-based: slicing a UTF-8 string at a byte offset
/// panics, and agent output is full of box-drawing and CJK.
pub fn truncate_chars(s: &str, max: usize) -> String {
    debug_assert!(max > 0, "a zero-length limit cannot hold the ellipsis");
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

/// Handle to a delivered notification, for later replacement or dismissal.
///
/// Opaque because the three platforms number them differently: a `u32` on
/// D-Bus, a string identifier on macOS, a tag on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NotificationHandle(pub u64);

/// Called when the user clicks a notification.
pub type ActivationHandler = Arc<dyn Fn(SessionId) + Send + Sync>;

/// What a platform notification backend does.
pub trait Notifier: Send + Sync {
    /// Whether the backend can deliver right now, and why not if it cannot.
    fn capability(&self) -> Support;

    /// Deliver. Returns a handle usable with [`Notifier::close`].
    fn notify(&self, notification: &Notification) -> Result<NotificationHandle, Unavailable>;

    /// Withdraw a delivered notification.
    fn close(&self, handle: NotificationHandle) -> Result<(), Unavailable>;

    /// Install the click handler. Replaces any previous one.
    ///
    /// Until this is called, a backend has nowhere to send a click, and the
    /// D-Bus backend answers that by advertising no actions at all. Install it
    /// before the first [`Notifier::notify`] or the first notification is the
    /// one that cannot be clicked.
    fn set_activation_handler(&self, handler: ActivationHandler) -> Result<(), Unavailable>;
}

/// Connect to this platform's notification service.
///
/// Fails rather than returning a silent no-op sink, so a caller that wants to
/// grey out a "notify me" toggle can.
pub fn notifier() -> Result<Box<dyn Notifier>, Unavailable> {
    #[cfg(target_os = "linux")]
    {
        linux::DbusNotifier::connect().map(|n| Box::new(n) as Box<dyn Notifier>)
    }
    #[cfg(target_os = "macos")]
    {
        macos::MacNotifier::connect().map(|n| Box::new(n) as Box<dyn Notifier>)
    }
    #[cfg(target_os = "windows")]
    {
        windows::ToastNotifier::connect().map(|n| Box::new(n) as Box<dyn Notifier>)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err(Unavailable::not_implemented(format!(
            "no notification backend is compiled for {}",
            std::env::consts::OS
        )))
    }
}
