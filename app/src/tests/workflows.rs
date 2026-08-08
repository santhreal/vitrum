//! The pipeline every pull request has to pass must be able to pass.
//!
//! These check three things a workflow file can get wrong in a way that looks
//! like a broken change rather than a broken pipeline: a Linux job that does
//! not install the system webview fails to link and blames the diff, a job
//! with no timeout holds a runner for six hours, and a workflow with no
//! concurrency group starts a fresh matrix for every push to a branch while
//! the superseded ones keep the runners the new one is queued behind. A queued
//! job that waits too long is cancelled with no steps and no reason.
//!
//! Written against the workflow text because there is nowhere else the facts
//! live. The alternative is discovering each of them once per rewrite, on a
//! contributor's pull request.

/// Every workflow that builds this workspace on Linux installs the webview.
///
/// Named through the one script rather than four copies of an apt line, so
/// adding a package is one edit. A job that misses it fails at link time with
/// an error about `webkit2gtk` that says nothing about the cause.
#[test]
fn every_linux_build_job_installs_the_system_webview() {
    for (name, text) in workflows() {
        let builds = text.contains("cargo build")
            || text.contains("cargo test")
            || text.contains("build-release-asset.sh")
            || text.contains("cargo publish");
        if !builds || !runs_on_linux(&text) {
            continue;
        }
        assert!(
            text.contains("./.github/system-webview.sh"),
            "{name} builds this workspace but never installs the system webview, \
             so its Linux job cannot link the client"
        );
        assert!(
            !text.contains("libwebkit2gtk-4.1-dev"),
            "{name} names the webview packages itself; that list lives in \
             .github/system-webview.sh so it is changed in one place"
        );
    }
}

/// Whether a workflow runs anything on Linux.
///
/// A workflow that builds only on macOS and Windows has nothing to link
/// WebKitGTK into, and requiring the install script there would be requiring an
/// apt call on a mac. `vitrum` is the label of the self-hosted Linux runner, so
/// it names a Linux job exactly as `ubuntu-latest` does; miss it and the guard
/// above stops covering the workflow every pull request actually runs.
fn runs_on_linux(text: &str) -> bool {
    text.contains("ubuntu-latest")
        || text.contains("'vitrum'")
        || text.contains("runner.os == 'Linux'")
}

/// The three ways a workflow says Linux, and the one way it says it does not.
#[test]
fn a_workflow_is_linux_when_it_names_a_linux_runner() {
    assert!(runs_on_linux("runs-on: ubuntu-latest"));
    // No `ubuntu-latest` in this one, or it would pass on the first clause and
    // prove nothing about the label.
    assert!(runs_on_linux("runs-on: ${{ inputs.fast && 'vitrum' || 'macos-latest' }}"));
    assert!(runs_on_linux("if: runner.os == 'Linux'"));
    assert!(!runs_on_linux("os: [macos-latest, windows-latest]"));
}

/// Every job carries a timeout.
///
/// The default is six hours. A job that hangs, and one has, holds a runner
/// that everything queued behind it is waiting for, and the fleet a fork's
/// pull requests draw from is small.
#[test]
fn every_job_has_a_timeout() {
    for (name, text) in workflows() {
        for job in jobs(&text) {
            assert!(
                job.body.contains("timeout-minutes:"),
                "job `{}` in {name} has no timeout, so a hang holds its runner \
                 for the six-hour default",
                job.name
            );
        }
    }
}

/// A workflow that runs on pull requests supersedes its own older runs.
///
/// Without this, pushing three times to a branch runs three full matrices at
/// once and the third waits for runners the first two are holding for a
/// verdict nobody will read.
#[test]
fn a_pull_request_workflow_cancels_the_run_it_replaces() {
    for (name, text) in workflows() {
        if !text.contains("pull_request:") {
            continue;
        }
        assert!(
            text.contains("concurrency:") && text.contains("cancel-in-progress: true"),
            "{name} runs on pull requests without a concurrency group, so every \
             push leaves a full matrix running against a commit that is gone"
        );
    }
}

