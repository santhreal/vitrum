//! Decoding the working directory a shell reports.
//!
//! These are the shapes real shells actually send. `__vte_osc7`, Ghostty's own
//! shell integration and fish all emit a hostname; zsh setups often emit none;
//! and anything with a space or a non-ASCII character in the path arrives
//! percent-encoded.

use std::path::PathBuf;

use crate::pwd::pwd_path;

/// The ordinary case: a hostname the sender chose, and a plain path.
#[test]
fn a_reported_directory_becomes_a_path() {
    assert_eq!(
        pwd_path("file://myhost/home/me/proj"),
        Some(PathBuf::from("/home/me/proj"))
    );
}

/// An empty authority is just as valid and is what several shells send.
#[test]
fn a_report_with_no_hostname_still_names_a_directory() {
    assert_eq!(
        pwd_path("file:///home/me/proj"),
        Some(PathBuf::from("/home/me/proj"))
    );
}

/// Percent escapes are decoded, or the path is one nothing can open.
#[test]
fn an_escaped_path_is_decoded() {
    assert_eq!(
        pwd_path("file:///home/me/two%20words"),
        Some(PathBuf::from("/home/me/two words"))
    );
}

/// Multi-byte characters survive, which they only do if decoding happens on
/// bytes and the result is validated as UTF-8 afterwards rather than per escape.
#[test]
fn an_escaped_multibyte_character_survives() {
    assert_eq!(
        pwd_path("file:///home/me/%E4%B8%96"),
        Some(PathBuf::from("/home/me/\u{4e16}"))
    );
}

/// A Windows report carries a drive letter behind a slash that is URL syntax,
/// not a root. Passing it through would name a directory that cannot exist.
#[test]
fn a_windows_drive_letter_loses_its_leading_slash() {
    assert_eq!(
        pwd_path("file:///C:/Users/me"),
        Some(PathBuf::from("C:/Users/me"))
    );
}

/// A truncated escape is refused rather than guessed at. Guessing is how a
/// path silently becomes a different path.
#[test]
fn a_truncated_escape_is_refused() {
    assert_eq!(pwd_path("file:///home/me/%2"), None);
    assert_eq!(pwd_path("file:///home/me/%"), None);
}

/// An escape that is not hexadecimal means the sender and this decoder
/// disagree about the string.
#[test]
fn a_nonsense_escape_is_refused() {
    assert_eq!(pwd_path("file:///home/me/%zz"), None);
}

/// Bytes that are not UTF-8 do not become a path.
#[test]
fn an_undisplayable_path_is_refused() {
    assert_eq!(pwd_path("file:///home/%ff%fe"), None);
}

/// Only `file://` names somewhere this machine could be.
#[test]
fn another_scheme_is_not_a_directory() {
    assert_eq!(pwd_path("http://example.com/x"), None);
    assert_eq!(pwd_path("/home/me"), None);
    assert_eq!(pwd_path(""), None);
}

/// A URL with an authority and nothing after it names no directory.
#[test]
fn a_hostname_on_its_own_is_not_a_directory() {
    assert_eq!(pwd_path("file://myhost"), None);
}

/// The engine reports this payload, so the decoder has to accept what the
/// engine actually produces rather than what this file assumes it produces.
#[test]
fn the_engine_report_round_trips_through_the_decoder() {
    let mut vt = crate::Vt::new(crate::VtOptions {
        cols: 20,
        rows: 3,
        max_scrollback: 0,
    })
    .expect("engine");
    vt.feed(b"\x1b]7;file://box/tmp/two%20words\x07");
    let raw = vt.events().take_pwd().expect("the engine reported a pwd");

    assert_eq!(pwd_path(&raw), Some(PathBuf::from("/tmp/two words")));
}
