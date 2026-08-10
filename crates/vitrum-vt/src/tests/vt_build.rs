//! How the vendored engine is compiled.
//!
//! WHY: `libghostty-vt-sys` chooses its zig optimize mode from cargo's `DEBUG`
//! flag, so every dev-profile build of this workspace would otherwise link a
//! Debug-mode engine. That engine reads uninitialised stack memory — valgrind
//! reports it on a four-column scroll through `vitrum-replay` — and the
//! dev-profile test binary faults with `STATUS_ACCESS_VIOLATION` on
//! windows-latest, while the release profile has never shown either. The
//! workspace pins `LIBGHOSTTY_VT_SYS_OPTIMIZE` so no profile can select that
//! build again.
//!
//! What this does not catch: whether the engine honoured the request. That
//! lives in the sys crate's build script, and a wrong value there would show
//! up as the uninitialised read returning, not as a wrong config file.

use std::path::Path;

/// The value cargo exports for `key`, or `None` when nothing under `[env]`
/// sets it.
///
/// Commented lines are dropped and table headers tracked, so a key that has
/// been commented out, or moved out from under `[env]`, reads as absent rather
/// than as set — which is how cargo reads it.
fn env_entry(text: &str, key: &str) -> Option<String> {
    let mut table = "";
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            table = header.trim();
            continue;
        }
        if let Some((name, value)) = line.split_once('=')
            && table == "env"
            && name.trim() == key
        {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// The workspace config, read from the source tree rather than the process
/// environment: reading the environment would pass whenever the caller
/// happened to export the variable, which is the case the pin exists to make
/// unnecessary.
fn workspace_config() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<name> sits two levels below the workspace root")
        .join(".cargo/config.toml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn the_vendored_engine_is_built_optimized_in_every_profile() {
    let mode = env_entry(&workspace_config(), "LIBGHOSTTY_VT_SYS_OPTIMIZE")
        .expect("the workspace pins the engine's optimize mode under [env]");
    assert!(
        mode == "ReleaseFast" || mode == "ReleaseSafe",
        "the engine must not be built in Debug mode, got {mode:?}"
    );
}

#[test]
fn only_a_live_entry_under_env_counts_as_a_pin() {
    // The reader above has to tell a live entry from a commented one and from
    // a key under another table, because cargo does. Without that, a pin that
    // had been commented out would still read as set and the guard would pass
    // over a Debug engine.
    let key = "LIBGHOSTTY_VT_SYS_OPTIMIZE";
    assert_eq!(env_entry("[env]\nLIBGHOSTTY_VT_SYS_OPTIMIZE = \"ReleaseFast\"\n", key).as_deref(), Some("ReleaseFast"));
    assert_eq!(env_entry("# [env]\n# LIBGHOSTTY_VT_SYS_OPTIMIZE = \"ReleaseFast\"\n", key), None);
    assert_eq!(env_entry("[build]\nLIBGHOSTTY_VT_SYS_OPTIMIZE = \"ReleaseFast\"\n", key), None);
    assert_eq!(env_entry("[env]\nOTHER = \"1\"\n", key), None);
}
