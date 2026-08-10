//! What the token file has to guarantee, proved against a real filesystem.
//!
//! WHY: the daemon spawns arbitrary commands for anyone who completes the
//! handshake, so this file's mode is the only thing standing between another
//! account on the machine and code execution as this user. The class these
//! tests close is "the secret was written somewhere readable, or a comparison
//! accepted something that was not the secret". They do not cover the
//! transport: that a wrong token is refused at the handshake is asserted in
//! `vitrum-server`, where the socket is.

use super::*;

/// A written token is readable by its owner and by nobody else.
///
/// The mode is the enforcement, so it is asserted as a number rather than
/// inferred from the write succeeding.
#[cfg(unix)]
#[test]
fn a_written_token_is_private_to_its_owner() {
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch("private");
    let path = dir.join("token");
    let token = create_at(&path).expect("writing a token");

    let mode = std::fs::metadata(&path)
        .expect("the token exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "token file mode is {mode:o}, not 0600");

    let dir_mode = std::fs::metadata(&dir)
        .expect("the directory exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        dir_mode, 0o700,
        "token directory mode is {dir_mode:o}, not 0700"
    );

    assert_eq!(load_from(&path).expect("reading it back"), token);
    cleanup(&dir);
}

/// Two tokens generated in a row differ, and each is the published shape.
///
/// A generator that returned a constant would pass every other test here.
#[test]
fn each_token_is_fresh_and_well_formed() {
    let dir = scratch("fresh");
    let a = create_at(&dir.join("a")).expect("first");
    let b = create_at(&dir.join("b")).expect("second");
    assert_ne!(a, b, "two generated tokens were identical");
    for t in [&a, &b] {
        assert_eq!(t.len(), TOKEN_HEX_LEN, "{t} is the wrong length");
        assert!(is_well_formed(t), "{t} is not lowercase hex");
    }
    cleanup(&dir);
}

/// A missing file is `Missing`, not a generic IO error.
///
/// The client renders that case as "no daemon has run as you", which is the
/// only actionable thing to say, and it can only do that if the variant is
/// distinguishable.
#[test]
fn an_absent_token_is_named_as_absent() {
    let dir = scratch("absent");
    let path = dir.join("token");
    match load_from(&path) {
        Err(TokenError::Missing { path: reported }) => assert_eq!(reported, path),
        other => panic!("expected Missing, got {other:?}"),
    }
}

/// Every shape that is not a token is refused, including the near misses.
///
/// Derived as a table rather than one representative, because the defect this
/// closes is a validator that checks length and not alphabet, or alphabet and
/// not length.
#[test]
fn nothing_but_sixty_four_lowercase_hex_characters_is_a_token() {
    let good = "0123456789abcdef".repeat(4);
    assert_eq!(good.len(), TOKEN_HEX_LEN);
    assert!(is_well_formed(&good));

    let bad = [
        ("empty", String::new()),
        ("one short", good[1..].to_string()),
        ("one long", format!("{good}a")),
        ("uppercase", good.to_uppercase()),
        ("non hex", format!("{}g", &good[1..])),
        ("embedded space", format!("{} {}", &good[..31], &good[32..])),
        ("all zeroes but short", "0".repeat(TOKEN_HEX_LEN - 1)),
    ];
    for (name, value) in bad {
        assert!(!is_well_formed(&value), "{name} was accepted as a token");
        assert!(
            validate(&value, "VITRUM_TOKEN").is_err(),
            "{name} passed validate"
        );
    }
}

/// A token file with surrounding whitespace still reads, and the value is
/// trimmed.
///
/// Anyone who copies the file with `echo` or an editor leaves a newline on it,
/// and refusing that would send an operator hunting a corruption that is not
/// there.
#[test]
fn surrounding_whitespace_is_not_corruption() {
    let dir = scratch("whitespace");
    std::fs::create_dir_all(&dir).expect("scratch");
    let path = dir.join("token");
    let good = "0123456789abcdef".repeat(4);
    std::fs::write(&path, format!("\n  {good}\t\n")).expect("planting");
    assert_eq!(load_from(&path).expect("reading"), good);
    assert_eq!(validate(&format!(" {good}\n"), "VITRUM_TOKEN").unwrap(), good);
    cleanup(&dir);
}

/// A file whose contents are not a token is `Malformed`, and an environment
/// value that is not a token is `MalformedValue` naming its source.
///
/// The two send an operator to different places, so collapsing them would send
/// half of them to the wrong one.
#[test]
fn a_bad_value_says_which_input_was_bad() {
    let dir = scratch("malformed");
    std::fs::create_dir_all(&dir).expect("scratch");
    let path = dir.join("token");
    std::fs::write(&path, "hello").expect("planting");
    match load_from(&path) {
        Err(TokenError::Malformed { path: reported }) => assert_eq!(reported, path),
        other => panic!("expected Malformed, got {other:?}"),
    }
    match validate("hello", "VITRUM_TOKEN") {
        Err(TokenError::MalformedValue { source }) => assert_eq!(source, "VITRUM_TOKEN"),
        other => panic!("expected MalformedValue, got {other:?}"),
    }
    let rendered = load_from(&path).unwrap_err().to_string();
    assert!(
        rendered.contains(&path.display().to_string()),
        "the error must name the file: {rendered}"
    );
    cleanup(&dir);
}

/// A token file far larger than a token is refused without being read whole.
///
/// WHY: `load_from` used `read_to_string`, so anything that could write this
/// path chose how much memory the reader allocated before the 64-character
/// shape was ever checked. The client reads this file at startup and the
/// daemon reads it on every connection, so the cost lands on the two paths a
/// user waits for.
///
/// The verdict is its own variant rather than `Malformed`, which is what makes
/// this testable at all: an unbounded read of a file whose first line is a
/// valid token also ends in `Malformed`, so a test that accepted `Malformed`
/// would pass against the defect it is named for. It also says something
/// different to the operator — something else is writing to that path.
#[test]
fn a_token_file_larger_than_a_token_is_refused_by_size() {
    let dir = scratch("oversized");
    std::fs::create_dir_all(&dir).expect("scratch");
    let path = dir.join("token");
    let good = "0123456789abcdef".repeat(4);

    // The token is there, at the front, followed by a megabyte of anything.
    std::fs::write(&path, format!("{good}\n{}", "x".repeat(1024 * 1024))).expect("planting");
    match load_from(&path) {
        Err(TokenError::TooLarge { path: reported, limit }) => {
            assert_eq!(reported, path);
            assert!(limit >= TOKEN_HEX_LEN, "the bound must admit a token: {limit}");
        }
        other => panic!("expected TooLarge, got {other:?}"),
    }
    let rendered = load_from(&path).unwrap_err().to_string();
    assert!(
        rendered.contains("larger than") && rendered.contains(&path.display().to_string()),
        "the refusal names the size and the file: {rendered}"
    );

    // A token with a page of trailing newlines is still a token, so the bound
    // has not become a second format rule.
    std::fs::write(&path, format!("{good}{}", "\n".repeat(512))).expect("planting");
    assert_eq!(load_from(&path).expect("still a token"), good);

    cleanup(&dir);
}

/// The comparison accepts exactly the secret and nothing adjacent to it.
///
/// Every case here is a way a sloppy comparison says yes: a prefix, a
/// truncation, a one-character change at each end, and the empty string, which
/// is what a client that forgot the field sends.
#[test]
fn the_comparison_accepts_only_the_secret() {
    let secret = "0123456789abcdef".repeat(4);
    assert!(matches(&secret, &secret));

    let mut last_flipped = secret.clone();
    last_flipped.pop();
    last_flipped.push('0');
    let mut first_flipped = secret.clone();
    first_flipped.replace_range(0..1, "1");

    for (name, candidate) in [
        ("empty", String::new()),
        ("prefix", secret[..63].to_string()),
        ("longer", format!("{secret}0")),
        ("first character", first_flipped),
        ("last character", last_flipped),
        ("all zeroes", "0".repeat(TOKEN_HEX_LEN)),
    ] {
        assert!(
            !matches(&secret, &candidate),
            "{name} was accepted as the secret"
        );
    }
}

/// The resolved path is under a directory this user owns and is named `token`.
///
/// Not an assertion about one platform's layout, which the environment
/// decides: an assertion that resolution produces an absolute path with the
/// agreed file name, which is what every caller depends on.
#[test]
fn the_default_path_is_absolute_and_named() {
    match path() {
        Ok(p) => {
            assert!(p.is_absolute(), "{} is not absolute", p.display());
            assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("token"));
        }
        // A build environment with no HOME and no runtime directory is a real
        // outcome and must be an error rather than a guess at `/tmp`.
        Err(TokenError::NoDirectory { .. }) => {}
        Err(other) => panic!("unexpected resolution failure: {other}"),
    }
}

