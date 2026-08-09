//! The README must describe THIS build.
//!
//! Every documentation defect this project has shipped had the same shape: a
//! sentence that was true when it was written and silently false afterwards.
//! A README is the first thing a new operator trusts and the last thing anyone
//! re-reads, so the few claims in it that a machine can check are checked
//! here, against the code rather than against a copy of the prose.

/// The installers fetch targets the release workflow actually builds.
///
/// The README used to carry a per-platform paste, and this test checked the
/// paste. The paste is gone: it named an archive and extracted it without
/// verifying a digest, so the documented install is now the two scripts in
/// the repository root. That moves the same disagreement one file over. The
/// release matrix names the targets, `update.rs` names the archive, and the
/// scripts name both, and a typo in any of them is a 404 for whoever runs it.
/// Checked against the workflow rather than against prose.
#[test]
fn the_installers_fetch_targets_the_release_workflow_builds() {
    let manifest = include_str!("../../Cargo.toml");
    let release = include_str!("../../../.github/workflows/release.yml");
    let sh = include_str!("../../../install.sh");
    let ps1 = include_str!("../../../install.ps1");
    assert!(
        manifest.contains("name = \"vitrum\""),
        "the client package is no longer named `vitrum`, so its binary is not `vitrum`"
    );

    let built = target_triples(release);
    assert!(
        !built.is_empty(),
        "the release workflow builds no recognisable target triple"
    );
    for (script, text) in [("install.sh", sh), ("install.ps1", ps1)] {
        let wanted = target_triples(text);
        assert!(
            !wanted.is_empty(),
            "{script} names no target triple, so it cannot be asking for a \
             published archive"
        );
        for target in &wanted {
            assert!(
                built.contains(target),
                "{script} downloads `{target}`, which the release workflow \
                 does not build"
            );
        }
        for binary in ["vitrum", "vitrum-server"] {
            assert!(
                text.contains(binary),
                "{script} never places `{binary}`, and the client will not run \
                 without the daemon beside it"
            );
        }
    }

    let readme = include_str!("../../../README.md");
    assert!(
        readme.contains("vitrum update"),
        "the README never tells the operator how to update"
    );
    assert!(
        !readme.contains("vitrum-app"),
        "the README still refers to `vitrum-app`, which is not a binary"
    );
}

/// Every Rust target triple named anywhere in `text`.
///
/// Derived by scanning rather than from a hardcoded list, so a new platform
/// in the release matrix is compared instead of silently ignored.
fn target_triples(text: &str) -> std::collections::BTreeSet<String> {
    const SUFFIXES: [&str; 4] = [
        "-unknown-linux-gnu",
        "-unknown-linux-musl",
        "-apple-darwin",
        "-pc-windows-msvc",
    ];
    let mut found = std::collections::BTreeSet::new();
    for token in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
        if SUFFIXES.iter().any(|s| token.ends_with(s)) && !token.starts_with('-') {
            found.insert(token.to_string());
        }
    }
    found
}

/// No command in the README carries a version literal.
///
/// This used to require the opposite: every download URL had to name
/// `v{CARGO_PKG_VERSION}`, pinned so the prose could not drift from the
/// crate. It drifted anyway, in the worse direction. The URLs named
/// `refs/tags/v0.1.0` and no such tag was ever pushed, so the pin held a
/// version literal steady against a release that did not exist and the first
/// command a new operator ran was a 404.
///
/// A version cannot be pinned in prose to something the repository cannot
/// prove. What it can prove is that no command hardcodes one: the installers
/// resolve the latest release at run time and a source build clones the
/// repository, so neither can go stale. The version appears once, as a
/// statement of fact in Status, and that one is pinned.
#[test]
fn no_command_in_the_readme_hardcodes_a_version() {
    let readme = include_str!("../../../README.md");
    let v = env!("CARGO_PKG_VERSION");
    for (n, block) in fenced_blocks(readme).iter().enumerate() {
        for stale in [v, "refs/tags/"] {
            assert!(
                !block.contains(stale),
                "code block {n} in the README contains `{stale}`, which pins a \
                 command to a release this repository cannot prove exists:\n{block}"
            );
        }
    }
    assert!(
        readme.contains(&format!("version {v}")),
        "the Status section no longer states version {v}"
    );
}

/// The body of every fenced code block in `text`.
fn fenced_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut inside: Option<String> = None;
    for line in text.lines() {
        match (&mut inside, line.trim_start().starts_with("```")) {
            (None, true) => inside = Some(String::new()),
            (Some(_), true) => blocks.push(inside.take().expect("just matched")),
            (Some(body), false) => {
                body.push_str(line);
                body.push('\n');
            }
            (None, false) => {}
        }
    }
    blocks
}

