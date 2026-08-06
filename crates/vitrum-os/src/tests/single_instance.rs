//! Single-instance handoff: the wire protocol, path limits, and the real
//! lock-and-socket race on Unix.

// Only the real lock-and-socket race needs these, and that race is Unix-only.
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::sync::mpsc;
#[cfg(unix)]
use std::time::Duration;

use vitrum_proto::SessionId;

use crate::deeplink::{DeepLink, DeepLinkError};
use crate::paths::Platform;
use crate::single_instance::{
    ACTIVATION_PROTOCOL, Activation, ActivationError, MAX_ACTIVATION_LEN, SingleInstanceError,
    check_socket_path, decode_activation, encode_activation, unix_socket_path_limit,
    windows_mutex_name, windows_pipe_name,
};
#[cfg(unix)]
use crate::tests::support::TempDir;

/// Both activations must round-trip through the wire format.
#[test]
fn both_activations_round_trip() {
    for activation in [
        Activation::Focus,
        Activation::Open(DeepLink::Home),
        Activation::Open(DeepLink::Session(SessionId(42))),
        Activation::Open(DeepLink::Session(SessionId(u64::MAX))),
    ] {
        let bytes = encode_activation(&activation);
        assert_eq!(decode_activation(&bytes), Ok(activation.clone()), "round trip");
    }
}

/// The encoded form is a wire contract between two builds, so pin it.
///
/// A second launch may be a different build than the running one during an
/// upgrade. The banner is what makes that detectable instead of silently
/// misparsed.
#[test]
fn the_encoded_form_is_exactly_this() {
    assert_eq!(encode_activation(&Activation::Focus), b"vitrum-instance/1 focus\n");
    assert_eq!(
        encode_activation(&Activation::Open(DeepLink::Session(SessionId(9)))),
        b"vitrum-instance/1 open vitrum://session/9\n"
    );
    assert_eq!(ACTIVATION_PROTOCOL, "vitrum-instance/1");
}

/// A message with the wrong banner must be rejected, naming what arrived.
///
/// The socket is reachable by anything running as this user. A stray probe or
/// a mismatched build must not be able to drive the app.
#[test]
fn a_wrong_banner_is_rejected() {
    assert_eq!(
        decode_activation(b"vitrum-instance/2 focus\n"),
        Err(ActivationError::WrongProtocol { found: "vitrum-instance/2".to_string() })
    );
    assert_eq!(
        decode_activation(b"GET / HTTP/1.1\r\n"),
        Err(ActivationError::WrongProtocol { found: "GET".to_string() })
    );
    assert_eq!(
        decode_activation(b""),
        Err(ActivationError::WrongProtocol { found: String::new() })
    );
}

/// An unknown verb must be an error, never a silent fall back to focus.
///
/// Falling back would hide both a bug and a probe, and would mean a future verb
/// silently means "raise the window" on every older build.
#[test]
fn an_unknown_verb_is_rejected() {
    assert_eq!(
        decode_activation(b"vitrum-instance/1 quit\n"),
        Err(ActivationError::UnknownVerb { verb: "quit".to_string() })
    );
    assert_eq!(
        decode_activation(b"vitrum-instance/1\n"),
        Err(ActivationError::UnknownVerb { verb: String::new() })
    );
}

/// `open` without a URL must be an error.
#[test]
fn open_without_a_url_is_rejected() {
    assert_eq!(decode_activation(b"vitrum-instance/1 open\n"), Err(ActivationError::MissingUrl));
    assert_eq!(decode_activation(b"vitrum-instance/1 open \n"), Err(ActivationError::MissingUrl));
}

/// A malformed URL must surface the deep-link error, not a generic one.
///
/// The operator debugging "clicking the notification does nothing" needs to see
/// which part failed.
#[test]
fn a_malformed_url_surfaces_the_deep_link_error() {
    assert_eq!(
        decode_activation(b"vitrum-instance/1 open http://evil/1\n"),
        Err(ActivationError::BadUrl(DeepLinkError::WrongScheme { found: "http".to_string() }))
    );
    assert_eq!(
        decode_activation(b"vitrum-instance/1 open vitrum://session/-1\n"),
        Err(ActivationError::BadUrl(DeepLinkError::InvalidId {
            target: "session",
            value: "-1".to_string()
        }))
    );
}

/// Trailing data after `focus` must be rejected.
///
/// It is the shape of a smuggled second command.
#[test]
fn trailing_data_after_focus_is_rejected() {
    assert_eq!(
        decode_activation(b"vitrum-instance/1 focus extra\n"),
        Err(ActivationError::TrailingData { extra: "extra".to_string() })
    );
}

