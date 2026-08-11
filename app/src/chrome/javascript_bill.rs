//! The JavaScript ceiling.
//!
//! The session data plane is Rust, and the point of moving it was that it
//! stays moved. Nothing in the build stops someone vendoring a date library
//! for one call site, or answering a layout problem with a fresh inline
//! script, and neither would show up in a diff review as anything but a large
//! file somebody skipped. So the remaining JavaScript is measured, written
//! down in the `chrome` module doc, and pinned here.
//!
//! The bill is enumerated from the tree at run time rather than compared
//! against a hardcoded list of names alone: a list of names cannot notice a
//! file nobody listed, which is the exact failure this is for. A `*.js` on
//! disk that is not in [`BILL`] is a new script, and it turns the suite red
//! until somebody records the decision by adding a row.

use std::path::PathBuf;

/// Every JavaScript file this product ships, with its size in source bytes.
///
/// The recorded number is the measurement at the time the row was written,
/// and it is a ceiling: a file that grows fails. A file that SHRINKS fails
/// too, because the same numbers are quoted in the `chrome` module doc as the
/// standing bill, and a bill that silently drifts low is how a reader ends up
/// budgeting against a figure that has not been true for a year. Shrinking is
/// welcome; shrinking and updating the row is the whole ritual.
///
/// Paths are relative to the repository root and use `/` on every platform.
const BILL: [(&str, usize); 4] = [
    ("app/src/vendor/xterm.js", 289_441),
    ("app/src/vendor/addon-webgl.js", 100_856),
    ("app/src/bootstrap.js", 38_665),
    ("vendor/src/js/native_eval.js", 1_675),
];

/// The repository root, from the crate this test is compiled into.
fn repo_root() -> PathBuf {
    crate::tests::tree::root()
}

/// Every `*.js` this repository tracks, as (path relative to the root, byte
/// length).
///
/// Tracked rather than walked. A directory walk had to be told which
/// directories to ignore, and the list was written from one machine's idea of
/// what a checkout contains: a release build drops a `.zig-cache` full of
/// glslang's and libxev's JavaScript into the tree, and the bill then reported
/// seven scripts nobody here wrote as unrecorded. What ships is what is
/// committed, and that is the set this measures.
///
/// A tracked path with no file behind it is a deletion that has not been
/// committed yet, and a script that is not on disk is not shipped. Retiring a
/// vendored bundle otherwise turns the whole file red between the `rm` and the
/// commit, which is the one moment its author is reading it.
fn shipped_scripts() -> Vec<(String, usize)> {
    let root = repo_root();
    let mut found: Vec<(String, usize)> = crate::tests::tree::tracked()
        .iter()
        .filter(|rel| rel.ends_with(".js"))
        .filter_map(|rel| {
            let len = std::fs::metadata(root.join(rel)).ok()?.len();
            Some((rel.clone(), usize::try_from(len).expect("no script is 4 GB")))
        })
        .collect();
    found.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    found
}

/// The scan has to find something, or every assertion below is vacuous.
///
/// WHY: a walk that silently returns nothing — wrong root, a rename of
/// `app`, a test run from a packaged crate without its sibling directories —
/// would make the set comparison pass trivially and the ceiling meaningless.
/// This is the one existence check in the file, and existence is the contract.
#[test]
fn the_scan_finds_the_tree() {
    let found = shipped_scripts();
    assert!(
        found.len() >= BILL.len(),
        "scanning from {} found {} scripts and the bill lists {}; the scan is \
         looking in the wrong place",
        repo_root().display(),
        found.len(),
        BILL.len()
    );
}

