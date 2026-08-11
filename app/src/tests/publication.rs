//! What must never appear in a file this repository publishes.
//!
//! This repository is public and its history is already pushed. Deleting a
//! string from `HEAD` does not unpublish it: it stays readable at every prior
//! commit, and on a forge it also survives in the pull request refs, which a
//! force push cannot rewrite and branch deletion does not remove. The only
//! affordable moment to catch one of these strings is before it is committed,
//! so the checks are here rather than in a review checklist.
//!
//! Six classes are refused:
//!
//! 1. A path rooted in a real machine's storage: a removable mount, a volume,
//!    or a home directory belonging to a name nobody recorded as synthetic.
//! 2. A build target directory written into a tracked file. The host's cargo
//!    configuration owns that directory. A file that overrides it points at
//!    whatever disk the author happened to have.
//! 3. A private network address, which names a machine on somebody's LAN.
//! 4. A credential: an API token, a forge token, or a private key block.
//! 5. Profanity, including inside a quotation kept for accuracy.
//! 6. A conversational attribution: prose that sources a requirement to a
//!    person in a conversation rather than stating it as a product fact.
//!
//! Three of these carry an allowlist rather than a flat ban, because the tree
//! legitimately contains near-misses: synthetic home directories in path
//! tests, a documentation address in the settings round trip. Each allowlist
//! is compared against what the tree actually holds, so adding a new member
//! turns this suite red until somebody records it here and, in recording it,
//! looks at whether it is synthetic. A guard whose allowlist is only consulted
//! and never audited goes stale in silence, which is the same failure as
//! having no guard.
//!
//! ## What this deliberately does not catch
//!
//! - **Anything already committed.** These checks read the working tree. A
//!   string that reached a previous commit is out of reach of any test.
//! - **A leak inside an image.** A directory field, a title bar or a terminal
//!   pane in a screenshot is pixels, and no check here reads pixels. Read
//!   every visible string in a picture before committing it.
//! - **A real path that is structurally indistinguishable from a synthetic
//!   one.** `/home/ada` and `/src/vitrum` are accepted, and a machine whose
//!   user is `ada` with a checkout at `/src/vitrum` leaks nothing a reader can
//!   act on. The class being closed is a path that identifies a machine, not
//!   the character `/`.
//! - **A paraphrased instruction with no attribution wrapper.** Only the
//!   wrapper is mechanically detectable. Prose that reproduces a conversation
//!   without naming a person reads as ordinary product prose.
//! - **A transcript used as a fixture.** A synthesised transcript and a real
//!   one have the same shape. The guard on that is not committing one.
//! - **Its own source.** This file names every banned string, so it is skipped
//!   by name. That skip is exactly one path.

use std::collections::BTreeSet;

/// This file, repository-relative. Skipped: it holds every banned literal.
const SELF: &str = "app/src/tests/publication.rs";

/// Home directory names that appear in this tree and are synthetic.
///
/// Every one is a placeholder in a path-formatting or path-detection test. A
/// name not on this list is refused, whether or not it belongs to anybody.
const SYNTHETIC_HOMES: &[&str] = &[
    "...", "1", "MK", "Some", "a", "ada", "dev", "m", "me", "mk", "mk2", "mkother", "op", "other",
    "someone", "u", "user", "x", "you",
];

/// Private IPv4 addresses this tree is allowed to contain.
///
/// `127.0.0.1` is the daemon's own loopback and is the product's behaviour,
/// not a machine. `10.0.0.4` is the worked example in the remote-access
/// settings. Any other private address names somebody's host.
const DOCUMENTED_ADDRESSES: &[&str] = &["127.0.0.1", "10.0.0.4"];

/// Mount points that only exist on a specific machine.
const MOUNT_ROOTS: &[&str] = &["/media/", "/mnt/", "/Volumes/", "/run/media/", "/cygdrive/"];

/// Ways a file can name a build target directory.
const TARGET_DIR_OVERRIDES: &[&str] = &["--target-dir", "CARGO_TARGET_DIR="];

/// Credential shapes. Each is a prefix followed by enough body to be real.
const TOKEN_PREFIXES: &[(&str, usize)] = &[
    ("ghp_", 20),
    ("gho_", 20),
    ("ghs_", 20),
    ("github_pat_", 20),
    ("glpat-", 20),
    ("xoxb-", 20),
    ("xoxp-", 20),
    ("AKIA", 16),
    ("ASIA", 16),
    ("AIza", 35),
];

/// A private key block, however the surrounding format wraps it.
const KEY_BLOCK: &str = "PRIVATE KEY-----";

