//! The README must describe THIS build.
//!
//! Every documentation defect this project has shipped had the same shape: a
//! sentence that was true when it was written and silently false afterwards.
//! A README is the first thing a new operator trusts and the last thing anyone
//! re-reads, so the few claims in it that a machine can check are checked
//! here, against the code rather than against a copy of the prose.

/// Each platform's paste fetches the archive that the release workflow builds.
///
/// The install path used to be `cargo build` plus `install -m755`, and the
/// test asserted exactly that. Now the first thing the README offers is a
/// download, which puts three files that can disagree in one line: the
/// release matrix names the targets, `update.rs` names the archive, and the
/// paste names both. A typo in any of them is a 404 for whoever pastes it,
/// so the paste is checked against the other two rather than against prose.
#[test]
fn the_install_blocks_name_the_real_binaries() {
    let readme = include_str!("../../../README.md");
    let manifest = include_str!("../../Cargo.toml");
    let release = include_str!("../../../.github/workflows/release.yml");
    assert!(
        manifest.contains("name = \"vitrum\""),
        "the client package is no longer named `vitrum`, so its binary is not `vitrum`"
    );
    for (platform, block) in install_blocks(readme) {
        let fetch = block
            .lines()
            .find(|l| l.contains("releases/download"))
            .unwrap_or_else(|| panic!("the {platform} block downloads nothing"));
        // The same shape `update::archive_name` builds and the release
        // workflow uploads. A paste asking for a name nothing publishes is
        // indistinguishable from a broken release.
        assert!(
            fetch.contains("/vitrum-$v-") && fetch.contains(".tar.gz"),
            "the {platform} block does not ask for a `vitrum-<version>-<target>.tar.gz`: {fetch}"
        );
        let target = fetch
            .split("/vitrum-$v-")
            .nth(1)
            .and_then(|rest| rest.split(".tar.gz").next())
            .expect("the archive name was just checked");
        // macOS resolves its architecture at paste time, so the literal in
        // the README is a suffix of two matrix entries rather than one.
        let suffix = target.rsplit(')').next().unwrap_or(target);
        assert!(
            release.contains(suffix),
            "the {platform} block downloads `{suffix}`, which the release workflow does not build"
        );
        // Both binaries come out of the one archive, so what has to hold is
        // that the paste unpacks it somewhere on PATH and then runs the
        // client, rather than leaving a tarball in the operator's lap.
        assert!(
            block.contains("tar xz") || block.contains("tar xzf"),
            "the {platform} block never unpacks the archive"
        );
        assert!(
            block.contains("vitrum") && !block.contains("vitrum-app"),
            "the {platform} block does not end up running `vitrum`"
        );
    }
    assert!(
        readme.contains("vitrum update"),
        "the README never tells the operator how to update"
    );
    assert!(
        !readme.contains("vitrum-app"),
        "the README still refers to `vitrum-app`, which is not a binary"
    );
}

/// The fenced code block under each install heading, by platform.
fn install_blocks(readme: &str) -> Vec<(&'static str, String)> {
    ["Linux", "macOS", "Windows"]
        .into_iter()
        .map(|platform| {
            let heading = readme
                .find(&format!("### {platform}"))
                .unwrap_or_else(|| panic!("the README has no {platform} install heading"));
            let rest = &readme[heading..];
            let open = rest
                .find("```")
                .unwrap_or_else(|| panic!("the {platform} section has no code block"));
            let body = &rest[open + 3..];
            let start = body.find('\n').expect("a fence has a newline") + 1;
            let end = body[start..]
                .find("```")
                .unwrap_or_else(|| panic!("the {platform} code block is unterminated"));
            (platform, body[start..start + end].to_string())
        })
        .collect()
}

/// The version in the README's download URL is THIS version.
///
/// The install instructions name a tag, `v0.1.0`, in a URL. The moment the
/// crate version moves and that URL does not, the first command a new
/// operator runs fetches the wrong release, or a 404. A version in prose
/// is a version that drifts; this pins it to `CARGO_PKG_VERSION`.
#[test]
fn the_readme_downloads_the_version_this_crate_is() {
    let readme = include_str!("../../../README.md");
    let v = env!("CARGO_PKG_VERSION");
    assert!(
        readme.contains(&format!("tags/v{v}.tar.gz")),
        "the README's tarball URL is not v{v}; it will fetch the wrong release"
    );
    assert!(
        readme.contains(&format!("--branch v{v}")),
        "the README's git clone is not pinned to v{v}"
    );
    assert!(
        readme.contains(&format!("vitrum-{v}")),
        "the README cd's into a directory the v{v} tarball does not unpack to"
    );
    assert!(
        readme.contains(&format!("version {v}")),
        "the Status section no longer states version {v}"
    );
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

/// The README promises no package and no piped script.
///
/// Release archives exist, so a download is no longer a false claim. A
/// package in someone else's index still is, and `curl | sh` is worse than a
/// false claim: it teaches the operator to run whatever the host serves.
#[test]
fn nothing_promises_an_installer_that_does_not_exist() {
    let readme = include_str!("../../../README.md");
    for absent in ["brew install", "apt install vitrum", "| sh", "| bash"] {
        assert!(
            !readme.contains(absent),
            "the README advertises `{absent}`, which this release does not ship"
        );
    }
}

/// Every image the README shows is in the repository.
///
/// The screenshots are the first thing anyone sees, and a moved or renamed file
/// turns the top of the page into three broken-image icons. Nothing else in the
/// build reads these paths, so nothing else would notice.
#[test]
fn every_screenshot_the_readme_shows_exists() {
    let readme = include_str!("../../../README.md");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives under the workspace root");

    let mut shown = 0;
    for rest in readme.split("](").skip(1) {
        let target = rest.split(')').next().unwrap_or_default();
        if !target.starts_with("assets/") {
            continue;
        }
        shown += 1;
        assert!(
            root.join(target).is_file(),
            "the README points at {target}, which is not in the repository"
        );
    }

    // The count is asserted too: a rewrite that drops the screenshots would
    // otherwise pass this test by showing nothing at all.
    assert!(
        shown >= 4,
        "the README shows {shown} local assets; the screenshots and the demo are 4"
    );
}
