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
use crate::testkit::{HOUR, NOW, row};

#[derive(Props, Clone, PartialEq)]
struct HarnessProps {
    row: SessionView,
    section: Section,
    fields: RowFields,
    contested: Option<(usize, usize)>,
    /// The project directory this row was grouped under.
    root: Rc<str>,
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
            root: Rc::clone(&props.root),
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
        place: true,
        always_slim: false,
    }
}

/// One row's HTML, exactly as the webview would receive it.
fn render(view: SessionView, section: Section) -> String {
    render_with(view, section, all_fields())
}

/// One row's HTML with the operator's row-element switches set explicitly.
fn render_with(view: SessionView, section: Section, fields: RowFields) -> String {
    render_under(view, section, fields, "")
}

/// One row's HTML, grouped under a given project directory.
fn render_under(view: SessionView, section: Section, fields: RowFields, root: &str) -> String {
    let mut dom = VirtualDom::new_with_props(
        Harness,
        HarnessProps {
            row: view,
            section,
            fields,
            contested: None,
            root: Rc::from(root),
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
            .term_title("[ ! ] Action Required - codex")
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
        let tip = row_tooltip(&view, "/home/u", &crate::inbox::Pill::of(&view));
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

/// WHY: a `title` attribute is a tooltip this product does not control, and
/// reordering a list under a stationary cursor strands it over the wrong row.
///
/// The defect class: the platform paints a `title` in its own window, on its
/// own schedule, anchored to the POINTER rather than to the element. The
/// sidebar reorders on every status change — a row goes Approval and lifts to
/// the top of its band — and when it does, the row under the cursor changes
/// while the tooltip does not. The operator is then reading facts about a
/// session that is now three rows away, and there is no CSS, no z-index and
/// no re-render that can reach the surface saying them. The visible artefact
/// was a panel that lagged a reorder and a stale string over a live row.
///
/// The fix replaced every in-row `title=` with one `.rg-session__tip` span
/// that is a CHILD of the row: the same layout that moves the row moves it,
/// and `:hover` is recomputed by that layout, so a frame where the two
/// disagree cannot exist.
///
/// So the assertion is ABSENCE, over the row's whole state space, because
/// one surviving `title=` on one state is the whole defect back. The states
/// are built from [`vitrum_model::ALL_STATUSES`] and [`HintState::ALL`] at
/// run time and the coverage is asserted against `ALL_STATUSES`, so a sixth
/// status turns this red until somebody adds a row that produces it. The
/// other axes — section, the operator's row-element switches, and the
/// contested badge, which carried a `title` of its own — are multiplied
/// through, because each of them emits different markup.
///
/// What it does NOT catch: a `title` on a surface outside a session row.
/// The panel's own controls legitimately keep theirs; they do not reorder.
#[test]
fn no_row_asks_the_platform_for_a_tooltip() {
    // A row per state, keyed by the state it actually resolves to rather
    // than by the one it was built for: the resolver is what decides, and a
    // case that stopped producing its status must not silently drop out of
    // the matrix.
    let mut cases: Vec<(String, SessionView)> = Vec::new();
    for hint in vitrum_proto::HintState::ALL {
        cases.push((
            format!("hint {hint:?}"),
            row(4)
                .command("claude")
                .title("review auth")
                .cwd("/src/vitrum")
                .running()
                .hint(hint, None, NOW)
                .build(),
        ));
    }
    cases.push((
        "a bad exit".to_string(),
        row(4).command("codex").exited(Some(1)).build(),
    ));
    // THE BADGE CALL SITES. Four of the deleted `title` attributes were on
    // badges, not on the row box, and a matrix of live and exited rows never
    // renders one: a badge needs a disposition or an unseen completion. A
    // mutation that puts `title` back on a badge stays green without these,
    // which is the same as not testing those call sites at all.
    cases.push((
        "a finish nobody has seen".to_string(),
        row(4).command("codex").exited(Some(0)).build(),
    ));
    cases.push((
        "snoozed".to_string(),
        row(4)
            .command("gemini")
            .running()
            .snooze(NOW - HOUR, NOW + HOUR)
            .build(),
    ));
    cases.push((
        "woken by its own output".to_string(),
        row(4)
            .command("gemini")
            .running()
            .waiting(Some(true))
            .snooze(NOW - 2 * HOUR, NOW - HOUR)
            .last_activity_ms(NOW - 60_000)
            .build(),
    ));

    // The done badge has its own call site, and no row above reaches it: a
    // finished-unseen row is a live session whose agent declared Ready after
    // the last visit, not an exited one.
    cases.push((
        "finished while nobody was looking".to_string(),
        row(5)
            .command("claude")
            .running()
            .waiting(Some(true))
            .hint(vitrum_proto::HintState::Ready, None, NOW - 5 * 60_000)
            .visited(NOW - HOUR)
            .last_activity_ms(NOW - 5 * 60_000)
            .build(),
    ));

    let covered: Vec<SidebarStatus> = cases
        .iter()
        .map(|(_, view)| Pill::of(view).status)
        .collect();
    for status in vitrum_model::ALL_STATUSES {
        assert!(
            covered.contains(&status),
            "{} is a status the sidebar can show and no row in this matrix \
             produces it, so nothing here proves it drops its title",
            status.label()
        );
    }

    let shapes = [
        ("everything on", all_fields()),
        (
            "no branch, no time",
            RowFields {
                branch: false,
                time: false,
                ..all_fields()
            },
        ),
        (
            "no status word",
            RowFields {
                status_word: false,
                ..all_fields()
            },
        ),
        (
            "always slim",
            RowFields {
                always_slim: true,
                ..all_fields()
            },
        ),
    ];

    // Every element that used to carry a `title`, so a mutation putting one
    // back on any of them has something to be caught on. Without this the
    // matrix can silently stop rendering a badge and go on passing.
    let mut drew: Vec<&str> = Vec::new();
    let mut rendered = 0usize;
    for (what, view) in &cases {
        for section in [Section::Active, Section::Snoozed, Section::Settled] {
            for (shape, fields) in &shapes {
                // `None` and a real contest: the contested badge and the
                // contested note each used to carry a `title` of their own.
                for contested in [None, Some((3usize, 2usize))] {
                    let html = render_case(view.clone(), section, *fields, contested);
                    rendered += 1;
                    if let Some(at) = title_attribute(&html) {
                        panic!(
                            "a {what} row ({shape}, {section:?}, contested \
                             {contested:?}) still asks the platform for a \
                             tooltip at byte {at}; a reorder strands it over \
                             another session: {html}"
                        );
                    }
                    assert!(
                        html.contains("rg-session__tip"),
                        "a {what} row ({shape}, {section:?}) dropped its \
                         title and grew nothing in its place, so the row's \
                         facts are now unreachable: {html}"
                    );
                    for element in [
                        "rg-pill",
                        "rg-badge--done",
                        "rg-badge--snoozed",
                        "rg-badge--woke",
                        "rg-session__contest",
                    ] {
                        if html.contains(element) && !drew.contains(&element) {
                            drew.push(element);
                        }
                    }
                }
            }
        }
    }
    for element in [
        "rg-pill",
        "rg-badge--done",
        "rg-badge--snoozed",
        "rg-badge--woke",
        "rg-session__contest",
    ] {
        assert!(
            drew.contains(&element),
            "no row in this matrix ever drew .{element}, which is one of the \
             elements the deleted `title` attributes were on, so nothing \
             here proves that call site dropped it"
        );
    }
    assert_eq!(
        rendered,
        cases.len() * 3 * shapes.len() * 2,
        "the matrix collapsed rather than the markup"
    );
}

/// Byte offset of a `title` ATTRIBUTE, if the markup carries one.
///
/// `contains("title=")` is not the test: `rg-session__title` and the row's
/// own `data-` payloads contain the word, and an attribute name only starts
/// after whitespace inside a tag. This looks for the token with a boundary
/// in front of it, which is what the parser sees.
fn title_attribute(html: &str) -> Option<usize> {
    html.match_indices("title=").find_map(|(at, _)| {
        let before = html[..at].chars().next_back()?;
        (before.is_ascii_whitespace() || before == '<').then_some(at)
    })
}

/// One row's HTML with every axis of its state set explicitly.
fn render_case(
    view: SessionView,
    section: Section,
    fields: RowFields,
    contested: Option<(usize, usize)>,
) -> String {
    let mut dom = VirtualDom::new_with_props(
        Harness,
        HarnessProps {
            row: view,
            section,
            fields,
            contested,
            root: Rc::from("/src/vitrum"),
        },
    );
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

/// WHY: `title` is a plain string on the wire and the empty string is a legal
/// value of it, so a session can reach the panel with no name at all.
///
/// The defect class is an element that is present, laid out and empty. The
/// row still draws its agent mark, its status pill, its badges and its close
/// button, and the one thing saying WHICH session this is becomes a
/// zero-width gap between the mark and the status. It is not a crash and not
/// a blank panel, so every guard in this file that asks "did the row render"
/// stays green while the list is unusable. `ui/search.rs` had already refused
/// to do it and fell back to `Session {n}`; the sidebar, the row menu and the
/// notification title all read `inbox::row_title`, and that function did not,
/// so all three drew the blank.
///
/// It is asserted over the WHOLE state space rather than on one row, because
/// the row has two shapes and three bands and the nameless line is drawn by
/// all of them. The status axis is [`vitrum_model::ALL_STATUSES`]. The
/// disposition axis is parsed out of the `Disposition` enum in
/// `vitrum-model/src/disposition.rs` at run time rather than listed here, so
/// a fifth disposition turns this red until somebody gives it a mechanism and
/// a ruling. Each case is keyed by the state it RESOLVES to and that
/// resolution is asserted, so a mechanism that quietly stops producing its
/// disposition fails here instead of shrinking the matrix in silence.
///
/// The pairs that never occur are ruled on rather than skipped:
/// [`vitrum_model::SessionView::blocks_on_operator`] holds a row in the inbox
/// regardless of a snooze or an explicit settle, so Approval and Input are
/// Active under every mechanism, and this asserts that instead of assuming
/// it.
///
/// The mutations this catches: `row_title` handing back `&info.title` for an
/// empty title, which draws a nameless line in every cell of the matrix; the
/// fallback applied at the sidebar's card call site only, which leaves the
/// slim shape and the hover detail blank; a fallback that is a constant
/// rather than keyed on the session id, which collapses two unnamed sessions
/// to one name; and a hover detail that stops opening with the row's name.
///
/// What it does NOT catch: a name that is non-empty and still useless, and
/// anything about how the title is truncated or elided by CSS.
#[test]
fn a_session_with_no_title_still_draws_a_whole_row_in_every_state() {
    let clock = TimeFormat::new(vitrum_fmt::Timestamp::from_millis(NOW as i64), 0);
    let model_clock = inbox::model_clock(clock);
    let policy = vitrum_model::DispositionPolicy::default();

    // THE STATUS AXIS. One nameless base row per state the sidebar can show.
    // Declared states come from the hint channel and Failed from a bad exit,
    // which is how the resolver reaches each of them without guessing; the
    // coverage is checked against ALL_STATUSES below rather than trusted.
    let bases: Vec<SessionView> = vec![
        row(4)
            .command("claude")
            .title("")
            .running()
            .hint(vitrum_proto::HintState::Approval, None, NOW)
            .build(),
        row(4)
            .command("claude")
            .title("")
            .running()
            .hint(vitrum_proto::HintState::Input, None, NOW)
            .build(),
        row(4)
            .command("gemini")
            .title("")
            .running()
            .hint(vitrum_proto::HintState::Working, None, NOW)
            .build(),
        row(4)
            .command("codex")
            .title("")
            .running()
            .hint(vitrum_proto::HintState::Ready, None, NOW)
            .build(),
        row(4).command("codex").title("").exited(Some(1)).build(),
    ];

    // THE DISPOSITION AXIS, one mechanism per variant. Each is the real way
    // the product reaches that state: a lapsed snooze nobody has looked at
    // since, a live snooze too fresh for anything before it to count as a
    // raised hand, and the operator's own settle ruling.
    let mechanisms: [(&str, Disposition, fn(SessionView) -> SessionView); 4] = [
        ("Active", Disposition::Active, |view| view),
        ("Woke", Disposition::Woke, |mut view| {
            view.snooze = Some(vitrum_model::Snooze {
                snoozed_at_ms: NOW - 3 * HOUR,
                wake_at_ms: NOW - 2 * HOUR,
            });
            view
        }),
        // Parked AFTER the row's last activity. A snooze stamped before it is
        // a raised hand for any row whose agent is waiting at a prompt, which
        // is the product's rule and would make this mechanism produce Woke for
        // half the status axis instead of the state it is here to cover.
        ("Snoozed", Disposition::Snoozed, |mut view| {
            view.snooze = Some(vitrum_model::Snooze {
                snoozed_at_ms: NOW,
                wake_at_ms: NOW + HOUR,
            });
            view
        }),
        ("Settled", Disposition::Settled, |mut view| {
            view.settle_override = Some(vitrum_model::SettleOverride::Settled);
            view
        }),
    ];

    let declared = disposition_variants();
    let wired: Vec<String> = mechanisms
        .iter()
        .map(|(name, _, _)| (*name).to_string())
        .collect();
    assert_eq!(
        wired, declared,
        "`Disposition` in vitrum-model declares {declared:?} and this matrix \
         has mechanisms for {wired:?}. A disposition with no mechanism is a \
         band nothing here ever renders an unnamed row in."
    );

    let mut produced: Vec<(SidebarStatus, Disposition)> = Vec::new();
    for base in &bases {
        let status = Pill::of(base).status;
        for (name, want, apply) in &mechanisms {
            let view = (*apply)(base.clone());
            let got = view.disposition(model_clock, policy);
            if view.blocks_on_operator() && *want != Disposition::Active {
                assert_eq!(
                    got,
                    Disposition::Active,
                    "the {name} mechanism parked a {} row. Blocking on the \
                     operator outranks a snooze and an explicit settle, or an \
                     approval request can be hidden from the one person who \
                     can answer it.",
                    status.label()
                );
            } else if !base.info.status.is_live()
                && !base.has_unseen_completion()
                && *want == Disposition::Active
            {
                // A process that is gone, and looked at, settles. The Active
                // mechanism changes nothing about a row, so on an exited base
                // there is nothing for it to hold against that rule; the other
                // three mechanisms still have to produce their own state.
                assert_eq!(
                    got,
                    Disposition::Settled,
                    "an exited {} row with no unseen completion must settle",
                    status.label()
                );
            } else {
                assert_eq!(
                    got,
                    *want,
                    "the {name} mechanism stopped producing {want:?} for a {} \
                     row, so this matrix silently covers less than it claims",
                    status.label()
                );
            }

            // The band the panel would really put it in, so the row is
            // rendered in the shape it actually ships in.
            let section = got.section();
            let html = render_case(view.clone(), section, all_fields(), None);
            let cell = format!("{} / {got:?} / {section:?}", status.label());

            let Some(title) = element_text(&html, "rg-session__title") else {
                panic!("{cell}: the row drew no title element at all: {html}");
            };
            assert!(
                !title.trim().is_empty(),
                "{cell}: the row's name is blank, so the only thing saying \
                 which session this is is a gap between the agent mark and \
                 the status: {html}"
            );
            assert!(
                title.contains(&view.id().0.to_string()),
                "{cell}: an unnamed row must still be told apart from the next \
                 unnamed row, and {title:?} names nothing unique: {html}"
            );
            assert!(
                html.contains("rg-session__agent"),
                "{cell}: the row lost its agent mark: {html}"
            );
            assert!(
                html.contains("rg-session__close"),
                "{cell}: the row lost its close button: {html}"
            );

            let Some(tip) = element_text(&html, "rg-session__tip") else {
                panic!("{cell}: the row drew no hover detail: {html}");
            };
            assert!(
                tip.starts_with(title),
                "{cell}: the hover detail opens {tip:?} rather than with the \
                 row's own name, so two surfaces name one session two ways: \
                 {html}"
            );

            // Only the Active band draws a card, and the pill is the card's.
            if section == Section::Active {
                let modifier = inbox::status_modifier(status);
                assert!(
                    html.contains(modifier),
                    "{cell}: the card drew no .{modifier} pill, so a nameless \
                     row is also a stateless one: {html}"
                );
            }

            if !produced.contains(&(status, got)) {
                produced.push((status, got));
            }
        }
    }

    for status in vitrum_model::ALL_STATUSES {
        assert!(
            produced.iter().any(|(s, _)| *s == status),
            "{} is a state the sidebar can show and no base row in this \
             matrix produces it, so nothing here proves an unnamed row in it \
             draws a name",
            status.label()
        );
    }
    for name in &declared {
        assert!(
            produced.iter().any(|(_, d)| format!("{d:?}") == *name),
            "no row in this matrix ever resolved to {name}, so nothing here \
             proves an unnamed row draws a name in that band"
        );
    }

    // The suffix is the session id and nothing softer. Two unnamed rows that
    // read the same are the defect with an extra step.
    let one = render_case(
        row(4).command("claude").title("").running().build(),
        Section::Active,
        all_fields(),
        None,
    );
    let two = render_case(
        row(9).command("claude").title("").running().build(),
        Section::Active,
        all_fields(),
        None,
    );
    assert_ne!(
        element_text(&one, "rg-session__title"),
        element_text(&two, "rg-session__title"),
        "two unnamed sessions drew the same name, which is exactly what the \
         id suffix exists to prevent"
    );
}

/// The `Disposition` variants, read out of the enum at run time.
///
/// A list written here would go stale the moment a fifth disposition lands,
/// and go stale in silence, which is the same as not having the axis at all.
/// The enum is the source of truth; this reads it.
fn disposition_variants() -> Vec<String> {
    let src = include_str!("../../../../crates/vitrum-model/src/disposition.rs");
    let (_, rest) = src
        .split_once("pub enum Disposition {")
        .expect("vitrum-model declares `pub enum Disposition`");
    let (body, _) = rest
        .split_once('}')
        .expect("the `Disposition` enum closes");
    body.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//") && !line.starts_with('#'))
        .filter_map(|line| line.strip_suffix(','))
        .map(str::to_string)
        .collect()
}

/// The text inside the first element carrying exactly `class`.
///
/// The closing quote is part of the match, so `rg-session__title` does not
/// also find the line wrapper's `rg-session__line--title`. Text-only
/// elements, which is every element this is asked for.
fn element_text<'a>(html: &'a str, class: &str) -> Option<&'a str> {
    let at = html.find(&format!("class=\"{class}\""))?;
    let open = at + html[at..].find('>')? + 1;
    let close = open + html[open..].find('<')?;
    Some(&html[open..close])
}

/// WHY (defect 7): a Gemini or Claude Code trust prompt renders `Working`,
/// and that is the honest answer rather than a fix somebody forgot to write.
///
/// Both agents open with "Do you trust the files in this folder?" and sit
/// there until it is answered. The row says Working the whole time, which
/// reads as a miss of the product's headline promise, so the reason it is not
/// one belongs attached to a test rather than rediscovered every time
/// somebody notices it.
///
/// The daemon's waiting probe answers one question: is the foreground process
/// blocked reading the terminal. Node never blocks in `read(2)` on stdin.
/// libuv registers the tty fd with the event loop and the process waits in
/// `epoll_wait` — `kqueue` on macOS — which is not a terminal read. So for a
/// Node TUI the probe is `Some(false)` for the entire life of the process,
/// prompt on screen or not, and the resolver's honest arm for a live session
/// that is not blocked on its terminal is Working.
///
/// No correct non-guessing rule closes it. `epoll` says nothing about what is
/// being waited FOR, and "the tty is in the set" is true of a Node TUI from
/// the moment it starts, so even the Linux-only introspection answers a
/// different question. Scraping the pane for the sentence is a guess that
/// goes wrong on any transcript quoting it, and a wrong Approval is worse
/// than a late one: it teaches the operator to disbelieve the column the
/// whole panel is sorted by. What does close it is a DECLARATION — a
/// [`vitrum_proto::HintState`] or a terminal-title claim — which is why both
/// channels exist and why both are consulted before the probe.
///
/// The mutations this catches: an arm inferring Approval from
/// `waiting == Some(false)` on a known Node agent; an arm inferring it from
/// the command name alone; and the `--inferred` hedge painted on a state that
/// came from the probe rather than from a claim, which would dress a
/// certainty up as a guess.
///
/// What it does NOT catch: anything about the probe itself, which is
/// `vitrum-core`'s, or a declaration channel quietly ceasing to be consulted.
#[test]
fn a_node_agents_trust_prompt_reads_as_working_and_claims_nothing_else() {
    for command in ["gemini", "claude"] {
        let html = render(
            row(4)
                .command(command)
                .title("review auth")
                .running()
                // What the probe reports for a Node TUI, prompt or no prompt.
                .waiting(Some(false))
                .build(),
            Section::Active,
        );
        assert!(
            html.contains("rg-pill--working"),
            "`{command}` with the probe saying not-blocked drew something \
             other than Working, so a state was invented from a signal that \
             cannot carry it: {html}"
        );
        assert!(
            !html.contains("rg-pill--approval") && !html.contains("rg-pill--input"),
            "`{command}` claimed the operator is blocked on a state nothing \
             declared: {html}"
        );
        assert!(
            !html.contains("rg-pill--inferred"),
            "the hedge belongs to a title-derived claim; a probe answer is \
             not a claim and must not wear it: {html}"
        );
    }
}

/// WHY: the working directory has to reach the MARKUP, and it has to be the
/// one the session is in now.
///
/// This is the defect class this file exists for, and the one the feature is
/// most exposed to: `RowFields::status_word` was read, carried through props,
/// compared by `PartialEq` and then never consulted by any markup, so the
/// switch persisted and changed nothing. A directory resolved correctly by
/// `place_label` and never emitted would fail in exactly that way.
///
/// The four arms are the whole rule. A row at the project root that is on a
/// branch emits an EMPTY element, because the group header above it stands
/// for that path and `.rg-session__place:empty` collapses the box; the SAME
/// row with no branch emits its directory, because otherwise the whole
/// context line goes blank; a row below the root emits the remainder; a row
/// outside the project entirely — a worktree, or a session an agent moved
/// with OSC 7 — emits its own home-shortened path, which is the case where
/// the group header is actively misleading.
///
/// The switch is asserted in both directions, because a control that
/// round-trips to disk and changes no markup is the defect above.
///
/// What it does NOT catch: the CSS actually collapsing the empty element, or
/// anything about how the daemon decides a session moved.
#[test]
fn the_rows_working_directory_reaches_the_markup() {
    let at_root = render_under(
        row(1).cwd("/src/vitrum").branch(Some("main")).build(),
        Section::Active,
        all_fields(),
        "/src/vitrum",
    );
    assert_eq!(
        element_text(&at_root, "rg-session__place"),
        Some(""),
        "a row at the project root with a branch must emit the element and \
         leave it empty"
    );

    let at_root_no_branch = render_under(
        row(1).cwd("/src/vitrum").branch(None).build(),
        Section::Active,
        all_fields(),
        "/src/vitrum",
    );
    assert_eq!(
        element_text(&at_root_no_branch, "rg-session__place"),
        Some("/src/vitrum"),
        "a root row outside a repository must not leave its context line blank"
    );

    let below = render_under(
        row(1).cwd("/src/vitrum/crates/vitrum-core").build(),
        Section::Active,
        all_fields(),
        "/src/vitrum",
    );
    assert_eq!(
        element_text(&below, "rg-session__place"),
        Some("crates/vitrum-core"),
        "a row below the root must draw the remainder"
    );

    let outside = render_under(
        row(1).cwd("/home/u/worktrees/topic").build(),
        Section::Active,
        all_fields(),
        "/src/vitrum",
    );
    assert_eq!(
        element_text(&outside, "rg-session__place"),
        Some("~/worktrees/topic"),
        "a row outside the project must draw where it actually is"
    );

    let off = RowFields {
        place: false,
        ..all_fields()
    };
    assert_eq!(
        element_text(
            &render_under(
                row(1).cwd("/src/vitrum/app").build(),
                Section::Active,
                off,
                "/src/vitrum",
            ),
            "rg-session__place"
        ),
        Some(""),
        "the switch is off and the directory is still drawn"
    );
}