/// Every script on disk is a script somebody recorded.
///
/// WHY: this is the half that closes the class rather than the incident. A
/// ceiling on the files we already know about does nothing against the way
/// JavaScript actually comes back, which is a new file: a vendored library, a
/// polyfill, a second bridge for one feature. The set is derived from the
/// tree, so a new script is red by default and stays red until its row and
/// its reason exist.
#[test]
fn no_script_ships_unrecorded() {
    let found = shipped_scripts();
    let recorded: Vec<&str> = BILL.iter().map(|(p, _)| *p).collect();

    let unrecorded: Vec<&str> =
        found.iter().map(|(p, _)| p.as_str()).filter(|p| !recorded.contains(p)).collect();
    assert!(
        unrecorded.is_empty(),
        "these scripts ship and are in nobody's bill: {unrecorded:?}. Adding \
         JavaScript is a decision; record it in BILL and in the chrome module \
         doc, with what it does that Rust cannot"
    );

    let missing: Vec<&str> = recorded
        .iter()
        .copied()
        .filter(|p| !found.iter().any(|(f, _)| f == p))
        .collect();
    assert!(
        missing.is_empty(),
        "the bill lists scripts that no longer exist: {missing:?}. Deleting \
         JavaScript is the point, so drop the row and the module doc's table \
         row with it"
    );
}

/// No script has grown, and none has shrunk without the bill being updated.
///
/// WHY: the ceiling itself. `xterm.js` and the addons are vendored and are
/// replaced wholesale by a version bump, which is precisely the change that
/// should be a conscious one; `bootstrap.js` is ours and is only ever
/// supposed to get smaller. Both directions fail, because the number is also
/// published in the module doc and a published number that drifts is worse
/// than no number.
#[test]
fn no_script_has_grown() {
    let found = shipped_scripts();
    for (path, recorded) in BILL {
        let actual = found
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, len)| *len)
            .unwrap_or_else(|| panic!("{path} is in the bill and not on disk"));
        assert_eq!(
            actual, recorded,
            "{path} is {actual} bytes and the bill records {recorded}. {}",
            if actual > recorded {
                "It grew: say what the new bytes do that Rust cannot, then \
                 raise the row."
            } else {
                "It shrank: lower the row and the module doc's table."
            }
        );
    }
}

/// The total the module doc publishes is the total of the bill.
///
/// WHY: the doc is where a reader looks, and prose numbers rot faster than
/// anything else in a repository. The figure is parsed out of the source so
/// the two cannot disagree.
#[test]
fn the_module_doc_publishes_the_real_total() {
    let total: usize = BILL.iter().map(|(_, n)| n).sum();
    let doc = include_str!("../chrome.rs");
    let claimed = doc
        .split_once("totalling\n//! **")
        .expect("the chrome module doc no longer publishes a JavaScript total")
        .1;
    let claimed = claimed.split_once("**").expect("the published total is unterminated").0;
    let claimed: usize = claimed.replace(',', "").parse().expect("the published total is a number");
    assert_eq!(
        claimed, total,
        "the chrome module doc publishes {claimed} bytes of JavaScript and the \
         bill adds up to {total}"
    );
}

/// Every row of the module doc's table is a row of the bill.
///
/// WHY: the table is the part anyone actually reads, and it carries a byte
/// count per file. Without this, the enforced ledger and the published one
/// are two lists maintained by hand, which is the same defect this whole file
/// exists to prevent, one level up.
#[test]
fn the_module_doc_table_matches_the_bill() {
    let doc = include_str!("../chrome.rs");
    let mut rows = Vec::new();
    for line in doc.lines() {
        let Some(row) = line.strip_prefix("//! | ") else { continue };
        let mut cells = row.split(" | ");
        let (Some(bytes), Some(path)) = (cells.next(), cells.next()) else { continue };
        let Ok(bytes) = bytes.trim().replace(',', "").parse::<usize>() else { continue };
        rows.push((path.trim().trim_matches('`').to_string(), bytes));
    }
    let bill: Vec<(String, usize)> =
        BILL.iter().map(|(p, n)| ((*p).to_string(), *n)).collect();
    assert_eq!(
        rows, bill,
        "the chrome module doc's JavaScript table and BILL disagree; they are \
         the same ledger and must be written the same way, largest first"
    );
}
