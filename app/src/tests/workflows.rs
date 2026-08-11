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

/// Every workflow says what its token may do, and only a release may write.
///
/// WHY: a workflow with no `permissions:` block inherits the repository
/// default, which is a token with write access to the repository handed to
/// every action, every crate build script and every dependency the job
/// downloads. Five of the seven workflows here only read, and three of those
/// run on pull requests from forks.
///
/// Every block is read, top level and per job, and compared against what is
/// recorded for that workflow, so a job cannot quietly widen what the file
/// says at the top. The decision is recorded per workflow rather than as a
/// rule about which ones "look like" a release, so adding a workflow turns
/// this red until somebody says what its token is for.
#[test]
fn every_workflow_declares_the_token_it_needs() {
    // What each workflow is allowed to ask for, anywhere in the file. The two
    // that publish: `cut.yml` pushes the release commit and tag and dispatches
    // the publish; `release.yml` uploads the assets, and its publish job also
    // signs them, which needs an OIDC token and the attestations store.
    let recorded: [(&str, &[&str]); 2] = [
        ("cut.yml", &["contents: write", "actions: write"]),
        (
            "release.yml",
            &[
                "contents: write",
                "id-token: write",
                "attestations: write",
            ],
        ),
    ];

    for (name, text) in workflows() {
        let blocks = permission_blocks(&text);
        assert!(
            !blocks.is_empty(),
            "{name} declares no permissions, so its token inherits the \
             repository default and every action it runs gets write access to \
             this repository"
        );
        assert!(
            text.lines().any(|line| line.starts_with("permissions:")),
            "{name} sets permissions per job but not at the top level, so any \
             job it grows starts from the repository default again"
        );

        let allowed: Vec<&str> = recorded
            .iter()
            .find(|(who, _)| *who == name)
            .map_or(vec!["contents: read"], |(_, scopes)| scopes.to_vec());
        for scope in blocks.iter().flatten() {
            assert!(
                allowed.contains(&scope.as_str()),
                "{name} asks for `{scope}`, which is not recorded for it: \
                 {allowed:?}"
            );
        }
    }
}

/// Every `permissions:` block in a workflow, top level and per job, as the
/// scope lines it grants.
fn permission_blocks(text: &str) -> Vec<Vec<String>> {
    let mut blocks = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim_start() != "permissions:" {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let mut scopes = Vec::new();
        while let Some(next) = lines.peek() {
            let body = next.split('#').next().unwrap_or("").trim();
            let deeper = next.len() - next.trim_start().len() > indent;
            if !body.is_empty() && !deeper {
                break;
            }
            if !body.is_empty() {
                scopes.push(body.to_string());
            }
            lines.next();
        }
        blocks.push(scopes);
    }
    blocks
}

/// A job's block is found as well as the file's, and a comment is not a scope.
#[test]
fn a_permissions_block_is_read_wherever_it_sits() {
    let text = concat!(
        "permissions:\n",
        "  contents: read\n",
        "\n",
        "jobs:\n",
        "  one:\n",
        "    # a comment, not a scope\n",
        "    permissions:\n",
        "      contents: write\n",
        "      id-token: write\n",
        "    steps: []\n",
    );
    assert_eq!(
        permission_blocks(text),
        vec![
            vec!["contents: read".to_string()],
            vec!["contents: write".to_string(), "id-token: write".to_string()],
        ]
    );
}

/// Every third-party action is pinned to a commit, not to a tag.
///
/// WHY: `uses: someone/thing@v2` resolves at run time to whatever `v2` points
/// at that morning. The owner can move it, and an account takeover moves it
/// for them; the action then runs in the publish job, which holds a token that
/// can write to this repository, sign artifacts and upload release assets. A
/// commit SHA is the one reference that cannot be repointed.
///
/// GitHub's own `actions/*` are taken on trust, as is any local `./` action in
/// this repository: those are the two publishers a compromise of this pipeline
/// already implies. Everything else names a commit, with the version it was
/// at in a comment beside it, because a bare SHA nobody can date is a pin
/// nobody will ever update.
#[test]
fn every_third_party_action_is_pinned_to_a_commit() {
    for (name, text) in workflows() {
        for line in text.lines() {
            let Some((_, uses)) = line.trim().split_once("uses:") else {
                continue;
            };
            let reference = uses.trim();
            if reference.starts_with("./") || reference.starts_with("actions/") {
                continue;
            }
            let (action, version) = reference
                .split_once('@')
                .unwrap_or_else(|| panic!("{name} uses `{reference}` with no version at all"));
            let sha = version.split('#').next().unwrap_or("").trim();
            assert!(
                sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
                "{name} pins {action} to `{sha}`, which is a tag the owner can \
                 move under us, not a commit"
            );
            assert!(
                version.contains('#'),
                "{name} pins {action} to a commit with no version beside it, so \
                 nobody can tell how old the pin is"
            );
        }
    }
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

/// Runner labels this repository has agreed on.
///
/// One file, read here and by the CI step that checks the same thing. The two
/// used to be hand-written copies of each other and had already drifted: this
/// one did not carry the self-hosted label four registered runners answer to.
fn allowed_runners() -> Vec<String> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the app crate has a parent directory")
        .join("tools/release/runners.txt");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} cannot be read: {e}", path.display()));
    let labels: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect();
    assert!(
        !labels.is_empty(),
        "{} names no label at all, so every workflow would fail this",
        path.display()
    );
    labels
}

