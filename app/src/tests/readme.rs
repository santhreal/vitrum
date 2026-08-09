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

/// The stated platform gap matches what the code does.
///
/// Collision detection has a real watcher on Linux and a refusal everywhere
/// else. If a watcher lands for another platform and the page still calls it
/// Linux-only, the most useful thing in the product stays hidden behind a
/// sentence saying it does not work.
#[test]
fn the_stated_platform_gap_is_the_real_one() {
    let readme = include_str!("../../../README.md");
    assert!(
        readme.contains("Linux only") || readme.contains("Linux-only"),
        "the README no longer states the collision-detection platform gap"
    );
}

/// The remote instructions name the thing that keeps agents alive.
///
/// A daemon started as a child of an SSH session dies with the session and
/// takes every PTY with it. `loginctl enable-linger` is the one line between
/// sessions surviving a disconnect and sessions dying when the laptop closes.
///
/// Pinned to `docs/remote.md`, which owns the instructions. It used to be
/// pinned to the README, which is how a systemd unit, an SSH tunnel and a
/// survival table ended up on the front page: the README was the only file
/// any test looked at, so every checkable claim was written there.
#[test]
fn the_remote_instructions_keep_the_daemon_alive() {
    let remote = include_str!("../../../docs/remote.md");
    assert!(
        remote.contains("loginctl enable-linger"),
        "the remote setup does not enable lingering, so logging out kills \
         every agent"
    );
    assert!(
        remote.contains("packaging/vitrum-server.service"),
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

/// Both pages state the limit that cannot be engineered around.
///
/// Sessions do not survive the daemon, because the PTYs are its children.
/// The README claims the durability, so the README carries the exception;
/// `docs/remote.md` carries the survival table it is a row of. A rewrite that
/// drops either one leaves the product claiming durability it does not have.
#[test]
fn losing_the_daemon_loses_the_sessions_on_every_page_that_promises_otherwise() {
    for (name, text) in [
        ("README.md", include_str!("../../../README.md")),
        ("docs/remote.md", include_str!("../../../docs/remote.md")),
    ] {
        assert!(
            text.contains("every session dies") || text.contains("do not survive"),
            "{name} no longer states that sessions die with the daemon"
        );
    }
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

/// Installing is one command, and the installer does the whole job.
///
/// The README used to promise a one-command install and then carry three
/// platform blocks that wrote a desktop entry, edited `PATH` and defined an
/// alias by hand. Anything an operator has to paste after the install command
/// is work the installer refused to do, and it lands on the front page because
/// there is nowhere else for it to go.
///
/// So the capability is asserted in the scripts, and its absence is asserted
/// in the README. Moving a launcher entry back into prose fails here.
#[test]
fn the_installer_finishes_the_install() {
    let sh = include_str!("../../../install.sh");
    let ps1 = include_str!("../../../install.ps1");

    for (script, text, needles) in [
        (
            "install.sh",
            sh,
            [
                "vitrum.desktop",           // Linux launcher entry
                "vitrum.app",               // macOS bundle
                "alias vu=",                // the update shortcut
                "export PATH=",             // PATH, persisted
                "--no-integrate",           // and an opt out for images
            ],
        ),
        (
            "install.ps1",
            ps1,
            [
                "vitrum.lnk",
                "CreateShortcut",
                "function vu",
                "SetEnvironmentVariable('Path'",
                "NoIntegrate",
            ],
        ),
    ] {
        for needle in needles {
            assert!(
                text.contains(needle),
                "{script} never writes `{needle}`, so installing leaves that \
                 step for the operator to paste"
            );
        }
    }

    let readme = include_str!("../../../README.md");
    for pasted in [
        "[Desktop Entry]",
        "update-desktop-database",
        "CFBundleIdentifier",
        "CreateShortcut",
        "alias vu=",
        "function vu",
        "export PATH=",
    ] {
        assert!(
            !readme.contains(pasted),
            "the README asks the operator to paste `{pasted}`, which the \
             installer already does"
        );
    }
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
/// Every number in `docs/performance.md` is written by
/// `harness/readme_perf.py` from `harness/reports/readme-perf.json`, and CI
/// re-renders them to catch a stale table. That check cannot fire if the
/// markers are gone: a rewrite that deletes a region, or closes it with the
/// wrong name, leaves nothing to compare and passes. So the markers are
/// asserted here, where an ordinary `cargo test` sees them.
#[test]
fn the_generated_performance_regions_are_intact() {
    let doc = include_str!("../../../docs/performance.md");
    let snapshot = include_str!("../../../harness/reports/readme-perf.json");

    assert!(
        snapshot.contains("\"schema\": \"vitrum-footprint-v1\""),
        "the snapshot the tables are rendered from is not the schema \
         readme_perf.py writes"
    );

    for region in ["footprint", "idle"] {
        let open = format!("<!-- BENCH:{region}:start -->");
        let close = format!("<!-- BENCH:{region}:end -->");
        let at = doc
            .find(&open)
            .unwrap_or_else(|| panic!("docs/performance.md has no {open}"));
        let end = doc.find(&close).unwrap_or_else(|| {
            panic!("docs/performance.md opens BENCH:{region} and never closes it")
        });
        let body = &doc[at + open.len()..end];
        assert!(
            body.contains('|') && body.contains("Reproduce:"),
            "BENCH:{region} holds no table and no reproduction command; run \
             `make perf-tables`"
        );
    }
}

/// Every local document the README links to exists.
///
/// The README is a landing page: what the product is, how to install it, and
/// links to everything else. That shape only works while the links resolve,
/// and a moved page is invisible to a compiler. Derived by scanning the link
/// targets rather than from a list, so a page added to the table is checked
/// without anyone remembering to check it.
#[test]
fn every_document_the_readme_links_to_exists() {
    let readme = include_str!("../../../README.md");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives under the workspace root");

    let mut checked = 0;
    for target in readme
        .split("](")
        .skip(1)
        .filter_map(|rest| rest.split(')').next())
    {
        // Anchors and URLs are somebody else's problem; local paths are ours.
        if target.starts_with('#') || target.contains("://") {
            continue;
        }
        let path = target.split('#').next().unwrap_or(target);
        assert!(
            root.join(path).exists(),
            "the README links to {path}, which is not in the repository"
        );
        checked += 1;
    }
    assert!(
        checked >= 8,
        "only {checked} local links were found in the README; the page no \
         longer links out to the documentation it moved its detail into"
    );
}
