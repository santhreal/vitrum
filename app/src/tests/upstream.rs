//! The vendored fork's provenance must describe the fork that is actually here.
//!
//! `tools/upstream/check.sh` is the real check, but it needs the network: it
//! downloads the pristine crate and diffs it. These run in the ordinary test
//! suite and cover the half that does not, which is the three places the same
//! fact is written down and can disagree: `vendor/UPSTREAM.toml`,
//! `vendor/Cargo.toml` and `vendor/README.md`.
//!
//! That matters because the network check trusts `UPSTREAM.toml` for the
//! version it downloads. A version recorded there that does not match the crate
//! version would have it diffing against the wrong release and reporting
//! nonsense, or reporting clean while the fork sat on something else entirely.

use std::path::PathBuf;

/// The repository root, from this crate's manifest directory.
fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the app crate has a parent directory")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = repo().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|why| panic!("cannot read {}: {why}", path.display()))
}

/// The first `key = "value"` in a TOML-ish file, by key.
fn field(text: &str, key: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(key)?.trim().strip_prefix('=').map(str::trim))
        .map(|v| v.trim_matches('"').to_string())
}

/// Every `file = "..."` entry, in order.
fn declared_files(manifest: &str) -> Vec<String> {
    manifest
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("file")?.trim().strip_prefix('=').map(str::trim))
        .map(|v| v.trim_matches('"').to_string())
        .collect()
}

/// The recorded upstream version is the version the crate claims to be.
///
/// WHY: the fork is a copy of `dioxus-desktop` at one release, and its own
/// version deliberately mirrors that release so a reader can tell at a glance
/// what it is a copy of. Two places holding the same number is two places to
/// bump, and the network check downloads whichever one `UPSTREAM.toml` says. A
/// mismatch has it diffing against a release nobody is running.
#[test]
fn the_recorded_upstream_version_matches_the_crate() {
    let upstream = read("vendor/UPSTREAM.toml");
    let cargo = read("vendor/Cargo.toml");

    let recorded = field(&upstream, "version").expect("UPSTREAM.toml declares a version");
    let crate_version = field(&cargo, "version").expect("vendor/Cargo.toml declares a version");

    assert_eq!(
        recorded, crate_version,
        "vendor/UPSTREAM.toml tracks {recorded} but the crate is {crate_version}; \
         bump both or the upstream check diffs against the wrong release"
    );
    assert_eq!(
        field(&upstream, "crate").as_deref(),
        Some("dioxus-desktop"),
        "the fork is of dioxus-desktop and the check downloads whatever this names"
    );
}

/// Every declared divergence names a file that is really there.
///
/// WHY: a typo in a path makes the network check compare a file that does not
/// exist. It would then report that path as a dead divergence and the real file
/// as an undeclared one, which reads as two problems and is actually a typo.
#[test]
fn every_declared_divergence_is_a_real_file() {
    let manifest = read("vendor/UPSTREAM.toml");
    let files = declared_files(&manifest);
    assert!(!files.is_empty(), "a fork with no declared divergence is not a fork");

    for file in &files {
        assert!(
            file.starts_with("src/"),
            "only src/ is compared, so {file} would never be checked"
        );
        let path = repo().join("vendor").join(file);
        assert!(path.is_file(), "vendor/UPSTREAM.toml declares {file}, which does not exist");
    }
}

/// Each divergence carries a reason, and no two entries name the same file.
///
/// WHY: an entry with no reason is a change nobody can evaluate at absorption
/// time, which is the one moment the reason is needed. A duplicated file is
/// two reasons for one change, and only one of them is being read.
#[test]
fn every_divergence_is_justified_once() {
    let manifest = read("vendor/UPSTREAM.toml");
    let files = declared_files(&manifest);

    // Count keys, not the word: a `reason` mentioned in a comment is prose,
    // and counting it would make this pass on a manifest that is missing one.
    let key = |name: &str| {
        manifest
            .lines()
            .map(str::trim)
            .filter(|line| {
                line.strip_prefix(name)
                    .is_some_and(|rest| rest.trim_start().starts_with('='))
            })
            .count()
    };

    assert_eq!(
        key("reason"),
        files.len(),
        "{} divergences and {} reasons; every change needs one",
        files.len(),
        key("reason")
    );

    // An entry has to say whether it belongs upstream. That is the field that
    // decides, at absorption time, whether to keep carrying it or to send it.
    assert_eq!(
        key("upstreamable"),
        files.len(),
        "every divergence declares whether it should be sent upstream"
    );
}

/// The fork's README names every file that diverges.
///
/// WHY: `UPSTREAM.toml` is the machine's copy and the README is the human's,
/// and the human's is the one read first. The README already claimed exactly
/// one divergence while there were three; nobody noticed because nothing
/// checked. This is that check.
#[test]
fn the_fork_readme_names_every_divergence() {
    let manifest = read("vendor/UPSTREAM.toml");
    let readme = read("vendor/README.md");

    for file in declared_files(&manifest) {
        assert!(
            readme.contains(&file),
            "vendor/README.md never mentions {file}, which diverges from upstream"
        );
    }
}

/// The absorption procedure points at a check that exists and is runnable.
///
/// WHY: the whole mechanism is one script plus one scheduled job. A README
/// step naming a path that moved is a procedure that stops at step one, and
/// the failure surfaces only when someone finally tries to absorb a release.
#[test]
fn the_absorption_procedure_is_runnable() {
    let readme = read("vendor/README.md");
    let script = "tools/upstream/check.sh";
    assert!(readme.contains(script), "vendor/README.md does not tell anyone to run {script}");

    let path = repo().join(script);
    assert!(path.is_file(), "{script} does not exist");

    let workflow = read(".github/workflows/upstream.yml");
    assert!(
        workflow.contains(script),
        "the scheduled workflow does not run {script}, so nothing asks weekly"
    );
    assert!(
        workflow.contains("schedule:"),
        "the upstream workflow has no schedule, so it only ever runs when vendor/ changes"
    );
}