/// A workflow GitHub cannot parse is a workflow that never runs.
///
/// This shipped. `ci.yml` carried `run: cargo test ... --locked assets::`,
/// and a plain scalar ending in a colon is a mapping key to YAML, so the file
/// was invalid from the moment it merged. GitHub does not report that as an
/// error anyone sees: it creates the run, gives it zero jobs, marks it failed
/// and writes no annotation, no check run and no log. On the commit list it is
/// a red dot with nothing behind it, and the pipeline is simply not running.
///
/// The label guard below cannot cover this and neither can any step in CI: a
/// file that does not parse has no job to run a check in. So it is checked
/// here, on the machine making the change, before the push.
#[test]
fn every_workflow_parses() {
    for (name, text) in workflows() {
        let documents = match saphyr::LoadableYamlNode::load_from_str(&text) {
            Ok(documents) => documents,
            Err(why) => panic!(
                "{name} is not valid YAML: {why}. GitHub accepts the push, \
                 creates a run with no jobs, and marks it failed with nothing \
                 to read, so the pipeline stops without reporting that it has."
            ),
        };
        let document: &saphyr::Yaml = documents
            .first()
            .unwrap_or_else(|| panic!("{name} parses to no document at all"));
        assert!(
            document
                .as_mapping()
                .is_some_and(|top| top.contains_key(&saphyr::Yaml::value_from_str("jobs"))),
            "{name} parses, and declares no jobs"
        );
    }
}

/// Every runner label a workflow can hand to GitHub.
///
/// Three shapes reach a runner: a bare `runs-on` scalar, a quoted operand of
/// `&&`/`||` inside a `runs-on` expression, which is how the fork ternary picks
/// one, and a matrix `os:` value in either the inline-list or the one-per-entry
/// form. A label read from a repository variable resolves at run creation and
/// is invisible from here; that one is taken on trust, and the fallback beside
/// it is not.
fn runner_labels(text: &str) -> Vec<String> {
    let mut labels = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim_start_matches('-').trim();
        if key != "runs-on" && key != "os" {
            continue;
        }
        let value = value.trim();
        if value.contains("${{") {
            // Only an operand of `&&` or `||` can be the label the expression
            // settles on. Reading every quoted string instead invites the
            // opposite mistake: `'pull_request'` and a repository path are
            // quoted too, and filtering them out by shape is what let
            // `'vitrum'` — a bare word, and the label that starved this
            // queue — pass as prose.
            let mut rest = value;
            while let Some(operator) = rest.find("&&").into_iter().chain(rest.find("||")).min() {
                let after = &rest[operator + 2..];
                let Some(open) = after.find(|c: char| !c.is_whitespace()) else {
                    break;
                };
                if after[open..].starts_with('\'') {
                    let quoted = &after[open + 1..];
                    if let Some(close) = quoted.find('\'') {
                        labels.push(quoted[..close].to_string());
                    }
                }
                rest = after;
            }
        } else if let Some(list) = value.strip_prefix('[') {
            for item in list.trim_end_matches(']').split(',') {
                let item = item.trim().trim_matches(['"', '\'']);
                if !item.is_empty() {
                    labels.push(item.to_string());
                }
            }
        } else {
            let value = value.trim_matches(['"', '\'']);
            if !value.is_empty() && !value.starts_with('#') {
                labels.push(value.to_string());
            }
        }
    }
    labels
}