/// A scratch directory under the system temporary directory, unique per test.
fn scratch(name: &str) -> PathBuf {
    let unique = format!(
        "vitrum-token-test-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    );
    let dir = std::env::temp_dir().join(unique);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn cleanup(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

/// A daemon can be stopped and started again while its old token is still on
/// disk.
///
/// WHY: the runtime directory survives until logout, so the previous daemon's
/// token is normally still there on the second start. This product tells the
/// operator to restart the daemon in its own protocol-skew message and after
/// an update, so a start that refuses because of its own leftover file turns
/// routine advice into a dead end. The class is "a stale artefact we wrote
/// locks the user out of their own machine".
#[test]
fn a_restart_replaces_the_previous_daemons_token() {
    let dir = scratch("restart");
    let path = dir.join("token");

    let first = create_at(&path).expect("first start");
    assert_eq!(load_from(&path).expect("first read"), first);

    let second = create_at(&path).expect("second start with the first token still there");
    assert_ne!(first, second, "a restart must mint a fresh token");
    assert_eq!(
        load_from(&path).expect("second read"),
        second,
        "a client reading after the restart must get the new token"
    );

    // A third, because a bug that only survives one replacement would pass the
    // pair above.
    let third = create_at(&path).expect("third start");
    assert_eq!(load_from(&path).expect("third read"), third);
    assert_ne!(third, second);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path)
            .expect("it exists")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "a replaced token has mode {mode:o}");
    }

    // The temporary the rename came from must not be left behind, or the
    // runtime directory accretes one file per daemon start.
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .expect("listing")
        .filter_map(|e| e.ok().map(|e| e.file_name()))
        .filter(|n| n != "token")
        .collect();
    assert!(leftovers.is_empty(), "left {leftovers:?} behind");
    cleanup(&dir);
}