/// A top-level job block: its name and everything indented under it.
struct Job {
    name: String,
    body: String,
}

/// Split a workflow's `jobs:` mapping into its top-level jobs.
///
/// Two-space indentation, which is what every file here uses; a job key is the
/// only thing at that depth once `jobs:` has been entered.
fn jobs(text: &str) -> Vec<Job> {
    let mut out: Vec<Job> = Vec::new();
    let mut in_jobs = false;
    for line in text.lines() {
        if line.starts_with("jobs:") {
            in_jobs = true;
            continue;
        }
        if !in_jobs {
            continue;
        }
        // Back at column zero: the `jobs:` mapping has ended.
        if !line.starts_with(' ') && !line.trim().is_empty() {
            break;
        }
        let is_job_key = line.starts_with("  ")
            && !line.starts_with("   ")
            && line.trim_end().ends_with(':')
            && !line.trim_start().starts_with('#');
        if is_job_key {
            out.push(Job {
                name: line.trim().trim_end_matches(':').to_string(),
                body: String::new(),
            });
        } else if let Some(job) = out.last_mut() {
            job.body.push_str(line);
            job.body.push('\n');
        }
    }
    assert!(!out.is_empty(), "a workflow with no jobs was parsed");
    out
}

/// Every workflow file, by name.
fn workflows() -> Vec<(String, String)> {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the app crate has a parent directory")
        .join(".github/workflows");
    let mut out: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|why| panic!("cannot list {}: {why}", dir.display()))
        .map(|e| e.expect("a readable directory entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "yml" || e == "yaml"))
        .map(|p| {
            let text = std::fs::read_to_string(&p)
                .unwrap_or_else(|why| panic!("cannot read {}: {why}", p.display()));
            (
                p.file_name()
                    .expect("a file with an extension has a name")
                    .to_string_lossy()
                    .into_owned(),
                text,
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!out.is_empty(), "no workflows were found to check");
    out
}

/// Every Zig pin agrees with every other one, and with the contributor guide.
///
/// The version is load-bearing rather than cosmetic: it is the one Ghostty
/// pins, and a newer Zig fails Ghostty's own build-version check, so a
/// contributor who installs the latest release gets a different failure rather
/// than a safer one. The number therefore lives in six workflow steps and in
/// `CONTRIBUTING.md`, and nothing made them agree.
///
/// Drift here is silent in the direction that matters. Bumping CI leaves the
/// guide telling every new contributor to install a version CI no longer uses,
/// and CI going green is exactly what hides it.
#[test]
fn the_zig_version_is_pinned_consistently_and_documented() {
    let mut pins: Vec<(String, String)> = Vec::new();
    for (name, text) in workflows() {
        let lines: Vec<&str> = text.lines().collect();
        for (at, line) in lines.iter().enumerate() {
            // A `uses:` step, not the prose about caching that names the
            // action too: a comment pins nothing and would fail below.
            let trimmed = line.trim();
            let step = trimmed.strip_prefix("- ").unwrap_or(trimmed);
            if !step.starts_with("uses:") || !step.contains("setup-zig") {
                continue;
            }
            // `with:` then `version:` follow the `uses:`, a couple of lines down.
            let version = lines[at + 1..lines.len().min(at + 6)]
                .iter()
                .find_map(|next| next.trim().strip_prefix("version:"))
                .unwrap_or_else(|| panic!("{name}: a setup-zig step pins no version"));
            pins.push((name.clone(), version.trim().to_string()));
        }
    }

    assert!(
        pins.len() >= 5,
        "only {} Zig pins were found; the parser is not reading the steps",
        pins.len()
    );

    let (first_file, pinned) = pins[0].clone();
    for (name, version) in &pins {
        assert_eq!(
            version, &pinned,
            "{name} pins Zig {version} and {first_file} pins {pinned}; one \
             engine build would use a Zig the other rejects"
        );
    }

    let guide = include_str!("../../../CONTRIBUTING.md");
    assert!(
        guide.contains(&pinned),
        "CI pins Zig {pinned} and CONTRIBUTING.md does not name it, so the \
         guide is telling contributors to install something else"
    );
}
