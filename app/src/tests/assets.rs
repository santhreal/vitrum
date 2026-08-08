//! Every image this repository publishes is declared, pinned to its bytes, and
//! described in terms of the agents on screen.
//!
//! AGENTS.md gates this on review: "A PR that adds or changes any image answers
//! this in its body: which agents are on screen, and in which states?" A review
//! gate is a person remembering. The assets that shipped a `bash #6` row, a bare
//! `$` prompt and a `make - fleet shards` session got past exactly that gate,
//! and then sat in the tree unreferenced, where nothing looked at them again.
//!
//! [`crate::fixture`] closes the case where the picture came from the fixture: a
//! shell row cannot be built there any more. This module closes the routes that
//! bypass it. Each test below exists because the obvious version of this gate
//! did not stop the corresponding move:
//!
//! 1. Add a picture and describe nothing.
//! 2. Leave the ledger alone and overwrite a declared file's BYTES. A
//!    regenerated `hero.png` full of `cargo test` needs no ledger edit at all,
//!    so the declaration is pinned to a digest.
//! 3. Use a suffix the walker does not know. This was not hypothetical:
//!    `assets/logo/vitrum.ico` was already in the tree and an extension list of
//!    the seven obvious formats did not see it. Files are found by where they
//!    live and by their magic bytes instead.
//! 4. Host it somewhere else and hotlink it, leaving the tree clean.
//! 5. Put the shell in the alt text, which AGENTS.md bans in the same breath as
//!    the picture and which no test read.
//! 6. Park it in a directory the walker skips and point the README at it.
//!    `vendor/` is skipped precisely because its contents are not ours to
//!    describe, which made it the one place a picture could sit undeclared and
//!    still render at the top of the page. The README may now only show a path
//!    the ledger lists, in either image syntax.
//!
//! Those six are not a list of things that seemed possible. Each was carried
//! out against this suite, and each one passed until the test beside it
//! existed.
//!
//! What this does NOT catch, stated plainly because the gap is the interesting
//! part: a declaration that lies. `hero.png` could show a shell session and be
//! declared as four agents, and nothing here would know, because nothing here
//! decodes an image. The digest narrows even that: the lie has to be told in
//! the same commit that changes the bytes, in a diff, next to the picture.
//! Reading the image is the reviewer's job, and it is the only part of this
//! that stayed a review gate.
//!
//! The digest is FNV-1a, not a cryptographic hash. The threat modelled here is
//! a regenerated screenshot that nobody looked at, not an adversary crafting a
//! collision; someone willing to do that can also just write a false
//! description.

use crate::agent::AgentKind;
use std::path::{Path, PathBuf};

/// The declarations, compiled in so a missing ledger is a build error rather
/// than a test that quietly passes over an empty list.
const LEDGER: &str = include_str!("../../../assets/CONTENTS.md");

const README: &str = include_str!("../../../README.md");

/// Build tools, which are not shells and so are invisible to [`AgentKind`].
///
/// The shell half of this vocabulary is derived from `agent.rs` rather than
/// repeated here, so adding a shell to its `SHELLS` table bans it here too.
/// There is no equivalent table for build tools to read.
const BUILD_TOOLS: [&str; 10] = [
    "cargo", "make", "npm", "yarn", "docker", "gradle", "maven", "webpack", "htop", "tmux",
];

/// Directories whose contents this repository does not publish as its own.
const SKIP: [&str; 5] = [".git", "target", "vendor", ".internal", "node_modules"];

/// The one host allowed to serve an image into the README.
///
/// Badges are status, not demonstration: they render a number this project does
/// not own and cannot commit. Every other remote image is a demo asset that
/// escaped the ledger by being hosted somewhere else.
const BADGE_HOST: &str = "img.shields.io";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the app crate has a parent directory")
        .to_path_buf()
}