/// Words that do not appear in a file this project publishes.
///
/// Matched on whole words, case-insensitively, so `Scunthorpe`, `classic` and
/// a variable named `assign` are untouched.
const PROFANITY: &[&str] = &[
    "arse",
    "arsehole",
    "ass",
    "asshole",
    "bastard",
    "bitch",
    "bollocks",
    "bullshit",
    "crap",
    "cunt",
    "damn",
    "dick",
    "dickhead",
    "dumbass",
    "fuck",
    "fucked",
    "fucking",
    "goddamn",
    "piss",
    "pissed",
    "shit",
    "shitty",
    "twat",
    "wank",
    "wanker",
];

/// Phrases that source a requirement to a person in a conversation.
///
/// Narrow on purpose. "the operator asked for" and "the operator wanted" are
/// how this product describes a person acting on its own interface, and are
/// not here. What is here is prose that only makes sense if the reader knows
/// a conversation happened: a report, a complaint, a quotation.
const ATTRIBUTION: &[&str] = &[
    "the user reported",
    "the user complained",
    "the user said",
    "the user rejected",
    "the user's complaint",
    "the user's message",
    "the user's request",
    "the operator reported",
    "the operator complained",
    "the operator said",
    "the operator rejected",
    "the operator's complaint",
    "the operator's message",
    "the operator's request",
    "quoted the operator",
    "quoted the user",
    "quoted verbatim",
    "as the user asked",
    "as the operator asked",
    "per the user",
    "per the operator",
];

/// Filenames that carry internal process material rather than product content.
///
/// Each of these existed here once and was removed. A tracked file with one of
/// these names, or any tracked file under `.internal/` or `docs/reviews/`, is
/// process material that a reader outside this project cannot use.
const INTERNAL_NAMES: &[&str] = &["GOAL.md", "SPEC.md", "BACKLOG.md", "WORKFLOW.md"];

/// Directories whose whole contents are internal.
const INTERNAL_DIRS: &[&str] = &[".internal/", "docs/reviews/"];

/// Every tracked file that is UTF-8, as `(path, contents)`.
///
/// A file that is not UTF-8 is not prose and carries no string a reader can
/// find. `Cargo.lock` and every image fall out here.
fn readable() -> Vec<(&'static str, String)> {
    let root = super::tree::root();
    super::tree::tracked()
        .iter()
        .filter(|rel| rel.as_str() != SELF)
        .filter_map(|rel| {
            std::fs::read(root.join(rel))
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .map(|text| (rel.as_str(), text))
        })
        .collect()
}

