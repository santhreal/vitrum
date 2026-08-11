//! Every picture in this tree is explained by a document, and no picture is
//! described through a shell.
//!
//! Two defects have shipped from this repository's images. The first was an
//! asset nobody could account for: a file added for one draft of the front
//! page, left behind when the page changed, and then never looked at again.
//! The second was the category error AGENTS.md names — a demo that shows a
//! shell session argues this is a terminal multiplexer, which is a category
//! where tmux and WezTerm already win and where nothing this product does is
//! visible.
//!
//! AGENTS.md gates both on review: "A PR that adds or changes any image
//! answers this in its body: which agents are on screen, and in which states?"
//! A review gate is a person remembering. The assets that shipped a shell row
//! and a bare prompt passed exactly that gate. What is checked here is the part
//! a machine can see:
//!
//! 1. The set of pictures is enumerated from the tree at run time, so a new one
//!    is covered the moment it lands. Nothing is listed in this file, and there
//!    is no ledger to forget to edit.
//! 2. A picture nothing points at is a defect on its own. An orphan is how the
//!    first one accumulated, and an orphan is also where a banned image hides:
//!    unreferenced, it is in the tree and in the release tarball while no
//!    reviewer ever renders it.
//! 3. Every reference carries a description, and the description is read for
//!    shell vocabulary. Alt text is prose about the picture, banned in the same
//!    breath as the picture itself, and it is the only description a screen
//!    reader gets.
//! 4. A screenshot's description answers the review question in words: which
//!    agent, and in which state.
//!
//! What this does NOT catch, stated plainly because the gap is the interesting
//! part. Nothing here decodes an image, so a description that lies passes: a
//! screenshot full of a build log, described as three agents waiting for
//! approval, is invisible to every assertion below. Reading the picture stays
//! the reviewer's job. This closes the routes that need no lie at all — adding
//! a picture and describing nothing, and describing one honestly as a shell.

use std::path::{Path, PathBuf};

/// Directories whose contents this repository does not publish as its own.
///
/// The vendored forks carry upstream's own images, which are not ours to
/// describe or to delete. A picture parked in one is out of scope here, which
/// is why the README may not point at one — see
/// [`every_reference_resolves_to_something_we_publish`]. Build output, caches
/// and scratch need no entry: the tree is what git tracks, so nothing that was
/// never committed can reach these guards.
///
/// Named per fork rather than by a `vendor` prefix, because a prefix rule
/// would silently start skipping any future directory that happens to begin
/// with it.
const SKIP: [&str; 2] = ["vendor-pty", "vendor-ghostty-vt-sys"];

/// The one host allowed to serve an image into a document.
///
/// A badge renders a number this project does not own and cannot commit. Every
/// other remote image is a demo asset that got out of the tree by being hosted
/// somewhere else, and out of the tree is out of reach of everything below.
const BADGE_HOST: &str = "img.shields.io";

/// The vocabulary that turns a demo into a picture of a terminal, and the
/// reason each word is here.
///
/// This is the single owner of the list. The tests below read it and nothing
/// else, so banning a new tool is one line here.
///
/// Four words that belong to the category are deliberately absent: `make`,
/// `top`, `screen` and `dash` are ordinary English before they are programs
/// ("makes the window transparent", "the top of the sidebar", "a second
/// screen", "an em dash"). A gate that fails on honest prose is a gate someone
/// deletes, and the compounds that do appear in a real shell demo — `cargo`,
/// `htop`, `zsh` — are on the list. That is the known hole, and it is smaller
/// than the hole a disabled test leaves.
const SHELL_VOCABULARY: [(&str, &str); 24] = [
    ("bash", "a shell: a session named for one is the banned demo exactly"),
    ("zsh", "a shell"),
    ("ksh", "a shell"),
    ("csh", "a shell"),
    ("tcsh", "a shell"),
    ("sh", "a shell, and the name a `/bin/sh` session takes in a row"),
    ("fish", "a shell"),
    ("powershell", "a shell"),
    ("pwsh", "a shell"),
    ("cmd", "a shell"),
    ("cargo", "a build tool: its output is a build log, not an agent"),
    ("npm", "a build tool"),
    ("yarn", "a build tool"),
    ("pnpm", "a build tool"),
    ("gradle", "a build tool"),
    ("maven", "a build tool"),
    ("webpack", "a build tool"),
    ("docker", "a build tool"),
    ("git", "version control: `git status` in a pane is the tmux screenshot"),
    ("ls", "a system utility, and the first thing typed in a fake demo"),
    ("cat", "a system utility"),
    ("htop", "a system utility"),
    ("df", "a system utility"),
    ("grep", "a system utility"),
];