/// A label with no runner behind it does not fail a job, it queues it.
///
/// `runs-on: vitrum` named a self-hosted runner nobody ever registered. Six
/// jobs per push asked for it, each was accepted and queued and never
/// assigned, and 233 of them accumulated until they starved the servable jobs
/// for the whole repository. The same defect in its other form —
/// `macos-13`, an image GitHub retired — is why the v0.1.0 release matrix
/// never completed and the tag carries no assets.
///
/// CI checks this too, from the one job that asks for a runner GitHub always
/// has. It is here as well because a queue that has already starved is a
/// maintainer cancelling runs by hand.
#[test]
fn every_runner_label_is_one_a_runner_answers_to() {
    let allowed = allowed_runners();
    let mut seen = 0usize;
    for (name, text) in workflows() {
        for label in runner_labels(&text) {
            seen += 1;
            assert!(
                allowed.contains(&label),
                "{name} asks for the runner label {label:?}, which this \
                 repository has not agreed on. A label no machine answers to \
                 does not fail: the job queues until GitHub discards it a day \
                 later, and enough of those starve every servable job here. \
                 Add it to tools/release/runners.txt if a runner really \
                 answers to it."
            );
        }
    }
    assert!(
        seen >= 4,
        "no runner label was found at all, so this proved nothing"
    );
}

/// The extractor sees a label in each shape a workflow writes it, including a
/// bare word.
///
/// Without this, the guard above passes on a matcher that quietly stopped
/// matching, which is indistinguishable from having no guard. The bare word is
/// the case that matters: the first version of this extractor skipped any
/// operand without a hyphen, on the theory that a label looks like
/// `ubuntu-latest`, and so read `'vitrum'` — the exact label that starved this
/// repository's queue — as prose and passed.
#[test]
fn a_runner_label_is_found_in_every_shape() {
    let text = concat!(
        "jobs:\n",
        "  one:\n",
        "    runs-on: ubuntu-latest\n",
        "  two:\n",
        "    runs-on: ${{ github.event_name == 'push' && vars.CI_RUNNER || 'macos-latest' }}\n",
        "  three:\n",
        "    strategy:\n",
        "      matrix:\n",
        "        os: [windows-latest, macos-15-intel]\n",
        "  four:\n",
        "    strategy:\n",
        "      matrix:\n",
        "        include:\n",
        "          - os: ubuntu-latest\n",
        "  five:\n",
        "    runs-on: ${{ github.event_name != 'pull_request' && 'vitrum' || 'ubuntu-latest' }}\n",
    );
    let mut found = runner_labels(text);
    found.sort();
    found.dedup();
    assert_eq!(
        found,
        vec![
            "macos-15-intel".to_string(),
            "macos-latest".to_string(),
            "ubuntu-latest".to_string(),
            "vitrum".to_string(),
            "windows-latest".to_string(),
        ],
        "the extractor missed a shape a workflow really writes"
    );
}

