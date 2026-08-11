//! Every way the connection can fail, and what the operator is told.
//!
//! The reported defect was instability, and most of it lives on this seam: a
//! daemon that dies, a socket that comes back, a token that went stale under a
//! restart, a daemon one release out of step. Each of those has a wrong answer
//! that looks like a working client, which is why they are pinned here rather
//! than left to the log.
//!
//! Everything below drives the shipped decision functions. They are pure by
//! construction, so a case here is the real path with only the signal writes
//! left out; the writes themselves are one line each and are visible above
//! each call site.
//!
//! Not covered: whether the socket task actually reconnects on the schedule,
//! which needs a daemon and belongs to the server's seam suites.

use super::*;
use vitrum_proto::{PROTOCOL_VERSION, ServerMsg, SessionStatus};

/// The reconnect schedule must grow, stop growing, and end.
///
/// WHY: three separate failures live in one function. A schedule that does not
/// grow dials a dead daemon four times a second forever, which is what shipped
/// and what put 75 refusals in the log in twenty seconds. A schedule that grows
/// without a ceiling means a laptop that closed its lid for a week comes back
/// and waits hours. A schedule that never ends is a polling timer by another
/// name, in a program whose whole idle claim is that it has none.
///
/// The termination assertion is the one that cannot be replaced by reading the
/// constants: it is a bound on a loop, and a bound is what a wrong answer here
/// costs in wall time rather than in a wrong value.
///
/// What this does NOT catch: whether anything calls it, which is
/// `schedule_reconnect`, and whether the URL it dials is the resolved one.
#[test]
fn the_reconnect_schedule_doubles_then_holds_then_ends() {
    // The schedule reads the live bus, so the lease is what makes this the
    // default document's schedule rather than whatever a parallel test left.
    let _bus = crate::state::live::exclusive();
    let prefs = crate::state::ConnectionPrefs::default();
    let max_ms = u64::from(prefs.reconnect_max_ms);
    let attempts_max = prefs.reconnect_attempts;
    assert_eq!(reconnect_delay_ms(0), Some(RECONNECT_BASE_MS));
    assert_eq!(reconnect_delay_ms(1), Some(RECONNECT_BASE_MS * 2));
    assert_eq!(reconnect_delay_ms(2), Some(RECONNECT_BASE_MS * 4));

    // Monotone up to the ceiling, and never past it.
    let mut prev = 0;
    let mut total = 0u64;
    let mut attempts = 0u32;
    while let Some(delay) = reconnect_delay_ms(attempts) {
        assert!(delay >= prev, "attempt {attempts} waits less than the one before");
        assert!(
            delay <= max_ms,
            "attempt {attempts} waits {delay}ms, past the ceiling"
        );
        prev = delay;
        total += delay;
        attempts += 1;
        assert!(
            attempts <= 1_000,
            "the schedule does not terminate; it is a polling loop"
        );
    }
    assert_eq!(attempts, attempts_max);
    assert_eq!(
        reconnect_delay_ms(attempts_max),
        None,
        "past the last attempt the window must say the daemon is gone and \
         offer Retry, not keep dialling"
    );
    // The whole schedule is minutes, not hours: a machine that slept through
    // the daemon's absence has to find it again on the first look.
    assert!(
        total < 15 * 60 * 1_000,
        "the schedule spans {total}ms before it gives up"
    );
}

/// A daemon that dies mid-session must reach the banner as a sentence.
///
/// WHY: the socket in that case closes with no close frame at all, which is
/// code 1006 and is the single most common failure this product has. "code
/// 1006" tells an operator nothing.
#[test]
fn a_daemon_that_dies_mid_session_is_reported_as_a_dropped_connection() {
    assert_eq!(
        socket::close_reason(1006, ""),
        "the connection dropped",
        "the code every killed daemon produces must not reach the banner as a \
         number"
    );
    // And that sentence is what the reducer records, because nothing was
    // recorded before it.
    assert_eq!(
        plan_close(&ConnState::Live { server_version: "0.3.1".into() }, Some("the connection dropped".into())),
        Some("the connection dropped".to_string())
    );
}