/// The agent vocabulary a screenshot's description has to use.
///
/// AGENTS.md asks which agents are on screen. A description that names none of
/// these is not answering it, whatever else it says.
const AGENT_WORDS: [&str; 6] = ["agent", "agents", "claude", "codex", "gemini", "veyyon"];

/// The states a row can be in: the five sidebar statuses and the four
/// dispositions.
///
/// The second half of the review question is which states, and these are the
/// only words that answer it. `finished` and its friends are not here on
/// purpose: the description should use the word the row uses.
const STATE_WORDS: [&str; 9] = [
    "approval", "input", "working", "failed", "ready", "active", "woke", "snoozed", "settled",
];

fn repo() -> PathBuf {
    super::tree::root()
}

/// Does this file's content say it is a picture, whatever it is called?
///
/// Suffixes are what an author types; magic bytes are what a viewer reads. A
/// file renamed to `.dat` still renders when a document points an `<img>` at
/// it, and an extension list of the seven obvious formats already missed an
/// `.ico` that was sitting in the tree.
fn looks_like_an_image(head: &[u8]) -> bool {
    let starts = |sig: &[u8]| head.starts_with(sig);
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
    if head.len() >= 12 {
        if &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
            return true;
        }
        if &head[4..8] == b"ftyp" {
            return true;
        }
    }
    // SVG is text. Only the head is read, so a source file discussing `<svg`
    // halfway down is not mistaken for a picture.
    String::from_utf8_lossy(head).contains("<svg")
}

/// The first bytes of a file, which is all either sniff needs.
fn head_of(path: &Path) -> Vec<u8> {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut head = Vec::new();
    let _ = file.take(512).read_to_end(&mut head);
    head
}

/// Every tracked file, minus [`SKIP`], repository-relative and sorted.
///
/// Tracked rather than walked. The walk this replaces carried a list of
/// directory names to avoid, and a checkout that has built `vitrum-vt` holds a
/// `.zig-cache` of package sources whose documentation images then read as
/// pictures this repository publishes and fails to describe. What this
/// repository publishes is what it commits.
fn tree() -> Vec<String> {
    super::tree::tracked()
        .iter()
        .filter(|rel| {
            !SKIP
                .iter()
                .any(|dir| rel.starts_with(&format!("{dir}/")) || rel.contains(&format!("/{dir}/")))
        })
        .cloned()
        .collect()
}

/// Every file this repository publishes as a picture.
///
/// Two rules, because either alone leaks. Anything under `assets/` is published
/// whatever its type, which covers a format nothing here can sniff. Anything
/// anywhere whose bytes say picture is published too, which covers one parked
/// outside `assets/` under a name that does not look like an image.
fn published_images() -> Vec<String> {
    tree()
        .into_iter()
        .filter(|rel| {
            // Prose about the pictures is not a picture.
            if rel.ends_with(".md") {
                return false;
            }
            rel.starts_with("assets/") || looks_like_an_image(&head_of(&repo().join(rel)))
        })
        .collect()
}

/// Every Markdown file in the tree, as (repository-relative path, text).
fn documents() -> Vec<(String, String)> {
    tree()
        .into_iter()
        .filter(|rel| rel.ends_with(".md"))
        .filter_map(|rel| {
            let text = std::fs::read_to_string(repo().join(&rel)).ok()?;
            Some((rel, text))
        })
        .collect()
}

