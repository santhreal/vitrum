use super::*;
use std::net::TcpListener;

/// A scratch directory that cleans itself up.
#[cfg(unix)]
struct Scratch(PathBuf);

#[cfg(unix)]
impl Scratch {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "vitrum-daemon-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&path).expect("a temp dir must be creatable");
        Self(path)
    }
}

#[cfg(unix)]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A loopback port with nothing listening on it.
///
/// The obvious version asked the OS for port 0 and dropped the socket,
/// which races anything else on the machine. Linux hands out
/// 32768..60999 for every outbound connection, and this suite spawns
/// daemons and webviews that take them constantly, so a port drawn from
/// that range can be gone before it is probed.
/// `no_autostart_leaves_a_dead_port_alone` saw exactly that twice: it
/// probed a port the kernel had just handed to somebody else and got
/// `AlreadyRunning` where it asserts `Disabled`.
///
/// 12_000..20_000 is below the ephemeral range (see
/// `/proc/sys/net/ipv4/ip_local_port_range`) so the kernel never assigns
/// it automatically. The only way to collide is a service deliberately
/// bound there, which the bind below catches by walking to the next port.
/// The counter keeps two callers in this process apart; the pid offset
/// keeps two concurrent `cargo test` runs apart.
fn free_port() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};

    static NEXT: AtomicU16 = AtomicU16::new(0);
    let base = (std::process::id() % 8_000) as u16;

    for _ in 0..500 {
        let offset = NEXT.fetch_add(1, Ordering::Relaxed);
        let port = 12_000 + (base.wrapping_add(offset) % 8_000);
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) {
            drop(listener);
            return port;
        }
    }
    panic!("no free loopback port after 500 attempts");
}

/// The URL is parsed to a `host:port` without a URL crate, and a shape
/// this program never emits is rejected rather than guessed at.
///
/// Getting this wrong points the probe at the wrong port, which makes the
/// client spawn a second daemon beside a perfectly good one.
#[test]
fn the_daemon_address_comes_out_of_the_websocket_url() {
    assert_eq!(
        ws_authority("ws://127.0.0.1:7737").unwrap(),
        "127.0.0.1:7737"
    );
    assert_eq!(
        ws_authority("ws://127.0.0.1:7737/socket").unwrap(),
        "127.0.0.1:7737"
    );
    assert_eq!(
        ws_authority("wss://box.local:9000").unwrap(),
        "box.local:9000"
    );
    // A missing port takes the scheme's default rather than silently
    // becoming port 0.
    assert_eq!(ws_authority("ws://box.local").unwrap(), "box.local:80");
    assert_eq!(ws_authority("wss://box.local").unwrap(), "box.local:443");

    assert!(ws_authority("http://127.0.0.1:7737").is_err());
    assert!(ws_authority("ws://").is_err());
    assert!(ws_authority("ws://user:pw@host:1").is_err());
}

/// A machine with no daemon binary gets a sentence naming the binary, the
/// places that were searched, and the command to run.
///
/// This is the defect the whole feature exists for: before it, a first
/// launch showed "disconnected" and a Retry button that could never
/// succeed, and never said that a second binary had to be started by hand.
#[test]
fn a_missing_daemon_is_named_not_reported_as_disconnected() {
    let outcome = Autostart::NotFound {
        looked: vec![PathBuf::from("/opt/vitrum/vitrum-server")],
        command: "vitrum-server".to_string(),
    };
    assert!(!outcome.connectable());
    let msg = outcome.failure().expect("a failure must say something");
    assert!(msg.contains(DAEMON_BIN), "{msg}");
    assert!(msg.contains("/opt/vitrum/vitrum-server"), "{msg}");
    assert!(
        msg.contains("Start it yourself with: vitrum-server"),
        "{msg}"
    );
    assert!(
        !msg.to_lowercase().contains("disconnected"),
        "the generic word is exactly what this replaces: {msg}"
    );
}

/// Each failure mode says a different thing. A shared message would send
/// the operator to look at the wrong machine.
#[test]
fn every_failure_mode_reads_differently() {
    let messages: Vec<String> = vec![
        Autostart::NotFound {
            looked: Vec::new(),
            command: "vitrum-server".into(),
        },
        Autostart::Died {
            detail: "exit status 1: binding 127.0.0.1:7737: Address already in use".into(),
            log: Some(PathBuf::from("/tmp/daemon.log")),
        },
        Autostart::BadAddress {
            url: "http://nope".into(),
            detail: "not a ws:// or wss:// URL".into(),
        },
        Autostart::Unresponsive {
            address: "127.0.0.1:7737".into(),
            waited_ms: 3000,
        },
    ]
    .into_iter()
    .map(|o| o.failure().expect("all four are failures"))
    .collect();

    for (i, a) in messages.iter().enumerate() {
        for b in messages.iter().skip(i + 1) {
            assert_ne!(a, b, "two failure modes share a message");
        }
    }
    // The one an operator is most likely to hit carries the daemon's own
    // words rather than swallowing them.
    assert!(
        messages[1].contains("Address already in use"),
        "{}",
        messages[1]
    );
    assert!(messages[1].contains("/tmp/daemon.log"), "{}", messages[1]);
}

