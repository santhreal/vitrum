use super::*;

/// This module's own stylesheet.
const SEARCH_CSS: &str = include_str!("../../../assets/parts/21-search.css");
/// The shell's stylesheet, which owns `.rg-layer` and its dim modifier.
const APP_CSS: &str = include_str!("../../app.css");

fn hit(visible: &[u8], start: u32, end: u32) -> SearchHit {
    SearchHit {
        session: SessionId(1),
        line_seq: 0,
        visible: visible.to_vec(),
        match_start: start,
        match_end: end,
        before: Vec::new(),
        after: Vec::new(),
    }
}

fn answer(hits: Vec<SearchHit>, truncated: bool, scanned: u64) -> Answer {
    Answer {
        pattern: "boom".to_string(),
        hits,
        truncated,
        bytes_scanned: scanned,
    }
}

/// **The bug this whole module is shaped around.** A line carrying an
/// invalid byte must still highlight the bytes the daemon matched.
///
/// `0xFF` is one byte and decodes to `U+FFFD`, which is THREE bytes of
/// UTF-8. Decode the line first and every offset past that byte is two
/// too small, so the highlight lands on `boom`'s neighbours instead of on
/// `boom`. Nothing anywhere reports an error when it does.
#[test]
fn an_invalid_byte_does_not_slide_the_highlight() {
    let raw = b"pre \xff post boom tail";
    let start = raw.windows(4).position(|w| w == b"boom").unwrap() as u32;
    let end = start + 4;
    let split = split_hit(raw, start, end);

    assert_eq!(split.matched, "boom");
    assert_eq!(split.before, "pre \u{fffd} post ");
    assert_eq!(split.after, " tail");
    assert!(!split.empty_mark);

    // The naive route, spelled out, so this test says what it defends
    // against rather than only that the good path works. One 0xFF grows
    // the decoded line by two bytes, so a highlight taken at 11..15 of
    // the decoded string starts two bytes early and shows the tail of
    // `post` plus the head of `boom`.
    let decoded = String::from_utf8_lossy(raw);
    let naive = &decoded[start as usize..end as usize];
    assert_eq!(
        naive, "t bo",
        "decode-then-index no longer misplaces the highlight; if that is \
         genuinely true this test is obsolete, but check before deleting it"
    );
}

/// A match boundary landing inside a multi-byte character must stay where
/// the daemon put it.
///
/// A byte-oriented matcher on a line with a stray byte can cut a UTF-8
/// sequence in half. Slicing the raw bytes yields a replacement character
/// on each side of the cut, which is honest. The WRONG FIX this exists to
/// stop is "repairing" the boundary by snapping the offsets out to the
/// nearest character edge to make the `U+FFFD` pair go away: that widens
/// the highlight over bytes the daemon never matched, and it looks like a
/// tidy-up rather than a behaviour change.
///
/// **Do not delete this as a duplicate of the render test.** It looks like
/// one, because `render::an_invalid_byte_line_renders_the_highlight_on_the_matched_bytes`
/// covers "the same" invalid-byte bug end to end. Measured by mutation,
/// they are complementary, not redundant:
///
/// - Break `split_hit` into decode-then-index: three tests fail, that
///   render test among them. It stands alone on the headline bug.
/// - Apply the boundary-snapping wrong fix above: 31 tests pass and ONLY
///   this one fails. The render fixture's offsets land on
///   non-continuation bytes, so it never exercises a mid-character cut.
///
/// A green suite can be green against a mutant purely because its fixture
/// never reaches the case.
#[test]
fn a_boundary_inside_a_character_is_not_moved() {
    // "a\u{e9} b": 0x61, 0xC3 0xA9, 0x20, 0x62. Cut between the two bytes.
    let raw = b"a\xc3\xa9 b";
    let split = split_hit(raw, 2, 5);
    assert_eq!(split.before, "a\u{fffd}");
    assert_eq!(split.matched, "\u{fffd} b");
    assert_eq!(split.after, "");
}