/// One picture, shown by one document.
struct Reference {
    /// The document, repository-relative.
    doc: String,
    /// The target exactly as written, which may be a URL.
    target: String,
    /// Where the target lands in the tree, or `None` when it is remote.
    resolved: Option<String>,
    /// The alt text: the description a screen reader is given.
    alt: String,
    /// The paragraph the picture sits in and the ones on either side, minus
    /// fenced code. A fence beside a picture is instructions to the reader, not
    /// a description of it, and `docs/states.md` legitimately shows a shell
    /// hooking `vitrum hint` a few lines from a screenshot.
    prose: String,
}

/// Every picture every document shows.
fn references() -> Vec<Reference> {
    let mut out = Vec::new();
    for (doc, text) in documents() {
        let paragraphs: Vec<&str> = text
            .split("\n\n")
            .filter(|par| !par.trim_start().starts_with("```"))
            .collect();
        for (index, par) in paragraphs.iter().enumerate() {
            let found = images_in(par);
            if found.is_empty() {
                continue;
            }
            let before = index.checked_sub(1).and_then(|i| paragraphs.get(i));
            let after = paragraphs.get(index + 1);
            let prose = [before.copied(), Some(par), after.copied()]
                .into_iter()
                .flatten()
                .collect::<Vec<&str>>()
                .join("\n");
            for (alt, target) in found {
                out.push(Reference {
                    doc: doc.clone(),
                    resolved: resolve(&doc, &target),
                    target,
                    alt,
                    prose: prose.clone(),
                });
            }
        }
    }
    out
}

/// The (alt, target) pairs in one paragraph, in both image syntaxes.
///
/// Both, because a document that may only use one of them is a document with an
/// unchecked half. The README writes HTML to centre a picture; everything else
/// writes Markdown.
fn images_in(par: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for chunk in par.split("<img").skip(1) {
        let tag = chunk.split('>').next().unwrap_or_default();
        if let Some(src) = attr(tag, "src") {
            out.push((attr(tag, "alt").unwrap_or_default(), src));
        }
    }
    for chunk in par.split("![").skip(1) {
        let Some((alt, rest)) = chunk.split_once("](") else {
            continue;
        };
        let Some(target) = rest.split(')').next() else {
            continue;
        };
        out.push((alt.to_string(), target.trim().to_string()));
    }
    out
}

/// The value of a double-quoted attribute in an HTML tag.
fn attr(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let rest = tag.split(&needle).nth(1)?;
    Some(rest.split('"').next().unwrap_or_default().to_string())
}

/// Where a target written inside `doc` lands, repository-relative.
///
/// `None` for a remote target, which is checked separately. `..` is resolved
/// rather than left in the string, so `docs/x.md` pointing at
/// `../assets/logo/vitrum.svg` is recognised as the same file the README shows
/// as `assets/logo/vitrum.svg`.
fn resolve(doc: &str, target: &str) -> Option<String> {
    if target.starts_with("http://") || target.starts_with("https://") || target.starts_with("//") {
        return None;
    }
    let target = target.split(['#', '?']).next().unwrap_or(target);
    let mut parts: Vec<&str> = doc.split('/').collect();
    parts.pop();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// The words in a piece of prose, lowercased.
fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
}

/// The first banned word in `text`, with the reason it is banned.
fn shell_vocabulary_in(text: &str) -> Option<(&'static str, &'static str)> {
    words(text).find_map(|word| {
        SHELL_VOCABULARY
            .iter()
            .find(|(banned, _)| *banned == word)
            .copied()
    })
}

/// Does `text` contain a bare prompt?
///
/// A prompt is punctuation, so no word list sees it, and it is the single most
/// recognisable mark of the picture AGENTS.md bans: a `$` with a command after
/// it says shell before any word does.
fn bare_prompt(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim_start();
        ["$ ", "% ", "# $", "❯ ", "➜ ", "PS>"]
            .iter()
            .any(|mark| line.starts_with(mark))
            || line.contains("@localhost:")
            || line.contains(":~$")
            || line.contains(":~#")
    })
}