/// A symlink at the token's path is refused, not followed.
///
/// Following it would write this daemon's secret into whatever the link points
/// at, which is the classic way a 0600 file ends up somewhere an attacker can
/// read. The refusal has to be at `symlink_metadata`, so a check written with
/// `metadata` fails this test.
#[cfg(unix)]
#[test]
fn a_symlink_at_the_token_path_is_refused() {
    let dir = scratch("symlink");
    std::fs::create_dir_all(&dir).expect("scratch");
    let target = dir.join("elsewhere");
    std::fs::write(&target, "").expect("planting a target");
    let path = dir.join("token");
    std::os::unix::fs::symlink(&target, &path).expect("planting a symlink");

    match create_at(&path) {
        Err(TokenError::Foreign { path: p, reason }) => {
            assert_eq!(p, path);
            assert!(reason.contains("symbolic link"), "wrong reason: {reason}");
        }
        other => panic!("expected Foreign, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(&target).expect("the target is readable"),
        "",
        "the secret was written through the link"
    );
    cleanup(&dir);
}

/// A regular file at the token's path with a mode this code never writes is
/// refused.
///
/// Every token this code produces is 0600. A 0644 one is therefore something
/// else, and writing a fresh secret into it would hand that secret to whoever
/// arranged the mode. Ownership by another uid is the same class and cannot be
/// constructed by an unprivileged test, so the mode is the reachable half of
/// it and both are checked by the same function.
#[cfg(unix)]
#[test]
fn a_token_file_with_a_mode_we_never_write_is_refused() {
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch("tampered");
    std::fs::create_dir_all(&dir).expect("scratch");
    let path = dir.join("token");
    std::fs::write(&path, "0".repeat(TOKEN_HEX_LEN)).expect("planting");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("loosening");

    match create_at(&path) {
        Err(TokenError::Foreign { reason, .. }) => {
            assert!(reason.contains("0600"), "wrong reason: {reason}")
        }
        other => panic!("expected Foreign, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(&path).expect("still readable"),
        "0".repeat(TOKEN_HEX_LEN),
        "a refused write must leave the file alone"
    );
    cleanup(&dir);
}

/// A directory at the token's path is refused rather than producing a
/// confusing IO error.
#[test]
fn a_directory_at_the_token_path_is_refused() {
    let dir = scratch("isdir");
    let path = dir.join("token");
    std::fs::create_dir_all(&path).expect("planting a directory");
    match create_at(&path) {
        Err(TokenError::Foreign { reason, .. }) => {
            assert!(reason.contains("regular file"), "wrong reason: {reason}")
        }
        other => panic!("expected Foreign, got {other:?}"),
    }
    cleanup(&dir);
}