/// FNV-1a over a file's bytes, rendered as the ledger writes it.
fn digest(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Does this file's content say it is a picture, whatever it is called?
///
/// Suffixes are what an author types; magic bytes are what a viewer reads. A
/// file renamed to `.dat` still renders when a README points an `<img>` at it.
fn looks_like_an_image(bytes: &[u8]) -> bool {
    let starts = |sig: &[u8]| bytes.starts_with(sig);
    if starts(b"\x89PNG\r\n\x1a\n")           // png
        || starts(b"\xff\xd8\xff")            // jpeg
        || starts(b"GIF87a")
        || starts(b"GIF89a")
        || starts(b"\x00\x00\x01\x00")        // ico
        || starts(b"BM")                      // bmp
        || starts(b"II*\x00")                 // tiff, little endian
        || starts(b"MM\x00*")                 // tiff, big endian
    {
        return true;
    }
    // RIFF....WEBP, and the ISO base media family: mp4, mov, avif, heic.
    if bytes.len() >= 12 {
        if &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            return true;
        }
        if &bytes[4..8] == b"ftyp" {
            return true;
        }
    }
    // SVG is text. Look only at the head, so a stray `<svg` deep inside a
    // source file discussing SVG is not mistaken for a picture.
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    head.contains("<svg")
}

/// Every file this repository publishes as a picture.
///
/// Two rules, because either alone leaks. Anything under `assets/` is published
/// whatever its type, which catches a format nothing here can sniff. Anything
/// anywhere whose bytes say picture is published too, which catches one parked
/// outside `assets/` under a name that does not look like an image.
fn published_images() -> Vec<String> {
    let root = repo();
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out.sort();
    out.dedup();
    out
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if path.is_dir() {
            if !SKIP.contains(&name.as_str()) {
                walk(&path, root, out);
            }
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        // Prose about the assets is not an asset.
        if rel.ends_with(".md") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if rel.starts_with("assets/") || looks_like_an_image(&bytes) {
            out.push(rel);
        }
    }
}

/// The ledger's claims, as (path, digest, what it shows).
fn declarations() -> Vec<(&'static str, &'static str, &'static str)> {
    LEDGER
        .lines()
        .filter_map(|line| line.strip_prefix("- "))
        .filter_map(|line| line.split_once(": "))
        .filter_map(|(head, shows)| {
            let (path, hash) = head.split_once(" @ ")?;
            Some((path.trim(), hash.trim(), shows.trim()))
        })
        .collect()
}

/// Split a claim into words a command name could hide in.
fn words(claim: &str) -> Vec<String> {
    claim
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| word.to_ascii_lowercase())
        .collect()
}

/// An image in the tree with no line in the ledger fails, by default.
///
/// This is the direction that matters. A reviewer who forgets the question, or
/// never saw AGENTS.md, still cannot land a picture nobody described.
#[test]
fn every_published_image_is_declared() {
    let declared: Vec<&str> = declarations().into_iter().map(|(path, ..)| path).collect();
    let found = published_images();

    for image in &found {
        assert!(
            declared.contains(&image.as_str()),
            "{image} is in the tree with no line in assets/CONTENTS.md. Add one \
             saying which agents are on screen and in which states, or delete \
             the file. See AGENTS.md, \"Demos show agents, not shell output\"."
        );
    }

    // Liveness. A walker that silently found nothing would pass every assertion
    // above it, which is the failure this floor exists to catch.
    assert!(
        found.len() >= 12,
        "only {} images were found; the walker is not reaching the tree",
        found.len()
    );
}

/// Changing what a picture shows changes its digest.
///
/// Without this the ledger only guards the file NAME, and the cheapest way to
/// put a shell back on the front page is to regenerate `hero.png` in place and
/// touch nothing else. A stale digest is not a nit: it means the description
/// above it was written about a different picture.
#[test]
fn every_declared_image_still_has_the_bytes_it_was_described_with() {
    let root = repo();
    for (path, expected, shows) in declarations() {
        let Ok(bytes) = std::fs::read(root.join(path)) else {
            continue; // absence is the next test's failure, not this one's
        };
        let actual = digest(&bytes);
        assert_eq!(
            actual, expected,
            "{path} is no longer the file that was described as {shows:?}. \
             Look at the new picture, say what is in it, and put {actual} in \
             assets/CONTENTS.md."
        );
    }
}

/// A ledger line whose file is gone is a claim about nothing.
///
/// Deleting an image and leaving its description behind is how the list drifts
/// into fiction, and a stale line makes the count above look healthy.
#[test]
fn every_declaration_describes_a_file_that_exists() {
    let root = repo();
    for (path, _, _) in declarations() {
        assert!(
            root.join(path).is_file(),
            "assets/CONTENTS.md describes {path}, which is not in the repository"
        );
    }
}