/// Every picture in the tree is shown by a document that describes it.
///
/// Enumerated from the tree, so this is not a list anyone can forget to add to:
/// a picture that lands with nothing pointing at it turns the suite red on the
/// commit that adds it. That is the whole point. An orphan is both a file
/// nobody can account for and the one place a banned image survives review,
/// because no reviewer ever renders a file no page shows.
///
/// The description has to be more than a label. Five words is not a high bar;
/// it is the bar below which "screenshot" and "hero" live, and those describe
/// nothing to a reader who cannot see the picture.
#[test]
fn every_picture_in_the_tree_is_explained_by_a_document() {
    let images = published_images();
    assert!(
        images.len() >= 5,
        "the walker found {} pictures, which is fewer than the assets known to \
         be in this tree; it is skipping something it should read",
        images.len()
    );
    let references = references();

    for image in &images {
        let shown: Vec<&Reference> = references
            .iter()
            .filter(|reference| reference.resolved.as_deref() == Some(image.as_str()))
            .collect();
        assert!(
            !shown.is_empty(),
            "{image} is in the tree and no document shows it. An asset nobody \
             points at is an asset nobody looks at again: reference it from a \
             document with a description, or delete it."
        );
        assert!(
            shown.iter().any(|reference| words(&reference.alt).count() >= 5),
            "{image} is shown by {} with no real description. Alt text is the \
             only account of the picture a reader who cannot see it gets, and \
             it is what the review question is answered in.",
            shown[0].doc
        );
    }
}

/// A screenshot's description names an agent and a state.
///
/// This is AGENTS.md's review question — which agents are on screen, and in
/// which states — asked of the text instead of the reviewer. It cannot tell
/// whether the answer is true, and it can tell that one was given. The two
/// vocabularies come from the product: [`AGENT_WORDS`] and the nine states in
/// [`STATE_WORDS`], so a screenshot described as "the main window" fails and a
/// screenshot described as "codex waiting for approval" passes.
#[test]
fn every_screenshot_says_which_agent_and_which_state() {
    let references = references();
    let mut checked = 0;

    for image in published_images() {
        if !image.starts_with("assets/screenshots/") {
            continue;
        }
        checked += 1;
        let described = references
            .iter()
            .filter(|reference| reference.resolved.as_deref() == Some(image.as_str()))
            .any(|reference| {
                let mut said_agent = false;
                let mut said_state = false;
                for word in words(&reference.alt) {
                    said_agent |= AGENT_WORDS.contains(&word.as_str());
                    said_state |= STATE_WORDS.contains(&word.as_str());
                }
                said_agent && said_state
            });
        assert!(
            described,
            "no description of {image} says both which agent is on screen and \
             which state it is in. The states are {STATE_WORDS:?}."
        );
    }

    assert!(
        checked >= 4,
        "only {checked} screenshots were read, so this gate is passing over the \
         set it exists for"
    );
}

/// No picture is described through a shell.
///
/// Read over the alt text and the prose around every picture in every Markdown
/// file, because AGENTS.md bans "prose or alt text that describes the product
/// through a shell task" in the same list as the picture. A caption saying a
/// pane is running `cargo test` makes the same argument the screenshot would,
/// and it makes it to a search engine as well as to a reader.
#[test]
fn no_picture_is_described_through_a_shell() {
    let references = references();
    let mut checked = 0;

    for reference in &references {
        checked += 1;
        let Reference {
            doc, target, alt, ..
        } = reference;
        if let Some((word, why)) = shell_vocabulary_in(alt) {
            panic!(
                "the alt text for {target} in {doc} names {word:?}: {why}. \
                 If the same picture could have been taken in tmux, it does \
                 not ship."
            );
        }
        if let Some((word, why)) = shell_vocabulary_in(&reference.prose) {
            panic!(
                "the prose around {target} in {doc} names {word:?}: {why}. \
                 Describe the agents on screen and the states they are in."
            );
        }
        assert!(
            !bare_prompt(alt) && !bare_prompt(&reference.prose),
            "the description of {target} in {doc} shows a bare prompt, which \
             is the picture this product must never argue it is"
        );
    }

    assert!(
        checked >= 6,
        "only {checked} picture references were read across every Markdown \
         file, so this gate is not looking at the front page"
    );
}