/// Non-UTF-8 bytes must be rejected rather than lossily converted.
#[test]
fn non_utf8_is_rejected() {
    assert_eq!(decode_activation(&[0xff, 0xfe, 0xfd]), Err(ActivationError::NotUtf8));
}

/// An oversized message must be rejected on length before it is scanned.
///
/// A peer that can make the primary allocate arbitrarily is a local denial of
/// service against the one process that owns the user's whole session.
#[test]
fn an_oversized_message_is_rejected() {
    let bytes = vec![b'x'; MAX_ACTIVATION_LEN + 1];
    assert_eq!(
        decode_activation(&bytes),
        Err(ActivationError::TooLong { len: MAX_ACTIVATION_LEN + 1 })
    );
}

/// Both line endings must be accepted.
///
/// A Windows peer writes `\r\n`.
#[test]
fn both_line_endings_are_accepted() {
    assert_eq!(decode_activation(b"vitrum-instance/1 focus\r\n"), Ok(Activation::Focus));
    assert_eq!(decode_activation(b"vitrum-instance/1 focus"), Ok(Activation::Focus));
}

/// A launch with a deep link in its arguments must open it.
#[test]
fn a_deep_link_argument_becomes_an_open() {
    assert_eq!(
        Activation::from_args(&["/usr/bin/vitrum", "vitrum://session/5"]),
        Activation::Open(DeepLink::Session(SessionId(5)))
    );
}

/// A launch with no deep link must focus, and unknown flags must not break it.
///
/// Desktop environments append their own arguments. Rejecting an unrecognised
/// flag would mean a second launch from a launcher does nothing at all.
#[test]
fn unknown_arguments_still_focus() {
    assert_eq!(Activation::from_args(&["/usr/bin/vitrum"]), Activation::Focus);
    assert_eq!(
        Activation::from_args(&["/usr/bin/vitrum", "--gapplication-service", "-v"]),
        Activation::Focus
    );
    assert_eq!(Activation::from_args::<&str>(&[]), Activation::Focus);
}

/// The first valid deep link wins, and a malformed one does not shadow it.
#[test]
fn the_first_valid_link_wins() {
    assert_eq!(
        Activation::from_args(&["vitrum://nope", "vitrum://session/2", "vitrum://session/3"]),
        Activation::Open(DeepLink::Session(SessionId(2)))
    );
}

/// `Activation::link` must expose the payload for a caller that only cares
/// about the target.
#[test]
fn the_link_accessor_exposes_the_payload() {
    assert_eq!(Activation::Focus.link(), None);
    assert_eq!(
        Activation::Open(DeepLink::Session(SessionId(1))).link(),
        Some(DeepLink::Session(SessionId(1)))
    );
}

/// The `sockaddr_un` limits must be the real per-platform ones.
///
/// 108 on Linux, 104 on the BSDs including macOS. Using the Linux number
/// everywhere means a macOS path between 104 and 108 bytes passes the check and
/// then fails at `bind` with `ENAMETOOLONG`, which surfaces as "the app will not
/// start" with no explanation.
#[test]
fn the_socket_path_limits_are_per_platform() {
    assert_eq!(unix_socket_path_limit(Platform::Linux), 108);
    assert_eq!(unix_socket_path_limit(Platform::MacOs), 104);
}

/// An over-long socket path must be rejected up front, naming the limit.
#[test]
fn an_over_long_socket_path_is_rejected_with_its_limit() {
    let long = std::path::PathBuf::from(format!("/run/user/1000/{}/instance.sock", "x".repeat(120)));
    let len = long.as_os_str().as_encoded_bytes().len();
    assert_eq!(
        check_socket_path(Platform::Linux, &long),
        Err(SingleInstanceError::SocketPathTooLong { path: long.clone(), len, limit: 107 })
    );
    let message = check_socket_path(Platform::Linux, &long).unwrap_err().to_string();
    assert!(message.contains("the platform limit is 107"), "unhelpful message: {message}");
}

/// A path exactly at the limit must be accepted, and one byte more rejected.
///
/// The terminator has to be counted. An off-by-one here is a socket that binds
/// on Linux and not on macOS, or vice versa.
#[test]
fn the_socket_path_boundary_is_exact() {
    let at_limit = std::path::PathBuf::from("/".repeat(107));
    assert_eq!(check_socket_path(Platform::Linux, &at_limit), Ok(()));
    let over = std::path::PathBuf::from("/".repeat(108));
    assert!(check_socket_path(Platform::Linux, &over).is_err());
    // The same path is already too long for macOS.
    assert!(check_socket_path(Platform::MacOs, &at_limit).is_err());
    assert_eq!(check_socket_path(Platform::MacOs, &std::path::PathBuf::from("/".repeat(103))), Ok(()));
}

