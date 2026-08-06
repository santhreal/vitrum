//! Notification payload construction for all three platforms.
//!
//! The payloads are values, so the D-Bus arguments, the WinRT toast XML and the
//! `UNNotificationRequest` fields are all asserted here from one test run. The
//! platform-specific part of each backend is the four lines that hand the value
//! to the OS.

use vitrum_proto::SessionId;

use crate::deeplink::{self, DeepLink};
use crate::notify::{
    HintValue, MAX_BODY_CHARS, MAX_TITLE_CHARS, MacInterruptionLevel, Notification,
    NotificationKind, Urgency, escape_body_markup, escape_xml, sanitize_pty_text, session_label,
    truncate_chars,
};

/// The three kinds must produce the three documented titles.
///
/// These strings are what the user reads on a lock screen. A missing space or a
/// capitalisation change is the difference between a product and a prototype.
#[test]
fn each_kind_produces_its_title() {
    let s = SessionId(7);
    assert_eq!(Notification::finished(s, "build", "").title, "build finished");
    assert_eq!(Notification::needs_approval(s, "build", "").title, "build needs approval");
    assert_eq!(Notification::failed(s, "build", "").title, "build failed");
}

/// An untitled session must get a stable fallback label, not a leading space.
///
/// A session is untitled for its first few seconds of life, which is exactly
/// when a fast-failing command notifies. " failed" is a visible defect.
#[test]
fn an_untitled_session_falls_back_to_its_id() {
    assert_eq!(session_label(SessionId(12), ""), "Session 12");
    assert_eq!(session_label(SessionId(12), "   "), "Session 12");
    // A title that sanitises away to nothing is also untitled.
    assert_eq!(session_label(SessionId(12), "\u{1b}[2K"), "Session 12");
    assert_eq!(Notification::failed(SessionId(12), "", "").title, "Session 12 failed");
}

/// Urgency must follow the kind, and approval must be critical.
///
/// A normal-urgency approval prompt is dismissed by the desktop after a few
/// seconds, so the agent sits blocked while the one notification that mattered
/// has already vanished.
#[test]
fn approval_and_failure_are_critical() {
    assert_eq!(NotificationKind::Finished.urgency(), Urgency::Normal);
    assert_eq!(NotificationKind::NeedsApproval.urgency(), Urgency::Critical);
    assert_eq!(NotificationKind::Failed.urgency(), Urgency::Critical);
    assert_eq!(Urgency::Low.dbus_byte(), 0);
    assert_eq!(Urgency::Normal.dbus_byte(), 1);
    assert_eq!(Urgency::Critical.dbus_byte(), 2);
}

/// The D-Bus arguments must be exactly these.
///
/// All eight are positional on the wire. Swapping `summary` and `body`, or
/// dropping the `desktop-entry` hint that lets the desktop attribute and group
/// the notification, produces something that still "works" and is subtly wrong.
#[test]
fn the_dbus_arguments_are_exactly_this() {
    let n = Notification::finished(SessionId(7), "build", "exit 0");
    let args = n.dbus_args(0);
    assert_eq!(args.app_name, "Vitrum");
    assert_eq!(args.replaces_id, 0);
    assert_eq!(args.app_icon, "vitrum");
    assert_eq!(args.summary, "build finished");
    assert_eq!(args.body, "exit 0");
    assert_eq!(args.actions, vec!["default".to_string(), "Show".to_string()]);
    assert_eq!(args.hint("urgency"), Some(&HintValue::Byte(1)));
    assert_eq!(args.hint("desktop-entry"), Some(&HintValue::Str("vitrum".to_string())));
    assert_eq!(
        args.hint("category"),
        Some(&HintValue::Str("x-vitrum.session.finished".to_string()))
    );
    assert_eq!(args.hint("x-vitrum-session"), Some(&HintValue::Str("7".to_string())));
    assert_eq!(args.expire_timeout, -1);
}

/// A critical notification must never expire on its own.
///
/// `-1` means "server default", which on GNOME is a few seconds. For an
/// approval prompt that is a missed notification and a stalled agent.
#[test]
fn a_critical_notification_never_expires() {
    let n = Notification::needs_approval(SessionId(1), "agent", "run rm -rf?");
    let args = n.dbus_args(0);
    assert_eq!(args.expire_timeout, 0);
    assert_eq!(args.hint("urgency"), Some(&HintValue::Byte(2)));
    assert_eq!(
        args.hint("category"),
        Some(&HintValue::Str("x-vitrum.session.approval".to_string()))
    );
}

