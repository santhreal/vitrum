//! Decides how libghostty is obtained on this machine, and refuses to guess.
//!
//! There is exactly one routing table ([`ROUTES`]) and one decision function
//! ([`route`]). Everything downstream, including the runtime record in
//! `src/linkage.rs`, reads the result rather than repeating the reasoning.
//!
//! # Why this file exists at all
//!
//! `libghostty-vt-sys` already knows how to link a system library and how to
//! build a vendored one, but when the system library is missing it quietly
//! builds the vendored one instead. That fallback is the failure we care about:
//! a machine that was asked to link the platform's Ghostty would instead clone
//! Ghostty and compile it, and the only symptom is a build that took ten
//! minutes and produced a binary tracking a different engine than intended.
//!
//! So the route is decided here, before that crate runs, and a route that
//! cannot be satisfied is a build error naming the exact missing piece and the
//! command that installs it.

use std::fmt;

/// How the engine is linked.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Route {
    /// Build Ghostty from the pinned source with Zig.
    Vendored,
    /// Link a libghostty the platform already provides.
    System,
}

impl Route {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Vendored => "vendored",
            Self::System => "system",
        }
    }
}

impl fmt::Display for Route {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a given operating system can do, and what to tell someone whose
/// machine cannot do it yet.
///
/// Keeping this a table rather than a chain of `cfg!` blocks is deliberate: a
/// new platform is a new row, and the decision logic below never changes.
struct PlatformRule {
    /// Value of `CARGO_CFG_TARGET_OS`.
    os: &'static str,
    /// Whether `system` can work here at all.
    system_supported: bool,
    /// How to install the system library on this platform.
    system_hint: &'static str,
    /// How to install a Zig toolchain on this platform.
    zig_hint: &'static str,
}

/// The whole per-platform policy, in one place.
const ROUTES: &[PlatformRule] = &[
    PlatformRule {
        os: "linux",
        system_supported: true,
        system_hint: "install your distribution's ghostty (or libghostty) development package, \
                      so that `pkg-config --exists libghostty-vt` succeeds",
        zig_hint: "install Zig 0.15.2 from https://ziglang.org/download/ and put it on PATH",
    },
    PlatformRule {
        os: "macos",
        system_supported: true,
        system_hint: "brew install ghostty, then confirm `pkg-config --exists libghostty-vt`",
        zig_hint: "brew install zig (0.15.2), or download it from https://ziglang.org/download/",
    },
    PlatformRule {
        os: "windows",
        // Windows has no pkg-config convention, and the sys crate reaches an
        // installed library only through pkg-config. Saying so is better than
        // offering a switch that silently builds from source anyway.
        system_supported: false,
        system_hint: "not available on Windows: build with the default `vendored` feature",
        zig_hint: "winget install zig.zig --version 0.15.2, or download it from \
                   https://ziglang.org/download/",
    },
];

fn main() {
    println!("cargo::rerun-if-env-changed=VITRUM_VT_LINKAGE");
    println!("cargo::rerun-if-env-changed=GHOSTTY_SOURCE_DIR");
    println!("cargo::rerun-if-changed=build.rs");

    // docs.rs builds documentation with no Zig and no system library. The sys
    // crate ships checked-in bindings for exactly this case, so the decision
    // below would reject a build that actually works.
    if std::env::var_os("DOCS_RS").is_some() {
        emit("vendored", "docs.rs");
        return;
    }

    let os = std::env::var("CARGO_CFG_TARGET_OS").expect("cargo sets CARGO_CFG_TARGET_OS");
    let rule = ROUTES.iter().find(|r| r.os == os);

    let chosen = route(&os, rule.is_some_and(|r| r.system_supported));

    match chosen {
        Route::System => {
            let hint = rule.map_or("install libghostty", |r| r.system_hint);
            require_system_library(hint);
            emit(chosen.as_str(), "pkg-config");
        }
        Route::Vendored => {
            let hint = rule.map_or("install Zig 0.15.2", |r| r.zig_hint);
            let source = require_zig_toolchain(hint);
            emit(chosen.as_str(), source);
        }
    }
}

/// Pick the route from the explicit override, then the features.
///
/// The override exists because a machine that has both a Zig toolchain and a
/// system library still needs a way to say which one this build used, without
/// editing a manifest.
fn route(os: &str, system_supported: bool) -> Route {
    let requested = std::env::var("VITRUM_VT_LINKAGE").ok();
    let from_env = match requested.as_deref() {
        None | Some("") => None,
        Some("vendored") => Some(Route::Vendored),
        Some("system") => Some(Route::System),
        Some(other) => panic!(
            "VITRUM_VT_LINKAGE={other:?} is not a linkage route. \
             Use \"vendored\" (build Ghostty with Zig) or \"system\" (link an installed libghostty)."
        ),
    };

    let from_features = if cfg!(feature = "system") {
        Route::System
    } else {
        Route::Vendored
    };

    let chosen = from_env.unwrap_or(from_features);

    assert!(
        !(chosen == Route::System && !system_supported),
        "the `system` linkage route is not available on {os}. \
         Build with the default `vendored` feature instead."
    );

    chosen
}

/// Fail unless a system libghostty is actually present.
///
/// This runs before `libghostty-vt-sys` gets a chance to fall back to a
/// vendored build, which is the entire point of the check.
fn require_system_library(hint: &str) {
    let found = pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("libghostty-vt");

    if let Err(error) = found {
        panic!(
            "the `system` linkage route needs an installed libghostty-vt, and pkg-config \
             could not find one: {error}\n\
             {hint}\n\
             Or drop the `system` feature to build the pinned engine from source."
        );
    }
}

/// Fail unless the vendored build can actually run, and report which input it
/// will use.
///
/// A local Ghostty checkout still needs Zig to build, so the toolchain check
/// applies to both cases; only the source differs.
fn require_zig_toolchain(hint: &str) -> &'static str {
    let zig = std::process::Command::new("zig").arg("version").output();