/// `path`, `line number`, `the line` for every line matching `hit`.
fn scan(hit: impl Fn(&str) -> bool) -> Vec<String> {
    readable()
        .iter()
        .flat_map(|(rel, text)| {
            text.lines()
                .enumerate()
                .filter(|(_, line)| hit(line))
                .map(|(i, line)| format!("{rel}:{}: {}", i + 1, line.trim()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The home directory name in `rest`, which begins just past a home root.
fn home_name(rest: &str) -> &str {
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'))
        .unwrap_or(rest.len());
    &rest[..end]
}

/// Every home directory name this tree names, at any home root.
fn home_names() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for (_, text) in readable() {
        for root in ["/home/", "/Users/", "C:\\Users\\", "C:\\\\Users\\\\"] {
            let mut from = 0;
            while let Some(at) = text[from..].find(root) {
                let start = from + at + root.len();
                let name = home_name(&text[start..]);
                if !name.is_empty() {
                    found.insert(name.to_string());
                }
                from = start.max(from + at + 1);
            }
        }
    }
    found
}

/// Every dotted quad in `text` that is in a private range.
fn private_addresses(text: &str) -> BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut found = BTreeSet::new();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() || (i > 0 && !is_boundary(bytes[i - 1])) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        // Sentence punctuation: `192.168.0.1.` is four octets and a full stop.
        let word = text[start..i].trim_end_matches('.');
        if i < bytes.len() && !is_boundary(bytes[i]) {
            continue;
        }
        let parts: Vec<&str> = word.split('.').collect();
        if parts.len() != 4 || parts.iter().any(|p| p.is_empty() || p.len() > 3) {
            continue;
        }
        let Ok(octets) = parts
            .iter()
            .map(|p| p.parse::<u16>())
            .collect::<Result<Vec<_>, _>>()
        else {
            continue;
        };
        if octets.iter().any(|o| *o > 255) {
            continue;
        }
        let private = match (octets[0], octets[1]) {
            (10, _) | (127, _) => true,
            (192, 168) => true,
            (172, b) => (16..=31).contains(&b),
            _ => false,
        };
        if private {
            found.insert(word.to_string());
        }
    }
    found
}

fn is_boundary(b: u8) -> bool {
    !(b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

/// Whole-word, case-insensitive containment.
fn has_word(line: &str, word: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(at) = lower[from..].find(word) {
        let start = from + at;
        let end = start + word.len();
        let before_ok = start == 0 || !is_word_byte(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_word_byte(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// No tracked file names a mount that exists on one machine.
///
/// WHY: a removable mount or a volume is the single clearest statement of
/// where the author keeps their work, and unlike a home directory there is no
/// synthetic form of one that this project needs. The published tree uses
/// `~/src/<project>` or `/src/<project>` and never a mount.
#[test]
fn no_tracked_file_names_a_mount() {
    let hits = scan(|line| MOUNT_ROOTS.iter().any(|root| line.contains(root)));
    assert!(
        hits.is_empty(),
        "a mount point names the machine that produced the file. Use \
         /src/<project>, ~/src/<project> or /opt/<name>:\n{}",
        hits.join("\n")
    );
}

/// Every home directory this tree names is one somebody recorded as synthetic.
///
/// WHY: the failure mode is a real `$HOME` copied into a test fixture, a doc
/// example or an error message, which names the account that produced it. The
/// allowlist is the audit: a name reaching the tree without an entry here
/// fails, and adding the entry is the moment to decide whether the name is a
/// placeholder or somebody's login.
#[test]
fn every_home_directory_named_here_is_synthetic() {
    let allowed: BTreeSet<&str> = SYNTHETIC_HOMES.iter().copied().collect();
    let found = home_names();
    let unrecorded: Vec<&String> = found
        .iter()
        .filter(|n| !allowed.contains(n.as_str()))
        .collect();
    assert!(
        unrecorded.is_empty(),
        "these home directory names are in tracked files and are not recorded \
         as synthetic: {unrecorded:?}. If one is a placeholder, add it to \
         SYNTHETIC_HOMES. If it is somebody's login, it does not ship."
    );
}

/// The synthetic-home list describes this tree and nothing more.
///
/// WHY: an allowlist that outlives its entries stops being an audit. A name
/// that no longer appears has to come off the list, so the list can never
/// silently pre-approve a name that later turns out to be real.
#[test]
fn the_synthetic_home_list_has_no_dead_entries() {
    let found = home_names();
    let dead: Vec<&&str> = SYNTHETIC_HOMES
        .iter()
        .filter(|n| !found.contains(**n))
        .collect();
    assert!(
        dead.is_empty(),
        "SYNTHETIC_HOMES records names no tracked file uses: {dead:?}. \
         Remove them, so the list stays an audit of what is here."
    );
}

/// No tracked file overrides the build target directory.
///
/// WHY: the target directory belongs to the host's cargo configuration. A
/// script, doc or workflow that sets it writes one machine's disk layout into
/// the tree, and a scratch target directory additionally points a build at a
/// filesystem that may be a fraction of the size the build needs.
#[test]
fn no_tracked_file_sets_a_build_target_directory() {
    let hits = scan(|line| {
        TARGET_DIR_OVERRIDES
            .iter()
            .any(|pattern| line.contains(pattern))
            && !line.contains("`CARGO_TARGET_DIR`")
    });
    assert!(
        hits.is_empty(),
        "the host's cargo configuration owns the target directory:\n{}",
        hits.join("\n")
    );
}

/// Every private address in this tree is one the docs mean to publish.
///
/// WHY: a LAN address is a machine on somebody's network, and in a measurement
/// harness it also discloses what that network holds. Loopback is exempt as a
/// range rather than as two literals: 127/8 is this machine whoever reads the
/// file, so it names nobody, and a test that exercises the parser's handling
/// of 127.0.1.9 must be able to write it down. Everything else is one worked
/// example the settings document.
#[test]
fn every_private_address_here_is_documentation() {
    let allowed: BTreeSet<&str> = DOCUMENTED_ADDRESSES.iter().copied().collect();
    let loopback = |a: &str| a.starts_with("127.");
    let mut hits = Vec::new();
    for (rel, text) in readable() {
        for (i, line) in text.lines().enumerate() {
            for address in private_addresses(line) {
                if !allowed.contains(address.as_str()) && !loopback(&address) {
                    hits.push(format!("{rel}:{}: {address}", i + 1));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "a private address names a host on somebody's network:\n{}",
        hits.join("\n")
    );
}

/// No tracked file carries a credential.
///
/// WHY: a committed token is live from the moment it is pushed and stays live
/// after it is deleted from `HEAD`. This catches the shapes that are
/// self-identifying; it cannot catch a bare password, which is why credentials
/// live outside the tree rather than being scanned for.
#[test]
fn no_tracked_file_carries_a_credential() {
    let hits = scan(|line| {
        if line.contains(KEY_BLOCK) {
            return true;
        }
        TOKEN_PREFIXES.iter().any(|(prefix, body)| {
            line.match_indices(prefix).any(|(at, _)| {
                line[at + prefix.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .count()
                    >= *body
            })
        })
    });
    assert!(
        hits.is_empty(),
        "a credential in a tracked file is live the moment it is pushed:\n{}",
        hits.join("\n")
    );
}

/// No tracked file contains profanity.
///
/// WHY: including inside a quotation kept for accuracy. The quotation is the
/// case that gets argued for, and it publishes the word either way.
#[test]
fn no_tracked_file_contains_profanity() {
    let hits = scan(|line| PROFANITY.iter().any(|word| has_word(line, word)));
    assert!(
        hits.is_empty(),
        "profanity does not ship, quotation marks included:\n{}",
        hits.join("\n")
    );
}

/// No tracked file sources a requirement to a person in a conversation.
///
/// WHY: prose reading "the operator reported X" tells a reader that a
/// conversation happened somewhere they cannot see, and attributes a product
/// decision to a person instead of to the behaviour that forced it. State the
/// defect. This does not fire on "the operator asked for", which is this
/// product describing somebody using its own interface.
#[test]
fn no_tracked_file_attributes_a_requirement_to_a_conversation() {
    let hits = scan(|line| {
        let lower = line.to_ascii_lowercase();
        ATTRIBUTION.iter().any(|phrase| lower.contains(phrase))
    });
    assert!(
        hits.is_empty(),
        "state the requirement as a product fact and name the real actor: the \
         caller, the setting, the flag, the session, the file:\n{}",
        hits.join("\n")
    );
}

/// No tracked file is internal process material.
///
/// WHY: a requirements ledger, a review artefact directory and a backlog
/// describe how work is produced here. They were tracked once, they are
/// gitignored now, and the way one comes back is somebody using `git add -f`
/// or moving a file out of `.internal/` to make a link resolve.
#[test]
fn no_tracked_file_is_internal_process_material() {
    let hits: Vec<&str> = internal_material(super::tree::tracked());
    assert!(
        hits.is_empty(),
        "process material belongs in the gitignored .internal/ directory, not \
         in a published tree: {hits:?}"
    );
}

/// Every path in `paths` that is internal process material.
fn internal_material<S: AsRef<str>>(paths: &[S]) -> Vec<&str> {
    paths
        .iter()
        .map(AsRef::as_ref)
        .filter(|rel| {
            INTERNAL_DIRS.iter().any(|dir| rel.starts_with(dir))
                || INTERNAL_NAMES
                    .iter()
                    .any(|name| rel.rsplit('/').next() == Some(name))
        })
        .collect()
}

/// The internal-material predicate recognises every shape that was here once.
///
/// WHY: the check above is green on this tree and would stay green if the
/// predicate were broken, because a passing check and a check that recognises
/// nothing look identical. Staging a file to prove otherwise is not something
/// a test may do, so the predicate is exercised against a synthetic listing
/// holding one of each shape, alongside paths that must stay accepted.
#[test]
fn the_internal_material_predicate_recognises_each_shape() {
    let listing = [
        "GOAL.md",
        "SPEC.md",
        "BACKLOG.md",
        "WORKFLOW.md",
        "docs/reviews/whatsnew/before.png",
        ".internal/cutover.md",
        "README.md",
        "docs/keys.md",
        "app/src/main.rs",
        "crates/vitrum-model/README.md",
    ];
    let hits = internal_material(&listing);
    assert_eq!(
        hits,
        [
            "GOAL.md",
            "SPEC.md",
            "BACKLOG.md",
            "WORKFLOW.md",
            "docs/reviews/whatsnew/before.png",
            ".internal/cutover.md",
        ],
        "the predicate must catch every shape that was tracked here once, and \
         nothing else"
    );
}

/// The guard reads a tree, and a tree it cannot read is not a green tree.
///
/// WHY: every check above passes when `readable()` is empty, because "nothing
/// matched" and "nothing was examined" are the same result. This pins the size
/// and the content of what was scanned, so a broken listing fails loudly
/// instead of certifying an empty set.
#[test]
fn the_scan_covered_this_repository() {
    let seen = readable();
    assert!(
        seen.len() > 200,
        "only {} readable tracked files were scanned; the listing is wrong",
        seen.len()
    );
    for required in ["README.md", "CONTRIBUTING.md", "harness/run.sh"] {
        assert!(
            seen.iter().any(|(rel, _)| *rel == required),
            "{required} was not scanned, so the listing is not this repository"
        );
    }
    assert!(
        !seen.iter().any(|(rel, _)| *rel == SELF),
        "this file names every banned string and must be skipped"
    );
}