/// Offsets arrive over a socket and must never index out of bounds.
///
/// `&visible[start..end]` with `end` past the line, or with `start >
/// end`, panics inside a render, which takes the window down rather than
/// dropping one row.
#[test]
fn out_of_range_offsets_are_clamped_instead_of_panicking() {
    let line = b"short";
    assert_eq!(split_hit(line, 0, 99).matched, "short");
    assert_eq!(split_hit(line, 99, 99).before, "short");
    assert_eq!(split_hit(line, 99, 99).matched, "");

    // start > end: the clamp collapses the range rather than inverting it.
    let inverted = split_hit(line, 4, 1);
    assert_eq!(inverted.before, "shor");
    assert_eq!(inverted.matched, "");
    assert_eq!(inverted.after, "t");
}

/// A zero-width match must still be visible.
///
/// `^`, `x*` and `\b` all match nothing at all, and a highlight painted
/// as a background behind an empty span is a hit with no visible reason
/// for being one. The caret modifier is what makes it show.
#[test]
fn a_zero_width_match_is_marked_as_a_caret() {
    let split = split_hit(b"line", 2, 2);
    assert!(split.empty_mark);
    assert_eq!(
        mark_class(split.empty_mark),
        "rg-search__mark rg-search__mark--empty"
    );
    assert_eq!(mark_class(false), "rg-search__mark");
}

/// A match landing on a control byte sanitises to nothing and must take
/// the caret too, for the same reason as a zero-width match.
#[test]
fn a_match_on_a_control_byte_is_marked_as_a_caret() {
    let split = split_hit(b"a\x07b", 1, 2);
    assert_eq!(split.before, "a");
    assert_eq!(split.matched, "");
    assert!(split.empty_mark);
    assert_eq!(split.after, "b");
}

/// A carriage return in a hit must not survive into the row.
///
/// The daemon strips escape SEQUENCES; a bare `\r` is not one. Left in,
/// it returns the cursor in any terminal this text is pasted into and
/// renders as a hole in the row.
#[test]
fn bare_control_characters_are_flattened_to_one_space() {
    assert_eq!(line_text(b"a\rb\tc"), "a b c");
    assert_eq!(line_text(b"\x1b[31mred\x1b[0m"), "red");
}

/// Clean text must survive at EVERY length, not just the three the other
/// tests happen to use.
///
/// Found by mutation: a fault conditioned on a specific segment length,
/// `if raw.len() == 10 { drop the last byte }`, passed the whole suite,
/// because no fixture fed a ten-byte run through `line_text`. A
/// length-conditioned bug is not hypothetical here: this function is the
/// one place bytes become glyphs, and its callers hand it slices whose
/// length is whatever the daemon's match offsets made them.
#[test]
fn clean_text_round_trips_at_every_length() {
    for len in 1..=48usize {
        let ascii: String = (0..len).map(|i| (b'a' + (i % 26) as u8) as char).collect();
        assert_eq!(
            line_text(ascii.as_bytes()),
            ascii,
            "a {len}-byte run did not survive decoding"
        );
    }
}

/// A truncated answer must say so, and must never read as a total.
///
/// The daemon stops at its cap and sets `truncated`. An operator shown
/// "12 matches" who was really shown 12 of 400 will conclude the other
/// 388 do not exist, which is the entire failure mode the flag was added
/// to prevent.
#[test]
fn a_truncated_answer_says_these_are_the_first_n() {
    let hits = vec![hit(b"boom", 0, 4), hit(b"boom", 0, 4)];
    let out = summary(Some(&answer(hits, true, 2 * 1024 * 1024)), false, 0);
    assert_eq!(
        out.text,
        "First 2 matches in 1 session of every session's scrollback, then the sweep hit its \
         cap of 500. There are more. Swept 2.0 MiB so far.",
    );
    assert_eq!(
        out.class,
        "rg-search__summary rg-search__summary--truncated"
    );

    let complete = summary(
        Some(&answer(vec![hit(b"boom", 0, 4)], false, 2 * 1024 * 1024)),
        false,
        0,
    );
    assert_eq!(
        complete.text,
        "1 match in 1 session of every session's scrollback. Swept 2.0 MiB."
    );
    assert_eq!(complete.class, "rg-search__summary rg-search__summary--ok");
}

