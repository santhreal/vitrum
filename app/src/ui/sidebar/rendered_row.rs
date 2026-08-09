//! The session row, RENDERED.
//!
//! Every other guard in this file reads `sidebar.rs` as text or calls a pure
//! function. Both pass while the markup that reaches the operator is wrong,
//! and that is not hypothetical: this product shipped a status dot with four
//! colour modifiers and no box, a "Show" button on every notification that
//! could not be clicked, and a whole search result path the client discarded.
//! Each was correct code with one missing link, and a green suite the whole
//! time. A test that builds the component and looks at the HTML is the only
//! kind that can see that class of defect.

use super::*;
use crate::testkit::{NOW, row};

#[derive(Props, Clone, PartialEq)]
struct HarnessProps {
    row: SessionView,
    section: Section,
    fields: RowFields,
    contested: Option<(usize, usize)>,
}

#[component]
fn Harness(props: HarnessProps) -> Element {
    rsx! {
        SessionRow {
            row: props.row.clone(),
            section: props.section,
            fields: props.fields,
            active: false,
            picked: false,
            clock: TimeFormat::new(vitrum_fmt::Timestamp::from_millis(NOW as i64), 0),
            home: Rc::from("/home/u"),
            contested: props.contested,
            on_select: move |_: (SessionId, Click)| {},
            on_close: move |_: SessionId| {},
            on_menu: move |_: (f64, f64, SessionId)| {},
        }
    }
}

/// Every optional row element on, which is the shipped default.
fn all_fields() -> RowFields {
    RowFields {
        branch: true,
        time: true,
        status_word: true,
        always_slim: false,
    }
}

/// One row's HTML, exactly as the webview would receive it.
fn render(view: SessionView, section: Section) -> String {
    render_with(view, section, all_fields())
}

/// One row's HTML with the operator's row-element switches set explicitly.
fn render_with(view: SessionView, section: Section, fields: RowFields) -> String {
    let mut dom = VirtualDom::new_with_props(
        Harness,
        HarnessProps {
            row: view,
            section,
            fields,
            contested: None,
        },
    );
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

/// The row must build at all, in both shapes.
///
/// A panic in `SessionRow` is an empty sidebar, which is what the operator
/// reported seeing, and no pure-function test in this file can produce it.
#[test]
fn a_row_renders_in_both_shapes() {
    for section in [Section::Active, Section::Settled] {
        let html = render(row(4).title("review auth").build(), section);
        assert!(
            html.contains("review auth"),
            "the {section:?} row rendered without its title: {html}"
        );
    }
}

/// Each agent gets ITS OWN mark, in the rendered output.
///
/// The marks and the resolver were written, tested and correct, and the
/// only surface that drew them was the tab strip. Deleting the strip
/// deleted every drawn agent identity in the product, and `agent.rs` went
/// dead without one assertion failing. This is the link that was missing:
/// it asserts the exact path data reaches the HTML, so an unwired mark is
/// a failure here rather than a blank sidebar nobody's test can see.
#[test]
fn each_agent_draws_its_own_mark_on_the_row() {
    let cases = [
        ("claude", AgentKind::Claude),
        ("codex", AgentKind::Codex),
        ("gemini", AgentKind::Gemini),
        ("opencode", AgentKind::Opencode),
        ("veyyon", AgentKind::Veyyon),
        ("bash", AgentKind::Shell),
        ("some-unknown-tool", AgentKind::Unknown),
    ];
    let mut drawn = Vec::new();
    for (command, kind) in cases {
        let html = render(row(4).command(command).build(), Section::Active);
        let stroke = kind.mark().stroke;
        assert!(
            html.contains(stroke),
            "`{command}` rendered without {kind:?}'s mark; the row draws no \
             agent identity at all: {html}"
        );
        drawn.push(stroke);
    }
    let mut unique = drawn.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        drawn.len(),
        "two agents drew the same mark, so the row cannot tell them apart"
    );
}

/// THE PRODUCT'S HEADLINE PROMISE, in the painted markup: a Codex session that
/// is blocked on an approval gate must say so on its row.
///
/// The bug this closes was live. A `codex` session sitting on "Would you like
/// to run the following command? 1. Yes, proceed" rendered as `Ready`: the pane
/// said the operator was blocked and the row said nothing was needed. Codex
/// announces the state in its terminal title, which is a channel already being
/// parsed, so nothing about it was unknowable.
///
/// Rendered rather than resolved, because a correct `Pill` that never reaches
/// the markup is exactly the class of defect this file exists for.
#[test]
fn a_codex_row_blocked_on_approval_paints_the_hedged_approval_pill() {
    let html = render(
        row(4)
            .command("codex")
            .title("[ ! ] Action Required - codex")
            .waiting(Some(true))
            .build(),
        Section::Active,
    );
    assert!(
        html.contains("rg-pill--approval"),
        "a blocked Codex row painted no approval pill: {html}"
    );
    assert!(
        html.contains("rg-pill--inferred"),
        "a title-derived state must paint the hedge, not imply certainty: {html}"
    );

    // The same row before the banner went up, so the assertions above are
    // pinned to the title and not to something every row happens to carry.
    let quiet = render(
        row(4)
            .command("codex")
            .title("codex")
            .waiting(Some(true))
            .build(),
        Section::Active,
    );
    assert!(!quiet.contains("rg-pill--approval"), "{quiet}");
    assert!(!quiet.contains("rg-pill--inferred"), "{quiet}");
}

