//! Which of the three token inputs wins, and what each failure says.
//!
//! WHY THIS EXISTS
//!
//! The daemon spawns commands on request, so a client that can talk to it can
//! run code as the user who started it. The token is the whole boundary on the
//! client side, and it reaches a client three ways: `VITRUM_TOKEN` for a
//! daemon on another machine behind a tunnel, `--token-file` for a copy of
//! that machine's file, and the file a local daemon writes.
//!
//! An order that is not pinned is an order that drifts. A client that
//! preferred the local file would silently talk to the wrong daemon on a
//! machine that has both, which is the case the environment variable exists
//! for.
//!
//! WHAT IS PROVED HERE
//!
//! The precedence, in every combination; that a blank variable is not a
//! choice; that a bad value from each source names that source rather than
//! another; that no path accepts something that is not a token; and which
//! failures stop the handshake here rather than letting the daemon answer.
//!
//! WHAT IS NOT
//!
//! Not what the daemon does with it. `vitrum-proto` owns the comparison and
//! `vitrum-server` owns the refusal.

use super::*;

use vitrum_proto::token::{TOKEN_HEX_LEN, TokenError};

/// A syntactically valid token that is not any real one.
fn a_token(seed: char) -> String {
    std::iter::repeat_n(seed, TOKEN_HEX_LEN).collect()
}

/// A scratch token file, removed when it drops.
struct TokenFile(std::path::PathBuf);

impl TokenFile {
    fn new(name: &str, contents: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "vitrum-token-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, contents).expect("writing the scratch token");
        Self(path)
    }

    fn as_str(&self) -> &str {
        self.0.to_str().expect("a temp path is UTF-8 here")
    }
}

impl Drop for TokenFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

/// The token, or a panic naming what came back instead.
fn present(outcome: Token) -> String {
    match outcome {
        Token::Present(token) => token,
        other => panic!("a well formed token was not accepted: {other:?}"),
    }
}

/// The error from a source the operator named.
fn named(outcome: Token) -> TokenError {
    match outcome {
        Token::Named(e) => e,
        other => panic!("a named source's failure was not reported as one: {other:?}"),
    }
}

/// The environment wins over a named file, which wins over the default file.
///
/// The default file is not exercised by name here: whether it exists depends
/// on whether a daemon has run as whoever is running the suite, and the point
/// of this test is the order, not the contents of that machine's runtime
/// directory.
#[test]
fn the_environment_outranks_a_named_file() {
    let file = TokenFile::new("precedence", &a_token('b'));
    let from_env = a_token('a');

    assert_eq!(
        present(resolve_token_from(Some(&from_env), Some(file.as_str()))),
        from_env,
        "a named file beat the environment, so a tunnelled client would talk to the local daemon"
    );
    assert_eq!(
        present(resolve_token_from(None, Some(file.as_str()))),
        a_token('b')
    );
}

/// An exported but blank variable is a script that meant to set it and did
/// not, and must not break a client that has a perfectly good local file.
#[test]
fn a_blank_variable_is_not_a_choice() {
    let file = TokenFile::new("blank", &a_token('c'));
    for blank in ["", "   ", "\n"] {
        assert_eq!(
            present(resolve_token_from(Some(blank), Some(file.as_str()))),
            a_token('c'),
            "a blank {blank:?} was taken as an answer"
        );
    }
}

/// Surrounding whitespace is trimmed, from both a file and a variable.
///
/// A token copied out of a terminal or written by `echo` arrives with a
/// newline, and refusing that would fail a correct secret over an invisible
/// byte.
#[test]
fn a_trailing_newline_is_not_a_malformed_token() {
    let file = TokenFile::new("newline", &format!("{}\n", a_token('d')));
    assert_eq!(
        present(resolve_token_from(None, Some(file.as_str()))),
        a_token('d')
    );
    assert_eq!(
        present(resolve_token_from(
            Some(&format!(" {} ", a_token('d'))),
            None
        )),
        a_token('d')
    );
}

