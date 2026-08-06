//// Render smoke tests.
////
//// Everything in `search.rs`'s own test module exercises pure functions and
//// stylesheet text. Nothing there has ever built the markup, so a panic in the
//// RSX, a bad `key`, or a highlight that is computed correctly and then
//// dropped on the floor by the markup would all pass 25 green tests.
////
//// These render the real component through `dioxus-ssr` and assert on the HTML
//// that comes out.

use super::{Answer, Options, Search};
use dioxus::prelude::*;
use vitrum_proto::{SearchHit, SessionId};

#[derive(Props, Clone, PartialEq)]
struct HarnessProps {
    query: String,
    options: Options,
    answer: Option<Answer>,
    searching: bool,
    scope: usize,
}

#[component]
fn Harness(props: HarnessProps) -> Element {
    rsx! {
        Search {
            query: props.query.clone(),
            options: props.options,
            answer: props.answer.clone(),
            searching: props.searching,
            scope: props.scope,
            titles: vec![(SessionId(3), "claude".to_string())],
            on_query: move |_: String| {},
            on_toggle: move |_| {},
            on_submit: move |()| {},
            on_activate: move |_: (SessionId, u64)| {},
            on_dismiss: move |()| {},
        }
    }
}

fn render(props: HarnessProps) -> String {
    let mut dom = VirtualDom::new_with_props(Harness, props);
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

fn idle() -> HarnessProps {
    HarnessProps {
        query: String::new(),
        options: Options::default(),
        answer: None,
        searching: false,
        scope: 0,
    }
}

/// The component must build at all. A panic here is a window that dies on
/// Ctrl+Shift+F, which no pure-function test can catch.
#[test]
fn the_surface_renders_before_anything_has_been_searched() {
    let html = render(idle());
    assert!(html.contains("rg-search__input"), "{html}");
    assert!(html.contains("Pattern, then Enter"), "{html}");
    assert!(html.contains("rg-search__summary--idle"), "{html}");
    // Not searched yet means no results container at all, not an empty one.
    assert!(!html.contains("rg-search__results"), "{html}");
    // The three switches are words on the surface, not glyphs.
    for word in ["Regex", "Ignore case", "Whole word"] {
        assert!(html.contains(word), "{word} missing from {html}");
    }
}

/// **The test that justifies the whole `Vec<u8>` design.**
///
/// If you are here because `split_hit` looks like it does three redundant
/// decode calls and you want to simplify it to one: this test is why you
/// cannot. Decoding the line once and then indexing it with the daemon's
/// offsets is two lines shorter and WRONG, and it is wrong only on lines
/// containing a byte that is not valid UTF-8, which are precisely the
/// lines an operator is searching for when something has gone wrong.
///
/// Measured by mutation, not asserted: replacing the body with the
/// decode-then-index version makes this test fail with `t bo` against
/// `boom`. One `0xFF` decodes to a three-byte `U+FFFD`, so every offset
/// past it is two too small and the highlight slides onto the tail of
/// `post` plus the head of `boom`.
///
/// This asserts it where it matters, in the rendered DOM rather than in
/// the pure function, because a highlight computed perfectly and then
/// dropped or mis-nested by the markup would pass every unit test in
/// `search.rs`. Testing the input to the renderer is not testing the
/// renderer.
#[test]
fn an_invalid_byte_line_renders_the_highlight_on_the_matched_bytes() {
    let raw = b"pre \xff post boom tail".to_vec();
    let start = raw.windows(4).position(|w| w == b"boom").unwrap() as u32;
    let hit = SearchHit {
        session: SessionId(3),
        line_seq: 4096,
        visible: raw,
        match_start: start,
        match_end: start + 4,
        before: vec![b"context above".to_vec()],
        after: vec![b"context below".to_vec()],
    };
    let html = render(HarnessProps {
        answer: Some(Answer {
            pattern: "boom".to_string(),
            hits: vec![hit],
            truncated: false,
            bytes_scanned: 2 * 1024 * 1024,
        }),
        ..idle()
    });

    // The highlight span, and what is inside it.
    let mark = html
        .split_once("rg-search__mark\">")
        .expect("no highlight span in the rendered hit")
        .1
        .split_once("</span>")
        .expect("unterminated highlight span")
        .0;
    assert_eq!(mark, "boom", "the highlight is on the wrong substring");

    // The stray byte survives as one replacement character, in the
    // text BEFORE the mark, where it belongs.
    let pre = html
        .split_once("rg-search__pre\">")
        .expect("no pre span")
        .1
        .split_once("</span>")
        .unwrap()
        .0;
    assert_eq!(pre, "pre \u{fffd} post ");

    // Context lines render, and the session heading is the title rather
    // than the id.
    assert!(html.contains("context above"), "{html}");
    assert!(html.contains("context below"), "{html}");
    assert!(html.contains("claude"), "{html}");
    assert!(html.contains("1 match"), "{html}");
}

/// A truncated answer must carry the warning into the DOM, not just into a
/// String that some future refactor forgets to render.
#[test]
fn a_truncated_answer_renders_the_first_n_warning() {
    let hit = SearchHit {
        session: SessionId(3),
        line_seq: 1,
        visible: b"boom".to_vec(),
        match_start: 0,
        match_end: 4,
        before: Vec::new(),
        after: Vec::new(),
    };
    let html = render(HarnessProps {
        answer: Some(Answer {
            pattern: "boom".to_string(),
            hits: vec![hit],
            truncated: true,
            bytes_scanned: 1024,
        }),
        ..idle()
    });
    assert!(html.contains("rg-search__summary--truncated"), "{html}");
    assert!(html.contains("First 1 match"), "{html}");
    assert!(html.contains("There are more"), "{html}");
    assert!(html.contains("1.0 KiB"), "{html}");
}

/// EVERY hit and EVERY session must reach the DOM, not just the first.
///
/// Found by mutation, and it is the worst hole this suite has had.
/// `.take(1)` on either the group loop or the hit loop passed all 34
/// other tests, because every other render fixture holds one hit in one
/// session. The feature exists to answer "which of my twenty agents hit
/// an OOM"; a silent truncation to the first row is that question
/// answered wrongly, with a summary above it still honestly reporting
/// hits the operator cannot see. Structural guards cannot catch it: the
/// classes are all styled, the markup is well formed, and the count is
/// correct. Only counting the rendered rows can.
#[test]
fn every_hit_in_every_session_reaches_the_dom() {
    let line = |session: u64, seq: u64, text: &str| SearchHit {
        session: SessionId(session),
        line_seq: seq,
        visible: text.as_bytes().to_vec(),
        match_start: 0,
        match_end: 4,
        before: Vec::new(),
        after: Vec::new(),
    };
    let hits = vec![
        line(3, 1, "oom in alpha one"),
        line(3, 2, "oom in alpha two"),
        line(9, 3, "oom in beta one"),
        line(9, 4, "oom in beta two"),
    ];
    let html = render(HarnessProps {
        answer: Some(Answer {
            pattern: "oom".to_string(),
            hits,
            truncated: false,
            bytes_scanned: 1024,
        }),
        ..idle()
    });

    assert_eq!(
        html.matches("rg-search__hit").count(),
        4,
        "not every hit rendered: {html}"
    );
    assert_eq!(
        html.matches("rg-search__group\"").count(),
        2,
        "not every session rendered: {html}"
    );
    // The tail, not the whole line: `match_end: 4` splits each line
    // across the pre, mark and post spans, so the contiguous text never
    // appears in the DOM. Asserting on the unique remainder checks the
    // right thing and would have caught a swapped span too.
    for tail in ["in alpha one", "in alpha two", "in beta one", "in beta two"] {
        assert!(html.contains(tail), "{tail} missing from {html}");
    }
    assert_eq!(
        html.matches("rg-search__mark\">oom ").count(),
        4,
        "not every hit highlighted its match: {html}"
    );
    // The untitled session is headed by its id, beside the titled one.
    assert!(html.contains("claude"), "{html}");
    assert!(html.contains("Session 9"), "{html}");
}

/// Zero hits and not-yet-searched must produce visibly different DOM.
#[test]
fn no_matches_and_not_searched_yet_render_differently() {
    let none = render(HarnessProps {
        answer: Some(Answer {
            pattern: "nothing".to_string(),
            hits: Vec::new(),
            truncated: false,
            bytes_scanned: 4096,
        }),
        ..idle()
    });
    let never = render(idle());

    assert!(none.contains("rg-search__summary--none"), "{none}");
    assert!(none.contains("No matches for"), "{none}");
    assert!(never.contains("rg-search__summary--idle"), "{never}");
    assert_ne!(none, never, "the two empty states render identically");
    // Neither draws a results list.
    assert!(!none.contains("rg-search__results"));
    assert!(!never.contains("rg-search__results"));
}

/// Context lines must be sanitised too, not only the matched line.
///
/// Found by mutation: swapping `line_text(line)` for a bare
/// `String::from_utf8_lossy(line)` in the context markup passed the whole
/// suite. Every other test drives the MATCHED line, so the two context
/// loops either side of it were rendered by code nothing exercised. A
/// bare `\r` survives the daemon's escape stripper, and left in a flex
/// row it returns the cursor in any terminal this text is pasted into.
#[test]
fn a_control_byte_in_a_context_line_is_sanitised_in_the_dom() {
    let hit = SearchHit {
        session: SessionId(3),
        line_seq: 1,
        visible: b"needle".to_vec(),
        match_start: 0,
        match_end: 6,
        before: vec![b"above\rwrapped".to_vec()],
        after: vec![b"below\x1b[31mcoloured\x1b[0m".to_vec()],
    };
    let html = render(HarnessProps {
        answer: Some(Answer {
            pattern: "needle".to_string(),
            hits: vec![hit],
            truncated: false,
            bytes_scanned: 64,
        }),
        ..idle()
    });
    assert!(
        html.contains("above wrapped"),
        "the CR reached the DOM: {html}"
    );
    assert!(!html.contains("above\rwrapped"), "{html}");
    assert!(
        html.contains("belowcoloured"),
        "the SGR reached the DOM: {html}"
    );
    assert!(!html.contains("[31m"), "{html}");
}

/// A zero-width match must reach the DOM as the caret modifier.
#[test]
fn a_zero_width_match_renders_the_caret_modifier() {
    let hit = SearchHit {
        session: SessionId(3),
        line_seq: 1,
        visible: b"line".to_vec(),
        match_start: 2,
        match_end: 2,
        before: Vec::new(),
        after: Vec::new(),
    };
    let html = render(HarnessProps {
        answer: Some(Answer {
            pattern: "x*".to_string(),
            hits: vec![hit],
            truncated: false,
            bytes_scanned: 16,
        }),
        ..idle()
    });
    assert!(html.contains("rg-search__mark--empty"), "{html}");
}

/// A pressed switch must reach the DOM with both the modifier and the
/// accessibility state, or a screen reader reports every switch as off.
#[test]
fn a_pressed_switch_renders_its_modifier_and_aria_state() {
    let html = render(HarnessProps {
        options: Options::default().toggled(super::Toggle::Regex),
        ..idle()
    });
    assert!(html.contains("rg-search__opt--on"), "{html}");
    assert!(
        html.contains("aria-pressed:true") || html.contains("aria-pressed=\"true\""),
        "{html}"
    );
}