/// A scoped sweep must say so. The wire has always carried a session filter
/// and the client always sent an empty list, so selecting three rows and
/// searching swept all twenty. Once it narrows, "3 matches in 2 sessions" is
/// the same sentence for a sweep of twenty and a sweep of two, and only one
/// of those means the other eighteen agents are clean.
#[test]
fn a_scoped_sweep_names_what_it_covered() {
    let ans = answer(vec![hit(b"boom", 0, 4)], false, 1024);
    let all = summary(Some(&ans), false, 0);
    let two = summary(Some(&ans), false, 2);
    let one = summary(Some(&ans), false, 1);

    assert_eq!(
        all.text,
        "1 match in 1 session of every session's scrollback. Swept 1.0 KiB."
    );
    assert_eq!(
        two.text,
        "1 match in 1 session of the 2 sessions you selected. Swept 1.0 KiB."
    );
    assert_eq!(
        one.text,
        "1 match in 1 session of the 1 session you selected. Swept 1.0 KiB."
    );
    assert_eq!(
        summary(None, true, 3).text,
        "Sweeping the 3 sessions you selected."
    );
}

/// "Nothing was found" and "nothing was asked" are different facts and
/// must not share a rendering.
///
/// Both produce an empty list. Showing the same empty list for both tells
/// an operator their agents never printed the word when in truth no sweep
/// ever ran.
#[test]
fn not_searched_yet_and_no_matches_are_different() {
    let idle = summary(None, false, 0);
    let none = summary(Some(&answer(Vec::new(), false, 1024)), false, 0);
    let busy = summary(None, true, 0);

    assert_eq!(
        idle.text,
        "Not searched yet. Type a pattern and press Enter to sweep every session's scrollback."
    );
    assert_eq!(
        none.text,
        "No matches for \u{201c}boom\u{201d} in every session's scrollback. Swept 1.0 KiB."
    );
    assert_eq!(busy.text, "Sweeping every session's scrollback.");

    assert_ne!(idle.text, none.text);
    assert_ne!(idle.class, none.class);
    assert_ne!(busy.class, idle.class);
}

/// Removing the submit button must not remove the instruction to submit.
///
/// The button was deleted for repeating the field beside it. Its function
/// lives in Enter, and the only place the product says so is the idle
/// line and the placeholder. If either stops naming Enter, submitting
/// becomes an undiscoverable feature.
#[test]
fn the_idle_line_and_the_placeholder_both_name_enter() {
    assert!(summary(None, false, 0).text.contains("Enter"));
    let src = include_str!("../search.rs");
    let markup = src
        .split_once("#[cfg(test)]")
        .map_or(src, |(before, _)| before);
    assert!(
        markup.contains("placeholder: \"Pattern, then Enter\""),
        "the field no longer says how it submits"
    );
}

/// The summary quotes the pattern the DAEMON swept, not the field.
///
/// The operator keeps typing while a 90ms sweep runs. Quoting the live
/// field would attribute one pattern's results to another.
#[test]
fn the_summary_quotes_the_swept_pattern() {
    let mut ans = answer(Vec::new(), false, 0);
    ans.pattern = "OutOfMemory".to_string();
    assert!(summary(Some(&ans), false, 0).text.contains("OutOfMemory"));
}

/// An unbounded pattern must not be quoted whole into a one-line header.
#[test]
fn a_long_pattern_is_truncated_in_the_summary() {
    let mut ans = answer(Vec::new(), false, 0);
    ans.pattern = "x".repeat(400);
    let text = summary(Some(&ans), false, 0).text;
    assert!(text.contains('\u{2026}'), "{text}");
    assert!(text.len() < 140, "{} chars: {text}", text.len());
}

/// Hits must group by session in the order the daemon returned them.
///
/// The daemon sweeps in ascending session id and its global cap consumes
/// that order, which is the only thing making a truncated answer the
/// FIRST n hits. Re-sorting client-side would silently discard that.
#[test]
fn grouping_preserves_the_daemons_order() {
    let mut a1 = hit(b"one", 0, 3);
    a1.session = SessionId(7);
    let mut b1 = hit(b"two", 0, 3);
    b1.session = SessionId(2);
    let mut a2 = hit(b"three", 0, 5);
    a2.session = SessionId(7);

    let hits = vec![a1, b1, a2];
    let titles = vec![(SessionId(7), "claude".to_string())];
    let groups = group_by_session(&hits, &titles);

    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].session, SessionId(7));
    assert_eq!(groups[0].label, "claude");
    assert_eq!(groups[0].hits.len(), 2);
    assert_eq!(groups[1].session, SessionId(2));
    assert_eq!(groups[1].hits.len(), 1);
}