/// A close with no reason must not overwrite the reason the daemon already
/// gave.
///
/// WHY: the daemon says why it is refusing and THEN closes, and the close
/// carries nothing. Recording the close unconditionally replaced "restart
/// vitrum-server, and that ends every session it holds" with "the connection
/// dropped", leaving the operator a symptom and no action. This is the
/// ordering bug, and it only shows up in the sequence, never in either message
/// alone.
///
/// What this does NOT catch: a second distinct failure after a first, where
/// keeping the first is the deliberate cost of this rule.
#[test]
fn a_recorded_refusal_outranks_the_close_that_follows_it() {
    let refused = ConnState::failed(
        "vitrum-server 0.2.9 predates this client: restart vitrum-server.",
    );
    assert_eq!(
        plan_close(&refused, Some("the connection dropped".into())),
        None,
        "the close must not overwrite the reason that names the fix"
    );
    // A close with no detail at all still has to say something actionable when
    // there is nothing recorded.
    assert_eq!(
        plan_close(&ConnState::Connecting, None),
        Some("connection lost".to_string())
    );
}

/// A protocol mismatch must fail closed, name the fix, and stop using the
/// socket.
///
/// WHY: a client that keeps talking on a socket the daemon is about to close
/// looks connected and answers nothing. The half that keeps being missed is the
/// hang-up: the reason was recorded correctly and the client went on sending
/// `List` and `Attach` into a connection that was already refused.
///
/// What this does NOT catch: whether the daemon actually closes, which is the
/// server's claim.
#[test]
fn a_protocol_mismatch_names_the_restart_and_stops_using_the_socket() {
    for protocol in [PROTOCOL_VERSION - 1, PROTOCOL_VERSION + 1] {
        let mut daemon = state::DaemonState::default();
        daemon.apply(ServerMsg::Welcome {
            protocol,
            server_version: "0.2.9".to_string(),
        });
        let ConnState::Failed { detail } = &daemon.conn else {
            panic!("protocol {protocol} was accepted by a client speaking {PROTOCOL_VERSION}");
        };
        assert!(
            detail.contains("Restart vitrum-server"),
            "the mismatch must name the one action that fixes it: {detail}"
        );
        assert!(
            detail.contains(&protocol.to_string()) && detail.contains(&PROTOCOL_VERSION.to_string()),
            "both versions must be in the sentence for a bug report: {detail}"
        );
        assert_eq!(
            plan_welcome(&daemon.conn),
            WelcomePlan::HangUp,
            "a refused handshake must not be followed by List and Attach"
        );
    }

    // The accepted case, so the assertion above is about the mismatch and not
    // about the function refusing everything.
    let mut daemon = state::DaemonState::default();
    daemon.apply(ServerMsg::Welcome {
        protocol: PROTOCOL_VERSION,
        server_version: "0.3.1".to_string(),
    });
    assert_eq!(plan_welcome(&daemon.conn), WelcomePlan::Subscribe);
}

/// A `Welcome` that left the connection in no state at all must still hang up.
///
/// WHY: `plan_welcome` reads the state the fold produced, not the message. If a
/// future fold path leaves `Connecting` after a `Welcome`, the client would
/// otherwise carry on as though the handshake had been accepted. Defaulting to
/// hanging up is what makes a new outcome fail closed instead of silently
/// passing.
#[test]
fn an_unrecognised_post_welcome_state_hangs_up_rather_than_carrying_on() {
    assert_eq!(plan_welcome(&ConnState::Connecting), WelcomePlan::HangUp);
    assert_eq!(
        plan_welcome(&ConnState::failed("anything at all")),
        WelcomePlan::HangUp
    );
    assert_eq!(plan_welcome(&ConnState::Fixture), WelcomePlan::HangUp);
}