/// The Windows mutex must live in the per-logon-session namespace.
///
/// A `Global\` name would let one user's running instance block another user's
/// launch on a shared machine or a terminal server.
#[test]
fn the_windows_mutex_is_session_scoped() {
    assert_eq!(windows_mutex_name(), "Local\\dev.santhreal.vitrum.instance");
    assert!(!windows_mutex_name().starts_with("Global\\"));
}

/// The Windows pipe name must include the user, because pipes are machine-wide.
///
/// The mutex is per-session but the pipe namespace is not. Without the user in
/// the name, a second user's launch tries to hand off to the first user's
/// process and is refused by the pipe ACL, which looks like a hang.
#[test]
fn the_windows_pipe_name_is_scoped_to_the_user() {
    assert_eq!(windows_pipe_name("ada"), r"\\.\pipe\vitrum-instance-ada");
    assert_ne!(windows_pipe_name("ada"), windows_pipe_name("bob"));
}

/// Characters that are illegal in a pipe name must be replaced.
///
/// A domain account is `DOMAIN\user`, and a backslash in a pipe name creates a
/// path component the kernel rejects. Passing it through would make the app
/// fail to start for every domain user.
#[test]
fn illegal_characters_in_a_user_name_are_replaced() {
    assert_eq!(windows_pipe_name(r"CORP\ada"), r"\\.\pipe\vitrum-instance-CORP_ada");
    assert_eq!(windows_pipe_name("a b/c*d"), r"\\.\pipe\vitrum-instance-a_b_c_d");
    assert_eq!(windows_pipe_name("ada-1_2"), r"\\.\pipe\vitrum-instance-ada-1_2");
    assert_eq!(windows_pipe_name(""), r"\\.\pipe\vitrum-instance-");
}

/// The first launch must win the lock and the second must hand off to it.
///
/// This is the feature, exercised for real: two `acquire` calls against the
/// same lock file, in one process, through a real `flock` and a real Unix
/// socket. `flock` treats two open file descriptions independently even within
/// one process, which is exactly why this can be tested without spawning.
#[cfg(unix)]
#[test]
fn the_second_launch_hands_its_activation_to_the_first() {
    use crate::single_instance::{Acquisition, acquire};

    let dir = TempDir::new("si-race");
    let lock = dir.join("instance.lock");
    let socket = dir.join("instance.sock");

    let first = acquire(&lock, &socket, &Activation::Focus).expect("the first launch must win");
    let Acquisition::Primary(guard) = first else {
        panic!("the first launch must be primary");
    };
    assert!(socket.exists(), "the primary must bind the socket");

    let (tx, rx) = mpsc::channel();
    guard
        .listen(Arc::new(move |activation| {
            let _ = tx.send(activation);
        }))
        .expect("the primary must be able to listen");

    let wanted = Activation::Open(DeepLink::Session(SessionId(77)));
    let second = acquire(&lock, &socket, &wanted).expect("the second launch must hand off");
    assert!(!second.is_primary(), "the second launch must not become a second instance");
    assert!(matches!(second, Acquisition::HandedOff));

    let received = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the primary must receive the handoff");
    assert_eq!(received, wanted);
}

/// A garbage message must not reach the sink.
///
/// The sink drives real UI actions. A hostile local process writing junk into
/// the socket must be ignored, and the listener must survive to serve the next
/// legitimate handoff.
#[cfg(unix)]
#[test]
fn a_garbage_message_is_dropped_and_the_listener_survives() {
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    use crate::single_instance::{Acquisition, acquire};

    let dir = TempDir::new("si-garbage");
    let lock = dir.join("instance.lock");
    let socket = dir.join("instance.sock");

    let Acquisition::Primary(guard) =
        acquire(&lock, &socket, &Activation::Focus).expect("primary")
    else {
        panic!("must be primary");
    };
    let (tx, rx) = mpsc::channel();
    guard.listen(Arc::new(move |a| { let _ = tx.send(a); })).expect("listen");

    let mut junk = UnixStream::connect(&socket).expect("the socket accepts connections");
    junk.write_all(b"GET / HTTP/1.1\r\n\r\n").expect("write");
    junk.shutdown(std::net::Shutdown::Write).expect("half close");
    drop(junk);

    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "garbage must not reach the sink"
    );

    // The listener is still alive and still serves a real handoff.
    let wanted = Activation::Open(DeepLink::Home);
    acquire(&lock, &socket, &wanted).expect("handoff");
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).expect("real handoff arrives"), wanted);
}