/// The lines of a workflow that GitHub acts on, with the prose removed.
///
/// Every guard below asks whether a workflow *does* something, and the files
/// here carry more comment than YAML. `git tag` appears in `release.yml` in a
/// sentence about the nightly tag moving, and a guard that reads it as a tag
/// being made is a guard that fires on a paragraph.
fn instructions(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A platform regression is answered by the push that caused it.
///
/// `platforms.yml` used to run nightly, on a tag, and on a path filter. Two
/// Windows-only defects shipped in one week — an exit code from `vitrum icons`
/// that no other platform returned, and a test deadline that only blows there
/// — and both were found hours later by the nightly run, against a commit
/// nobody was still holding in their head. So a push to main runs a subset of
/// the matrix, and the two platforms that are not developed on here are in it.
///
/// This pins the cadence and the platforms, which is what silently reverts: a
/// trigger is one line, and removing it leaves a workflow that still looks
/// like it covers Windows. It does not check that the subset would catch
/// either defect, and cannot: that is a property of the suite those legs run.
#[test]
fn the_platform_matrix_runs_on_every_push_to_main() {
    let (_, text) = workflows()
        .into_iter()
        .find(|(name, _)| name == "platforms.yml")
        .expect("platforms.yml is the workflow that covers the shipped platforms");

    let triggers: String = text
        .lines()
        .skip_while(|line| !line.starts_with("on:"))
        .skip(1)
        .take_while(|line| line.starts_with(' ') || line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        triggers.contains("push:") && triggers.contains("branches: [main]"),
        "platforms.yml does not run on a push to main, so a break on a platform \
         nobody here develops on is not reported until the nightly run"
    );

    for label in ["macos-latest", "windows-latest"] {
        assert!(
            instructions(&text).contains(label),
            "platforms.yml no longer names {label}, so nothing between a push \
             and a release tag builds that platform at all"
        );
    }
}

/// Exactly one thing cuts a release, and a workflow reaches it by name.
///
/// A release is a version bump, a rolled changelog, a commit and an annotated
/// tag carrying the notes, and `tools/release/cut.sh` is where every refusal
/// that guards those lives. A workflow that made the tag itself would be a
/// second implementation with none of them, and the two would drift in the
/// direction nobody tests: the automated one.
///
/// It checks the workflows only. `cut.sh` being the single implementation in
/// the tree is what `make release` and the dry run exercise; this stops the
/// pipeline growing a copy of it.
#[test]
fn one_script_cuts_a_release() {
    let mut cutting: Vec<String> = Vec::new();
    for (name, text) in workflows() {
        let body = instructions(&text);
        let pushes_a_tag = body
            .lines()
            .any(|line| line.contains("git push") && line.contains("tag"));
        if !pushes_a_tag {
            continue;
        }
        cutting.push(name.clone());
        assert!(
            body.contains("tools/release/cut.sh"),
            "{name} pushes a release tag without running tools/release/cut.sh, \
             so the tag it publishes went through none of the refusals a cut owes"
        );
        for own_work in ["git tag -a", "versions.sh bump", "changelog.sh roll"] {
            assert!(
                !body.contains(own_work),
                "{name} runs `{own_work}` itself instead of leaving it to \
                 tools/release/cut.sh, which is a second release mechanism"
            );
        }
    }
    assert_eq!(
        cutting,
        vec!["cut.yml".to_string()],
        "a release tag is pushed by exactly one workflow"
    );
}

/// Every published target is installed by a job that runs on it.
///
/// WHY: `targets.sh check` makes the release matrix, both installers and the
/// docs table agree on which triples exist. Agreement is not coverage. A
/// target can be built, uploaded, listed in `SHA256SUMS`, named in the table
/// and downloaded by nobody, and the first machine to run the installer for
/// it is somebody else's. That was true of `aarch64-unknown-linux-gnu` for
/// every release so far: `install.yml` ran four package managers, all of them
/// x86_64.
///
/// The mapping is exhaustive on purpose. A sixth triple has no runner label
/// here, so it panics naming itself rather than passing, which is the only
/// arrangement in which adding a platform cannot quietly skip this.
#[test]
fn every_published_target_is_installed_on_a_runner_that_runs_it() {
    let (_, install) = workflows()
        .into_iter()
        .find(|(name, _)| name == "install.yml")
        .expect("install.yml is the workflow that runs the installers");
    let install = instructions(&install);

    for target in published_targets() {
        let label = match target.as_str() {
            "x86_64-unknown-linux-gnu" => "ubuntu-latest",
            "aarch64-unknown-linux-gnu" => "ubuntu-24.04-arm",
            "aarch64-apple-darwin" => "macos-latest",
            "x86_64-apple-darwin" => "macos-15-intel",
            "x86_64-pc-windows-msvc" => "windows-latest",
            "aarch64-pc-windows-msvc" => "windows-11-arm",
            other => panic!(
                "{other} is published and no runner label is recorded for it here, \
                 so nothing says which machine proves its archive installs"
            ),
        };
        assert!(
            install.contains(label),
            "{target} is published and install.yml never runs on {label}, so its \
             release archive is uploaded and never installed by anything"
        );
    }
}

/// The triples a release publishes, read from the one file that owns them.
fn published_targets() -> Vec<String> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the app crate has a parent directory")
        .join("tools/release/targets.sh");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} cannot be read: {e}", path.display()));
    let (_, after) = text
        .split_once("TARGETS='")
        .expect("targets.sh holds the published set in a TARGETS assignment");
    let (list, _) = after
        .split_once('\'')
        .expect("the TARGETS assignment is closed");
    let targets: Vec<String> = list
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    assert!(
        targets.len() >= 5,
        "targets.sh named {} targets, which is fewer than the set that has \
         shipped; the assignment was probably reformatted out from under this",
        targets.len()
    );
    targets
}