/// A session with no title must be headed by its id, never by nothing.
///
/// A blank heading over a list of hits reads as a rendering fault, and
/// the id is the one thing the operator can match to the sidebar.
#[test]
fn an_untitled_session_is_headed_by_its_id() {
    let hits = vec![hit(b"x", 0, 1)];
    assert_eq!(group_by_session(&hits, &[])[0].label, "Session 1");
    let blank = vec![(SessionId(1), "   ".to_string())];
    assert_eq!(group_by_session(&hits, &blank)[0].label, "Session 1");
}

/// `match` does not pluralise with a bare `s`.
///
/// `count_s` would render `2 matchs`, which is the kind of defect that
/// survives review because nobody reads the count.
#[test]
fn matches_is_spelled_correctly_at_every_count() {
    assert_eq!(matches_word(0), "0 matches");
    assert_eq!(matches_word(1), "1 match");
    assert_eq!(matches_word(2), "2 matches");
    assert_eq!(matches_word(1500), "1,500 matches");
}

/// A blank field must never reach the daemon.
///
/// A literal sweep for a space matches nearly every line of every ring:
/// 200 MiB of scanning and 90 ms of daemon CPU to answer a question
/// nobody asked.
#[test]
fn a_blank_pattern_produces_no_request() {
    assert!(request("", Options::default(), Vec::new()).is_none());
    assert!(request("   \t ", Options::default(), Vec::new()).is_none());
}

/// The request must carry the switches the operator set, and the caps the
/// summary reports.
///
/// A send site that chose its own `max_hits` would make the truncation
/// sentence, which names `MAX_HITS`, quote a number that was never sent.
#[test]
fn the_request_carries_the_options_and_the_declared_caps() {
    let options = Options::default()
        .toggled(Toggle::Regex)
        .toggled(Toggle::WholeWord);
    let msg = request("  oom  ", options, vec![SessionId(3)]).expect("a pattern was given");
    assert_eq!(
        msg,
        ClientMsg::Search {
            sessions: vec![SessionId(3)],
            pattern: "oom".into(),
            regex: true,
            case_insensitive: false,
            whole_word: true,
            context_lines: CONTEXT_LINES,
            max_hits: MAX_HITS,
        }
    );
}

/// `CONTEXT_LINES` must be pinned by something other than itself.
///
/// Found by mutation: changing it from 2 to 7 passed the whole suite,
/// because the only assertion touching it compared
/// `msg.context_lines` against `CONTEXT_LINES`, so both sides moved
/// together. That is an arithmetic identity wearing a test's clothes: it
/// cannot fail for any value.
///
/// The constant is load-bearing for layout and its doc comment says so,
/// which until now was a claim with no test behind it. A hit row is the
/// matched line plus context either side, each on the 20px line box
/// `--rg-lead-body` gives `.rg-search__hit`, plus that rule's 8px of
/// block padding top and bottom. So the row is
/// `(1 + 2 * CONTEXT_LINES) * 20 + 16`, and every number below is a
/// literal precisely so the assertion cannot move with the constant.
#[test]
fn the_context_budget_keeps_several_hits_on_screen_at_once() {
    assert_eq!(
        CONTEXT_LINES, 2,
        "changing the context budget changes the height of every hit row"
    );
    let row_px = (1 + 2 * u32::from(CONTEXT_LINES)) * 20 + 16;
    assert_eq!(
        row_px, 116,
        "a hit row is no longer five lines and its padding"
    );
    // A results area of 600px is what a 900px window leaves after the
    // layer's 48 of block padding, the sheet's 24, and the head, field,
    // switches and summary above the list.
    assert!(
        600 / row_px >= 4,
        "only {} hits fit on screen at once; at 7 lines of context it is 1",
        600 / row_px
    );
}