/// The three outcomes that leave something to connect to show no banner.
#[test]
fn a_working_daemon_produces_no_error_text() {
    for ok in [
        Autostart::AlreadyRunning,
        Autostart::Started {
            pid: 1234,
            path: PathBuf::from("/usr/bin/vitrum-server"),
        },
        Autostart::Disabled,
    ] {
        assert!(ok.connectable(), "{ok:?}");
        assert_eq!(ok.failure(), None, "{ok:?} put a banner over a live socket");
    }
}

/// Something already listening is reused, and nothing is spawned.
///
/// The failure this locks out is the expensive one: a second daemon means
/// a second set of PTYs, and the sidebar showing an empty machine while
/// twenty agents run in the daemon nobody is talking to.
#[test]
fn a_running_daemon_is_reused_and_never_duplicated() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("loopback binds");
    let port = listener.local_addr().unwrap().port();
    let url = format!("ws://127.0.0.1:{port}");

    // `allow_spawn` is true, so a spawn is what would happen if the probe
    // were broken. There is no daemon binary involved: reaching the spawn
    // path at all would produce NotFound or Died, never AlreadyRunning.
    assert_eq!(ensure_daemon(&url, true), Autostart::AlreadyRunning);
    drop(listener);
}

/// With autostart off, a dead port is reported as a choice rather than a
/// failure, and still nothing is spawned.
///
/// The precondition is established, not assumed. This test needs a port
/// with nothing listening, and no port number can be RESERVED in that
/// state: the moment `free_port` closes its probe socket, the number is
/// available to this process and every other one. Running alone the test
/// never failed in 20 runs; running beside its siblings it failed 2 times
/// in 12, always as `AlreadyRunning` where it asserts `Disabled`, which is
/// the signature of somebody holding the port rather than of broken logic.
///
/// Three attempts at picking a "safe" range did not fix that and could
/// not, because the race is not about which numbers are used. So the loop
/// below checks the precondition actually holds and moves to another port
/// when it does not. The assertion itself is unchanged and unguarded: once
/// a genuinely dead port is in hand, `Disabled` is required.
#[test]
fn no_autostart_leaves_a_dead_port_alone() {
    for attempt in 0..16 {
        let port = free_port();
        let url = format!("ws://127.0.0.1:{port}");
        let outcome = ensure_daemon(&url, false);

        if outcome == Autostart::AlreadyRunning {
            // The port was taken between `free_port` and the probe. That
            // is the race, not the behaviour under test.
            assert!(
                attempt < 15,
                "16 ports in a row were occupied between selection and \
                 probe; that is no longer a race, something is binding \
                 everything this process picks"
            );
            continue;
        }

        assert_eq!(outcome, Autostart::Disabled, "port {port}");
        assert!(
            outcome.connectable(),
            "the operator's own daemon may still appear"
        );
        assert_eq!(outcome.failure(), None);
        return;
    }
}

/// A URL that names nowhere is caught before anything is spawned.
#[test]
fn an_unusable_url_never_reaches_the_spawn_path() {
    let outcome = ensure_daemon("http://127.0.0.1:7737", true);
    match outcome {
        Autostart::BadAddress { url, .. } => assert_eq!(url, "http://127.0.0.1:7737"),
        other => panic!("expected BadAddress, got {other:?}"),
    }
}

/// A daemon that exits immediately reports its own output, not a shrug.
///
/// Driven with a real child process rather than a constructed value,
/// because the thing being tested is that the exit is noticed at all and
/// that what the process wrote survives into the message.
#[cfg(unix)]
#[test]
fn a_daemon_that_dies_reports_what_it_said() {
    let scratch = Scratch::new("dies");
    let fake = scratch.0.join(DAEMON_BIN);
    std::fs::write(
        &fake,
        "#!/bin/sh\necho 'binding 127.0.0.1:7737: Address already in use' >&2\nexit 1\n",
    )
    .expect("writing a script");
    make_executable(&fake);

    let log = scratch.0.join("daemon.log");
    let mut child = spawn_daemon(&fake, "127.0.0.1:7737", Some(&log)).expect("sh runs");
    let status = child.wait().expect("the script exits at once");
    assert!(!status.success());

    let detail = describe_exit(status, Some(&log));
    assert!(detail.contains("exit status 1"), "{detail}");
    assert!(detail.contains("Address already in use"), "{detail}");
}