/// Every keyboard shortcut the README advertises is bound.
///
/// A README that teaches a chord the product does not have is worse than
/// one that teaches none: the operator concludes the app is broken.
#[test]
fn every_advertised_shortcut_exists() {
    use crate::keymap::CHORDS;
    let readme = include_str!("../../../README.md");
    // Only the rows of the shortcut table, which are the advertised ones.
    for (chord, action) in [
        ("Ctrl+Shift+N", crate::keymap::KeyAction::NewSession),
        ("Ctrl+Shift+F", crate::keymap::KeyAction::OpenSearch),
        ("Ctrl+Shift+X", crate::keymap::KeyAction::CloseSession),
    ] {
        assert!(
            readme.contains(chord),
            "the README stopped documenting {chord}"
        );
        assert!(
            CHORDS.iter().any(|c| c.action == action),
            "the README documents {chord} but nothing binds {action:?}"
        );
    }
}

/// The README's platform gaps match what the code actually does.
///
/// Collision detection has a real watcher on Linux and an honest refusal
/// everywhere else. If a watcher lands for another platform and the README
/// still calls it Linux-only, the most useful thing in the product stays
/// hidden behind a sentence saying it does not work.
#[test]
fn the_stated_platform_gap_is_the_real_one() {
    let readme = include_str!("../../../README.md");
    assert!(
        readme.contains("Linux only") || readme.contains("Linux-only"),
        "the README no longer states the collision-detection platform gap"
    );
}

/// The remote instructions name the thing that actually keeps agents alive.
///
/// A daemon started as a child of an SSH session dies with the session, and
/// takes every PTY with it. `enable-linger` is what stops that, and it is
/// the single line between "your agents survive a disconnect" and "your
/// agents die when you close the laptop". A README that documents the
/// tunnel and omits it is worse than one that documents neither.
#[test]
fn the_remote_instructions_keep_the_daemon_alive() {
    let readme = include_str!("../../../README.md");
    assert!(
        readme.contains("loginctl enable-linger"),
        "the remote setup does not enable lingering, so logging out kills \
         every agent"
    );
    assert!(
        readme.contains("packaging/vitrum-server.service"),
        "the remote setup does not install the unit it ships"
    );
    // The unit exists and starts the daemon by the name we install.
    let unit = include_str!("../../../packaging/vitrum-server.service");
    assert!(unit.contains("ExecStart=%h/.local/bin/vitrum-server"));
    assert!(
        unit.contains("Restart=on-failure"),
        "the daemon does not come back after a crash"
    );
}

/// The README states the limit it cannot engineer around.
///
/// Sessions do not survive the daemon, because the PTYs are its children.
/// Every other row of the survival table is a promise this build keeps; if
/// that one is ever dropped from the docs while it is still true, the
/// product is claiming durability it does not have.
#[test]
fn the_readme_admits_that_losing_the_daemon_loses_the_sessions() {
    let readme = include_str!("../../../README.md");
    assert!(
        readme.contains("every session dies") || readme.contains("do not survive"),
        "the README no longer states that sessions die with the daemon"
    );
}