/// A replacement id must be carried through so updates coalesce.
///
/// Without it, twelve status updates about one session stack twelve
/// notifications deep and bury everything else.
#[test]
fn a_replacement_id_is_carried_through() {
    let n = Notification::finished(SessionId(1), "x", "");
    assert_eq!(n.dbus_args(4242).replaces_id, 4242);
}

/// Session ids beyond `u32` must survive as text in the hint.
///
/// The obvious hint type is an int, and D-Bus ints are 32-bit. A session id is
/// a `u64`, so an int hint truncates silently after four billion sessions or,
/// more realistically, after an id space that does not start at zero.
#[test]
fn a_large_session_id_survives_the_hint() {
    let n = Notification::finished(SessionId(u64::MAX), "x", "");
    assert_eq!(
        n.dbus_args(0).hint("x-vitrum-session"),
        Some(&HintValue::Str("18446744073709551615".to_string()))
    );
}

/// Markup metacharacters in the body must be escaped.
///
/// GNOME parses the body as a markup subset. An unescaped `&` makes the parse
/// fail and the daemon drops the entire body, so an agent that prints
/// `make && test` produces a notification with no text at all.
#[test]
fn markup_metacharacters_are_escaped_for_dbus() {
    let n = Notification::failed(SessionId(1), "a<b>", "make && ./x > out");
    let args = n.dbus_args(0);
    assert_eq!(args.summary, "a&lt;b&gt; failed");
    assert_eq!(args.body, "make &amp;&amp; ./x &gt; out");
}

/// Quotes must not be escaped for D-Bus.
///
/// They are only significant inside a tag attribute and our bodies never
/// contain a tag, so escaping them shows the user a literal `&quot;`.
#[test]
fn quotes_are_left_alone_for_dbus() {
    assert_eq!(escape_body_markup("say \"hi\" and 'bye'"), "say \"hi\" and 'bye'");
    assert_eq!(escape_body_markup("a&b<c>d"), "a&amp;b&lt;c&gt;d");
}

/// The toast XML must be exactly this document.
///
/// It is parsed by `XmlDocument::LoadXml`, which rejects malformed input, and
/// the `launch` attribute is the only channel Windows gives us for identifying
/// the clicked session.
#[test]
fn the_toast_xml_is_exactly_this() {
    let n = Notification::finished(SessionId(7), "build", "exit 0");
    assert_eq!(
        n.toast_xml(),
        "<toast launch=\"vitrum://session/7\" activationType=\"foreground\">\
         <visual><binding template=\"ToastGeneric\">\
         <text>build finished</text><text>exit 0</text>\
         </binding></visual></toast>"
    );
}

/// A critical toast must carry the urgent scenario.
#[test]
fn a_critical_toast_is_marked_urgent() {
    let n = Notification::needs_approval(SessionId(1), "agent", "");
    assert_eq!(
        n.toast_xml(),
        "<toast launch=\"vitrum://session/1\" activationType=\"foreground\" scenario=\"urgent\">\
         <visual><binding template=\"ToastGeneric\">\
         <text>agent needs approval</text>\
         </binding></visual></toast>"
    );
}

/// An empty body must omit the second text element entirely.
///
/// An empty `<text></text>` renders as a blank line under the title, which
/// looks like a truncated message.
#[test]
fn an_empty_body_omits_the_second_text_element() {
    let xml = Notification::finished(SessionId(1), "x", "").toast_xml();
    assert_eq!(xml.matches("<text>").count(), 1);
}

/// The toast XML must be escaped, including quotes in the attribute.
///
/// An unescaped `"` in the launch attribute ends the attribute early and the
/// document fails to parse, so no toast appears at all.
#[test]
fn the_toast_xml_escapes_everything_that_breaks_the_parser() {
    let n = Notification::failed(SessionId(3), "a\"b<c>&d'e", "x & y");
    let xml = n.toast_xml();
    assert!(
        xml.contains("<text>a&quot;b&lt;c&gt;&amp;d&apos;e failed</text>"),
        "unescaped title in {xml}"
    );
    assert!(xml.contains("<text>x &amp; y</text>"), "unescaped body in {xml}");
    assert_eq!(escape_xml("&<>\"'"), "&amp;&lt;&gt;&quot;&apos;");
}

