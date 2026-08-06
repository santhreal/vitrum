//! Directory resolution for all three platforms, asserted with exact paths.
//!
//! Synthetic Windows environments use forward slashes. Windows accepts them,
//! and it keeps one expectation string correct on every host, which is the
//! whole point of resolution being a pure function.

use crate::paths::{AppPaths, PathEnv, PathError, Platform};
use crate::tests::support::assert_path;

fn linux_env() -> PathEnv {
    PathEnv::from_pairs([("HOME", "/home/ada")])
}

/// XDG defaults must be exactly the ones in the specification.
///
/// Every one of these is a place a user's backup tool, dotfile manager or
/// `rm -rf ~/.cache` expects to find us. Getting `.local/share` wrong by
/// writing to `.local/data` produces a product that silently loses settings
/// when someone migrates a home directory.
#[test]
fn linux_falls_back_to_the_xdg_specified_defaults() {
    let p = AppPaths::resolve(Platform::Linux, &linux_env()).expect("HOME is set");
    assert_path(&p.config_dir, "/home/ada/.config/vitrum");
    assert_path(&p.data_dir, "/home/ada/.local/share/vitrum");
    assert_path(&p.cache_dir, "/home/ada/.cache/vitrum");
    assert_path(&p.state_dir, "/home/ada/.local/state/vitrum");
}

/// Every XDG variable must be honoured when it is set.
///
/// A resolver that reads `HOME` and ignores `XDG_CONFIG_HOME` looks correct on
/// a stock desktop and breaks for every user who relocated their config, plus
/// every test harness and container that sets these to a scratch directory.
#[test]
fn linux_honours_every_xdg_variable() {
    let env = PathEnv::from_pairs([
        ("HOME", "/home/ada"),
        ("XDG_CONFIG_HOME", "/xdg/cfg"),
        ("XDG_DATA_HOME", "/xdg/data"),
        ("XDG_CACHE_HOME", "/xdg/cache"),
        ("XDG_STATE_HOME", "/xdg/state"),
        ("XDG_RUNTIME_DIR", "/run/user/1000"),
    ]);
    let p = AppPaths::resolve(Platform::Linux, &env).expect("HOME is set");
    assert_path(&p.config_dir, "/xdg/cfg/vitrum");
    assert_path(&p.data_dir, "/xdg/data/vitrum");
    assert_path(&p.cache_dir, "/xdg/cache/vitrum");
    assert_path(&p.state_dir, "/xdg/state/vitrum");
    assert_path(&p.runtime_dir, "/run/user/1000/vitrum");
}

/// A relative XDG value must be ignored, per the specification.
///
/// The spec says a relative path is invalid and must be ignored. Resolving it
/// against the current directory would put the config wherever the app was
/// launched from, so `cd /tmp && vitrum` would silently start with an empty
/// profile and write a new one into `/tmp`.
#[test]
fn linux_ignores_a_relative_xdg_value() {
    let env = PathEnv::from_pairs([("HOME", "/home/ada"), ("XDG_CONFIG_HOME", "relative/cfg")]);
    let p = AppPaths::resolve(Platform::Linux, &env).expect("HOME is set");
    assert_path(&p.config_dir, "/home/ada/.config/vitrum");
}

/// An exported-but-empty XDG value must be treated as unset.
///
/// `XDG_CONFIG_HOME=` is the normal shape of "I cleared this in my shell rc".
/// Reading it as a path yields the filesystem root, and the app then tries to
/// write to `/vitrum`.
#[test]
fn linux_treats_an_empty_xdg_value_as_unset() {
    let env = PathEnv::from_pairs([("HOME", "/home/ada"), ("XDG_DATA_HOME", "")]);
    let p = AppPaths::resolve(Platform::Linux, &env).expect("HOME is set");
    assert_path(&p.data_dir, "/home/ada/.local/share/vitrum");
}

/// Without `XDG_RUNTIME_DIR` the runtime directory must stay under the
/// per-user cache, never `/tmp`.
///
/// The single-instance socket lives here. A world-writable `/tmp` path lets any
/// local user pre-create the socket and receive another user's deep links.
#[test]
fn linux_runtime_dir_falls_back_under_the_cache_not_tmp() {
    let p = AppPaths::resolve(Platform::Linux, &linux_env()).expect("HOME is set");
    assert_path(&p.runtime_dir, "/home/ada/.cache/vitrum/run");
}

