//! `vitrum://` parsing, including every hostile shape a browser or shell can
//! produce.
//!
//! A deep link is attacker-influenced input: a web page can navigate to
//! `vitrum://...` and the OS will hand us the string. The parser must therefore
//! be total, allocation-bounded, and must never repair a malformed URL into a
//! plausible action.

use vitrum_proto::{ProjectId, SessionId};

use crate::deeplink::{self, DeepLink, DeepLinkError, MAX_URL_LEN};

/// The documented happy path must produce the documented session.
#[test]
fn a_session_url_resolves_to_that_session() {
    assert_eq!(deeplink::parse("vitrum://session/42"), Ok(DeepLink::Session(SessionId(42))));
}

/// Projects are a separate target and must not collapse into sessions.
#[test]
fn a_project_url_resolves_to_that_project() {
    assert_eq!(deeplink::parse("vitrum://project/7"), Ok(DeepLink::Project(ProjectId(7))));
}

/// A bare scheme must mean "just raise the window".
///
/// This is what a desktop launcher emits when it re-activates a running app,
/// and rejecting it would make the second launch appear to do nothing.
#[test]
fn a_bare_scheme_means_raise_the_window() {
    assert_eq!(deeplink::parse("vitrum://"), Ok(DeepLink::Home));
    assert_eq!(deeplink::parse("vitrum://home"), Ok(DeepLink::Home));
}

/// Schemes are case-insensitive per RFC 3986 and Windows upper-cases them in
/// some registry paths, so `VITRUM://` must work.
#[test]
fn the_scheme_is_case_insensitive() {
    assert_eq!(deeplink::parse("VITRUM://session/1"), Ok(DeepLink::Session(SessionId(1))));
    assert_eq!(deeplink::parse("ViTrUm://session/1"), Ok(DeepLink::Session(SessionId(1))));
}

/// The authority is case-insensitive too; a URL is not a file path.
#[test]
fn the_target_is_case_insensitive() {
    assert_eq!(deeplink::parse("vitrum://SESSION/9"), Ok(DeepLink::Session(SessionId(9))));
}

/// Surrounding whitespace must be tolerated.
///
/// A shell handler, an AppleEvent and a `xdg-open` invocation all routinely
/// append a newline. Failing on it makes deep links work from a terminal and
/// mysteriously not from a browser.
#[test]
fn surrounding_whitespace_is_tolerated() {
    assert_eq!(deeplink::parse("  vitrum://session/3\n"), Ok(DeepLink::Session(SessionId(3))));
    assert_eq!(deeplink::parse("\tvitrum://session/3\r\n"), Ok(DeepLink::Session(SessionId(3))));
}

/// One trailing slash is idiomatic and must be accepted.
#[test]
fn a_single_trailing_slash_is_accepted() {
    assert_eq!(deeplink::parse("vitrum://session/8/"), Ok(DeepLink::Session(SessionId(8))));
}

/// A query string and a fragment must be ignored, not rejected.
///
/// Browsers append tracking parameters and fragments to whatever they navigate
/// to. Rejecting them would break the exact path most users arrive by.
#[test]
fn a_query_and_fragment_are_ignored() {
    assert_eq!(
        deeplink::parse("vitrum://session/5?utm_source=x#top"),
        Ok(DeepLink::Session(SessionId(5)))
    );
    // A fragment may itself contain `?`; stripping the query first would leave
    // the fragment behind and turn the id into `5#a`.
    assert_eq!(deeplink::parse("vitrum://session/5#a?b"), Ok(DeepLink::Session(SessionId(5))));
}

/// Leading zeros are unambiguous decimal and must parse.
#[test]
fn leading_zeros_parse_as_decimal() {
    assert_eq!(deeplink::parse("vitrum://session/007"), Ok(DeepLink::Session(SessionId(7))));
    assert_eq!(deeplink::parse("vitrum://session/0"), Ok(DeepLink::Session(SessionId(0))));
}

/// The full `u64` range must round-trip.
///
/// Session ids come from a server-side counter with no ceiling below `u64`, so
/// a parser that used `u32` or `i64` would start rejecting valid links after a
/// long uptime rather than at a test-visible boundary.
#[test]
fn the_whole_u64_range_parses() {
    let url = format!("vitrum://session/{}", u64::MAX);
    assert_eq!(deeplink::parse(&url), Ok(DeepLink::Session(SessionId(u64::MAX))));
}

/// A wrong scheme must be rejected, naming what was found.
///
/// Accepting any scheme would let `http://session/42` from a hijacked handler
/// drive the app.
#[test]
fn a_foreign_scheme_is_rejected() {
    assert_eq!(
        deeplink::parse("http://session/42"),
        Err(DeepLinkError::WrongScheme { found: "http".to_string() })
    );
    assert_eq!(
        deeplink::parse("vitrumx://session/42"),
        Err(DeepLinkError::WrongScheme { found: "vitrumx".to_string() })
    );
}