/// The README promises no package that does not exist, and no install that
/// skips verification.
///
/// A package in someone else's index is a false claim while nothing is
/// published there. `curl | sh` used to be banned here on the grounds that it
/// teaches the operator to run whatever the host serves, which is true, and
/// the alternative that shipped under that ban was worse: three pastes that
/// resolved a version, downloaded an archive and extracted it onto `PATH`
/// having checked no digest at all, on a page that claimed elsewhere that this
/// project verifies its downloads.
///
/// So the rule is the property that actually protects the operator, rather
/// than the shape of the command. Whatever the README tells someone to run has
/// to be a script in this repository, and that script has to refuse an archive
/// it cannot match against the release `SHA256SUMS`.
#[test]
fn the_documented_install_verifies_what_it_downloads() {
    let readme = include_str!("../../../README.md");
    for absent in ["brew install", "apt install vitrum", "cargo install vitrum"] {
        assert!(
            !readme.contains(absent),
            "the README advertises `{absent}`, which this project does not publish"
        );
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives under the workspace root");

    let mut piped = 0;
    for script in ["install.sh", "install.ps1"] {
        if !readme.contains(script) {
            continue;
        }
        piped += 1;
        assert!(
            root.join(script).is_file(),
            "the README tells the operator to run {script}, which is not in \
             the repository"
        );
        let text = std::fs::read_to_string(root.join(script))
            .unwrap_or_else(|why| panic!("cannot read {script}: {why}"));
        // Naming the file is not verifying it. A script that mentions
        // SHA256SUMS in a comment and installs regardless passes a substring
        // check and fails the operator, so what is required is the whole
        // path: fetch the manifest, compute a digest with a real tool, and
        // abort on a mismatch having installed nothing.
        for (needle, missing) in [
            ("SHA256SUMS", "never fetches the release SHA256SUMS"),
            ("checksum mismatch", "has no mismatch path, so it cannot refuse"),
            ("nothing was installed", "does not promise to leave nothing behind on a mismatch"),
        ] {
            assert!(
                text.contains(needle),
                "{script} is the documented install and {missing}"
            );
        }
        let hashes = ["sha256sum", "shasum", "Get-FileHash"]
            .iter()
            .any(|tool| text.contains(tool));
        assert!(
            hashes,
            "{script} computes no SHA-256 anywhere, so whatever it compares \
             against SHA256SUMS is not the archive it downloaded"
        );
    }

    assert!(
        piped >= 2,
        "only {piped} install scripts were found in the README; the install \
         section no longer names the scripts this test can check"
    );
}

/// Every image the README shows is in the repository, and none is a picture
/// of a shell.
///
/// The page ships no pictures at all right now. Every one it used to carry
/// was deleted: a GIF, an MP4 and two screenshots that showed `bash`,
/// `cargo test` or `git log` filling the pane with a path from the recording
/// machine in the launcher, then a hero that was a photograph of the test
/// fixture, captioned as four real sessions. Each argued the product was a
/// terminal multiplexer, or was simply not the product.
///
/// So there is no floor here, and that is deliberate. A count only ever
/// forced someone to keep a bad picture on the page to keep the build green,
/// which is exactly how the fixture screenshot survived. What is asserted is
/// the pair of rules a replacement has to satisfy: it exists, and it is not
/// named for a shell or a build tool. See AGENTS.md, "Demos show agents, not
/// shell output".
#[test]
fn every_image_the_readme_shows_exists_and_is_not_a_shell() {
    let readme = include_str!("../../../README.md");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives under the workspace root");

    // Markdown `](path)` and HTML `src="path"` both put an image on the page,
    // and a scan for one is blind to the other. The banner was HTML.
    let referenced = readme
        .split("](")
        .skip(1)
        .filter_map(|rest| rest.split(')').next())
        .chain(
            readme
                .split("src=\"")
                .skip(1)
                .filter_map(|rest| rest.split('"').next()),
        )
        .filter(|target| target.starts_with("assets/"));

    for target in referenced {
        assert!(
            root.join(target).is_file(),
            "the README points at {target}, which is not in the repository"
        );
        for banned in [
            "bash", "zsh", "fish", "shell", "cargo", "git", "make", "npm", "docker", "htop",
        ] {
            assert!(
                !target.contains(banned),
                "the README shows {target}, and a demo asset named for a shell \
                 or a build tool puts vitrum in tmux's category"
            );
        }
    }
}

/// The generated performance regions are present, closed, and filled.
///
/// Every number under "What it costs to run" is written by
/// `harness/readme_perf.py` from `harness/reports/readme-perf.json`, and CI
/// re-renders them to catch a stale table. That check cannot fire if the markers
/// themselves are gone: a rewrite that deletes a region, or closes it with the
/// wrong name, leaves nothing to compare and passes. So the markers are asserted
/// here, where an ordinary `cargo test` sees them.
#[test]
fn the_generated_performance_regions_are_intact() {
    let readme = include_str!("../../../README.md");
    let snapshot = include_str!("../../../harness/reports/readme-perf.json");

    assert!(
        snapshot.contains("\"schema\": \"vitrum-footprint-v1\""),
        "the snapshot the tables are rendered from is not the schema \
         readme_perf.py writes"
    );

    for region in ["footprint", "idle"] {
        let open = format!("<!-- BENCH:{region}:start -->");
        let close = format!("<!-- BENCH:{region}:end -->");
        let at = readme
            .find(&open)
            .unwrap_or_else(|| panic!("the README has no {open}"));
        let end = readme[at..]
            .find(&close)
            .unwrap_or_else(|| panic!("the README opens BENCH:{region} and never closes it"));
        let body = &readme[at + open.len()..at + end];
        assert!(
            body.contains('|') && body.contains("Reproduce:"),
            "BENCH:{region} holds no table and no reproduction command; run \
             `make readme-perf`"
        );
    }
}