/// **The shipped client must actually reach this module.**
///
/// This ticket exists because the daemon has had a complete cross-session
/// scrollback search all along and no UI could call it: `state.rs` read
/// `ServerMsg::SearchResults` and returned `Broadcast::None`. Every other
/// test in this file passes just as happily when the module is orphaned
/// again, which is the defect restored one layer up and invisible to the
/// suite written to prevent it.
///
/// So this reads the shipped `main.rs`. That file is not mine, and the
/// guard deliberately lives with the contract it defends rather than with
/// the code it reads: the wiring is what my module needs in order to
/// exist at all, and its owner should learn from a red test rather than
/// from a user finding a dead chord.
///
/// Four links, each able to break alone: the layer renders the component,
/// a chord opens that layer, the send site builds its request through
/// [`request`] so the caps the summary quotes are the caps sent, and the
/// request actually reaches the socket.
#[test]
fn the_shell_still_reaches_this_module() {
    shell_reaches_search(&crate::testkit::shell())
        .expect("the shipped client no longer reaches this module");
}

/// The four links the shell needs for this module to be reachable, and
/// why each one matters if it breaks alone.
const LINKS: [(&str, &str); 5] = [
    (
        "Layer::Search => {",
        "no layer arm renders the search surface, so the overlay can never appear",
    ),
    (
        "ui::search::Search {",
        "the layer arm no longer renders this module's component",
    ),
    (
        "KeyAction::OpenSearch => toggle_layer(st, Layer::Search)",
        "no chord opens the search layer, so the feature is unreachable from the keyboard",
    ),
    (
        "ui::search::request(",
        "the send site no longer builds its request here, so the caps the summary quotes \
         are not the caps that were sent",
    ),
    ("bridge.msg(&msg);", "the request is built and never sent"),
];

/// Does `main` still wire this module into the running client?
///
/// Takes the source as an argument rather than reading it, so the guard
/// can be pointed at a doctored copy and shown to fail. Proving it that
/// way needs no write to `main.rs`, which is not mine and sits in a tree
/// other agents are building from.
fn shell_reaches_search(main: &str) -> Result<(), String> {
    // NOT `split_once("#[cfg(test)]")`. main.rs carries
    // `#[cfg(test)] mod testkit;` near the top, so the short anchor
    // truncates the scan to a couple of dozen lines, and an empty scan
    // satisfies every `contains` below by having nothing in it.
    let shipped = main
        .split_once("\n#[cfg(test)]\nmod tests {")
        .map_or(main, |(before, _)| before);
    // Anchored on a landmark, NOT on a fraction of the file. A ratio
    // assumes shipped code outweighs tests, which is false for any module
    // that is mostly its own suite: search.rs is two thirds tests, and a
    // "more than half survived" rule fails it while checking nothing at
    // all about whether the scan worked. `fn main(` is in the shipped
    // half by definition, so its absence is the collapse and its presence
    // is proof the split found the real boundary.
    if !shipped.contains("\nfn main(") {
        return Err(format!(
            "the scan kept {} of {} bytes and lost `fn main(`, so it cut in the \
             wrong place and every check below is vacuous",
            shipped.len(),
            main.len()
        ));
    }
    for (needle, why) in LINKS {
        if !shipped.contains(needle) {
            return Err(format!("{why} (looked for {needle:?})"));
        }
    }
    Ok(())
}

/// Every link must be able to fail on its own, and the collapse guard
/// must fire on the anchor that would silence all of them.
///
/// Without this, `the_shell_still_reaches_this_module` is a check nobody
/// has ever seen say no. It is also the one guard in this file that reads
/// a file I do not own, so it has to be demonstrably discriminating
/// rather than merely green.
#[test]
fn the_reachability_guard_fails_on_each_broken_link() {
    let real = crate::testkit::shell();
    let real = real.as_str();
    assert!(shell_reaches_search(real).is_ok());

    for (needle, _) in LINKS {
        let broken = real.replacen(needle, "/* unwired */", 1);
        assert_ne!(broken, real, "{needle:?} is not in main.rs to begin with");
        assert!(
            shell_reaches_search(&broken).is_err(),
            "breaking {needle:?} left the guard green"
        );
    }

    // The trap that makes every clause above vacuous: anchoring on the
    // short `#[cfg(test)]` would cut the scan at `mod testkit;`.
    let truncated = &real[..real.find("\n#[cfg(test)]\nmod testkit;").unwrap_or(200)];
    assert!(
        shell_reaches_search(truncated).is_err(),
        "a collapsed scan passed, so the guard can be silenced by a stray attribute"
    );
}