/// A string with no colon at all is a wrong scheme, not a panic.
#[test]
fn text_without_a_scheme_is_rejected() {
    assert_eq!(
        deeplink::parse("session/42"),
        Err(DeepLinkError::WrongScheme { found: "session/42".to_string() })
    );
}

/// The opaque form without `//` must be rejected rather than guessed.
///
/// Both `vitrum:session/42` and `vitrum://session/42` would be "obviously"
/// intended, but only the second is what the registered handlers emit, and
/// accepting both means two parsing paths that can disagree.
#[test]
fn the_opaque_form_without_an_authority_is_rejected() {
    assert_eq!(deeplink::parse("vitrum:session/42"), Err(DeepLinkError::MissingAuthority));
    assert_eq!(deeplink::parse("vitrum:/session/42"), Err(DeepLinkError::MissingAuthority));
    assert_eq!(deeplink::parse("vitrum:\\\\session\\42"), Err(DeepLinkError::MissingAuthority));
}

/// An unknown target must be reported, lowercased, and must not fall back to
/// raising the window.
#[test]
fn an_unknown_target_is_rejected() {
    assert_eq!(
        deeplink::parse("vitrum://settings/1"),
        Err(DeepLinkError::UnknownTarget { target: "settings".to_string() })
    );
}

/// Userinfo or a port in the authority must not be silently stripped.
///
/// `vitrum://evil@session/42` looks like a session link to a human skimming a
/// log. Treating the whole authority as the target means it is rejected.
#[test]
fn userinfo_and_ports_in_the_authority_are_rejected() {
    assert_eq!(
        deeplink::parse("vitrum://evil@session/42"),
        Err(DeepLinkError::UnknownTarget { target: "evil@session".to_string() })
    );
    assert_eq!(
        deeplink::parse("vitrum://session:8080/42"),
        Err(DeepLinkError::UnknownTarget { target: "session:8080".to_string() })
    );
}

/// A target that needs an id and got none must say so.
#[test]
fn a_missing_id_is_reported() {
    assert_eq!(deeplink::parse("vitrum://session"), Err(DeepLinkError::MissingId { target: "session" }));
    assert_eq!(
        deeplink::parse("vitrum://session/"),
        Err(DeepLinkError::MissingId { target: "session" })
    );
}

/// A non-numeric id must be rejected.
#[test]
fn a_non_numeric_id_is_rejected() {
    assert_eq!(
        deeplink::parse("vitrum://session/abc"),
        Err(DeepLinkError::InvalidId { target: "session", value: "abc".to_string() })
    );
}

/// A leading `+` must be rejected even though `str::parse::<u64>` accepts it.
///
/// This is the trap: `"+42".parse::<u64>()` is `Ok(42)`, so the obvious
/// implementation quietly accepts a form no legitimate producer emits and that
/// no log filter matching `session/[0-9]+` would flag.
#[test]
fn a_signed_id_is_rejected_despite_parse_accepting_it() {
    assert_eq!("+42".parse::<u64>(), Ok(42), "this is the standard-library behaviour we guard against");
    assert_eq!(
        deeplink::parse("vitrum://session/+42"),
        Err(DeepLinkError::InvalidId { target: "session", value: "+42".to_string() })
    );
    assert_eq!(
        deeplink::parse("vitrum://session/-1"),
        Err(DeepLinkError::InvalidId { target: "session", value: "-1".to_string() })
    );
}

/// An id that overflows `u64` must be rejected, not wrapped or saturated.
#[test]
fn an_overflowing_id_is_rejected() {
    assert_eq!(
        deeplink::parse("vitrum://session/18446744073709551616"),
        Err(DeepLinkError::InvalidId {
            target: "session",
            value: "18446744073709551616".to_string(),
        })
    );
    // Longer than twenty digits is refused before `parse` is even reached.
    assert_eq!(
        deeplink::parse("vitrum://session/999999999999999999999999"),
        Err(DeepLinkError::InvalidId {
            target: "session",
            value: "999999999999999999999999".to_string(),
        })
    );
}

/// Percent-encoded digits must not be decoded into an id.
///
/// `%34%32` is "42". Decoding it would let an attacker slip an id past any
/// audit log or filter that inspected the raw URL, for zero benefit: no
/// legitimate producer percent-encodes a decimal digit.
#[test]
fn percent_encoded_digits_are_rejected() {
    assert_eq!(
        deeplink::parse("vitrum://session/%34%32"),
        Err(DeepLinkError::InvalidId { target: "session", value: "%34%32".to_string() })
    );
}

/// Path traversal in the id position must be rejected.
///
/// The id is never used as a path, but a parser that accepted `..` would be one
/// refactor away from a directory traversal, and the rejection costs nothing.
#[test]
fn path_traversal_is_rejected() {
    assert_eq!(
        deeplink::parse("vitrum://session/.."),
        Err(DeepLinkError::InvalidId { target: "session", value: "..".to_string() })
    );
    assert_eq!(
        deeplink::parse("vitrum://session/../../etc/passwd"),
        Err(DeepLinkError::TrailingSegments { target: "session" })
    );
}

