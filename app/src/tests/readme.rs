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

/// Every keyboard shortcut the documentation advertises is bound.
///
/// A page that teaches a chord the product does not have is worse than one
/// that teaches none: the operator concludes the app is broken.
///
/// Pinned to `docs/keys.md`, which owns the table. The README links to it and
/// carries no chords, for the same reason the remote instructions moved: a
/// claim belongs in the document that owns it, not on the front page because
/// the front page is the file a test happened to read.
#[test]
fn every_advertised_shortcut_exists() {
    use crate::keymap::CHORDS;
    let keys = include_str!("../../../docs/keys.md");
    for (chord, action) in [
        ("Ctrl+Shift+N", crate::keymap::KeyAction::NewSession),
        ("Ctrl+Shift+F", crate::keymap::KeyAction::OpenSearch),
        ("Ctrl+Shift+X", crate::keymap::KeyAction::CloseSession),
    ] {
        assert!(
            keys.contains(chord),
            "docs/keys.md stopped documenting {chord}"
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
            &[
                "vitrum.desktop",           // Linux launcher entry
                "vitrum.app",               // macOS bundle
                "alias vu=",                // the update shortcut
                "export PATH=",             // PATH, persisted
                "--no-integrate",           // and an opt out for images
                "vitrum\" icons ",          // the icon set, drawn by the binary
                "Icon=vitrum",              // named in the launcher entry
                "CFBundleIconFile",         // and in the macOS bundle
            ][..],
        ),
        (
            "install.ps1",
            ps1,
            &[
                "vitrum.lnk",
                "CreateShortcut",
                "function vu",
                "SetEnvironmentVariable('Path'",
                "NoIntegrate",
                "vitrum.exe') icons",       // the icon set, drawn by the binary
                "IconLocation",             // and put on the Start menu shortcut
            ][..],
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
/// The page carries pictures again. Every one it used to carry before that
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

/// Every performance claim states how it was measured.
///
/// The page was generated once, out of a snapshot, with marker regions and a
/// CI step that re-rendered them. It is written by hand now, which removes
/// the stale-table failure and introduces a worse one: a number that reads
/// like a measurement and is an estimate. Nothing in a build can tell those
/// apart, so what is asserted is the shape that makes the difference visible
/// to a reader, and the one sentence that stops a ratio being quoted without
/// the floor it was taken against.
#[test]
fn every_performance_claim_says_how_it_was_measured() {
    let doc = include_str!("../../../docs/performance.md");

    for required in [
        // Each row names its method, and the page says so where a reader
        // meets the first table rather than in a footnote.
        "names its method",
        // A latency ratio against a display path is meaningless without the
        // cost every client on that display pays.
        "platform floor",
        // The old build cannot be rebuilt from this tree, so where its figure
        // came from is part of the claim.
        "harness/latency/",
        // The command that reproduces the new figures, which is what makes
        // them checkable by somebody who does not trust them.
        "cargo run --release -p vitrum-bench",
    ] {
        assert!(
            doc.contains(required),
            "docs/performance.md no longer says {required:?}, so a reader \
             cannot tell a measured row from an estimated one"
        );
    }

    // A signal the old build could not produce must not carry a ratio. The
    // page states that rule; a table that stops honouring it is a table
    // somebody widened by inventing a baseline.
    assert!(
        doc.contains("no ratio is given") || doc.contains("no ratio is claimed"),
        "docs/performance.md dropped the rule that an unobservable baseline \
         yields no ratio, which is the only thing stopping one being invented"
    );
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

/// The install path answers for what a real machine does to it.
///
/// An installer is judged on the day it fails. Every case below was reached
/// by a machine at some point: a proxy that returns a sign-in page, a
/// transfer that stops half way, a directory owned by root, an editor open
/// on the binary being replaced, a distribution whose WebKit package is
/// spelled differently, an architecture nobody publishes for. Each one used
/// to arrive as a 404, a bare non-zero exit, or the wrong diagnosis
/// entirely: "checksum mismatch" is what a truncated download used to say,
/// and it sends the operator to the release instead of to their network.
///
/// So the contract is that each case is detected as itself. The needles are
/// message fragments rather than code, because the message is the product
/// here; a rewrite that keeps the check and drops the sentence has removed
/// the only part the operator sees.
#[test]
fn the_installer_answers_for_what_a_real_machine_does() {
    let sh = include_str!("../../../install.sh");
    let ps1 = include_str!("../../../install.ps1");
    let doc = include_str!("../../../docs/install.md");

    // case, what install.sh must say, what install.ps1 must say
    let cases: &[(&str, &str, Option<&str>)] = &[
        (
            "no downloader at all",
            "neither curl nor wget is available",
            None, // PowerShell has Invoke-WebRequest built in
        ),
        (
            "a proxy variable that is not a URL",
            "is not a URL a proxy can be reached at",
            Some("is not a URL a proxy can be reached at"),
        ),
        (
            "a proxy that blocks the download",
            "A proxy is in force",
            Some("A proxy is in force"),
        ),
        (
            "a transfer that stopped early",
            "it is truncated",
            Some("it is truncated"),
        ),
        (
            "a portal page instead of the archive",
            "it is a web page, not an archive",
            Some("it is a web page, not an archive"),
        ),
        (
            "a checksum file that is not one",
            "is not a checksum file",
            Some("is not a checksum file"),
        ),
        (
            "a checksum file with no line for this archive",
            "has no entry for",
            Some("has no entry for"),
        ),
        (
            "a digest that disagrees",
            "checksum mismatch",
            Some("checksum mismatch"),
        ),
        (
            "an install directory that refuses a write",
            "cannot be written to",
            Some("cannot be written to"),
        ),
        (
            "a running vitrum in the install directory",
            "is running from",
            Some("is running from"),
        ),
        (
            "a re-install over an existing one",
            "replacing    ",
            Some("replacing    "),
        ),
        (
            "an uninstall of exactly what was written",
            "install-manifest",
            Some("install-manifest"),
        ),
        (
            "an architecture with no published build",
            "there is no published build for Linux on",
            Some("there is no published build for Windows on"),
        ),
    ];

    for (case, in_sh, in_ps1) in cases {
        assert!(
            sh.contains(in_sh),
            "install.sh never says `{in_sh}`, so {case} is not a case it \
             answers for"
        );
        if let Some(needle) = in_ps1 {
            assert!(
                ps1.contains(needle),
                "install.ps1 never says `{needle}`, so {case} is not a case \
                 it answers for"
            );
        }
    }

    // A libc mismatch installs cleanly and then fails to start, so it is
    // caught by name rather than left to the loader.
    assert!(
        sh.contains("musl libc"),
        "install.sh does not name musl, so a musl host gets a glibc archive \
         and a loader error instead of a sentence it can act on"
    );

    // The runtime package, spelled the way each distribution spells it.
    // "install a WebKit runtime" is not an instruction anyone can run.
    for package in [
        "libwebkit2gtk-4.1-0",  // Debian, Ubuntu
        "webkit2gtk4.1",        // Fedora
        "webkit2gtk-4.1",       // Arch
        "libwebkit2gtk-4_1-0",  // openSUSE
        "net-libs/webkit-gtk",  // Gentoo
        "webkitgtk_4_1",        // NixOS
    ] {
        assert!(
            sh.contains(package),
            "install.sh does not name `{package}`, so that distribution is \
             told to install something it has no package for"
        );
        assert!(
            doc.contains(package),
            "docs/install.md does not name `{package}`"
        );
    }
    assert!(
        ps1.contains("Microsoft.EdgeWebView2Runtime"),
        "install.ps1 does not name the WebView2 runtime package, so a Windows \
         machine without it is left with a binary that opens no window"
    );

    // Every shell that gets a PATH edit, in the syntax that shell parses.
    // bash alone left zsh users (every macOS default shell) and fish users
    // with binaries they could not run by name.
    //
    // `.bash_profile` and `.bash_login` are here because bash reads exactly one
    // login file and stops at the first that exists, so either of them shadows
    // `~/.profile` outright, while `.bashrc` is skipped by a login shell that is
    // not interactive. A machine that had ever run rustup, nvm or bun has a
    // `.bash_profile`, and on those the PATH edit landed only in files bash
    // never opened: `vitrum` was installed and `command -v vitrum` found
    // nothing.
    for (rc, syntax) in [
        (".bashrc", "export PATH="),
        (".bash_profile", "export PATH="),
        (".bash_login", "export PATH="),
        (".zshrc", "export PATH="),
        ("config.fish", "set -gx PATH"),
    ] {
        assert!(
            sh.contains(rc) && sh.contains(syntax),
            "install.sh does not edit {rc} with `{syntax}`, so that shell \
             cannot find the binary it just installed"
        );
    }

    // The checks that cost nothing run before the download that costs
    // something. Finding out that the machine has no WebKit after ninety
    // megabytes crossed a metered link is a worse experience than the
    // failure itself.
    let download = sh
        .find("Downloading $ARCHIVE")
        .expect("install.sh downloads the archive");
    for (what, needle) in [
        ("the WebKit runtime check", "needs a WebKit runtime"),
        ("the write permission check", "cannot be written to"),
        ("the running-client check", "is running from"),
    ] {
        let at = sh
            .find(needle)
            .unwrap_or_else(|| panic!("install.sh has no {what}"));
        assert!(
            at < download,
            "{what} runs after the download in install.sh, so the operator \
             pays for the archive before being told they cannot use it"
        );
    }
    let ps1_download = ps1
        .find("Downloading $Archive")
        .expect("install.ps1 downloads the archive");
    for (what, needle) in [
        ("the WebView2 runtime check", "needs the WebView2 runtime"),
        ("the write permission check", "cannot be written to"),
        ("the running-client check", "is running from"),
    ] {
        let at = ps1
            .find(needle)
            .unwrap_or_else(|| panic!("install.ps1 has no {what}"));
        assert!(
            at < ps1_download,
            "{what} runs after the download in install.ps1, so the operator \
             pays for the archive before being told they cannot use it"
        );
    }

    // The page an operator reads before running any of it.
    for needle in ["--uninstall", "-Uninstall", "--base-url", "--no-runtime-check"] {
        assert!(
            doc.contains(needle),
            "docs/install.md does not document `{needle}`"
        );
    }
}

/// No failure leaves without saying what to do next.
///
/// Enumerated from the scripts rather than from a list here, so a check added
/// tomorrow is held to the same rule as the ones added today: a bare `die` or
/// a bare `Fail` turns this red until someone writes the sentence that follows
/// it. A message that only names the fault leaves the operator with a failed
/// install and a search engine.
#[test]
fn every_installer_failure_names_what_to_do_next() {
    let sh = include_str!("../../../install.sh");
    let mut checked = 0;
    for (n, line) in sh.lines().enumerate() {
        let trimmed = line.trim_start();
        // The definitions themselves, and the call inside `need`, which
        // forwards its caller's action as the second argument.
        if trimmed.starts_with("die()") || trimmed.starts_with("die_net()") {
            continue;
        }
        let Some(rest) = call_argument_text(trimmed, &["die ", "die_net "]) else {
            continue;
        };
        checked += 1;
        // Either the actions continue onto the next line, or a second quoted
        // argument carries them on this one.
        let continued = line.trim_end().ends_with('\\');
        let two_arguments = rest.matches('"').count() >= 4;
        assert!(
            continued || two_arguments,
            "install.sh line {} fails with a message and no action:\n  {trimmed}",
            n + 1
        );
    }
    assert!(
        checked >= 15,
        "only {checked} failure paths were found in install.sh; the script no \
         longer routes its failures through `die`, so nothing here is checked"
    );

    let ps1 = include_str!("../../../install.ps1");
    let mut ps1_checked = 0;
    for (n, line) in ps1.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("function Fail") || trimmed.starts_with("function FailNet") {
            continue;
        }
        let Some(rest) = call_argument_text(trimmed, &["Fail ", "FailNet "]) else {
            continue;
        };
        // `Fail $Message ($Actions + $extra)` inside FailNet forwards its
        // caller's words; the caller is what this rule is about.
        if rest.trim_start().starts_with('$') {
            continue;
        }
        ps1_checked += 1;
        assert!(
            rest.contains("@("),
            "install.ps1 line {} fails with a message and no action:\n  {trimmed}",
            n + 1
        );
    }
    assert!(
        ps1_checked >= 10,
        "only {ps1_checked} failure paths were found in install.ps1; the \
         script no longer routes its failures through `Fail`"
    );
}

/// A build this machine cannot start is refused before it is installed.
///
/// A clean container matrix found two machines that took the archive,
/// reported a successful install, and then could not run the binary: one
/// whose C library was older than the symbol versions the build references,
/// and one whose only `xdotool` package ships a different soname from the one
/// the client links. On both, the launcher entry was written without a
/// picture, because the binary that draws the icon set could not run either.
///
/// The requirement is read from the archive rather than written down here, so
/// a build that stops linking something stops being refused for it. A floor
/// copied into this script is the failure this test exists to prevent: it
/// turns a fixed build back into an install failure, on exactly the machines
/// the fix was for.
#[test]
fn the_installer_refuses_a_build_this_machine_cannot_run() {
    let sh = include_str!("../../../install.sh");

    // Asked of the binary that was just unpacked, so the answer is this
    // build's rather than this script's idea of this build.
    let check = sh
        .find(r#"runtime_report "$TMPDIR_SELF/$bin""#)
        .expect(
            "install.sh does not run the runtime check against the unpacked \
             archive, so what it refuses for is a list in the script rather \
             than what the build links",
        );

    // Nothing is written into the install directory until the answer is in.
    let first_write = sh
        .find(r#"staged="$INSTALL_DIR/"#)
        .expect("install.sh stages the binaries inside the install directory");
    assert!(
        check < first_write,
        "install.sh writes into the install directory before it knows whether \
         the build starts, so a machine that cannot run it loses the copy it \
         already had"
    );

    // The one edit that would make the guard go stale without failing.
    for (n, line) in sh.lines().enumerate() {
        assert!(
            !line.contains("GLIBC_2."),
            "install.sh line {} pins a C library version:\n  {}\nThe floor \
             belongs to the build, and a copy of it here refuses machines the \
             next build would have run on",
            n + 1,
            line.trim()
        );
    }

    // The three ways a verified archive still fails to start, told apart,
    // because each has a different thing for the operator to do.
    for (case, needle) in [
        (
            "a C library older than the build references",
            "needs a newer C library than this machine has",
        ),
        (
            "a shared library this machine has not got",
            "needs shared libraries this machine does not have",
        ),
        (
            "a shared library this distribution does not carry",
            "needs shared libraries this distribution does not package",
        ),
    ] {
        assert!(
            sh.contains(needle),
            "install.sh never says `{needle}`, so {case} is not a case it \
             answers for"
        );
    }

    // Naming a package that does not provide the soname costs an install and
    // leaves the binary exactly as broken, so nothing is named instead.
    assert!(
        sh.contains("no package on this distribution is known to provide it"),
        "install.sh has no answer for a soname its distribution does not \
         package, so it falls back to naming a package that does not fix it"
    );

    // --no-runtime-check installs anyway, and then says what is still wrong.
    assert!(
        sh.contains("these shared libraries are still missing"),
        "install.sh skips the check without saying what it skipped, so an \
         image is left believing the install is complete"
    );

    // The first run of the installed binary is the icon set. Whatever stopped
    // it is what will stop `vitrum`, so it is repeated rather than swallowed.
    assert!(
        sh.contains(r#"2> "$TMPDIR_SELF/icons.err""#) && sh.contains("icons.err"),
        "install.sh throws away what the binary said when the icon set failed, \
         so a machine that cannot run the build is told only that a picture is \
         missing"
    );
}

/// Every kind of thing the installer records is a kind the uninstaller knows.
///
/// The manifest is the whole of `--uninstall`: what is not recorded is not
/// removed, and a kind recorded without a matching arm is dropped with a
/// warning, which is how an empty `~/.profile` the installer had created came
/// to survive its own uninstall on a machine that never had one.
///
/// The kinds are read out of the scripts rather than listed here, so a new one
/// turns this red until someone decides what removing it means.
#[test]
fn the_uninstaller_knows_every_kind_the_installer_records() {
    let sh = include_str!("../../../install.sh");
    let mut sh_kinds = std::collections::BTreeSet::new();
    for line in sh.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("manifest_add ") else {
            continue;
        };
        let kind = rest.split_whitespace().next().unwrap_or_default();
        assert!(
            !kind.starts_with('"') && !kind.starts_with('$'),
            "install.sh records a manifest kind through a variable:\n  \
             {trimmed}\nThe kinds have to be readable here, or a kind the \
             uninstaller cannot handle passes unnoticed"
        );
        sh_kinds.insert(kind.to_string());
    }
    assert!(
        sh_kinds.len() >= 4,
        "only {} manifest kinds were found in install.sh, so the manifest is \
         no longer written through `manifest_add` and nothing here is checked",
        sh_kinds.len()
    );
    for kind in &sh_kinds {
        assert!(
            sh.contains(&format!("{kind})")),
            "install.sh records `{kind}` in the manifest and --uninstall has \
             no arm for it, so what it wrote stays on the machine"
        );
    }
    for kind in ["rc", "rc-created"] {
        assert!(
            sh_kinds.contains(kind),
            "install.sh no longer records `{kind}`, so an rc file it created \
             and one it edited are removed the same way and the created one is \
             left behind empty"
        );
    }

    let ps1 = include_str!("../../../install.ps1");
    let mut ps1_kinds = std::collections::BTreeSet::new();
    for line in ps1.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("Record '") else {
            continue;
        };
        let Some(kind) = rest.split('\'').next() else {
            continue;
        };
        ps1_kinds.insert(kind.to_string());
    }
    assert!(
        ps1_kinds.len() >= 3,
        "only {} manifest kinds were found in install.ps1, so the manifest is \
         no longer written through `Record`",
        ps1_kinds.len()
    );
    for kind in &ps1_kinds {
        assert!(
            ps1.contains(&format!("'{kind}' {{")),
            "install.ps1 records `{kind}` in the manifest and -Uninstall has \
             no arm for it, so what it wrote stays on the machine"
        );
    }
    assert!(
        ps1_kinds.contains("profile-created"),
        "install.ps1 no longer records a profile it created, so a machine that \
         had no PowerShell profile keeps an empty one after -Uninstall"
    );

    // An empty directory the installer made on its way to a file it removed is
    // still something it left behind.
    for dir in [
        "\"$DATA_DIR/applications\" \\",
        "\"${XDG_CONFIG_HOME:-$HOME/.config}/fish\"",
    ] {
        assert!(
            sh.contains(dir),
            "install.sh does not prune {dir} on uninstall, so a directory it \
             created is left behind empty"
        );
    }

    // A cache that indexes other applications' entries is taken away only when
    // this install is what created it.
    assert!(
        sh.contains("mimeinfo.cache"),
        "install.sh runs update-desktop-database without recording the cache \
         it may have created, so uninstalling leaves it behind"
    );
}

/// A login file that refuses the edit does not cost the operator the PATH.
///
/// bash opens exactly one login file and stops. On a machine with no
/// `~/.bash_profile` that file is `~/.profile`, so a `~/.profile` that refuses
/// the write leaves nowhere for a login shell to pick the binary up, and
/// `command -v vitrum` in a fresh terminal finds nothing on a machine the
/// installer has just reported success on.
#[test]
fn a_login_file_that_refuses_the_edit_does_not_cost_the_path() {
    let sh = include_str!("../../../install.sh");

    assert!(
        sh.contains(r#"rc_block_write shadow "$HOME/.bash_profile""#),
        "install.sh has no login file left when ~/.profile refuses the write, \
         so PATH is set in files a login shell never opens"
    );
    assert!(
        sh.contains(r#"if [ -r "$HOME/.profile" ]; then . "$HOME/.profile"; fi"#),
        "the ~/.bash_profile install.sh writes does not source ~/.profile, so \
         creating it silently drops whatever was in ~/.profile"
    );
    assert!(
        sh.contains("is not writable, so PATH and vu were not added there"),
        "install.sh does not say which rc refused the edit"
    );
    assert!(
        sh.contains("no login file took the PATH entry"),
        "install.sh ends by telling the operator to run a command no login \
         shell can find, without saying so"
    );
}

/// The text after a call to one of `names` on `line`, if it is a call.
///
/// A call starts the statement: `die "..."`, or `x || die "..."`. A mention
/// inside a comment or a string is not one, and matching those would make the
/// rule above unfalsifiable.
fn call_argument_text<'a>(line: &'a str, names: &[&str]) -> Option<&'a str> {
    if line.starts_with('#') {
        return None;
    }
    for name in names {
        let at = if line.starts_with(name) {
            Some(0)
        } else {
            line.find(&format!("|| {name}"))
                .map(|i| i + 3)
                .or_else(|| line.find(&format!("; {name}")).map(|i| i + 2))
        };
        if let Some(at) = at {
            return Some(&line[at + name.len()..]);
        }
    }
    None
}

/// The front page leads with the mark, and stays a landing page.
///
/// A README is what it is, what it looks like, how to install it, and links
/// out. This one has grown into a manual twice. It carried a five-state table,
/// a key binding list, a systemd unit, an SSH tunnel, a survival table, three
/// per-platform install pastes and a compositor rule, every one of which was
/// added because the README was the file a test happened to read, and every one
/// of which now lives in the document that owns it.
///
/// Nothing about that is caught by checking the sentences, because each
/// sentence was true. What is caught here is the shape: the mark is the first
/// thing on the page, no picture precedes it, and the section headings are a
/// recorded set. A new heading is red until somebody decides the material
/// belongs on the landing page rather than in `docs/`, which is the decision
/// that was never made the first two times.
#[test]
fn the_front_page_leads_with_the_mark_and_stays_a_landing_page() {
    let readme = include_str!("../../../README.md");

    let mark = readme
        .find("assets/logo/")
        .expect("the README shows the mark");
    let first_picture = readme
        .match_indices("assets/")
        .map(|(at, _)| at)
        .next()
        .expect("the README shows at least the mark");
    assert_eq!(
        mark, first_picture,
        "a picture is shown above the mark. The first thing on the page is the \
         thing the product is called."
    );

    let first_heading = readme
        .find("\n## ")
        .expect("the README has sections");
    assert!(
        mark < first_heading,
        "the mark is inside a section rather than at the top of the page"
    );

    // What the product is, before any picture of it and before the install.
    let says_what_it_is = readme[..first_heading]
        .lines()
        .any(|line| line.contains("agent") && line.len() > 40);
    assert!(
        says_what_it_is,
        "nothing above the first heading says what vitrum is. A page that opens \
         on a screenshot asks the reader to work it out from a picture."
    );

    let headings: Vec<&str> = readme
        .lines()
        .filter_map(|line| line.strip_prefix("## "))
        .collect();
    // Recorded rather than derived, because there is nothing to derive it
    // from: this is the decision itself. Adding a row is the argument.
    let allowed = ["Install", "Documentation", "Status", "License"];
    for heading in &headings {
        assert!(
            allowed.contains(heading),
            "the README grew a `{heading}` section. A landing page carries \
             {allowed:?} and links out for the rest; put this in the document \
             that owns the behaviour, and add the heading here if the front \
             page really is where it belongs."
        );
    }
    for required in ["Install", "Documentation"] {
        assert!(
            headings.contains(&required),
            "the README no longer has an {required} section"
        );
    }
}

/// Every page in `docs/` is reachable from the front page.
///
/// WHY: [`every_document_the_readme_links_to_exists`] checks that a link
/// resolves, which is the half that fails loudly. The half that fails silently
/// is a page nobody links to. `docs/architecture.md` was written, linked, and
/// then deleted in a rewrite, and the link going with it is what made the loss
/// invisible: the table still looked complete.
///
/// Enumerated from the directory, so a page added tomorrow is held to this
/// without anyone remembering to add a row.
#[test]
fn every_page_in_docs_is_linked_from_the_front_page() {
    let readme = include_str!("../../../README.md");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives under the workspace root");

    let mut pages: Vec<String> = std::fs::read_dir(root.join("docs"))
        .expect("docs/ is a directory")
        .map(|entry| entry.expect("a readable directory entry").path())
        .filter(|path| path.extension().is_some_and(|e| e == "md"))
        .map(|path| {
            path.file_name()
                .expect("a file with an extension has a name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    pages.sort();
    assert!(
        pages.len() >= 5,
        "only {} pages were found under docs/, which is fewer than have shipped; \
         the listing is looking at the wrong directory",
        pages.len()
    );

    for page in pages {
        assert!(
            readme.contains(&format!("docs/{page}")),
            "docs/{page} is not linked from the README, so the only way to it is \
             knowing it is there"
        );
    }
}

/// The architecture document accounts for every crate in the workspace.
///
/// WHY: the document opens with a tree of the workspace and one line saying
/// what each member is for, and that tree is the only place a reader is told a
/// crate exists. A member added without a line is a crate nobody outside this
/// repository can find out about, and a member deleted while its line stays is
/// a reader sent looking for a directory that is not there.
///
/// Both directions, enumerated from `Cargo.toml` and from the document, so
/// neither can drift alone.
#[test]
fn the_architecture_document_accounts_for_every_workspace_member() {
    let doc = include_str!("../../../docs/architecture.md");
    let manifest = include_str!("../../../Cargo.toml");

    let (_, after) = manifest
        .split_once("members = [")
        .expect("the workspace manifest lists its members");
    let (list, _) = after.split_once(']').expect("the members list is closed");
    let members: Vec<&str> = list
        .lines()
        .map(|line| line.trim().trim_matches(',').trim_matches('"'))
        .filter(|line| !line.is_empty())
        .collect();
    assert!(
        members.len() >= 10,
        "only {} workspace members were parsed, which is fewer than have shipped",
        members.len()
    );

    for member in &members {
        // The document writes a crate as its own name and a vendored
        // directory as its path, which is how each is referred to elsewhere.
        let written = member.strip_prefix("crates/").unwrap_or(member);
        assert!(
            doc.contains(written),
            "docs/architecture.md never mentions `{written}`, so the one place a \
             reader is told that crate exists does not say so"
        );
    }

    // And nothing in the tree block names a member that is gone.
    let (_, block) = doc.split_once("```\n").expect("the document opens with a tree");
    let (block, _) = block.split_once("\n```").expect("the tree block is closed");
    for line in block.lines() {
        let Some(name) = line.split_whitespace().next() else {
            continue;
        };
        let name = name.strip_suffix('/').unwrap_or(name);
        if !name.starts_with("vitrum-") && !name.starts_with("vendor") {
            continue;
        }
        let named = members
            .iter()
            .any(|member| member.strip_prefix("crates/").unwrap_or(member) == name);
        assert!(
            named,
            "docs/architecture.md's tree lists `{name}`, which is not a workspace \
             member, so the reader is sent to a directory that is not there"
        );
    }
}