/// Toggling is an involution and touches exactly one switch.
#[test]
fn a_toggle_flips_one_switch_and_only_that_one() {
    let base = Options::default();
    for which in Toggle::ALL {
        let on = base.toggled(which);
        assert!(on.is_on(which));
        assert_eq!(on.toggled(which), base);
        for other in Toggle::ALL {
            if other != which {
                assert!(!on.is_on(other), "{which:?} moved {other:?}");
            }
        }
    }
    assert_eq!(opt_class(true), "rg-search__opt rg-search__opt--on");
    assert_eq!(opt_class(false), "rg-search__opt");
}

/// A switch must be readable without hovering it.
///
/// `.*`, `Aa` and a boxed `w` were the first draft. Every one needs a
/// tooltip to mean anything, and a tooltip is not an affordance on a
/// touchpad and does not survive a screenshot.
#[test]
fn every_switch_is_labelled_with_a_word() {
    for which in Toggle::ALL {
        let label = which.label();
        assert!(
            label.chars().all(|c| c.is_ascii_alphabetic() || c == ' '),
            "{which:?} is captioned {label:?}, which is a glyph rather than a word"
        );
        assert!(label.len() >= 5, "{label:?} is too terse to read");
    }
}

/// The switches must survive a restart, and an older profile must still
/// load.
///
/// `settings.rs` states the bar absolutely: a control ships only if
/// flipping it changes behaviour immediately AND survives a restart. The
/// settings audit found 47 controls and zero violations of the second
/// half, so without serde on `Options` these three would have been the
/// product's first. The camelCase keys are asserted because the rest of
/// the profile uses them, and a snake_case island would be silently
/// dropped by `serde(default)` rather than reported: the switch would
/// come back off and nothing anywhere would say why.
#[test]
fn the_switches_round_trip_through_a_profile() {
    let set = Options::default()
        .toggled(Toggle::Regex)
        .toggled(Toggle::WholeWord);
    let json = serde_json::to_string(&set).expect("options serialise");
    assert_eq!(
        json,
        r#"{"regex":true,"caseInsensitive":false,"wholeWord":true}"#
    );
    assert_eq!(serde_json::from_str::<Options>(&json).unwrap(), set);

    // An empty object, and a profile written before a switch existed,
    // must both load rather than fail the whole settings read.
    assert_eq!(
        serde_json::from_str::<Options>("{}").unwrap(),
        Options::default()
    );
    let partial: Options = serde_json::from_str(r#"{"regex":true}"#).unwrap();
    assert!(partial.regex);
    assert!(!partial.whole_word);
}

/// Every class this file writes must have a rule.
///
/// An unstyled class is not an error anywhere: the element renders with no
/// box, no padding and no colour, and reads as a layout bug rather than a
/// missing rule. Read out of the markup rather than a hand-kept list,
/// because a list is the thing that stops matching the markup. Only the
/// code above `#[cfg(test)]` is scanned; below it these same names are
/// assertion data.
#[test]
fn every_emitted_class_is_styled() {
    let src = include_str!("../search.rs");
    let markup = src
        .split_once("#[cfg(test)]")
        .map_or(src, |(before, _)| before);

    let mut seen = Vec::new();
    for (at, _) in markup.match_indices("class: \"") {
        let rest = &markup[at + 8..];
        let Some(end) = rest.find('"') else { continue };
        for token in rest[..end].split_whitespace() {
            if token.starts_with("rg-") && !token.contains('{') {
                seen.push(token);
            }
        }
    }
    seen.sort_unstable();
    seen.dedup();
    assert!(
        seen.len() > 12,
        "only found {} classes; the extraction broke rather than the markup",
        seen.len()
    );

    for class in seen {
        // `styled`, not `contains`. A bare substring test is satisfied by
        // a LONGER class name, so `.rg-search__group` is "found" by the
        // rules for `.rg-search__group-head` and `.rg-search__group-count`
        // and renaming its own rule out from under live markup leaves this
        // green. That is precisely the regression the guard names, and it
        // escaped a mutation until this line changed.
        let mine = styled(SEARCH_CSS, class);
        // `.rg-layer` and its dim modifier are app.css's and are reused
        // deliberately: the bridge queries `.rg-layer` to decide whether
        // Escape belongs to a layer, so a private backdrop class would
        // leave this surface un-dismissable.
        let shell = class.starts_with("rg-layer") && styled(APP_CSS, class);
        assert!(
            mine || shell,
            "search markup emits .{class} but no stylesheet has a rule for it"
        );
    }
}

/// Does `css` carry a rule for exactly `class`, not merely for something
/// beginning with it?
///
/// The next character after `.rg-foo` must not be able to continue a CSS
/// identifier, or `.rg-foo` matches `.rg-foo-bar`. Modifiers still
/// resolve, because `-` continues an identifier and `.rg-foo--on` is
/// therefore only found by a search for `rg-foo--on`.
fn styled(css: &str, class: &str) -> bool {
    let needle = format!(".{class}");
    css.match_indices(&needle).any(|(at, _)| {
        css[at + needle.len()..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'))
    })
}

/// Every class assembled at runtime must be styled too.
///
/// The test above skips interpolated `class: "{...}"` values by design,
/// which is exactly where the state modifiers live: the switch's pressed
/// state, the caret highlight and all five summary tones.
#[test]
fn every_runtime_class_is_styled() {
    let scanned = 4096;
    let names = [
        opt_class(true),
        opt_class(false),
        mark_class(true),
        mark_class(false),
        summary(None, true, 0).class,
        summary(None, false, 0).class,
        summary(Some(&answer(Vec::new(), false, scanned)), false, 0).class,
        summary(
            Some(&answer(vec![hit(b"x", 0, 1)], false, scanned)),
            false,
            0,
        )
        .class,
        summary(
            Some(&answer(vec![hit(b"x", 0, 1)], true, scanned)),
            false,
            0,
        )
        .class,
    ];

    for full in names {
        for class in full.split_whitespace() {
            assert!(
                SEARCH_CSS.contains(&format!(".{class}")),
                "21-search.css has no rule for .{class}"
            );
        }
    }
}

/// The five summary tones must be five distinct classes.
///
/// Two states sharing a modifier is how "no matches" quietly starts
/// looking like "not searched yet" again after the strings diverge.
#[test]
fn each_summary_state_has_its_own_modifier() {
    let scanned = 1024;
    let all = [
        summary(None, true, 0).class,
        summary(None, false, 0).class,
        summary(Some(&answer(Vec::new(), false, scanned)), false, 0).class,
        summary(
            Some(&answer(vec![hit(b"x", 0, 1)], false, scanned)),
            false,
            0,
        )
        .class,
        summary(
            Some(&answer(vec![hit(b"x", 0, 1)], true, scanned)),
            false,
            0,
        )
        .class,
    ];
    let mut unique = all.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), all.len(), "two summary states share a class");
}

