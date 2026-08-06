//! Replies, bells, title, and working directory.
//!
//! A VT stream contains questions. The reply path is the one that matters most:
//! a program that asks for the device attributes and never gets an answer does
//! not degrade, it hangs.

use super::support::Fixture;

/// Drain the pending reply bytes as a string.
fn replies(fx: &Fixture) -> String {
    let mut out = Vec::new();
    fx.vt.drain_pty_write(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn a_device_attributes_query_is_answered() {
    let mut fx = Fixture::new(10, 1);
    fx.write(b"\x1b[c");

    let reply = replies(&fx);
    assert!(reply.starts_with('\x1b'), "reply is an escape sequence: {reply:?}");
    assert!(reply.contains('c'), "reply is a DA response: {reply:?}");
}

#[test]
fn a_mode_query_is_answered() {
    let mut fx = Fixture::new(10, 1);
    fx.write(b"\x1b[?7$p");
    assert_eq!(replies(&fx), "\x1b[?7;1$y");
}

#[test]
fn draining_replies_empties_the_buffer() {
    // A reply delivered twice is a corrupt stream, not a duplicate message.
    let mut fx = Fixture::new(10, 1);
    fx.write(b"\x1b[?7$p");

    assert!(!replies(&fx).is_empty());
    assert!(replies(&fx).is_empty());
}

#[test]
fn replies_accumulate_in_order() {
    let mut fx = Fixture::new(10, 1);
    fx.write(b"\x1b[?7$p");
    fx.write(b"\x1b[?25$p");

    let reply = replies(&fx);
    let first = reply.find("?7;").expect("first reply present");
    let second = reply.find("?25;").expect("second reply present");
    assert!(first < second, "replies kept their order: {reply:?}");
}

#[test]
fn a_pending_reply_is_visible_before_it_is_drained() {
    let mut fx = Fixture::new(10, 1);
    assert!(!fx.vt.events().has_pty_write());

    fx.write(b"\x1b[?7$p");
    assert!(fx.vt.events().has_pty_write());
}

#[test]
fn draining_appends_rather_than_replaces() {
    // A host batching several sessions into one write must be able to reuse one
    // buffer without an intermediate copy.
    let mut fx = Fixture::new(10, 1);
    fx.write(b"\x1b[?7$p");

    let mut out = b"prefix:".to_vec();
    fx.vt.drain_pty_write(&mut out);
    assert!(out.starts_with(b"prefix:"));
    assert!(out.len() > b"prefix:".len());
}

#[test]
fn a_bell_is_counted_and_taken_once() {
    let mut fx = Fixture::new(10, 1);
    fx.write(b"\x07\x07\x07");

    assert_eq!(fx.vt.events().take_bells(), 3);
    assert_eq!(fx.vt.events().take_bells(), 0);
}

#[test]
fn the_title_is_reported() {
    let mut fx = Fixture::new(10, 1);
    fx.write(b"\x1b]2;a title\x1b\\");
    assert_eq!(fx.vt.events().take_title().as_deref(), Some("a title"));
}

#[test]
fn only_the_newest_title_survives() {
    // An older title is not information, it is a stale value, and a host that
    // applied both would flash the wrong one.
    let mut fx = Fixture::new(10, 1);
    fx.write(b"\x1b]2;first\x1b\\");
    fx.write(b"\x1b]2;second\x1b\\");

    assert_eq!(fx.vt.events().take_title().as_deref(), Some("second"));
    assert_eq!(fx.vt.events().take_title(), None);
}

#[test]
fn the_working_directory_is_reported() {
    // OSC 7 is what lets a new tab open where the old one was. The webview path
    // never had it.
    let mut fx = Fixture::new(10, 1);
    fx.write(b"\x1b]7;file://host/home/user/src\x1b\\");

    let pwd = fx.vt.events().take_pwd().expect("pwd reported");
    assert!(pwd.ends_with("/home/user/src"), "pwd is the path sent: {pwd:?}");
}

#[test]
fn a_stream_with_no_events_reports_none() {
    let mut fx = Fixture::new(10, 1);
    fx.write(b"just text\r\n");

    assert_eq!(fx.vt.events().take_bells(), 0);
    assert_eq!(fx.vt.events().take_title(), None);
    assert_eq!(fx.vt.events().take_pwd(), None);
    assert!(!fx.vt.events().has_pty_write());
}

#[test]
fn mouse_tracking_is_off_until_a_program_asks_for_it() {
    // The host uses this to decide whether a drag selects text or is forwarded
    // to the program. Getting it wrong makes selection impossible in an editor.
    let mut fx = Fixture::new(10, 1);
    assert!(!fx.vt.mouse_tracking().expect("readable"));

    fx.write(b"\x1b[?1000h");
    assert!(fx.vt.mouse_tracking().expect("readable"));

    fx.write(b"\x1b[?1000l");
    assert!(!fx.vt.mouse_tracking().expect("readable"));
}
