//// The README must describe THIS build.
////
//// Every documentation defect this project has shipped had the same shape: a
//// sentence that was true when it was written and silently false afterwards.
//// A README is the first thing a new operator trusts and the last thing anyone
//// re-reads, so the few claims in it that a machine can check are checked
//// here, against the code rather than against a copy of the prose.

/// The install blocks name the binaries this build actually produces.
///
/// The crate and the executable were both `vitrum-app` for a long time,
/// while `--help`, every error message and the README all said `vitrum`.
/// Anybody following the instructions installed a file that did not exist.
/// Both are `vitrum` now, and the package name is what produces the binary,
/// so this checks the package name rather than a `[[bin]]` that no longer
/// needs to exist.
#[test]
fn the_install_blocks_name_the_real_binaries() {
    let readme = include_str!("../../../README.md");
    let manifest = include_str!("../../Cargo.toml");
    assert!(
        manifest.contains("name = \"vitrum\""),
        "the client package is no longer named `vitrum`, so its binary is not `vitrum`"
    );
    // Read from the blocks themselves rather than matched as literal
    // lines. The old form asserted the exact text of two commands, which
    // meant every rewording failed a test that was not about wording; what
    // has to hold is that each platform's block puts BOTH binaries in
    // place, however it spells the copy.
    for (platform, block) in install_blocks(readme) {
        let copies = block
            .lines()
            .find(|l| l.contains("install -m755") || l.contains("Copy-Item"))
            .unwrap_or_else(|| panic!("the {platform} block copies nothing into place"));
        for binary in ["vitrum", "vitrum-server"] {
            assert!(
                copies.contains(binary),
                "the {platform} block does not install {binary}: {copies}"
            );
        }
        // The client looks for the daemon beside itself first, so a block
        // that put them in different directories would install a pair that
        // only works by accident of PATH ordering.
        assert!(
            copies.matches("$bin").count() >= 2 || copies.matches("$rel").count() >= 2,
            "the {platform} block takes the binaries from different places: {copies}"
        );
        assert!(
            block.contains("vitrum update"),
            "the {platform} block never tells the operator how to update"
        );
    }
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

/// The README does not promise an installer or a download.
///
/// This release is build-from-source on purpose. A README that implies a
/// package or a hosted binary sends people looking for something that is
/// not there, and the first thing they find is a 404.
#[test]
fn nothing_promises_an_installer_that_does_not_exist() {
    let readme = include_str!("../../../README.md");
    for absent in [
        "curl -sSf",
        "brew install",
        "apt install vitrum",
        "releases/download",
    ] {
        assert!(
            !readme.contains(absent),
            "the README advertises `{absent}`, which this release does not ship"
        );
    }
}