/// Extra path segments must be rejected, not ignored.
///
/// Ignoring them means `vitrum://session/42/delete` silently becomes
/// `vitrum://session/42`, which is how a future route addition turns into a
/// security surprise on old clients.
#[test]
fn extra_path_segments_are_rejected() {
    assert_eq!(
        deeplink::parse("vitrum://session/42/extra"),
        Err(DeepLinkError::TrailingSegments { target: "session" })
    );
    assert_eq!(
        deeplink::parse("vitrum://home/1"),
        Err(DeepLinkError::TrailingSegments { target: "home" })
    );
    // A second trailing slash is a second empty segment, not decoration.
    assert_eq!(
        deeplink::parse("vitrum://session/42//"),
        Err(DeepLinkError::TrailingSegments { target: "session" })
    );
}

/// A control character anywhere must be rejected, with its byte offset.
///
/// A NUL truncates the string inside any C API downstream; an ESC in a value
/// that later reaches a log or a terminal is a control-sequence injection.
#[test]
fn control_characters_are_rejected_with_their_offset() {
    assert_eq!(
        deeplink::parse("vitrum://session/4\u{0}2"),
        Err(DeepLinkError::ControlCharacter { at: 18 })
    );
    assert_eq!(
        deeplink::parse("vitrum://session/\u{1b}[31m42"),
        Err(DeepLinkError::ControlCharacter { at: 17 })
    );
    // Newlines inside, as opposed to around, are still control characters.
    assert_eq!(
        deeplink::parse("vitrum://ses\nsion/42"),
        Err(DeepLinkError::ControlCharacter { at: 12 })
    );
}

/// Empty and whitespace-only input must be reported as empty.
#[test]
fn empty_input_is_reported_as_empty() {
    assert_eq!(deeplink::parse(""), Err(DeepLinkError::Empty));
    assert_eq!(deeplink::parse("   \t\n "), Err(DeepLinkError::Empty));
}

/// An oversized URL must be rejected on its length, before any scanning.
///
/// Without the cap, a hostile handler invocation with a megabyte argument makes
/// the parser walk the whole thing looking for a colon.
#[test]
fn an_oversized_url_is_rejected_by_length() {
    let url = format!("vitrum://session/{}", "1".repeat(MAX_URL_LEN));
    let len = url.len();
    assert_eq!(deeplink::parse(&url), Err(DeepLinkError::TooLong { len }));
    // Exactly at the limit is accepted as far as length goes, and then fails
    // on the id, which proves the boundary is inclusive rather than off by one.
    let at_limit = format!("vitrum://session/{}", "1".repeat(MAX_URL_LEN - 17));
    assert_eq!(at_limit.len(), MAX_URL_LEN);
    assert!(matches!(deeplink::parse(&at_limit), Err(DeepLinkError::InvalidId { .. })));
}

/// Every link must round-trip through its canonical URL.
///
/// The activation payloads for all three platforms embed `to_url` output and
/// parse it back on click. A mismatch would make notifications open the wrong
/// session, or nothing.
#[test]
fn every_link_round_trips_through_its_url() {
    for link in [
        DeepLink::Home,
        DeepLink::Session(SessionId(0)),
        DeepLink::Session(SessionId(42)),
        DeepLink::Session(SessionId(u64::MAX)),
        DeepLink::Project(ProjectId(1)),
        DeepLink::Project(ProjectId(u64::MAX)),
    ] {
        let url = link.to_url();
        assert_eq!(deeplink::parse(&url), Ok(link), "round trip failed for {url}");
    }
}

/// The canonical URLs are exactly these strings.
///
/// They appear in the Windows toast `launch` attribute, the macOS `userInfo`
/// and the desktop file, so they are a wire format, not an implementation
/// detail.
#[test]
fn canonical_urls_are_stable() {
    assert_eq!(DeepLink::Session(SessionId(42)).to_url(), "vitrum://session/42");
    assert_eq!(DeepLink::Project(ProjectId(7)).to_url(), "vitrum://project/7");
    assert_eq!(DeepLink::Home.to_url(), "vitrum://home");
}

/// Error messages must name the problem precisely enough to debug from a log.
#[test]
fn error_messages_name_the_problem() {
    assert_eq!(
        deeplink::parse("ftp://x").unwrap_err().to_string(),
        "expected scheme `vitrum`, found `ftp`"
    );
    assert_eq!(
        deeplink::parse("vitrum://session/x").unwrap_err().to_string(),
        "`session` id `x` is not an unsigned decimal that fits u64"
    );
    assert_eq!(
        deeplink::parse("vitrum://nope").unwrap_err().to_string(),
        "unknown target `nope`"
    );
}