/// Dropping the guard must release the claim and remove the socket.
///
/// Otherwise a crash-free quit leaves a socket file that the next launch has to
/// distinguish from a live instance, which is the exact failure mode a pid file
/// has and `flock` exists to avoid.
#[cfg(unix)]
#[test]
fn dropping_the_guard_releases_the_claim() {
    use crate::single_instance::acquire;

    let dir = TempDir::new("si-drop");
    let lock = dir.join("instance.lock");
    let socket = dir.join("instance.sock");

    {
        let first = acquire(&lock, &socket, &Activation::Focus).expect("primary");
        assert!(first.is_primary());
        assert!(socket.exists());
    }
    assert!(!socket.exists(), "the socket must be removed on drop");

    let second = acquire(&lock, &socket, &Activation::Focus).expect("the claim must be free");
    assert!(second.is_primary(), "a released claim must be takeable again");
}

/// A primary whose activation listener never started must still free the claim
/// when it lets the guard go.
///
/// This is what makes failing open possible. If `listen` fails, the process
/// holds a slot it cannot serve: the lock still turns every later launch into
/// a handoff, and there is nothing on the other end to open a window, so
/// `vitrum` typed a second time produces nothing at all. The app's answer is
/// to drop the guard and run standalone, and that answer only works if a
/// never-listened guard releases exactly like a listening one does.
#[cfg(unix)]
#[test]
fn a_guard_whose_listener_failed_still_releases_the_claim() {
    use crate::single_instance::{Acquisition, acquire};

    let dir = TempDir::new("si-listen-fail");
    let lock = dir.join("instance.lock");
    let socket = dir.join("instance.sock");

    {
        let Acquisition::Primary(guard) =
            acquire(&lock, &socket, &Activation::Focus).expect("primary")
        else {
            panic!("the first acquire must be the primary");
        };
        guard.listen(Arc::new(|_| {})).expect("the first listen starts");
        // The second call is the deterministic stand-in for the real failure:
        // the listener slot is consumed, so this is the same `Err` the app
        // sees when the thread cannot be spawned at all.
        guard
            .listen(Arc::new(|_| {}))
            .expect_err("a second listen must fail, the slot is taken");
        assert!(socket.exists());
    }

    assert!(!socket.exists(), "the socket must be removed even after a failed listen");
    let second = acquire(&lock, &socket, &Activation::Focus)
        .expect("the next launch must be able to take the claim");
    assert!(
        second.is_primary(),
        "a launch after a failed listener must become primary and open a window, \
         not hand off to a process that cannot answer"
    );
}

/// A stale socket file left by a crash must not block the next launch.
///
/// After a `SIGKILL` the kernel releases the `flock` but nothing unlinks the
/// socket. A `bind` onto an existing path fails with `EADDRINUSE`, so the new
/// primary has to remove it, and it is safe to do so precisely because holding
/// the lock proves nobody else is using it.
#[cfg(unix)]
#[test]
fn a_stale_socket_from_a_crash_does_not_block_the_next_launch() {
    use crate::single_instance::acquire;

    let dir = TempDir::new("si-stale");
    let lock = dir.join("instance.lock");
    let socket = dir.join("instance.sock");
    std::fs::write(&socket, b"corpse").expect("plant a stale socket file");

    let acquired = acquire(&lock, &socket, &Activation::Focus)
        .expect("a stale socket must be cleared, not fatal");
    assert!(acquired.is_primary());
    assert!(socket.exists());
}

/// A socket path that cannot fit in `sockaddr_un` must fail before any
/// syscall, with the path in the message.
#[cfg(unix)]
#[test]
fn an_unbindable_socket_path_fails_before_touching_the_filesystem() {
    use crate::single_instance::acquire;

    let dir = TempDir::new("si-long");
    let lock = dir.join("instance.lock");
    let socket = dir.join(&"x".repeat(200));
    let err = acquire(&lock, &socket, &Activation::Focus)
        .expect_err("the path cannot be bound on any Unix");
    assert!(
        matches!(err, SingleInstanceError::SocketPathTooLong { .. }),
        "expected a path-length error, got {err}"
    );
    assert!(!lock.exists(), "nothing may be created before the path is validated");
}