    match zig {
        Ok(out) if out.status.success() => {}
        _ => panic!(
            "the `vendored` linkage route builds Ghostty from source and needs a Zig \
             toolchain on PATH, which is not there.\n\
             {hint}\n\
             Or enable the `system` feature to link a libghostty the platform provides."
        ),
    }

    if std::env::var_os("GHOSTTY_SOURCE_DIR").is_some() {
        "zig, GHOSTTY_SOURCE_DIR"
    } else {
        "zig, pinned upstream"
    }
}

/// Publish the decision to the crate, so it can be read at runtime.
fn emit(route: &str, source: &str) {
    println!("cargo::rustc-env=VITRUM_VT_LINKAGE_ROUTE={route}");
    println!("cargo::rustc-env=VITRUM_VT_LINKAGE_SOURCE={source}");
    println!("cargo::rustc-env=VITRUM_VT_ENGINE_VERSION={}", engine_version());
}

/// The engine version this build actually resolved to.
///
/// Read from the lockfile rather than the manifest, because the manifest holds
/// a requirement and a bug report needs the version that was built. When there
/// is no lockfile to read (documentation builds, a packaged crate compiled
/// standalone) the requirement is reported and labelled as such, so the string
/// is never a claim the build cannot support.
fn engine_version() -> String {
    let manifest = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    );

    for dir in manifest.ancestors() {
        let lock = dir.join("Cargo.lock");
        let Ok(text) = std::fs::read_to_string(&lock) else {
            continue;
        };
        println!("cargo::rerun-if-changed={}", lock.display());
        if let Some(version) = locked_version(&text, "libghostty-vt") {
            return version;
        }
    }

    // No lockfile: report the requirement from our own manifest, labelled, so
    // the string still says something true about what was built.
    let own = manifest.join("Cargo.toml");
    let requirement = std::fs::read_to_string(own)
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.trim_start().starts_with("libghostty-vt ="))
                .and_then(|line| line.split_once("version = \""))
                .and_then(|(_, rest)| rest.split('"').next().map(str::to_owned))
        })
        .unwrap_or_else(|| "unknown".to_owned());

    format!("{requirement} (unresolved)")
}

/// Version recorded for `name` in a `Cargo.lock`.
///
/// The format is stable and trivially shaped: a `[[package]]` header, then
/// `name` and `version` keys in any order before the next header. Parsing that
/// much needs no TOML crate in the build graph.
fn locked_version(lock: &str, name: &str) -> Option<String> {
    let mut in_package = false;
    let mut matched = false;
    let mut version: Option<String> = None;

    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            if matched && version.is_some() {
                return version;
            }
            in_package = true;
            matched = false;
            version = None;
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(value) = toml_string(line, "name") {
            matched = value == name;
        } else if let Some(value) = toml_string(line, "version") {
            version = Some(value);
        }
    }

    if matched { version } else { None }
}

/// The value of a `key = "value"` line, if it is that key.
fn toml_string(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix(key)?.trim_start().strip_prefix('=')?;
    Some(rest.trim().trim_matches('"').to_owned())
}