/// The daemon outlives the client. This is the one that must never
/// regress.
///
/// The entire reason the daemon is a separate process is that agents
/// survive the GUI. A spawn that dies with its parent destroys twenty
/// running agents every time somebody closes a window, and it would look
/// completely fine in every other test.
#[cfg(unix)]
#[test]
fn the_daemon_survives_the_client_that_started_it() {
    let scratch = Scratch::new("survives");
    let fake = scratch.0.join(DAEMON_BIN);
    let flag = scratch.0.join("still-here");
    // Sleeps, then proves it was still alive by writing the flag. Also
    // reports its own session id, which is what `setsid` changes.
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nps -o sid= -p $$ > {sid}\nsleep 2\ntouch {flag}\n",
            sid = scratch.0.join("sid").display(),
            flag = flag.display()
        ),
    )
    .expect("writing a script");
    make_executable(&fake);

    let child = spawn_daemon(&fake, "127.0.0.1:7737", None).expect("sh runs");
    let pid = child.id();
    // Dropping the handle is what the real code does: no wait, no kill.
    drop(child);

    // Its session id must differ from ours, which is what keeps a Ctrl-C
    // in the launching terminal from taking the daemon with it.
    //
    // Polled to a deadline rather than slept at for a fixed 400ms. The
    // fixed sleep was a guess about how long a machine takes to exec a
    // shell script, and this suite runs on machines that are also building
    // something else: at load 145 it failed two runs in five, which is a
    // flaky test reporting a bug that is not there. Waiting for the
    // condition is both faster when idle and correct when loaded.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let sid = loop {
        let raw = std::fs::read_to_string(scratch.0.join("sid")).unwrap_or_default();
        if let Ok(n) = raw.trim().parse::<i32>() {
            if n > 0 {
                break n;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the child never reported a session id"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_ne!(
        sid,
        own_session_id(),
        "the daemon is still in this process's session and dies with it"
    );
    assert_eq!(
        sid as u32, pid,
        "setsid makes the child its own session leader"
    );

    // And it is still running well after the handle was dropped.
    std::thread::sleep(Duration::from_millis(2000));
    for _ in 0..40 {
        if flag.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        flag.exists(),
        "the spawned process did not live long enough to finish its work"
    );
}

/// Two clients starting at once produce exactly one daemon.
///
/// Driven against the REAL `vitrum-server`, not a shell stand-in. The
/// first stand-in tried here was `nc -l`, which stops listening the
/// instant the probe connects to it, so the second racer correctly
/// concluded the port was dead and started a second daemon. The test was
/// wrong and the code was right, and no amount of staring at the code
/// would have shown that: the only way to know is to race the thing that
/// actually holds the port the way the product's daemon holds it.
#[cfg(unix)]
#[test]
fn two_simultaneous_launches_start_one_daemon() {
    let Some(real) = built_daemon() else {
        eprintln!(
            "skipping: no {DAEMON_BIN} built beside this test, so there is no daemon to race"
        );
        return;
    };
    let scratch = Scratch::new("race");
    let staged = scratch.0.join(DAEMON_BIN);
    std::fs::copy(&real, &staged).expect("copying the daemon into a scratch dir");
    make_executable(&staged);

    let port = free_port();
    let url = format!("ws://127.0.0.1:{port}");
    let results: Vec<Autostart> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let url = url.clone();
                let dir = scratch.0.clone();
                // `find_daemon` looks beside the executable and then on
                // PATH; the scratch dir is prepended so the staged copy
                // wins and the test never touches an installed one.
                scope.spawn(move || with_path_prefix(&dir, || ensure_daemon(&url, true)))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("no panic"))
            .collect()
    });

    let live = daemons_on_port(port);
    // Kill before asserting, so a failure does not strand a daemon.
    for pid in &live {
        kill_process(*pid);
    }

    assert!(
        results.iter().all(|r| r.connectable()),
        "a racer reported a failure: {results:?}"
    );
    let spawned = results
        .iter()
        .filter(|r| matches!(r, Autostart::Started { .. }))
        .count();
    assert_eq!(spawned, 1, "both racers claimed the spawn: {results:?}");
    assert_eq!(
        live.len(),
        1,
        "{} daemon processes are on port {port}; outcomes were {results:?}",
        live.len()
    );
}