/// Nothing that is not a token is accepted, whichever door it came through,
/// and the error names the door.
///
/// A bad `VITRUM_TOKEN` and a corrupt token file call for opposite responses
/// from the operator: unset the variable, or restart the daemon. An error that
/// does not say which sends them to the wrong one.
#[test]
fn a_bad_token_names_where_it_came_from() {
    let short = TokenFile::new("short", "abc");
    match named(resolve_token_from(None, Some(short.as_str()))) {
        TokenError::Malformed { path } => {
            assert_eq!(path, std::path::Path::new(short.as_str()));
        }
        other => panic!("a three-character file was not refused as a malformed file: {other:?}"),
    }

    match named(resolve_token_from(Some("not-a-token"), None)) {
        TokenError::MalformedValue { source } => assert_eq!(source, TOKEN_VAR),
        other => panic!("a bad variable was not refused as a bad variable: {other:?}"),
    }

    // Uppercase is a different string to the daemon's comparison, so accepting
    // it here would produce an authentication failure the operator cannot see
    // the cause of.
    match named(resolve_token_from(Some(&a_token('A')), None)) {
        TokenError::MalformedValue { source } => assert_eq!(source, TOKEN_VAR),
        other => panic!("an uppercase token was accepted or misreported: {other:?}"),
    }
}

/// A named file that is not there says so, with the path, rather than falling
/// back to the default file.
///
/// Falling back would connect a client to the local daemon while the operator
/// believed it was authenticating against a copied token, which is the failure
/// mode this whole flag exists to make explicit.
#[test]
fn a_named_file_that_is_absent_is_an_error_not_a_fallback() {
    let absent = std::env::temp_dir().join("vitrum-token-no-such-file-ever");
    std::fs::remove_file(&absent).ok();
    match named(resolve_token_from(None, absent.to_str())) {
        TokenError::Missing { path } => assert_eq!(path, absent),
        other => panic!("an absent --token-file did not report itself missing: {other:?}"),
    }
}

/// A default file that cannot be used never stops the handshake.
///
/// This client only guesses at where a daemon put its token. The daemon knows,
/// and a daemon from a release that predates tokens wants none at all, so a
/// local refusal put a guess on the screen in place of either answer: against
/// an older daemon it reported a missing token when the real problem was a
/// version skew, and the operator restarted nothing because the message named
/// no version.
///
/// The outcome is asserted by its class, not by its contents, because whether
/// this machine has a token file depends on whether a daemon has run as
/// whoever is running the suite. Either way it is never the operator's error.
#[test]
fn an_unnamed_token_is_never_this_clients_error() {
    assert!(
        !matches!(resolve_token_from(None, None), Token::Named(_)),
        "a token nobody named was reported as a named source's failure, which stops the \
         handshake before the daemon can say what it wants"
    );
}

/// The flag is parsed, needs a value, and does not become part of the URL or
/// any other option.
#[test]
fn the_flag_takes_a_path_and_nothing_else() {
    let opts = Options::parse(vec!["--token-file".to_string(), "/src/vitrum/tok".to_string()])
        .expect("a path parses");
    assert_eq!(opts.token_file, Some("/src/vitrum/tok"));
    assert_eq!(opts.server, wire::DEFAULT_WS_URL);

    let told = Options::parse(vec!["--token-file".to_string()]).expect_err("a value is required");
    assert_eq!(told.exit, Exit::Usage);
    assert!(
        told.message.starts_with("vitrum: --token-file needs a path"),
        "{told}"
    );

    let told = Options::parse(vec!["--token-file".to_string(), String::new()])
        .expect_err("an empty path is not a path");
    assert_eq!(told.exit, Exit::Usage);
}

/// The secret is never taken as an argument.
///
/// `ps` is readable by every account on the machine, so a `--token` flag would
/// publish the secret to exactly the local accounts the token exists to keep
/// out. Pinned on the help and on the parser, because the flag being absent is
/// the security property.
#[test]
fn there_is_no_way_to_pass_the_token_on_the_command_line() {
    let told = Options::parse(vec!["--token".to_string(), a_token('e')])
        .expect_err("--token must not be an option");
    assert_eq!(told.exit, Exit::Usage);
    assert!(told.message.contains("unknown argument --token"), "{told}");

    let help = usage();
    assert!(
        !help.contains("--token <"),
        "the help offers a way to pass the secret in argv"
    );
    assert!(
        help.contains("--token-file"),
        "the help does not mention where the token comes from"
    );
    assert!(
        help.contains(TOKEN_VAR),
        "the help does not name the variable a tunnelled client needs"
    );
}