/// A relative `XDG_RUNTIME_DIR` must be rejected like any other XDG value.
#[test]
fn linux_ignores_a_relative_runtime_dir() {
    let env = PathEnv::from_pairs([("HOME", "/home/ada"), ("XDG_RUNTIME_DIR", "run")]);
    let p = AppPaths::resolve(Platform::Linux, &env).expect("HOME is set");
    assert_path(&p.runtime_dir, "/home/ada/.cache/vitrum/run");
}

/// No `HOME` must be an error naming the variable, not a panic and not `/`.
#[test]
fn linux_without_home_reports_the_missing_variable() {
    let err = AppPaths::resolve(Platform::Linux, &PathEnv::default())
        .expect_err("resolution cannot invent a home directory");
    assert_eq!(err, PathError::MissingEnv { platform: Platform::Linux, var: "HOME" });
    assert_eq!(
        err.to_string(),
        "cannot resolve linux application directories: $HOME is unset or empty"
    );
}

/// macOS must use Application Support and Caches, keyed by the bundle id.
///
/// A macOS app that writes to `~/.config` is invisible to Migration Assistant,
/// is not excluded from Time Machine correctly, and shows up as a stray dotfile
/// in the user's home.
#[test]
fn macos_uses_the_apple_directory_layout() {
    let env = PathEnv::from_pairs([("HOME", "/Users/ada")]);
    let p = AppPaths::resolve(Platform::MacOs, &env).expect("HOME is set");
    assert_path(&p.config_dir, "/Users/ada/Library/Application Support/dev.santhreal.vitrum");
    assert_path(&p.data_dir, "/Users/ada/Library/Application Support/dev.santhreal.vitrum");
    assert_path(&p.state_dir, "/Users/ada/Library/Application Support/dev.santhreal.vitrum");
    assert_path(&p.cache_dir, "/Users/ada/Library/Caches/dev.santhreal.vitrum");
}

/// macOS uses `$TMPDIR`, which is per-user, for the runtime directory.
#[test]
fn macos_runtime_dir_uses_the_per_user_tmpdir() {
    let env =
        PathEnv::from_pairs([("HOME", "/Users/ada"), ("TMPDIR", "/var/folders/q7/T/")]);
    let p = AppPaths::resolve(Platform::MacOs, &env).expect("HOME is set");
    assert_path(&p.runtime_dir, "/var/folders/q7/T/dev.santhreal.vitrum");
}

/// Without `$TMPDIR` macOS falls back inside Application Support.
#[test]
fn macos_runtime_dir_falls_back_into_application_support() {
    let env = PathEnv::from_pairs([("HOME", "/Users/ada")]);
    let p = AppPaths::resolve(Platform::MacOs, &env).expect("HOME is set");
    assert_path(
        &p.runtime_dir,
        "/Users/ada/Library/Application Support/dev.santhreal.vitrum/run",
    );
}

/// macOS without `HOME` must name `HOME`, not Windows' variable.
#[test]
fn macos_without_home_reports_the_missing_variable() {
    let err = AppPaths::resolve(Platform::MacOs, &PathEnv::default())
        .expect_err("resolution cannot invent a home directory");
    assert_eq!(err, PathError::MissingEnv { platform: Platform::MacOs, var: "HOME" });
}

fn windows_env() -> PathEnv {
    PathEnv::from_pairs([
        ("APPDATA", "C:/Users/ada/AppData/Roaming"),
        ("LOCALAPPDATA", "C:/Users/ada/AppData/Local"),
    ])
}

/// Windows must split roaming settings from local caches.
///
/// The split is the entire reason Windows has two AppData roots. Putting a
/// cache in Roaming means a domain user's login copies it over the network at
/// every sign-in, which is a real and well-known way to make logins slow.
#[test]
fn windows_splits_roaming_settings_from_local_caches() {
    let p = AppPaths::resolve(Platform::Windows, &windows_env()).expect("both roots are set");
    assert_path(&p.config_dir, "C:/Users/ada/AppData/Roaming/santhreal/vitrum/config");
    assert_path(&p.data_dir, "C:/Users/ada/AppData/Roaming/santhreal/vitrum/data");
    assert_path(&p.cache_dir, "C:/Users/ada/AppData/Local/santhreal/vitrum/cache");
    assert_path(&p.state_dir, "C:/Users/ada/AppData/Local/santhreal/vitrum/state");
    assert_path(&p.runtime_dir, "C:/Users/ada/AppData/Local/santhreal/vitrum/run");
}