/// A token that was named and cannot be read must refuse before anything is
/// sent; a token nobody named must let the daemon answer.
///
/// WHY: these two are one line apart in the source and opposite in effect. A
/// daemon from before tokens existed wants no token at all, so refusing on a
/// missing default file would make this client unable to talk to it. A token
/// the operator DID name and that cannot be read is a configuration error the
/// daemon cannot diagnose, and sending an empty string there produces an
/// authentication failure that names nothing.
///
/// The stale case is the same shape: the daemon writes a new token every time
/// it starts, so a file left from a previous daemon fails validation and lands
/// in the same arm.
///
/// What this does NOT catch: a token that is well formed and simply wrong,
/// where the daemon's refusal is the only possible answer.
///
/// Every arm of `cli::Token` is covered, and the variant list is read out of
/// `cli.rs` at run time, so a fourth provenance turns this red until somebody
/// decides what it presents.
#[test]
fn a_named_token_that_cannot_be_read_refuses_and_an_unnamed_one_does_not() {
    // Every provenance the resolver can return, decided here and nowhere else.
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/cli.rs"))
        .expect("cli.rs is in the crate this test is compiled into");
    let body = source
        .split_once("pub(crate) enum Token {")
        .expect("cli.rs declares the Token enum")
        .1
        .split_once("\n}")
        .expect("the Token enum is closed")
        .0;
    let mut variants: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with('/'))
        .filter_map(|l| l.split(['(', ',', '{']).next())
        .map(str::trim)
        .filter(|l| l.chars().next().is_some_and(char::is_uppercase))
        .collect();
    variants.sort_unstable();
    assert_eq!(
        variants,
        ["Named", "Present", "Unnamed"],
        "a new token provenance needs a decision in plan_handshake and a case here"
    );

    // Nothing named a token and the default file is unusable: say hello with
    // no token and let the daemon name the file it wrote.
    let nowhere = vitrum_proto::token::TokenError::Missing {
        path: std::path::PathBuf::from("/src/vitrum/token"),
    };
    assert!(
        matches!(
            plan_handshake(cli::Token::Unnamed(nowhere)),
            Handshake::Anonymous(_)
        ),
        "a token nobody named must not refuse the handshake"
    );

    // Named on the command line and absent from disk: refuse, with the path in
    // the sentence.
    let named = cli::resolve_token_from(None, Some("/src/vitrum/no-such-token"));
    let Handshake::Refuse(detail) = plan_handshake(named) else {
        panic!("a named token file that does not exist must refuse");
    };
    assert!(
        detail.contains("no-such-token"),
        "the refusal must name the file the operator pointed at: {detail}"
    );

    // Named in the environment and malformed: the same arm, because a stale
    // token from a previous daemon is exactly this.
    let stale = cli::resolve_token_from(Some("not-hex"), None);
    assert!(
        matches!(plan_handshake(stale), Handshake::Refuse(_)),
        "a token that fails validation must not be presented"
    );

    // Well formed: presented verbatim.
    let good = "a".repeat(vitrum_proto::token::TOKEN_HEX_LEN);
    assert_eq!(
        plan_handshake(cli::resolve_token_from(Some(&good), None)),
        Handshake::Present(good)
    );
}

/// A session whose child exits must be marked exited even if nothing was
/// watching it.
///
/// WHY: the operator's report was a row that stayed alive forever. `Exited`
/// arrives for a session this window may never have attached to, and folding it
/// only for the attached one leaves nineteen rows claiming a running agent that
/// is gone. The exit code has to survive too: a session that failed and a
/// session that finished are different rows.
///
/// What this does NOT catch: a daemon that never sends `Exited`, which is why
/// `List` is re-sent on every accepted handshake.
#[test]
fn a_session_that_exits_unobserved_is_still_marked_exited() {
    let mut daemon = state::DaemonState::default();
    daemon.apply(ServerMsg::Sessions {
        sessions: vec![crate::testkit::info(4), crate::testkit::info(5)],
    });

    daemon.apply(ServerMsg::Exited {
        session: SessionId(5),
        code: Some(1),
    });

    let five = daemon.row(SessionId(5)).expect("the row is still listed");
    assert_eq!(five.info.status, SessionStatus::Exited { code: Some(1) });
    let four = daemon.row(SessionId(4)).expect("the other row is untouched");
    assert_eq!(four.info.status, SessionStatus::Running);
}