/// The mark carries the status hue, and only one of the four.
///
/// The mark answers WHO and its colour answers WHETHER it is still
/// running. Two hues on one element, or none, is a row that either
/// contradicts itself or renders the mark in dead grey while the pill
/// beside it says running.
#[test]
fn the_mark_carries_exactly_one_status_hue() {
    let cases = [
        (row(4).running().build(), "rg-session__agent--running"),
        (row(4).exited(Some(0)).build(), "rg-session__agent--exited"),
        (
            row(4).exited(Some(1)).build(),
            "rg-session__agent--exited-error",
        ),
    ];
    let all = [
        "rg-session__agent--starting",
        "rg-session__agent--running",
        "rg-session__agent--exited",
        "rg-session__agent--exited-error",
    ];
    for (view, expected) in cases {
        let html = render(view, Section::Active);
        // `--exited` is a prefix of `--exited-error`, so count whole
        // class tokens rather than substrings or this passes on both.
        let present: Vec<&str> = all
            .iter()
            .copied()
            .filter(|m| {
                html.match_indices(m).any(|(at, _)| {
                    html[at + m.len()..]
                        .chars()
                        .next()
                        .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                })
            })
            .collect();
        assert_eq!(
            present,
            vec![expected],
            "the mark must carry exactly one hue and it must be {expected}"
        );
    }
}

/// The mark must never make a title's left edge move.
///
/// It is the first thing on the line, so if its box could vary with the
/// agent then the list would lose the vertical rail every title shares and
/// twenty rows would read as ragged. The mark is `flex: 0 0 auto` at a
/// fixed 16px; what this proves is the markup half of that, that every
/// agent emits the same single element before the title with no extra
/// content wedged in.
#[test]
fn every_agent_puts_the_same_box_before_the_title() {
    let mut prefixes = Vec::new();
    for command in ["claude", "codex", "gemini", "opencode", "veyyon", "bash"] {
        let html = render(row(4).command(command).build(), Section::Active);
        let at = html
            .find("rg-session__title")
            .unwrap_or_else(|| panic!("`{command}` rendered no title: {html}"));
        // Everything before the title, with the two things that legitimately
        // differ by agent removed: the path data and the status hue.
        let head = &html[..at];
        // The ELEMENT, not the substring: a mark's class attribute reads
        // `rg-session__agent rg-session__agent--running`, so counting the
        // bare token finds two of them in one box.
        let marks = head.matches("class=\"rg-session__agent").count();
        prefixes.push((command, marks, head.matches("<svg").count()));
    }
    for (command, marks, svgs) in &prefixes {
        assert_eq!(
            (*marks, *svgs),
            (1, 1),
            "`{command}` put {marks} marks and {svgs} svgs before the title; \
             every row must put exactly one"
        );
    }
}

/// The tab strip must not come back.
///
/// It was a second switcher for the selection the sidebar already makes,
/// and it cost a whole chrome band above the terminal. The removal is only
/// durable if re-adding strip markup fails a test rather than quietly
/// restoring three bands of chrome.
#[test]
fn no_row_emits_tab_strip_markup() {
    let html = render(row(4).command("claude").build(), Section::Active);
    for banned in ["rg-tab", "rg-tabs", "rg-overflow"] {
        let needle = format!("\"{banned}");
        assert!(
            !html.contains(&needle) && !html.contains(&format!(" {banned} ")),
            "the row emits {banned}, which is tab strip markup: {html}"
        );
    }
}

/// The tooltip must name the agent.
///
/// Sixty real sessions produced fifty-seven rows reading `bash`, so the
/// title alone does not say what is in a session. The mark says it at a
/// glance and the tooltip says it in words; a mark with no word is a
/// shape the operator has to learn before it means anything.
#[test]
fn the_tooltip_names_the_agent_behind_the_session() {
    for (command, label) in [
        ("claude", "Claude Code"),
        ("codex", "Codex"),
        ("bash", "shell"),
        ("some-unknown-tool", "unknown agent"),
    ] {
        let view = row(4).command(command).cwd("/src/vitrum").build();
        let tip = row_tooltip(&view, "/home/u");
        assert!(
            tip.contains(label),
            "a `{command}` session's tooltip never says `{label}`: {tip}"
        );
    }
    let _ = NOW;
}

/// WHY: "Show the status word" round-tripped to disk and changed nothing.
///
/// `RowFields::status_word` was read out of `Settings`, carried into every
/// row's props and compared by `PartialEq`, and then no markup ever looked at
/// it: `rg-pill__word` was emitted unconditionally. So the switch persisted,
/// re-rendered the whole sidebar, and left the row identical. That is exactly
/// the failure `settings.rs`'s own module doc forbids in its first paragraph,
/// and no test in this file could see it, because every one of them rendered
/// with the field on.
///
/// Off drops the word and keeps the pill, which is what the collapsed rail
/// already draws through `.rg-sidebar--collapsed .rg-pill__word`. The
/// accessible name is asserted in both directions because dropping the word
/// from the DOM must not drop the state from a screen reader.
#[test]
fn the_status_word_switch_adds_and_removes_the_word() {
    let view = row(4).title("review auth").build();
    let word = inbox::status_word(inbox::StateWord::of(Pill::of(&view).status));

    let on = render_with(view.clone(), Section::Active, all_fields());
    assert!(
        on.contains(&format!(r#"<span class="rg-pill__word">{word}</span>"#)),
        "the word is missing with the switch on: {on}"
    );

    let off = render_with(
        view,
        Section::Active,
        RowFields {
            status_word: false,
            ..all_fields()
        },
    );
    assert!(
        !off.contains("rg-pill__word"),
        "the word survived the switch being off: {off}"
    );
    assert!(
        off.contains("rg-pill "),
        "the pill's own box and hue must stay: {off}"
    );
    assert!(
        off.contains(&format!(r#"aria-label="{word}""#)),
        "the state was lost to a screen reader, not just to the column: {off}"
    );
}