/// The workflow that pushes a release tag hands that tag to the one that
/// builds it.
///
/// WHY: this repository intends ten or more releases a day, which only works
/// while one dispatch is the whole operation. It is one because of a link
/// nothing else in the pipeline states: a tag pushed with the workflow token
/// starts no workflow, by GitHub's own rule against a run triggering a run, so
/// `release.yml` is asked for by name with the tag as an input. Delete that
/// step and the cut still succeeds. It commits, it tags, it pushes, it reports
/// green, and no archive is ever built for the tag it published, which is the
/// same end state as the v0.1.0 release that carries no assets.
///
/// The two ends are found rather than named. The cutting workflow is whichever
/// one pushes a tag, which [`one_script_cuts_a_release`] has already
/// established is exactly one, and the building workflow is whichever file
/// that one dispatches. Renaming either file keeps this true; dropping the
/// link between them does not.
#[test]
fn a_cut_hands_the_tag_to_the_workflow_that_builds_it() {
    let all = workflows();
    let (cutter, body) = all
        .iter()
        .map(|(name, text)| (name, instructions(text)))
        .find(|(_, body)| {
            body.lines()
                .any(|line| line.contains("git push") && line.contains("tag"))
        })
        .expect("one workflow pushes a release tag");

    let dispatched = body
        .split("gh workflow run ")
        .skip(1)
        .filter_map(|rest| rest.split_whitespace().next())
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        dispatched.len(),
        1,
        "{cutter} pushes a release tag and dispatches {dispatched:?}. A tag \
         pushed by a workflow token starts nothing, so exactly one dispatch is \
         what turns a cut into a release; none means the tag is never built, \
         and two means two builds race for the same release."
    );
    let built = &dispatched[0];

    let (_, target) = all
        .iter()
        .find(|(name, _)| name == built)
        .unwrap_or_else(|| {
            panic!(
                "{cutter} dispatches {built}, which is not a workflow in this \
                 repository. `gh workflow run` on a name that does not resolve \
                 fails after the tag has already been pushed."
            )
        });

    let target_body = instructions(target);
    assert!(
        target.contains("workflow_dispatch:") && target.contains("tag:"),
        "{built} takes no `tag` input, so the tag {cutter} just pushed cannot \
         be handed to it and the dispatch builds whatever main is at instead"
    );
    assert!(
        target_body.contains("gh release edit") || target_body.contains("gh release create"),
        "{built} is what {cutter} dispatches to publish a release and it never \
         touches a release, so a cut ends with a tag and no assets"
    );
}

/// Every channel a release publishes on is exercised by the install workflow.
///
/// WHY: a channel is a promise to a machine that already has vitrum on it. The
/// nightly channel published every day for weeks with nothing in the pipeline
/// ever downloading one of its archives, because `install.sh` cannot reach it
/// at all: it resolves `v<x.y.z>` tags and a nightly lives under a moving tag,
/// so `vitrum update --channel nightly` is the only way in and the only thing
/// that can prove the way in still works.
///
/// The channels are read out of `release.yml`, which decides what a run is, so
/// a third channel turns this red until somebody says how it is proved. The
/// mapping from a channel to its evidence is exhaustive for the same reason
/// the target mapping above is: a new member panics naming itself rather than
/// passing.
#[test]
fn every_release_channel_is_installed_by_something() {
    let all = workflows();
    let (_, release) = all
        .iter()
        .find(|(name, _)| name == "release.yml")
        .expect("release.yml is the workflow that publishes a release");

    let mut channels: Vec<String> = instructions(release)
        .split("channel=")
        .skip(1)
        .filter_map(|rest| rest.split(|c: char| !c.is_ascii_alphabetic()).next())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect();
    channels.sort();
    channels.dedup();
    assert!(
        channels.len() >= 2,
        "release.yml names {channels:?} as its channels, which is fewer than \
         the stable and nightly pair that has shipped; the assignment was \
         probably reformatted out from under this"
    );

    let (_, install) = all
        .iter()
        .find(|(name, _)| name == "install.yml")
        .expect("install.yml is the workflow that runs the installers");
    let install = instructions(install);

    for channel in channels {
        let evidence = match channel.as_str() {
            // The installer resolves the newest published stable and nothing
            // else, so the whole container matrix is evidence for this one.
            "stable" => "sh install.sh",
            // Unreachable from the installer by construction: `update` is the
            // only client of this channel anywhere.
            "nightly" => "--channel nightly",
            other => panic!(
                "release.yml publishes a `{other}` channel and nothing here \
                 says what proves a machine can install from it. Add the case \
                 with the step that downloads one of its archives."
            ),
        };
        assert!(
            install.contains(evidence),
            "release.yml publishes the {channel} channel and install.yml never \
             runs `{evidence}`, so archives are uploaded to it that nothing \
             ever downloads"
        );
    }
}