/// A missing `%APPDATA%` must be reported, naming that variable.
#[test]
fn windows_without_appdata_reports_appdata() {
    let env = PathEnv::from_pairs([("LOCALAPPDATA", "C:/Users/ada/AppData/Local")]);
    let err = AppPaths::resolve(Platform::Windows, &env).expect_err("APPDATA is required");
    assert_eq!(err, PathError::MissingEnv { platform: Platform::Windows, var: "APPDATA" });
}

/// A missing `%LOCALAPPDATA%` must be reported separately.
///
/// Deriving it from `%APPDATA%` by string surgery is a guess that breaks on a
/// redirected folder, and reporting `APPDATA` for a missing `LOCALAPPDATA`
/// sends whoever debugs it to the wrong variable.
#[test]
fn windows_without_localappdata_reports_localappdata() {
    let env = PathEnv::from_pairs([("APPDATA", "C:/Users/ada/AppData/Roaming")]);
    let err = AppPaths::resolve(Platform::Windows, &env).expect_err("LOCALAPPDATA is required");
    assert_eq!(err, PathError::MissingEnv { platform: Platform::Windows, var: "LOCALAPPDATA" });
}

/// Derived file paths must sit in the directories their purpose implies.
///
/// Window geometry is state, not config: putting it in the config directory
/// means a user who syncs their settings between two machines with different
/// monitors syncs the window position too.
#[test]
fn derived_files_land_in_the_right_directories() {
    let p = AppPaths::resolve(Platform::Linux, &linux_env()).expect("HOME is set");
    assert_path(&p.window_state_file(), "/home/ada/.local/state/vitrum/windows.json");
    assert_path(&p.instance_lock_file(), "/home/ada/.cache/vitrum/run/instance.lock");
    assert_path(&p.instance_socket_path(), "/home/ada/.cache/vitrum/run/instance.sock");
}

/// The compiled-in platform must match the target this binary was built for.
///
/// `Platform::current` feeds every live call. If it ever disagreed with the
/// target, a Linux build would silently resolve macOS paths.
#[test]
fn current_platform_matches_the_build_target() {
    let expected = if cfg!(target_os = "macos") {
        Platform::MacOs
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Linux
    };
    assert_eq!(Platform::current(), expected);
}

/// The convenience wrapper must be exactly the pure resolver over the captured
/// environment.
///
/// `for_current_platform` is what production calls and the pure resolver is
/// what every other test here calls. If the wrapper ever grew its own
/// fallback, an override or a `create_dir_all`, this whole file would stop
/// testing the code that actually runs.
#[test]
fn the_live_wrapper_is_the_pure_resolver_over_the_captured_environment() {
    let wrapper = AppPaths::for_current_platform();
    let explicit = AppPaths::resolve(Platform::current(), &PathEnv::from_process());
    assert_eq!(wrapper, explicit);
}

/// The captured environment must actually contain every variable resolution
/// consults, so a synthetic-environment test is a valid proxy for the real one.
///
/// Dropping `XDG_STATE_HOME` from the capture list would make the resolver
/// silently ignore a variable it documents as honoured, and every other test
/// in this file would still pass because they all build their own environment.
#[test]
fn the_capture_list_covers_every_variable_resolution_reads() {
    let live = format!("{:?}", PathEnv::from_process());
    let mut checked = 0;
    for key in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
        "XDG_RUNTIME_DIR",
        "TMPDIR",
        "APPDATA",
        "LOCALAPPDATA",
    ] {
        let Ok(value) = std::env::var(key) else { continue };
        if value.is_empty() {
            continue;
        }
        checked += 1;
        assert!(
            live.contains(key),
            "{key} is set in this process but the capture list omits it"
        );
    }
    // HOME is set in every environment this suite can run in, so a zero here
    // means the loop above tested nothing.
    assert!(checked > 0, "no directory variable was set, so nothing was verified");
}
