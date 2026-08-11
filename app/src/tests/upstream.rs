//! Every vendored fork's provenance must describe the fork that is actually
//! here, and every one of them must be declared.
//!
//! `tools/upstream/check.sh` is the real check, but it needs the network: it
//! downloads the pristine crate and diffs it. These run in the ordinary test
//! suite and cover the half that does not, which is the places the same fact is
//! written down and can disagree: a fork's `UPSTREAM.toml`, its `Cargo.toml`,
//! its `README.md`, and `NOTICE`.
//!
//! That matters because the network check trusts `UPSTREAM.toml` for the
//! version it downloads. A version recorded there that does not match the crate
//! version would have it diffing against the wrong release and reporting
//! nonsense, or reporting clean while the fork sat on something else entirely.
//!
//! Every guard below runs over every fork rather than over one of them. The
//! previous version of this file read the dioxus fork by name. That was the
//! whole set when it was written and stopped being it, and both of the forks it
//! stopped covering were carrying a defect of their own by the time anyone
//! looked: `vendor-ghostty-vt-sys/` holds the only escape-sequence parser in
//! the product and the pin that decides which CPUs a release runs on, and was
//! missing from `NOTICE`, which is undeclared redistribution rather than
//! untidiness; `vendor-pty/` had no `UPSTREAM.toml` at all, so nothing diffed
//! it, nothing could tell anyone upstream had published, and the only record
//! of what it changed was a paragraph no machine reads.
//!
//! So the set is derived: a workspace member with an `UPSTREAM.toml` is a fork.
//! Adding one extends every guard here by itself, and adding one without
//! declaring it turns the suite red on the commit that adds it.

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