/// The toast `launch` attribute must round-trip back to the same session.
///
/// This is the entire Windows activation path: the toast carries the URL, the
/// activation event hands it back, and the deep-link parser turns it into a
/// session. If those three disagree, clicking a toast does nothing.
#[test]
fn the_toast_launch_url_round_trips_to_the_session() {
    let session = SessionId(9_876_543_210);
    let n = Notification::finished(session, "x", "");
    let xml = n.toast_xml();
    let start = xml.find("launch=\"").expect("launch attribute") + "launch=\"".len();
    let end = start + xml[start..].find('"').expect("closing quote");
    assert_eq!(deeplink::parse(&xml[start..end]), Ok(DeepLink::Session(session)));
}

/// The macOS request must carry a per-session-per-kind identifier.
///
/// A shared identifier makes a "finished" replace a pending "needs approval";
/// a unique-per-call identifier stacks twelve notifications for one session.
#[test]
fn the_mac_plan_identifier_is_per_session_per_kind() {
    let a = Notification::finished(SessionId(7), "x", "").mac_plan();
    let b = Notification::finished(SessionId(7), "x", "different body").mac_plan();
    let c = Notification::needs_approval(SessionId(7), "x", "").mac_plan();
    let d = Notification::finished(SessionId(8), "x", "").mac_plan();
    assert_eq!(a.identifier, "dev.santhreal.vitrum.finished.7");
    assert_eq!(a.identifier, b.identifier, "same session and kind must coalesce");
    assert_ne!(a.identifier, c.identifier, "a different kind must not be replaced");
    assert_ne!(a.identifier, d.identifier, "a different session must not be replaced");
}

/// The macOS plan must carry the activation URL and the interruption level.
///
/// `TimeSensitive` is what breaks through Focus. Without it an approval prompt
/// is silently withheld from a user who has Do Not Disturb on, which is most
/// users while they are working.
#[test]
fn the_mac_plan_carries_the_url_and_interruption_level() {
    let plan = Notification::needs_approval(SessionId(5), "agent", "ok?").mac_plan();
    assert_eq!(plan.title, "agent needs approval");
    assert_eq!(plan.body, "ok?");
    assert_eq!(plan.thread_identifier, "dev.santhreal.vitrum");
    assert_eq!(
        plan.user_info,
        vec![
            ("url".to_string(), "vitrum://session/5".to_string()),
            ("session".to_string(), "5".to_string()),
        ]
    );
    assert_eq!(plan.interruption_level, MacInterruptionLevel::TimeSensitive);
    assert_eq!(MacInterruptionLevel::TimeSensitive.raw(), 2);

    let plan = Notification::finished(SessionId(5), "agent", "").mac_plan();
    assert_eq!(plan.interruption_level, MacInterruptionLevel::Active);
    assert_eq!(MacInterruptionLevel::Active.raw(), 1);
}

/// The macOS body must not be markup-escaped.
///
/// `UNNotificationContent.body` is plain text; escaping it shows the user
/// `&amp;`. Only the D-Bus path escapes, which is why escaping happens in the
/// per-backend builder rather than in the constructor.
#[test]
fn the_mac_body_is_plain_text() {
    let plan = Notification::failed(SessionId(1), "a<b>", "x & y").mac_plan();
    assert_eq!(plan.title, "a<b> failed");
    assert_eq!(plan.body, "x & y");
}

/// ANSI escape sequences must be stripped from body text.
///
/// The body is PTY output. Without this the user sees `[32mdone[0m` in a
/// notification, and on some daemons the raw ESC is rendered as a box glyph.
#[test]
fn ansi_sequences_are_stripped() {
    assert_eq!(sanitize_pty_text("\u{1b}[32mdone\u{1b}[0m"), "done");
    assert_eq!(sanitize_pty_text("\u{1b}[1;31;40mred\u{1b}[m"), "red");
    // Erase-line and cursor-position sequences, which agents emit constantly.
    assert_eq!(sanitize_pty_text("\u{1b}[2K\u{1b}[1;1Hhi"), "hi");
}

/// OSC sequences must be stripped whether they end in BEL or in ST.
///
/// A title-setting `OSC 0` is the single most common escape in agent output.
/// Handling only the BEL form leaves the ST form's payload in the body.
#[test]
fn osc_sequences_are_stripped_with_either_terminator() {
    assert_eq!(sanitize_pty_text("\u{1b}]0;window title\u{7}real text"), "real text");
    assert_eq!(sanitize_pty_text("\u{1b}]777;notify;x\u{1b}\\real text"), "real text");
    // DCS, PM and APC use the same string-terminated shape.
    assert_eq!(sanitize_pty_text("\u{1b}Pq#0;2\u{1b}\\after"), "after");
    assert_eq!(sanitize_pty_text("\u{1b}_G a=T\u{1b}\\after"), "after");
}