/// The sheet must fit at every window width, including beside the
/// sidebar's 224px floor.
///
/// The layer covers the whole window, so a fixed sheet width wider than a
/// narrow window overflows it and the results scroll sideways underneath.
/// The width has to be capped against the viewport, and the sheet has to
/// be allowed to shrink below the unwrappable monospace line inside it.
#[test]
fn the_sheet_width_is_capped_against_the_window() {
    let rule = css_rule(".rg-search {");
    assert!(
        rule.contains("max-width: 100%"),
        "the search sheet has no width cap: {rule}"
    );
    assert!(
        rule.contains("min-width: 0"),
        "the search sheet cannot shrink below its content: {rule}"
    );
}

/// Nothing in this stylesheet may loop, and nothing may animate.
///
/// Idle cost is the product's competitive claim. The shell's own guard
/// (`main.rs::stylesheets_never_loop_and_keep_transitions_brief`) only
/// covers files registered in `stylesheets()`, so this file guards itself
/// from the moment it exists rather than from the moment it is wired.
#[test]
fn the_stylesheet_declares_no_motion_at_all() {
    let code = strip_comments(SEARCH_CSS);
    // Nine, not the three anybody remembers. A guard whose subject is
    // "nothing on this surface may move" cannot name only the spellings
    // that came to mind: `scroll-behavior: smooth` and `@starting-style`
    // are motion and would have walked straight past the short list.
    // Bare words rather than `animation:`, so the longhands are covered
    // too.
    for banned in [
        "infinite",
        "animation",
        "transition",
        "@keyframes",
        "scroll-behavior",
        "@starting-style",
        "view-transition",
        "will-change",
        "offset-path",
    ] {
        assert!(
            !code.contains(banned),
            "21-search.css declares {banned}, which it has no reduced-motion block for"
        );
    }
}