/// The `vitrum-server` this workspace built, if there is one.
///
/// A test binary lives in `target/<profile>/deps/`, so the daemon is one
/// directory up from where [`find_daemon`] would look. Found here rather
/// than by loosening `find_daemon`, because the sibling rule is what the
/// product wants and `deps/` is a cargo detail.
#[cfg(unix)]
fn built_daemon() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?;
    for _ in 0..3 {
        let candidate = dir.join(DAEMON_BIN);
        if is_executable(&candidate) {
            return Some(candidate);
        }
        dir = dir.parent()?;
    }
    None
}

/// Every daemon process currently asked to serve `port`.
///
/// Read from `/proc` command lines rather than from the return values,
/// because the whole question is whether the return values are telling the
/// truth about how many processes exist.
#[cfg(target_os = "linux")]
fn daemons_on_port(port: u16) -> Vec<u32> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(raw) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let args: Vec<String> = raw
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        let is_daemon = args.first().is_some_and(|a| a.ends_with(DAEMON_BIN));
        if is_daemon && args.iter().any(|a| a == &port.to_string()) {
            out.push(pid);
        }
    }
    out
}

#[cfg(all(unix, not(target_os = "linux")))]
fn daemons_on_port(port: u16) -> Vec<u32> {
    let out = Command::new("pgrep")
        .arg("-f")
        .arg(format!("{DAEMON_BIN} --port {port}"))
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    out.lines().filter_map(|l| l.trim().parse().ok()).collect()
}

/// The binary beside this executable wins over one on `PATH`.
///
/// A `PATH` hit can be any version; the sibling is the build that shipped
/// with this client, and a client talking to a daemon from another release
/// fails at the protocol handshake rather than anywhere useful.
#[test]
fn the_sibling_binary_is_preferred_over_path() {
    let looked = match find_daemon() {
        DaemonBinary::Found(path) => {
            let exe = std::env::current_exe().expect("a running test has an executable");
            let beside = exe.parent().map(|d| d.join(DAEMON_BIN));
            // Whatever was found, the sibling was tried first.
            if let Some(beside) = beside
                && beside.exists()
            {
                assert_eq!(path, beside, "a PATH hit beat the sibling");
            }
            return;
        }
        DaemonBinary::Missing { looked } => looked,
    };
    // Nothing installed: the message still has to name where it looked.
    assert!(
        looked.iter().any(|p| p.ends_with(DAEMON_BIN)),
        "the search never tried a sibling: {looked:?}"
    );
}

/// The hand-run command names the port when it is not the default, so
/// copying it actually reproduces the configuration that failed.
#[test]
fn the_suggested_command_carries_a_nondefault_port() {
    assert_eq!(manual_command("127.0.0.1:7737"), DAEMON_BIN);
    assert_eq!(
        manual_command("127.0.0.1:9000"),
        "vitrum-server --port 9000"
    );
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).expect("just written").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod");
}

#[cfg(unix)]
fn own_session_id() -> i32 {
    // Safety: `getsid(0)` reads the calling process's session and has no
    // failure mode for that argument.
    unsafe { getsid(0) }
}

#[cfg(unix)]
unsafe extern "C" {
    fn getsid(pid: i32) -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(unix)]
fn kill_process(pid: u32) {
    // Safety: SIGTERM to a pid this test started. Worst case it has
    // already exited and the call returns ESRCH.
    unsafe {
        kill(pid as i32, 15);
    }
}

/// Run `job` with `dir` at the front of `PATH`.
///
/// Serialised on the spawn lock's sibling so two threads cannot fight over
/// the process environment.
#[cfg(unix)]
fn with_path_prefix<T>(dir: &Path, job: impl FnOnce() -> T) -> T {
    static PATH_EDIT: Mutex<()> = Mutex::new(());
    let _held = PATH_EDIT.lock().unwrap_or_else(|e| e.into_inner());
    let old = std::env::var_os("PATH");
    let mut entries = vec![dir.to_path_buf()];
    if let Some(old) = &old {
        entries.extend(std::env::split_paths(old));
    }
    let joined = std::env::join_paths(entries).expect("paths without colons");
    // Safety: the process-wide lock above makes this the only thread
    // touching PATH, and the value is restored before the lock is released.
    unsafe { std::env::set_var("PATH", &joined) };
    let out = job();
    unsafe {
        match old {
            Some(old) => std::env::set_var("PATH", old),
            None => std::env::remove_var("PATH"),
        }
    }
    out
}