/// Character-set designators must be stripped as two-character sequences.
#[test]
fn charset_designators_are_stripped() {
    assert_eq!(sanitize_pty_text("\u{1b}(Bplain"), "plain");
    assert_eq!(sanitize_pty_text("\u{1b}#8grid"), "grid");
}

/// A lone trailing ESC must not panic or swallow the text before it.
#[test]
fn a_truncated_escape_is_survivable() {
    assert_eq!(sanitize_pty_text("abc\u{1b}"), "abc");
    assert_eq!(sanitize_pty_text("abc\u{1b}["), "abc");
    assert_eq!(sanitize_pty_text("abc\u{1b}]"), "abc");
}

/// Control characters must become spaces and runs must collapse.
///
/// A NUL truncates the string inside the daemon; a newline renders as a glyph;
/// a carriage return moves the cursor. All three are routine in PTY output.
#[test]
fn control_characters_become_collapsed_whitespace() {
    assert_eq!(sanitize_pty_text("a\nb"), "a b");
    assert_eq!(sanitize_pty_text("a\u{0}b"), "a b");
    assert_eq!(sanitize_pty_text("a\r\n\tb"), "a b");
    assert_eq!(sanitize_pty_text("  lots   of   space  "), "lots of space");
    assert_eq!(sanitize_pty_text(""), "");
    assert_eq!(sanitize_pty_text("\n\n\n"), "");
}

/// Ordinary text must pass through untouched, including non-ASCII.
///
/// A sanitiser that stripped everything non-ASCII would mangle box drawing,
/// CJK and emoji, which agent output is full of.
#[test]
fn ordinary_text_passes_through() {
    assert_eq!(sanitize_pty_text("Ran 12 tests, all passed"), "Ran 12 tests, all passed");
    assert_eq!(sanitize_pty_text("ビルド完了 ✓"), "ビルド完了 ✓");
    assert_eq!(sanitize_pty_text("├─ crate"), "├─ crate");
}

/// Truncation must count characters, not bytes.
///
/// Slicing a UTF-8 string at a byte offset panics on a multi-byte boundary, and
/// agent output is full of box drawing and CJK. A panic here would take down
/// the app from a notification.
#[test]
fn truncation_counts_characters_not_bytes() {
    assert_eq!(truncate_chars("日本語テスト", 4), "日本語\u{2026}");
    assert_eq!(truncate_chars("日本語", 3), "日本語");
    assert_eq!(truncate_chars("abcdef", 3), "ab\u{2026}");
    assert_eq!(truncate_chars("abc", 10), "abc");
    assert_eq!(truncate_chars("", 5), "");
    assert_eq!(truncate_chars("ab", 1), "\u{2026}");
}

/// Long titles and bodies must be truncated to the documented limits.
#[test]
fn long_text_is_truncated_to_the_limits() {
    let long = "x".repeat(500);
    let n = Notification::finished(SessionId(1), &long, &long);
    assert_eq!(n.title.chars().count(), MAX_TITLE_CHARS);
    assert!(n.title.ends_with('\u{2026}'));
    assert_eq!(n.body.chars().count(), MAX_BODY_CHARS);
    assert!(n.body.ends_with('\u{2026}'));
}

/// The activation URL must name the session, for every backend.
#[test]
fn the_activation_url_names_the_session() {
    let n = Notification::failed(SessionId(31337), "x", "");
    assert_eq!(n.activation_url(), "vitrum://session/31337");
    assert_eq!(deeplink::parse(&n.activation_url()), Ok(DeepLink::Session(SessionId(31337))));
}

/// The kind tokens are stable, because they appear in identifiers and
/// categories that outlive a single run.
#[test]
fn kind_tokens_are_stable() {
    assert_eq!(NotificationKind::Finished.as_str(), "finished");
    assert_eq!(NotificationKind::NeedsApproval.as_str(), "needs-approval");
    assert_eq!(NotificationKind::Failed.as_str(), "failed");
    assert_eq!(NotificationKind::Failed.dbus_category(), "x-vitrum.session.failed");
}
