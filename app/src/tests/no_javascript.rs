//! The product ships no JavaScript.
//!
//! It shipped a lot of it once. A vendored JavaScript terminal emulator, its
//! WebGL renderer and a bridge script drew every pane inside a WebKit view,
//! which meant two escape-sequence parsers in one product, a working
//! directory and a prompt boundary held in a JavaScript addon's private state
//! where the sidebar could not read them, and a frame budget owned by a DOM
//! layout pass.
//! The pane is now a GTK drawing area with a wgpu surface on it, painted by
//! `vitrum-grid` from the grid the Ghostty parser maintains, and every script
//! this repository wrote is gone.
//!
//! One remains that it did not write. The window around the pane is still a
//! WebKit view driven by the vendored `dioxus-desktop` fork, and that
//! renderer injects its own interpreter as a script. It is named in
//! `ALLOWED`, path by path, with the reason. It goes away when the shell
//! stops being a webview.
//!
//! A deletion is not a decision until something stops it coming back. A
//! vendored script arrives one file at a time, each addition small and each
//! one reasonable on its own, and the way back to two parsers is paved with
//! them. So the tree is checked, not the intent: no tracked file is a script,
//! and no tracked file writes a `<script>` element into a document.
//!
//! What this does NOT catch: JavaScript a dependency vendors inside its own
//! crate source rather than in this tree, and a string this repository
//! assembles at run time out of pieces no literal contains. The first is
//! upstream's tree and not shipped from here; the second is a way of writing
//! code nobody has a reason to use now that the pane generates no document
//! at all.

use super::tree;

/// Extensions that are a script by their name alone.
const SCRIPT_SUFFIXES: [&str; 6] = [".js", ".mjs", ".cjs", ".jsx", ".ts", ".tsx"];

/// Tracked paths allowed to be, or to write, a script, each with its reason.
///
/// Three entries, all in `vendor/`, all belonging to the same fact: the
/// window itself is still a WebKit view driven by `dioxus-desktop`, which
/// this repository vendors as a fork. Its renderer applies every UI edit
/// through an interpreter it injects as a script, and its `Document` sends
/// head elements through the same channel. That is the shell, not the pane:
/// the terminal is a GTK drawing area painted by `vitrum-grid` and has no
/// script anywhere near it.
///
/// These are listed one path at a time rather than by directory, so a new
/// script vendored into the fork still turns this red. They go away when the
/// shell stops being a webview, and not before; nothing in this repository
/// can remove them while it is one.
const ALLOWED: [&str; 3] = [
    // The eval channel `dioxus-desktop` opens to its own interpreter. Its
    // `Document` impl routes stylesheet and meta creation through it.
    "vendor/src/js/native_eval.js",
    // The TypeScript that file is generated from, kept so the fork can be
    // rebased onto upstream.
    "vendor/src/ts/native_eval.ts",
    // Serves the interpreter into the view. A webview renderer cannot paint
    // without it.
    "vendor/src/protocol.rs",
];

#[test]
fn no_tracked_file_is_a_script() {
    let scripts: Vec<&str> = tree::tracked()
        .iter()
        .map(String::as_str)
        .filter(|p| SCRIPT_SUFFIXES.iter().any(|s| p.ends_with(s)))
        .filter(|p| !ALLOWED.contains(p))
        .collect();

    assert!(
        scripts.is_empty(),
        "this product renders its terminal natively and ships no JavaScript, \
         and these tracked files are scripts: {scripts:?}. A script here is a \
         second escape-sequence parser, a second theme, and a frame budget \
         this process does not control. Put the behaviour in Rust, or state \
         the exception in ALLOWED with the reason it cannot be."
    );
}

#[test]
fn no_source_file_emits_a_script_element() {
    let root = tree::root();
    let mut offenders: Vec<String> = Vec::new();

    for path in tree::tracked() {
        // This file names the tag in order to forbid it, and ALLOWED names
        // the shell's own renderer with its reason.
        if path == "app/src/tests/no_javascript.rs" || ALLOWED.contains(&path.as_str()) {
            continue;
        }
        if !path.ends_with(".rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(path)) else {
            continue;
        };
        if text.contains("<script") {
            offenders.push(path.clone());
        }
    }

    assert!(
        offenders.is_empty(),
        "these files write a script element into a document: {offenders:?}. \
         The window has no document to write one into, and reintroducing one \
         reintroduces the renderer this product removed."
    );
}

/// The name of the emulator that was removed, split so this file's own scan
/// cannot match it and so a reader cannot copy the whole word out of here by
/// accident.
const BANNED_WORD: [&str; 2] = ["xte", "rm"];

#[test]
fn no_tracked_file_names_the_emulator_that_was_removed() {
    // WHY: the extension and element guards above cover a script that arrives
    // as a file or as a tag. Neither can see the word itself, and the word is
    // what leaks: a comment explaining why a rule exists, a doc paragraph
    // describing the pane, alt text under a screenshot, a test name. Each of
    // those is a reader being told the product works a way it no longer does,
    // and each is a step back towards someone reintroducing it on purpose.
    //
    // The word is also a terminal type. `TERM` is set from a terminfo name and
    // the historical names carry it, so a future entry that legitimately needs
    // one has to be argued for here rather than added quietly; the assertion
    // names that case so the argument happens in a diff.
    let needle = BANNED_WORD.concat();
    let root = tree::root();
    let mut offenders: Vec<String> = Vec::new();

    for path in tree::tracked() {
        // The file that defines the ban.
        if path == "app/src/tests/no_javascript.rs" {
            continue;
        }
        // Generated from libghostty-vt's C headers. One constant in the
        // terminal-capability enum is named after the emulation mode it
        // selects, and that name is upstream's API: it cannot be reworded
        // here without the bindings ceasing to match the library. Bindgen
        // already drops the comments, so nothing else in this file is prose.
        if path == "vendor-ghostty-vt-sys/src/bindings.rs" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(path)) else {
            // Not UTF-8: an image or an archive, which cannot carry prose.
            continue;
        };
        if text.to_ascii_lowercase().contains(&needle) {
            offenders.push(path.clone());
        }
    }

    assert!(
        offenders.is_empty(),
        "these tracked files name the emulator this product removed: \
         {offenders:?}. The pane is native and there is no second parser, so a \
         mention is either stale prose or a dependency coming back. Rewrite \
         the sentence to say what is true now."
    );
}