/// Every length in this stylesheet is on the 4px grid.
///
/// Authored in rem at 1x, so a grid multiple is a multiple of 0.25rem.
/// Pixel literals are allowed only at 1px, the hairline, which is a
/// device-resolution artefact rather than a design measurement. A 3px
/// rail or a 6px radius is what put the sidebar's left edges on four
/// different columns.
///
/// The unit set is DERIVED, not listed. Checking `rem` and `px` and
/// nothing else leaves `padding: 0.4em` invisible to the guard whose
/// whole subject is the grid, so this collects every number glued to
/// letters and requires the units it finds to be exactly rem and px.
/// Percentages carry no letters and are untouched, which is what keeps
/// `width: 100%` and `max-width: 100%` legal.
#[test]
fn every_length_is_on_the_four_pixel_grid() {
    let code = strip_comments(SEARCH_CSS);
    let mut units: Vec<&str> = Vec::new();
    let mut checked = 0;

    let bytes = code.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        // A number may be preceded by a `-` or a `.`; `number_before`
        // reads back over the digits and the point for the value.
        let mut end = i;
        while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b'.') {
            end += 1;
        }
        let unit_start = end;
        let mut unit_end = end;
        while unit_end < bytes.len() && bytes[unit_end].is_ascii_alphabetic() {
            unit_end += 1;
        }
        if unit_end > unit_start {
            let unit = &code[unit_start..unit_end];
            let value: f64 = code[i..end].parse().unwrap_or(f64::NAN);
            if !units.contains(&unit) {
                units.push(unit);
            }
            match unit {
                "rem" => {
                    let px = value * 16.0;
                    assert!(
                        (px / 4.0).fract() < 1e-9,
                        "{value}rem is {px}px, which is off the 4px grid"
                    );
                }
                "px" => assert_eq!(
                    value, 1.0,
                    "{value}px is a literal length; only the 1px hairline is exempt"
                ),
                other => panic!(
                    "{value}{other} uses a unit this guard cannot place on the grid; \
                     author lengths in rem at 1x, or in 1px hairlines"
                ),
            }
            checked += 1;
        }
        i = unit_end.max(end);
    }

    units.sort_unstable();
    assert_eq!(
        units,
        ["px", "rem"],
        "the stylesheet grew a unit the grid guard was never written for"
    );
    // The sheet's own width plus one hairline on each of the four
    // controls. A floor rather than an exact count, so deleting one
    // border later does not read as a broken scan.
    assert!(checked >= 5, "only {checked} lengths found; the scan broke");
}

/// The CSS features Blitz cannot render must stay out.
///
/// Each one fails silently: the rule is dropped and the element renders
/// unstyled, which looks like a markup bug on the one renderer with no
/// devtools.
#[test]
fn the_stylesheet_stays_within_the_supported_subset() {
    let code = strip_comments(SEARCH_CSS);
    for banned in [
        "position: fixed",
        ":has(",
        "color-mix(",
        "oklch(",
        "@container",
        "!important",
    ] {
        assert!(!code.contains(banned), "21-search.css uses {banned}");
    }
}

/// The body of one rule, for asserting on what it declares.
fn css_rule(selector: &str) -> String {
    let (_, after) = SEARCH_CSS
        .split_once(selector)
        .unwrap_or_else(|| panic!("21-search.css has no rule for {selector}"));
    after
        .split_once('}')
        .map_or_else(|| after.to_string(), |(body, _)| body.to_string())
}

/// Drop `/* ... */` so a comment discussing a banned feature does not
/// trip the guards above.
fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(open) = rest.find("/*") {
        out.push_str(&rest[..open]);
        match rest[open + 2..].find("*/") {
            Some(close) => rest = &rest[open + 2 + close + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}