/// Every workspace member directory, from the root manifest.
fn workspace_members() -> Vec<String> {
    let manifest = read("Cargo.toml");
    let (_, after) = manifest
        .split_once("members = [")
        .expect("the workspace manifest lists its members");
    let (list, _) = after.split_once(']').expect("the members list is closed");
    list.lines()
        .map(|line| line.trim().trim_matches(',').trim_matches('"'))
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// A vendored fork: a workspace member that records where it came from.
struct Fork {
    /// The member directory, repository-relative, such as `vendor-pty`.
    dir: String,
    /// Its `UPSTREAM.toml`.
    manifest: String,
    /// Its `Cargo.toml`.
    cargo: String,
    /// Its `README.md`.
    readme: String,
}

/// Every fork in this repository, in workspace order.
///
/// The definition is `UPSTREAM.toml`, not a name, a path prefix or a list here.
/// A directory that records an upstream is a copy of somebody else's code and
/// is held to everything below; a member that records none is ours.
fn forks() -> Vec<Fork> {
    let forks: Vec<Fork> = workspace_members()
        .into_iter()
        .filter(|dir| repo().join(dir).join("UPSTREAM.toml").is_file())
        .map(|dir| Fork {
            manifest: read(&format!("{dir}/UPSTREAM.toml")),
            cargo: read(&format!("{dir}/Cargo.toml")),
            readme: read(&format!("{dir}/README.md")),
            dir,
        })
        .collect();
    assert!(
        forks.len() >= 2,
        "only {} vendored fork(s) were found, and this repository has carried \
         at least two for as long as the guards below have existed. Either the \
         members list stopped parsing or a fork lost its UPSTREAM.toml, and \
         either way every check here is now looking at almost nothing.",
        forks.len()
    );
    forks
}

/// The paths `tools/upstream/check.sh` compares for a fork.
///
/// A divergence outside them is never diffed, so declaring one there is a
/// change nobody is checking, recorded in the file whose whole job is to be
/// checkable.
fn compared_roots(manifest: &str) -> Vec<String> {
    field(manifest, "compare")
        .expect("UPSTREAM.toml says which paths are compared")
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// The recorded upstream version is the version the crate claims to be.
///
/// WHY: a fork is a copy of one release, and its own version deliberately
/// mirrors that release so a reader can tell at a glance what it is a copy of.
/// Two places holding the same number is two places to bump, and the network
/// check downloads whichever one `UPSTREAM.toml` says. A mismatch has it
/// diffing against a release nobody is running.
#[test]
fn the_recorded_upstream_version_matches_the_crate() {
    for fork in forks() {
        let dir = &fork.dir;
        let recorded =
            field(&fork.manifest, "version").unwrap_or_else(|| panic!("{dir}/UPSTREAM.toml declares a version"));
        let crate_version =
            field(&fork.cargo, "version").unwrap_or_else(|| panic!("{dir}/Cargo.toml declares a version"));

        assert_eq!(
            recorded, crate_version,
            "{dir}/UPSTREAM.toml tracks {recorded} but the crate is {crate_version}; \
             bump both or the upstream check diffs against the wrong release"
        );
        let upstream_crate = field(&fork.manifest, "crate")
            .unwrap_or_else(|| panic!("{dir}/UPSTREAM.toml names the crate it forked"));
        assert!(
            !upstream_crate.is_empty(),
            "{dir}/UPSTREAM.toml names no upstream crate, and the check downloads \
             whatever this names"
        );
    }
}

/// Every declared divergence names a file that is really there, inside the
/// paths the check compares.
///
/// WHY: a typo in a path makes the network check compare a file that does not
/// exist. It would then report that path as a dead divergence and the real file
/// as an undeclared one, which reads as two problems and is actually a typo. A
/// path outside `compare` is worse: the file exists, the entry looks right, and
/// nothing ever diffs it.
#[test]
fn every_declared_divergence_is_a_real_file() {
    for fork in forks() {
        let dir = &fork.dir;
        let files = declared_files(&fork.manifest);
        assert!(!files.is_empty(), "{dir} declares no divergence, so it is not a fork");

        let roots = compared_roots(&fork.manifest);
        for file in &files {
            assert!(
                roots.iter().any(|root| file == root || file.starts_with(&format!("{root}/"))),
                "{dir}/UPSTREAM.toml declares {file}, which is outside the compared \
                 paths {roots:?}, so that divergence is never diffed against upstream"
            );
            let path = repo().join(dir).join(file);
            assert!(
                path.is_file(),
                "{dir}/UPSTREAM.toml declares {file}, which does not exist"
            );
        }
    }
}

/// Each divergence carries a reason, and no two entries name the same file.
///
/// WHY: an entry with no reason is a change nobody can evaluate at absorption
/// time, which is the one moment the reason is needed. A duplicated file is
/// two reasons for one change, and only one of them is being read.
#[test]
fn every_divergence_is_justified_once() {
    for fork in forks() {
        let dir = &fork.dir;
        let files = declared_files(&fork.manifest);

        let mut unique = files.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            files.len(),
            "{dir}/UPSTREAM.toml declares the same file twice, so one of its two \
             reasons is being read and the other is not"
        );

        // Count keys, not the word: a `reason` mentioned in a comment is prose,
        // and counting it would make this pass on a manifest that is missing one.
        let key = |name: &str| {
            fork.manifest
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
            "{dir}: {} divergences and {} reasons; every change needs one",
            files.len(),
            key("reason")
        );

        // An entry has to say whether it belongs upstream. That is the field that
        // decides, at absorption time, whether to keep carrying it or to send it.
        assert_eq!(
            key("upstreamable"),
            files.len(),
            "{dir}: every divergence declares whether it should be sent upstream"
        );
    }
}

/// The fork's README names every file that diverges.
///
/// WHY: `UPSTREAM.toml` is the machine's copy and the README is the human's,
/// and the human's is the one read first. One README already claimed exactly
/// one divergence while there were three; nobody noticed because nothing
/// checked. This is that check.
#[test]
fn the_fork_readme_names_every_divergence() {
    for fork in forks() {
        let dir = &fork.dir;
        for file in declared_files(&fork.manifest) {
            assert!(
                fork.readme.contains(&file),
                "{dir}/README.md never mentions {file}, which diverges from upstream"
            );
        }
    }
}

/// The absorption procedure points at a check that exists and is runnable, and
/// something asks on a schedule.
///
/// WHY: the whole mechanism is one script plus one scheduled job. A README step
/// naming a path that moved is a procedure that stops at step one, and the
/// failure surfaces only when someone finally tries to absorb a release. A fork
/// the scheduled job does not name is a fork nothing asks about at all: that
/// was true of `vendor-ghostty-vt-sys` while the weekly workflow ran the check
/// for that one fork alone, and it is the fork whose upstream fix would let this
/// repository drop the fork entirely.
#[test]
fn the_absorption_procedure_is_runnable() {
    let script = "tools/upstream/check.sh";
    assert!(repo().join(script).is_file(), "{script} does not exist");

    let workflow = read(".github/workflows/upstream.yml");
    assert!(
        workflow.contains("schedule:"),
        "the upstream workflow has no schedule, so it only ever runs when a fork changes"
    );

    for fork in forks() {
        let dir = &fork.dir;
        assert!(
            fork.readme.contains(script),
            "{dir}/README.md does not tell anyone to run {script}"
        );
        assert!(
            workflow.contains(script),
            "the scheduled workflow does not run {script}, so nothing asks weekly"
        );
        // The first fork is the script's default and is checked by a bare
        // invocation; every later one has to be asked for by name, and the
        // name is the directory.
        let asked_for = workflow.contains(&format!("--fork {dir}"))
            || workflow
                .lines()
                .any(|line| line.contains(script) && !line.contains("--fork"));
        assert!(
            asked_for,
            "the scheduled workflow never checks {dir}, so an upstream release \
             of it goes unnoticed for as long as nobody edits the fork"
        );
    }
}

/// Every bundled fork is declared in `NOTICE`, and the count in the prose is
/// the count in the list.
///
/// WHY: `NOTICE` is the legal declaration that this repository carries other
/// people's code under other people's copyright. A fork missing from it is not
/// untidy, it is undeclared redistribution of a third party's work, and the
/// only reader who finds out is the one auditing a release.
///
/// `NOTICE` said "Two forks" while three shipped. Both halves are checked
/// because both go stale on their own: an entry can be added under a sentence
/// that still says two, and the sentence can be corrected without the entry.
#[test]
fn every_bundled_fork_is_declared_in_the_notice() {
    let notice = read("NOTICE");
    let forks = forks();

    for fork in &forks {
        let dir = &fork.dir;
        assert!(
            notice.contains(&format!("{dir}/")),
            "NOTICE never names {dir}/, which bundles somebody else's crate under \
             their copyright. An undeclared fork is undeclared redistribution."
        );
        let upstream_crate = field(&fork.manifest, "crate")
            .unwrap_or_else(|| panic!("{dir}/UPSTREAM.toml names the crate it forked"));
        assert!(
            notice.contains(&upstream_crate),
            "NOTICE names {dir}/ without naming `{upstream_crate}`, the crate it is \
             a copy of, so a reader cannot find the work it is declaring"
        );
        let license = field(&fork.cargo, "license")
            .unwrap_or_else(|| panic!("{dir}/Cargo.toml declares a license"));
        assert!(
            notice.contains(&license),
            "NOTICE declares {dir}/ without naming its {license} license"
        );
    }

    // The number in the sentence above the list. Spelled, because that is how
    // the sentence is written, and up to a count nobody is going to exceed.
    let spelled = ["no", "one", "two", "three", "four", "five", "six"];
    let claimed = spelled
        .iter()
        .position(|word| {
            notice
                .to_lowercase()
                .contains(&format!("{word} forks of other people's crates"))
        })
        .unwrap_or_else(|| {
            panic!(
                "NOTICE no longer says how many forks of other people's crates ship \
                 here, so the one sentence a reader counts against is gone"
            )
        });
    assert_eq!(
        claimed,
        forks.len(),
        "NOTICE says {} forks ship here and {} do: {:?}",
        spelled[claimed],
        forks.len(),
        forks.iter().map(|f| f.dir.as_str()).collect::<Vec<_>>()
    );
}