/// A screenshot has to name an agent, because that is the product.
///
/// The vocabulary is resolved through the same [`AgentKind::of`] the tab strip
/// paints with, so the accepted names are exactly the agents this build can
/// launch. Adding one to `agent.rs` widens this automatically; a claim that
/// names none of them is describing something that is not vitrum's subject.
#[test]
fn every_screenshot_declaration_names_an_agent() {
    for (path, _, shows) in declarations() {
        if shows.starts_with("brand mark") {
            continue;
        }
        let named = words(shows)
            .iter()
            .any(|word| !matches!(AgentKind::of(word), AgentKind::Shell | AgentKind::Unknown));
        assert!(
            named,
            "{path} is declared as {shows:?}, which names no agent this build \
             can launch. A picture whose subject is not an agent belongs in \
             tmux's category, not on this page."
        );
    }
}

/// No claim may describe a shell or a build tool, whatever the file is.
///
/// The shell half derives from `agent.rs`: every name in its `SHELLS` table
/// resolves to [`AgentKind::Shell`], so this covers all fifteen and grows with
/// them. A brand mark is checked too, because "the icon next to the bash tab"
/// describes a banned picture just as plainly as a screenshot of one.
#[test]
fn no_declaration_describes_a_shell_or_a_build_tool() {
    for (path, _, shows) in declarations() {
        for word in words(shows) {
            assert!(
                AgentKind::of(&word) != AgentKind::Shell,
                "{path} is declared as {shows:?}, and {word:?} is a shell; the \
                 same picture could have been taken in tmux"
            );
            assert!(
                !BUILD_TOOLS.contains(&word.as_str()),
                "{path} is declared as {shows:?}, and {word:?} is a build tool; \
                 a build log is not what this product does"
            );
        }
    }
}

/// Every picture on the front page is one this repository declares.
///
/// Two ways past the ledger meet here. A remote `<img>` puts the banned picture
/// at the top of the page with the tree still clean. A local one pointed at a
/// directory the walker skips does the same thing from inside the repository:
/// `vendor/` is not ours to declare, which is exactly why an asset hidden there
/// was found by nothing. So the README may only show a path the ledger lists.
#[test]
fn every_picture_the_readme_shows_is_one_this_repository_declares() {
    let declared: Vec<&str> = declarations().into_iter().map(|(path, ..)| path).collect();
    let html = README
        .split("src=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next());
    let markdown = README
        .split("![")
        .skip(1)
        .filter_map(|rest| rest.split_once("]("))
        .filter_map(|(_, rest)| rest.split(')').next());

    let mut local = 0;
    for target in html.chain(markdown) {
        if target.starts_with("http://") || target.starts_with("https://") {
            assert!(
                target.contains(BADGE_HOST),
                "the README shows {target}, which is served by someone else. A \
                 demo asset lives in assets/ and is declared in \
                 assets/CONTENTS.md; only {BADGE_HOST} badges are remote."
            );
            continue;
        }
        local += 1;
        assert!(
            declared.contains(&target),
            "the README shows {target}, which no line in assets/CONTENTS.md \
             describes. A picture in a directory the asset walker skips is \
             still a picture on the front page."
        );
    }

    assert!(local >= 2, "only {local} local pictures were read from the README");
}

/// Alt text is prose about the picture, and AGENTS.md bans it separately.
///
/// "prose or alt text that describes the product through a shell task" is in
/// the same list as the picture itself. Alt text is also the only description a
/// screen reader gets, so a shell in it is not a smaller version of the problem
/// than a shell in the image.
#[test]
fn no_alt_text_describes_a_shell_or_a_build_tool() {
    let mut checked = 0;
    for rest in README.split("alt=\"").skip(1) {
        let alt = rest.split('"').next().unwrap_or_default();
        checked += 1;
        for word in words(alt) {
            assert!(
                AgentKind::of(&word) != AgentKind::Shell,
                "README alt text {alt:?} names the shell {word:?}"
            );
            assert!(
                !BUILD_TOOLS.contains(&word.as_str()),
                "README alt text {alt:?} names the build tool {word:?}"
            );
        }
    }
    assert!(checked >= 5, "only {checked} alt attributes were read");
}