/// Every reference resolves to something this repository publishes.
///
/// Two escapes meet here. A remote `<img>` puts the banned picture at the top
/// of the page with the tree still clean, so only badges may be remote. A local
/// one pointed into a vendored fork does the same from inside the repository:
/// a fork is skipped precisely because its contents are not ours to
/// describe, which made it the one directory a picture could sit in undeclared
/// and still render on the front page. Both land as "the target is not a
/// picture this walker enumerates".
#[test]
fn every_reference_resolves_to_something_we_publish() {
    let images = published_images();
    for reference in references() {
        let Some(resolved) = &reference.resolved else {
            assert!(
                reference.target.contains(BADGE_HOST),
                "{} shows {}, which is served by someone else. A demo asset \
                 lives in assets/ and is described in a document here; only \
                 {BADGE_HOST} badges are remote.",
                reference.doc,
                reference.target
            );
            continue;
        };
        assert!(
            images.contains(resolved),
            "{} shows {}, which is not a picture this repository publishes. A \
             dead path renders as a broken image, and a live one under a \
             skipped directory renders a picture nothing here can check.",
            reference.doc,
            reference.target
        );
    }
}

/// The detector fires on the pictures this gate exists to stop.
///
/// Every case below is an asset that actually shipped from this repository or
/// a one-word variant of one. Without this, the two tests above pass on a
/// detector that matches nothing at all, which is the failure mode of every
/// word-list gate.
#[test]
fn the_detector_catches_the_demos_that_shipped() {
    for banned in [
        "a bash session building the project",
        "zsh in the second pane",
        "the output of cargo test in a pane",
        "git log scrolling in the terminal",
        "htop beside the sidebar",
        "an ls of the project directory",
        "running npm run dev",
        "a /bin/sh row in the sidebar",
    ] {
        assert!(
            shell_vocabulary_in(banned).is_some(),
            "{banned:?} is the banned demo and the detector reads it as clean"
        );
    }

    for prompt in ["$ cd ~/src/pathfinder", "  % make", "mk@localhost:/src", "user:~$ "] {
        assert!(bare_prompt(prompt), "{prompt:?} is a prompt and reads as prose");
    }

    // And it is quiet on the descriptions this product is actually made of.
    for honest in [
        "codex waiting for approval in the pathfinder project",
        "three agents working, one snoozed with a countdown, one failed",
        "the launcher over a sidebar of ready and failed rows",
        "settings, Appearance tab, over a populated sidebar",
    ] {
        assert!(
            shell_vocabulary_in(honest).is_none(),
            "{honest:?} describes agents and the detector refuses it; a gate \
             that fails on honest prose is a gate someone deletes"
        );
        assert!(!bare_prompt(honest), "{honest:?} is prose, not a prompt");
    }
}

/// A target is resolved against the document that writes it.
///
/// The same picture is written three ways across this tree, and a resolver that
/// got any of them wrong would report a real reference as an orphan and send
/// someone to delete the asset.
#[test]
fn a_target_resolves_against_the_document_that_writes_it() {
    assert_eq!(
        resolve("README.md", "assets/screenshots/launcher.png").as_deref(),
        Some("assets/screenshots/launcher.png")
    );
    assert_eq!(
        resolve("docs/appearance.md", "../assets/logo/vitrum.svg").as_deref(),
        Some("assets/logo/vitrum.svg")
    );
    assert_eq!(
        resolve("docs/states.md", "./../assets/screenshots/launcher.png#x").as_deref(),
        Some("assets/screenshots/launcher.png")
    );
    assert_eq!(resolve("README.md", "https://img.shields.io/badge/x"), None);
}
