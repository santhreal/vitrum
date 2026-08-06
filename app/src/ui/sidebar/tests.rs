use super::*;
use crate::testkit::{HOUR, NOW, row};
use vitrum_model::{Clock, SidebarStatus};
use vitrum_proto::{Attention, HintState, IDLE_ATTENTION_MS, SessionStatus};

fn clock() -> Clock {
    Clock::utc(NOW)
}

/// One chevron glyph, rotated by CSS. Emitting a second, right-pointing
/// glyph for the collapsed state would double-apply with
/// `.rg-project--collapsed`'s `rotate(-90deg)` and leave a collapsed group
/// pointing up, which reads as "expanded" to anyone who has used a tree
/// before.
#[test]
fn one_chevron_glyph_is_emitted_and_the_stylesheet_turns_it() {
    assert_eq!(CHEVRON, "\u{25be}");

    let shipped = shipped_markup();
    assert!(
        !shipped.contains("\\u{25b8}"),
        "the markup still carries a right-pointing chevron, so collapsing double-rotates"
    );

    let css = include_str!("../../../assets/sidebar.css");
    for state in [
        ".rg-project--collapsed .rg-project__chevron",
        ".rg-project__section--collapsed .rg-project__chevron",
    ] {
        assert!(
            css.contains(state),
            "{state} has no rule, so that disclosure never turns"
        );
    }
}

/// The shipped half of this file, without the test module.
///
/// Several tests below read the markup as text, which is the only way to
/// assert element ORDER without a layout engine in the crate. They have to
/// stop at the test module or they match their own assertions.
fn shipped_markup() -> &'static str {
    let markup = include_str!("../sidebar.rs");
    markup
        .split_once("#[cfg(test)]")
        .expect("this file has a test module")
        .0
}

/// Class tokens the `SessionRow` markup emits, in source order.
///
/// Scoped to that one function on purpose. The RSX is written in DOM
/// order, so source order IS child order, and that is the only way to
/// assert element ORDER without a layout engine in the crate. Widening the
/// scan to the whole file would pick up class names out of doc comments
/// and out of `row_class`, neither of which is a position in the tree.
fn row_markup_tokens() -> Vec<&'static str> {
    let src = shipped_markup()
        .split_once("fn SessionRow(")
        .expect("this file defines SessionRow")
        .1;
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = src[at..].find("rg-") {
        let start = at + found;
        let mut end = start;
        while end < bytes.len()
            && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'-' || bytes[end] == b'_')
        {
            end += 1;
        }
        out.push(&src[start..end]);
        at = end;
    }
    out
}

/// A row must never carry both an attention rail and the unread dot. They
/// share one slot in a sidebar whose worst-case title box is 102px;
/// emitting both pushes the timestamp out of the row and over the dot.
#[test]
fn an_attention_row_drops_the_unread_dot() {
    for attention in [
        Attention {
            bell: true,
            ..Attention::default()
        },
        Attention {
            failed: true,
            ..Attention::default()
        },
        Attention {
            waiting: Some(true),
            ..Attention::default()
        },
        Attention {
            idle_ms: IDLE_ATTENTION_MS,
            ..Attention::default()
        },
    ] {
        let rail = attention_modifier(&attention);
        assert!(rail.is_some(), "{attention:?} must light a rail");
        assert!(
            !show_unread_dot(true, rail),
            "{attention:?} lit a rail and still drew the unread dot"
        );
    }
}

/// An unread row with no attention must still draw its dot, or the unread
/// signal disappears for the common case.
#[test]
fn unread_without_attention_keeps_its_dot() {
    assert!(show_unread_dot(true, None));
    assert!(!show_unread_dot(false, None));
}

/// A row state with nothing lit, for tests to vary one field of.
fn plain() -> RowState {
    RowState {
        section: Section::Active,
        always_slim: false,
        status: SidebarStatus::Ready,
        active: false,
        picked: false,
        unread: false,
        woke: false,
        finished_unseen: false,
        attention: None,
    }
}

/// The exact class string per row state. The stylesheet keys every visual
/// state off these names, so a typo in one modifier drops that state with
/// no error anywhere: the row just renders as if it were ordinary.
#[test]
fn row_class_emits_exactly_the_stylesheet_modifiers() {
    assert_eq!(
        row_class(plain()),
        "rg-session rg-session--card rg-session--recede"
    );
    assert_eq!(
        row_class(RowState {
            section: Section::Snoozed,
            ..plain()
        }),
        "rg-session rg-session--slim rg-session--recede"
    );
    assert_eq!(
        row_class(RowState {
            section: Section::Settled,
            status: SidebarStatus::Failed,
            ..plain()
        }),
        "rg-session rg-session--slim"
    );
    assert_eq!(
        row_class(RowState {
            active: true,
            ..plain()
        }),
        "rg-session rg-session--card rg-session--active"
    );
    assert_eq!(
        row_class(RowState {
            picked: true,
            ..plain()
        }),
        "rg-session rg-session--card rg-session--picked"
    );
    assert_eq!(
        row_class(RowState {
            woke: true,
            ..plain()
        }),
        "rg-session rg-session--card rg-session--woke"
    );
    assert_eq!(
        row_class(RowState {
            status: SidebarStatus::Working,
            ..plain()
        }),
        "rg-session rg-session--card rg-session--recede rg-session--inflight"
    );
    assert_eq!(
        row_class(RowState {
            section: Section::Active,
            always_slim: false,
            status: SidebarStatus::Working,
            active: true,
            picked: true,
            unread: true,
            woke: true,
            finished_unseen: true,
            attention: Some("rg-session--attention-failed"),
        }),
        "rg-session rg-session--card rg-session--unread rg-session--woke rg-session--picked rg-session--active rg-session--attention-failed"
    );
}

/// Every row must declare one of the two shapes. `sidebar.css` gives
/// `.rg-session` no height, no padding and no direction of its own, so a
/// row carrying neither modifier collapses to a zero-height line of
/// overlapping text rather than failing visibly.
#[test]
fn every_band_picks_exactly_one_row_shape() {
    assert_eq!(row_variant(Section::Active, false), "rg-session--card");
    assert_eq!(row_variant(Section::Snoozed, false), "rg-session--slim");
    assert_eq!(row_variant(Section::Settled, false), "rg-session--slim");
    for section in [Section::Active, Section::Snoozed, Section::Settled] {
        assert_eq!(
            row_variant(section, true),
            "rg-session--slim",
            "always_slim must override the band for {section:?}"
        );
    }
    for section in [Section::Active, Section::Snoozed, Section::Settled] {
        let class = row_class(RowState { section, ..plain() });
        let shapes = ["rg-session--card", "rg-session--slim"]
            .into_iter()
            .filter(|shape| class.split(' ').any(|c| c == *shape))
            .count();
        assert_eq!(shapes, 1, "{section:?} produced {class:?}");
    }
}

/// The recede predicate, at every boundary. This is the single mechanism
/// that stops twenty equally bright rows reading as a spreadsheet, and
/// every one of these five exemptions exists because the row has a human
/// implicated in it: get one wrong and either the list flattens again or
/// the row that needs attention dims itself.
#[test]
fn a_row_recedes_only_when_nothing_wants_the_operator() {
    assert!(recedes(plain()), "a plain ready row must recede");
    for status in [
        SidebarStatus::Ready,
        SidebarStatus::Working,
        SidebarStatus::Approval,
        SidebarStatus::Input,
    ] {
        assert!(
            recedes(RowState { status, ..plain() }),
            "{status:?} must recede when nothing else is lit"
        );
    }
    assert!(
        !recedes(RowState {
            status: SidebarStatus::Failed,
            ..plain()
        }),
        "a failed row must never dim itself"
    );
    for (name, state) in [
        (
            "unread",
            RowState {
                unread: true,
                ..plain()
            },
        ),
        (
            "woke",
            RowState {
                woke: true,
                ..plain()
            },
        ),
        (
            "finished unseen",
            RowState {
                finished_unseen: true,
                ..plain()
            },
        ),
        (
            "active",
            RowState {
                active: true,
                ..plain()
            },
        ),
        (
            "picked",
            RowState {
                picked: true,
                ..plain()
            },
        ),
    ] {
        assert!(!recedes(state), "a {name} row must keep its prominence");
    }
}

/// The whole-row fade is narrower than the recede: only work that is
/// genuinely mid-turn. A finished-but-unacknowledged `Ready` row fading
/// out is the exact row the operator opened the sidebar to find.
#[test]
fn only_mid_turn_work_fades_the_whole_row() {
    for status in [
        SidebarStatus::Working,
        SidebarStatus::Approval,
        SidebarStatus::Input,
    ] {
        assert!(in_flight(RowState { status, ..plain() }), "{status:?}");
    }
    for status in [SidebarStatus::Ready, SidebarStatus::Failed] {
        assert!(!in_flight(RowState { status, ..plain() }), "{status:?}");
    }
    assert!(!in_flight(RowState {
        status: SidebarStatus::Working,
        active: true,
        ..plain()
    }));
    assert!(!in_flight(RowState {
        status: SidebarStatus::Working,
        picked: true,
        ..plain()
    }));
}

/// Neither quiet modifier may land on a row the operator is looking at or
/// has selected. The stylesheet defends against it by ordering, but a
/// markup bug that relies on that defence breaks the moment a rule moves.
#[test]
fn a_selected_or_focused_row_is_never_quiet() {
    for state in [
        RowState {
            active: true,
            status: SidebarStatus::Working,
            ..plain()
        },
        RowState {
            picked: true,
            status: SidebarStatus::Input,
            ..plain()
        },
    ] {
        let class = row_class(state);
        assert!(!class.contains("rg-session--recede"), "{class}");
        assert!(!class.contains("rg-session--inflight"), "{class}");
    }
}

/// Each shelf must carry exactly one band modifier, and a collapsed shelf
/// must add `--collapsed` without losing it. The band modifier is what
/// tints the Snoozed head and drains the Settled one; without it three
/// shelves render identically and the tail stops reading as history.
#[test]
fn each_shelf_declares_its_band_and_its_disclosure() {
    assert_eq!(
        section_class(Section::Active, true),
        "rg-project__section rg-project__section--active"
    );
    assert_eq!(
        section_class(Section::Snoozed, false),
        "rg-project__section rg-project__section--snoozed rg-project__section--collapsed"
    );
    assert_eq!(
        section_class(Section::Settled, true),
        "rg-project__section rg-project__section--settled"
    );
}

/// Selection and focus are different states and must stay separable. A
/// multi-selection of five rows has exactly one focused row, and collapsing
/// the two would make a bulk action look like it applied to one row.
#[test]
fn picked_and_active_are_independent_modifiers() {
    let picked_only = row_class(RowState {
        picked: true,
        ..plain()
    });
    let active_only = row_class(RowState {
        active: true,
        ..plain()
    });
    assert!(picked_only.contains("rg-session--picked"));
    assert!(!picked_only.contains("rg-session--active"));
    assert!(active_only.contains("rg-session--active"));
    assert!(!active_only.contains("rg-session--picked"));
}

/// Modifier mapping for the four click gestures. Getting Ctrl+Shift wrong
/// turns an additive range into a replacement, which silently discards
/// whatever the operator had already selected.
#[test]
fn click_modifiers_map_to_the_four_selection_gestures() {
    assert_eq!(click_kind(false, false), Click::Plain);
    assert_eq!(click_kind(true, false), Click::Toggle);
    assert_eq!(click_kind(false, true), Click::Range);
    assert_eq!(click_kind(true, true), Click::RangeAdditive);
}

/// Exactly one rail per row, taken from the protocol's priority ladder.
/// A failure outranks a block, a block outranks a bell, a bell outranks
/// silence. Two rails on one row would contend for the same border.
#[test]
fn the_rail_follows_the_protocol_priority_ladder() {
    let all = Attention {
        bell: true,
        idle_ms: IDLE_ATTENTION_MS,
        failed: true,
        waiting: Some(true),
    };
    assert_eq!(
        attention_modifier(&all),
        Some("rg-session--attention-failed")
    );
    assert_eq!(
        attention_modifier(&Attention {
            failed: false,
            ..all
        }),
        Some("rg-session--attention-waiting")
    );
    assert_eq!(
        attention_modifier(&Attention {
            failed: false,
            waiting: Some(false),
            ..all
        }),
        Some("rg-session--attention-bell")
    );
    assert_eq!(
        attention_modifier(&Attention {
            failed: false,
            waiting: Some(false),
            bell: false,
            ..all
        }),
        Some("rg-session--attention-idle")
    );
    assert_eq!(attention_modifier(&Attention::default()), None);
}

/// Every rail modifier the markup can emit must have a rule in one of the
/// two stylesheets. This is the seam between two agents' files; a rename
/// on either side is invisible at runtime because an unknown class simply
/// matches nothing.
#[test]
fn rail_modifiers_are_styled_somewhere() {
    let sidebar = include_str!("../../../assets/sidebar.css");
    let app = include_str!("../../app.css");
    for tier in [
        Attention {
            failed: true,
            ..Attention::default()
        },
        Attention {
            waiting: Some(true),
            ..Attention::default()
        },
        Attention {
            bell: true,
            ..Attention::default()
        },
        Attention {
            idle_ms: IDLE_ATTENTION_MS,
            ..Attention::default()
        },
    ] {
        let rail = attention_modifier(&tier).expect("tier lights a rail");
        let needle = format!(".{rail}");
        assert!(
            sidebar.contains(&needle) || app.contains(&needle),
            "no stylesheet has a rule for {rail}"
        );
    }
}

/// Every class name the sidebar markup emits must exist in a stylesheet.
/// The markup and the CSS are written by different agents against an
/// agreed list, and a class with no rule renders as an unstyled row rather
/// than an error.
#[test]
fn every_emitted_class_is_styled_somewhere() {
    let sidebar = include_str!("../../../assets/sidebar.css");
    let app = include_str!("../../app.css");
    // The footer's primary control is painted by the launcher's sheet,
    // because the control and the list it opens are one interaction and
    // splitting them across two files is how the two drift apart.
    let launcher = include_str!("../../../assets/parts/22-launcher.css");
    for class in [
        "rg-sidebar",
        "rg-sidebar--collapsed",
        "rg-sidebar__toolbar",
        "rg-sidebar__action",
        "rg-newbar",
        "rg-newbar--solo",
        "rg-newbar__go",
        "rg-newbar__what",
        "rg-newbar__pick",
        "rg-sidebar__search",
        "rg-sidebar__search-icon",
        "rg-sidebar__search-input",
        "rg-sidebar__search-kbd",
        "rg-sidebar__status",
        "rg-sidebar__status-text",
        "rg-sidebar__body",
        "rg-sidebar__empty",
        "rg-sidebar__empty--no-matches",
        "rg-sidebar__footer",
        "rg-sidebar__resizer",
        "rg-attn-count",
        "rg-project",
        "rg-project--collapsed",
        "rg-project__header",
        "rg-project__chevron",
        "rg-project__name",
        "rg-project__more",
        "rg-project__sessions",
        "rg-project__empty",
        "rg-project__section",
        "rg-project__section--active",
        "rg-project__section--snoozed",
        "rg-project__section--settled",
        "rg-project__section--collapsed",
        "rg-project__section-head",
        "rg-project__section-label",
        "rg-project__section-rule",
        "rg-project__section-count",
        "rg-rollup",
        "rg-rollup__chip",
        "rg-rollup__chip--woke",
        "rg-rollup__chip--snoozed",
        "rg-rollup__dot",
        "rg-session",
        "rg-session--card",
        "rg-session--slim",
        "rg-session--recede",
        "rg-session--inflight",
        "rg-session--active",
        "rg-session--picked",
        "rg-session--unread",
        "rg-session--woke",
        "rg-pill",
        "rg-pill--inferred",
        "rg-pill--snoozed",
        "rg-pill__aux",
        "rg-pill__word",
        "rg-badge",
        "rg-badge--woke",
        "rg-badge--snoozed",
        "rg-badge--done",
        "rg-badge--pulse",
        "rg-badge__icon",
        "rg-session__line",
        "rg-session__line--title",
        "rg-session__line--tail",
        "rg-session__slot",
        "rg-session__actions",
        "rg-session__title",
        "rg-session__branch",
        "rg-session__time",
        "rg-session__unread",
        "rg-session__close",
        "rg-empty__title",
        "rg-empty__hint",
        "rg-btn",
        "rg-btn--primary",
        "rg-btn-inline",
        "rg-project__header--static",
        "rg-project__section-head--static",
    ] {
        let needle = format!(".{class}");
        assert!(
            sidebar.contains(&needle) || app.contains(&needle) || launcher.contains(&needle),
            "sidebar markup emits .{class} but no stylesheet has a rule for it"
        );
    }
}

/// Does `css` carry a rule for exactly this class selector?
///
/// A plain `contains(".rg-foo")` is TRUE for a stylesheet that only ever
/// mentions `.rg-foo2`, because the first is a substring of the second.
/// That hole shipped in three separate guards in this repo, each reporting
/// a class styled when nothing painted it. A CSS identifier continues
/// through `[A-Za-z0-9_-]`, so the match is real only when the character
/// after the needle cannot continue the name.
fn selector_present(css: &str, needle: &str) -> bool {
    css.match_indices(needle).any(|(at, _)| {
        css[at + needle.len()..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
    })
}

/// The list above must not rot: every class the markup literally writes
/// has to be in it.
///
/// `every_emitted_class_is_styled_somewhere` walks a hand-maintained
/// array, so it only proves things about classes somebody remembered to
/// add. A class added to the RSX and forgotten there is checked by
/// nothing, which is the failure it exists to prevent, one level up.
///
/// This reads the `class: "..."` literals out of this file instead. A
/// class that appears in the markup and in no stylesheet fails here even
/// if nobody touches the array. Interpolated values like `"{pill.class}"`
/// are skipped on purpose: they are built by `inbox::status_modifier` and
/// covered by `every_status_pill_modifier_is_styled`.
#[test]
fn no_emitted_class_escapes_the_styled_list() {
    let src = include_str!("../sidebar.rs");

    // Only the markup half of the file: below the test module these same
    // names appear as assertion data, which would prove nothing.
    let markup = src
        .split_once("\n#[cfg(test)]\nmod tests {")
        .map_or(src, |(before, _)| before);
    assert!(
        markup.contains("class: \"rg-session"),
        "the markup/test split ate the markup: nothing left to check"
    );

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
        seen.len() > 20,
        "only found {} classes; the extraction broke rather than the markup",
        seen.len()
    );

    for class in seen {
        let needle = format!(".{class}");
        let styled = crate::stylesheets()
            .iter()
            .any(|(_, css)| selector_present(css, &needle));
        assert!(
            styled,
            "sidebar markup emits .{class} but no stylesheet has a rule for it"
        );
    }
}

/// Every one of the five status modifiers must be styled, or a state
/// renders as an unpainted pill that reads as a different state.
#[test]
fn every_status_pill_modifier_is_styled() {
    let sidebar = include_str!("../../../assets/sidebar.css");
    let app = include_str!("../../app.css");
    for status in vitrum_model::ALL_STATUSES {
        let needle = format!(".{}", inbox::status_modifier(status));
        assert!(
            sidebar.contains(&needle) || app.contains(&needle),
            "no stylesheet has a rule for {needle}, so {} renders unpainted",
            status.label()
        );
    }
}

/// The rem value a custom property is declared with, in 1x pixels.
fn token_px(css: &str, name: &str) -> f64 {
    let (_, rest) = css
        .split_once(&format!("{name}:"))
        .unwrap_or_else(|| panic!("no stylesheet declares {name}"));
    let value = rest
        .split(';')
        .next()
        .expect("a declaration ends in a semicolon")
        .trim();
    let number = value
        .strip_suffix("rem")
        .unwrap_or_else(|| panic!("{name} is {value}, which is not a rem length"));
    number
        .trim()
        .parse::<f64>()
        .expect("a rem length is a number")
        * 16.0
}

/// The footer's three controls fit at the 224px sidebar floor, on the grid.
///
/// This arithmetic is the whole reason the primary control lives in the
/// footer rather than the toolbar. It launches on the first click, so it
/// has to carry the agent's name, and the toolbar cannot hold a word at
/// ANY width the product offers: that band is already a 116px filter field
/// plus a 36px attention chip plus gaps inside the same 192px of content.
///
/// Every number is read out of the stylesheet that owns it rather than
/// restated here, so retuning an inset or a control height fails this test
/// instead of silently overflowing the band at the one width nobody opens.
#[test]
fn the_footer_control_fits_at_the_sidebar_floor() {
    let spacing = include_str!("../../../assets/parts/10-spacing.css");
    let chrome = include_str!("../../../assets/parts/14-chrome.css");
    let launcher = include_str!("../../../assets/parts/22-launcher.css");

    let inset = token_px(spacing, "--rg-inset");
    let gap = token_px(spacing, "--rg-space-1");
    let control = token_px(spacing, "--rg-control-h");
    let pick = token_px(spacing, "--rg-space-5");
    let pad = token_px(spacing, "--rg-space-2");
    assert_eq!(
        [inset, gap, control, pick, pad],
        [16.0, 4.0, 32.0, 20.0, 8.0],
        "a token this test depends on has moved"
    );

    // Each token is checked against the rule that actually consumes it, or
    // the arithmetic below describes a layout the browser is not drawing.
    let footer = chrome
        .split_once(".rg-sidebar__footer {")
        .expect("14-chrome.css owns the footer band")
        .1
        .split_once('}')
        .expect("a rule closes")
        .0;
    assert!(footer.contains("gap: var(--rg-space-1)"));
    assert!(footer.contains("padding: 0 var(--rg-content-inset)"));
    assert!(launcher.contains("width: var(--rg-space-5);"));
    assert!(launcher.contains("padding: 0 var(--rg-space-2);"));
    assert!(launcher.contains("height: var(--rg-control-h);"));

    // Two hairlines: the go half is bordered, and the caret half drops its
    // left border so the seam between them is one line rather than two.
    let fixed = 2.0 * inset + 2.0 * (control + gap) + pick + 2.0 * pad + 2.0;
    let floor = crate::state::SIDEBAR_MIN_PX;
    let word = floor - fixed;
    assert_eq!(fixed, 142.0);
    assert_eq!(word, 82.0);
    // "opencode" is the longest name in launch.rs's agent table, eight
    // characters, and 13px UI type runs under 8px per character.
    assert!(
        word >= 64.0,
        "only {word}px left for the agent name at the {floor}px floor"
    );

    // The whole band is a 4px grid, floor included.
    for n in [inset, gap, control, pick, pad, floor] {
        assert_eq!(n % 4.0, 0.0, "{n}px is off the 4px grid");
    }
}

/// Row ids must be unique per session and match what the bridge is asked
/// to scroll into view. A mismatch makes keyboard traversal move focus off
/// screen with no scroll, which is indistinguishable from doing nothing.
#[test]
fn row_ids_are_derived_from_the_session_id() {
    assert_eq!(row_id(SessionId(0)), "rg-row-0");
    assert_eq!(row_id(SessionId(4321)), "rg-row-4321");
    assert_ne!(row_id(SessionId(1)), row_id(SessionId(2)));
}

/// A healthy running session's tooltip must not claim anything about
/// blocking when the daemon reported `waiting: Some(false)`.
#[test]
fn a_working_session_tooltip_states_only_facts() {
    let r = row(4)
        .title("review auth")
        .cwd("/src/vitrum")
        .waiting(Some(false))
        .build();
    assert_eq!(
        row_tooltip(&r, "/home/u"),
        "review auth\n/src/vitrum\nshell \u{2022} running\nRight-click for more"
    );
}

/// When the daemon cannot answer the blocking question, the row says so.
/// Windows has no equivalent of the Linux and macOS foreground-process
/// probe, and a shell that silently omitted the line would let a Windows
/// user read "running" as "not blocked".
#[test]
fn an_unknowable_platform_says_it_cannot_tell() {
    let r = row(4)
        .title("review auth")
        .cwd("/src/vitrum")
        .waiting(None)
        .build();
    assert_eq!(
        row_tooltip(&r, "/home/u"),
        "review auth\n/src/vitrum\nshell \u{2022} running\nthis platform cannot tell whether the agent is blocked\nRight-click for more"
    );
}

/// An observed block must name what was observed, not guess why.
/// "Blocked reading input" is a syscall fact; "waiting for approval" would
/// be an inference only the agent can make.
#[test]
fn an_observed_block_names_the_observation() {
    let r = row(4)
        .title("review auth")
        .cwd("/src/vitrum")
        .waiting(Some(true))
        .build();
    assert_eq!(
        row_tooltip(&r, "/home/u"),
        "review auth\n/src/vitrum\nshell \u{2022} running\nblocked reading input - needs you\nRight-click for more"
    );
}

/// An exited session must not carry the "cannot tell" note. The question
/// only applies to a live child, and asking it about a corpse is noise.
#[test]
fn an_exited_session_is_not_asked_whether_it_is_blocked() {
    let r = row(4)
        .title("review auth")
        .cwd("/src/vitrum")
        .exited(Some(0))
        .waiting(None)
        .build();
    assert_eq!(
        row_tooltip(&r, "/home/u"),
        "review auth\n/src/vitrum\nshell \u{2022} exited 0\nRight-click for more"
    );
}

/// The declared label now rides on the pill, not the row tooltip. Both
/// carrying it would put the same sentence on screen twice on hover; the
/// pill wins because that is the element the state belongs to.
#[test]
fn the_agent_label_lives_on_the_pill_not_the_row() {
    let r = row(4)
        .title("review auth")
        .cwd("/src/vitrum")
        .waiting(Some(true))
        .hint(HintState::Approval, Some("run rm -rf build?"), NOW)
        .build();
    assert!(!row_tooltip(&r, "/home/u").contains("rm -rf build"));
    assert!(Pill::of(&r).title.contains("agent says: run rm -rf build?"));
}

/// A woken row must emit both the pulse badge and the row modifier, and a
/// row that has never snoozed must emit neither. The badge is the only
/// signal that a woken row came back, because the inbox sort left it
/// exactly where it was.
#[test]
fn only_a_woken_row_carries_the_woke_treatment() {
    let woken = row(1)
        .waiting(Some(true))
        .snooze(NOW - 2 * HOUR, NOW - HOUR)
        .build();
    let badge = inbox::disposition_badge(&woken, clock(), Default::default())
        .expect("a woken row has a badge");
    assert!(badge.class.contains("rg-badge--woke"));
    assert!(badge.class.contains("rg-badge--pulse"));
    assert!(
        row_class(RowState {
            woke: true,
            ..plain()
        })
        .contains("rg-session--woke")
    );

    let never_snoozed = row(2).waiting(Some(true)).build();
    assert_eq!(
        inbox::disposition_badge(&never_snoozed, clock(), Default::default()),
        None
    );
    assert!(!row_class(plain()).contains("rg-session--woke"));
}

/// The pulse must be a one-shot. A looping badge repaints the window
/// forever, and the count of looping badges grows with the agent count,
/// so it is worst exactly when it matters most.
#[test]
fn the_woke_pulse_never_loops() {
    let app = include_str!("../../app.css");
    let (_, after) = app
        .split_once(".rg-badge--pulse {")
        .expect("app.css has no rule for the pulse badge");
    let rule = after.split_once('}').map(|(head, _)| head).unwrap_or(after);
    assert!(
        rule.contains("animation: rg-woke-pulse"),
        "the pulse badge does not run the pulse: {rule:?}"
    );
    assert!(!rule.contains("infinite"), "the Woke pulse loops: {rule:?}");
    assert!(
        rule.contains("animation-iteration-count: 1"),
        "the pulse must pin its iteration count to 1 rather than relying on the default: {rule:?}"
    );
}

/// The search chip must name a chord the bridge actually claims. A chip
/// promising a shortcut that does nothing is worse than no chip.
#[test]
fn search_chip_names_a_real_chord() {
    let has = crate::keymap::CHORDS
        .iter()
        .any(|c| c.action == crate::keymap::KeyAction::FocusSearch && c.rendered() == "Ctrl+K");
    assert!(has, "no Ctrl+K chord is bound to the filter field");
}

/// The pill and the per-tab agent mark are the only status surfaces. No
/// shipped code emits `rg-session__dot`, and the row must not grow one
/// back: two status vocabularies on one row is how they start disagreeing.
#[test]
fn the_sidebar_row_no_longer_emits_a_status_dot() {
    let shipped = shipped_markup();
    assert!(
        !shipped.contains("rg-session__dot"),
        "the sidebar row still emits the status dot the pill replaced"
    );
}

/// Every status the five-state pill can render must be reachable from a
/// real session shape, or a state exists only in the enum.
#[test]
fn all_five_pill_states_are_reachable_from_real_sessions() {
    let cases = [
        (
            SidebarStatus::Approval,
            row(1)
                .waiting(Some(true))
                .hint(HintState::Approval, None, NOW)
                .build(),
        ),
        (
            SidebarStatus::Input,
            row(2)
                .waiting(Some(true))
                .hint(HintState::Input, None, NOW)
                .build(),
        ),
        (SidebarStatus::Working, row(3).waiting(Some(false)).build()),
        (SidebarStatus::Failed, row(4).exited(Some(1)).build()),
        (SidebarStatus::Ready, row(5).waiting(Some(true)).build()),
    ];
    for (want, r) in cases {
        let pill = Pill::of(&r);
        assert_eq!(pill.status, want, "wrong status for {:?}", r.info.status);
        assert!(!pill.word.is_empty());
        // With the glyph gone the modifier is the ONLY thing that can
        // tell two states apart on screen, because it is what the
        // stylesheet draws both the hue and the mark from.
        assert!(
            pill.class.contains(inbox::status_modifier(want)),
            "the pill for {want:?} does not carry its own modifier: {:?}",
            pill.class
        );
    }
    assert!(matches!(
        row(6).exited(None).build().info.status,
        SessionStatus::Exited { code: None }
    ));
}

/// Every title starts at the same x, whatever the status word says.
///
/// This is the "ragged alignment" defect the V3 rewrite exists to fix. The
/// old row put the status label first, as a sibling of the title, so a row
/// reading "Needs approval" pushed its title 60px further right than one
/// reading "Ready" and twenty rows had twenty different title positions.
///
/// Three things have to hold together for the fix to be real, and this
/// checks all three: the words genuinely differ in width, so the hazard is
/// not imaginary; a card's title is the FIRST child of its own line, so
/// nothing at all precedes it; and a slim row's title is preceded only by
/// the glyph, whose width is a fixed token and not its content.
#[test]
fn every_title_starts_at_the_same_x_whatever_the_status_says() {
    // 1. The hazard. If every word were the same length this test would
    //    pass for the wrong reason.
    let widths: Vec<usize> = vitrum_model::ALL_STATUSES
        .into_iter()
        .map(|s| inbox::status_word(inbox::StateWord::of(s)).chars().count())
        .collect();
    assert_eq!(widths, vec![8, 5, 7, 6, 5]);

    // 2 and 3. Exactly two titles are emitted, one per variant, and this
    //    is what sits in front of each.
    let tokens = row_markup_tokens();
    let titles: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|(_, t)| **t == "rg-session__title")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        titles.len(),
        2,
        "expected one title per row variant, found {} in {tokens:?}",
        titles.len()
    );
    assert_eq!(
        tokens[titles[0] - 1],
        "rg-session__line--title",
        "the card's title is no longer the first child of its own line"
    );
    // There is deliberately no assertion on the token PRECEDING the slim
    // title. Nothing precedes it inside its own row any more, so that
    // index reaches back into the tail of the previous row and asserts on
    // an unrelated element. It used to name the monogram tile, which is
    // how the two variants shared an x: both titles were pushed off the
    // same fixed-width box. Clause 3 below now guards that tile's absence,
    // and the padding that replaced it is proven by pixel columns.

    // Nothing status-shaped may appear between a row's start and either
    // title. On the card the status sits on the line above, so `rg-pill`
    // may precede the title in source order; what may not is a pill
    // between the title's own line marker and the title.
    assert!(
        !tokens[..titles[1]]
            .iter()
            .rev()
            .take_while(|t| **t != "rg-session__line--tail")
            .any(|t| t.starts_with("rg-pill")),
        "a status label precedes the slim row's title: {tokens:?}"
    );

    // 3. NO leading tile, in either variant. Alignment used to come from a
    //    fixed-width monogram box that both rows drew, so the two titles
    //    shared an x by both being pushed off it. That monogram was a
    //    letter avatar and is gone; alignment is now the inline padding
    //    both variants take from one token.
    //
    //    This clause exists because the monogram CAME BACK ONCE, when two
    //    authors edited this file at the same time and one write landed on
    //    top of the other. It is a regression guard against exactly that,
    //    and it is checkable here in a way the padding is not: a unit test
    //    cannot resolve a CSS length, so the padding half is proven by
    //    sampling pixel columns from a screenshot instead.
    assert!(
        !tokens.iter().any(|t| *t == "rg-session__glyph"),
        "a fixed-width leading tile is back in the row: {tokens:?}"
    );
}

/// The row's right-hand column, pinned. Every `__slot` is a one-cell grid,
/// so whatever lands in one stacks rather than laying out side by side,
/// and the column's width never depends on which child is showing. That
/// is what fixed the collision at the 14rem floor, and every close button
/// has to stay inside one.
///
/// A card has TWO slots and that is the point of the shape. Line one's
/// slot holds the status pill ALONE, so the pill has nothing to cross-fade
/// with and hovering a row to read its state cannot blank that state out.
/// Line two's slot holds the timestamp and the hover actions, which is
/// where a cross-fade belongs: the age is the least valuable thing on the
/// row and losing it under the pointer costs nothing.
#[test]
fn every_close_button_lives_inside_a_stacking_slot() {
    let tokens = row_markup_tokens();
    let slots: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|(_, t)| **t == "rg-session__slot")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        slots.len(),
        3,
        "expected two slots on the card and one on the slim row: {tokens:?}"
    );

    let closes = tokens.iter().filter(|t| **t == "rg-session__close").count();
    assert_eq!(closes, 2, "expected one close button per row variant");

    // Every close button appears after a slot and before the next slot,
    // which is the only place a token scan can prove containment.
    for close in tokens
        .iter()
        .enumerate()
        .filter(|(_, t)| **t == "rg-session__close")
        .map(|(i, _)| i)
    {
        assert!(
            slots.iter().any(|slot| {
                let end = slots
                    .iter()
                    .find(|next| **next > *slot)
                    .copied()
                    .unwrap_or(tokens.len());
                (*slot..end).contains(&close)
            }),
            "a close button is outside every slot: {tokens:?}"
        );
    }

    // The status label must NOT share a cell with anything on the card.
    // One child means nothing to cross-fade with, which is the whole fix.
    let head_slot = slots[0];
    let title_slot = slots[1];
    assert!(
        !tokens[head_slot..title_slot].contains(&"rg-session__close"),
        "the close button is back in the card's status cell: {:?}",
        &tokens[head_slot..title_slot]
    );

    let css = include_str!("../../../assets/sidebar.css");
    let rule = css
        .split_once(".rg-session__slot {")
        .expect("sidebar.css has no rule for the slot")
        .1
        .split_once('}')
        .expect("unterminated rule")
        .0;
    assert!(
        rule.contains("display: grid"),
        "the slot no longer stacks its children, so they lay out side by side: {rule:?}"
    );
}

/// The card is EXACTLY two line boxes and NEITHER is conditional.
///
/// The bug: line three was emitted only when a badge existed, as
/// `meta_line = card && (disposition.is_some() || completion.is_some())`,
/// so one band held rows of two different heights and the badges shared
/// the title's own line. Measured at the 224px width floor that gave the
/// title 127px of box on a plain card, 69.5px with one badge against a
/// 328px string, and 12.5px with two — one character and an ellipsis,
/// with a chip reading "Done" outranking the name of the session. The
/// close button landed 33px, 90.5px and 147.5px from the right edge on
/// those same three rows. One list, three row heights, three title widths
/// and three positions for one control, all from one conditional line.
///
/// Three separate ways back in are locked out here: a third line marker
/// reappearing, a badge migrating onto the title's line, and the branch
/// losing its place as the tail's first child, which is what makes the
/// tail's right edge stable on rows that have no branch at all.
#[test]
fn the_card_is_exactly_two_lines_and_neither_is_conditional() {
    let tokens = row_markup_tokens();
    let lines: Vec<&str> = tokens
        .iter()
        .copied()
        .filter(|t| t.starts_with("rg-session__line"))
        .collect();
    assert_eq!(
        lines,
        vec![
            "rg-session__line",
            "rg-session__line--title",
            "rg-session__line",
            "rg-session__line--tail",
        ],
        "the card is no longer exactly two unconditional line boxes"
    );

    // Line one, in order: the title first, then the unread dot, then the
    // slot. Nothing precedes the title, and the slot is last because it is
    // the pinned right-hand column.
    let title_line = tokens
        .iter()
        .position(|t| *t == "rg-session__line--title")
        .expect("no title line");
    assert_eq!(
        &tokens[title_line + 1..title_line + 4],
        &[
            "rg-session__title",
            "rg-session__unread",
            "rg-session__slot",
        ]
    );

    // Line two always carries the branch, which is emitted even when it
    // is EMPTY because it is the flex spacer that pushes the rest of the
    // tail right. Drop it on the rows that have no branch and the tail
    // slides into the middle of the row on half the list.
    //
    // What must hold is that the spacer is present and that everything
    // which should be pushed right comes after it. A FIXED-width element
    // may precede it: the contest marker does, so that on a row with no
    // branch it sits under the title instead of floating alone against the
    // timestamp with the left half of the line empty.
    let tail = tokens
        .iter()
        .position(|t| *t == "rg-session__line--tail")
        .expect("no tail line");
    let branch = tokens
        .iter()
        .position(|t| *t == "rg-session__branch")
        .expect("the tail lost its flex spacer");
    assert!(branch > tail, "the branch is not on the tail line");
    assert!(
        tokens[tail + 1..branch]
            .iter()
            .all(|t| t.starts_with("rg-session__contest")),
        "something other than the fixed-width contest marker was put \
         ahead of the flex spacer: {:?}",
        &tokens[tail + 1..branch]
    );
    assert!(
        tokens[branch + 1..]
            .iter()
            .any(|t| *t == "rg-session__slot"),
        "the slot must come after the spacer or it is not pushed right"
    );

    // Every badge is on the tail line. One between the title's marker and
    // the tail's is the 12.5px title coming back.
    assert_eq!(
        tokens[title_line..tail]
            .iter()
            .filter(|t| t.starts_with("rg-badge"))
            .count(),
        0,
        "a badge is back on the title's own line: {:?}",
        &tokens[title_line..tail]
    );
    assert!(
        tokens[tail..]
            .iter()
            .take_while(|t| **t != "rg-session__title")
            .any(|t| t.starts_with("rg-badge")),
        "the tail line carries no badge at all: {:?}",
        &tokens[tail..]
    );

    // The project name is gone from the row. Every bucket already has a
    // header that names it, so the card printed the same word twice.
    assert!(
        !tokens.contains(&"rg-session__project"),
        "the card still repeats the project name the group header shows"
    );
}

/// The tilde is gone from the shipped markup and from the vocabulary. It
/// read as a rendering fault rather than as a hedge, and it stole width
/// from the one element on the row that has none to spare. The hedge is
/// now a dotted rule under the word, which costs nothing.
#[test]
fn no_status_word_carries_a_punctuation_hedge() {
    for status in vitrum_model::ALL_STATUSES {
        let word = inbox::status_word(inbox::StateWord::of(status));
        assert!(
            word.chars().all(|c| c.is_ascii_alphabetic()),
            "{word:?} is not a plain word"
        );
    }
    let inferred = row(1)
        .running()
        .waiting(None)
        .idle_ms(IDLE_ATTENTION_MS)
        .build();
    let pill = Pill::of(&inferred);
    assert!(pill.source.is_inferred());
    assert_eq!(pill.word, "Ready");
    assert!(pill.class.contains("rg-pill--inferred"));
    assert!(
        pill.title.contains("cannot probe the child"),
        "the hedge has to survive somewhere the operator can read it: {:?}",
        pill.title
    );
}

/// The Active band's caption is a CAPTION and not a dead disclosure.
///
/// It shipped as a `button` carrying a chevron, an `aria-expanded` and an
/// `onclick` into `on_toggle_section`, and it could never do anything:
/// `WindowState::toggle_section` returns early for `Section::Active` and
/// `WindowState::section_open` hardcodes `true` for it. A control that
/// renders, announces itself to assistive technology as expandable, and
/// cannot respond to any click is the exact defect this pass exists to
/// remove — and the comment above it claimed it had been made a button in
/// order to FIX a bug.
///
/// Both halves are pinned: the markup no longer offers the affordance,
/// and the state layer still refuses the action, which is what would make
/// the affordance a lie if it came back.
#[test]
fn the_active_bands_caption_is_not_a_dead_disclosure() {
    let src = shipped_markup();
    assert!(
        src.contains("rg-project__section-head rg-project__section-head--static"),
        "the Active caption lost its base class or its static modifier"
    );
    assert_eq!(
        src.matches("on_toggle_section.call").count(),
        1,
        "a second band head is wired to on_toggle_section, and only the \
         Snoozed/Settled loop may be"
    );
    assert!(
        !src.contains("Section::Active))"),
        "the Active band is wired to a toggle that returns early for it"
    );

    let mut window = UiState::default().window;
    let key = GroupKey::Unfiled;
    window.toggle_section(key, Section::Active);
    assert!(
        window.section_open(key, Section::Active),
        "Active became collapsible, so its caption should be a button again"
    );
}

/// A header that cannot collapse draws no disclosure GLYPH and keeps the
/// disclosure's BOX.
///
/// Two defects, one line of markup apart, and fixing the first caused the
/// second. The Unfiled bucket is deliberately not collapsible — there is
/// no name to look for its rows under — and it drew the same chevron every
/// collapsible header uses, so named grouping shipped a triangle that did
/// nothing on the one bucket every unfiled session lands in. Deleting the
/// element then moved that header's name 20px left of every other header
/// in the panel, the 12px chevron slot plus the 8px flex gap, which is one
/// of the two misalignments visible on screen right now.
///
/// So the span stays and its content goes. The box comes from the same
/// rule the real chevron uses, which is the only arrangement where the two
/// cannot drift; a padding override on the `--static` modifier would be a
/// second source of truth for one measurement, and it would be wrong the
/// first time `--rg-chevron-w` changed.
#[test]
fn a_static_header_keeps_the_chevrons_box_and_drops_its_glyph() {
    let src = shipped_markup();
    for anchor in [
        "rg-project__header rg-project__header--static",
        "rg-project__section-head rg-project__section-head--static",
    ] {
        let at = src
            .find(anchor)
            .unwrap_or_else(|| panic!("{anchor} is gone"));
        let block = &src[at..src.len().min(at + 700)];
        assert!(
            block.contains("class: \"rg-project__chevron\" }"),
            "{anchor} dropped the chevron's box, so its label starts 20px \
             left of every sibling header: {block}"
        );
        assert!(
            !block.contains(&format!("rg-project__chevron\", \"{{CHEVRON}}")),
            "{anchor} draws a disclosure triangle it cannot act on"
        );
        assert!(
            !block.contains("aria-expanded"),
            "{anchor} still announces itself as expandable"
        );
        assert!(!block.contains("onclick"), "{anchor} is clickable again");
    }

    // The collapsible header, by contrast, still carries the glyph. If it
    // did not, this test would pass by the disclosure disappearing
    // everywhere.
    assert!(
        src.contains("span { class: \"rg-project__chevron\", \"{CHEVRON}\" }"),
        "no header draws a disclosure glyph at all any more"
    );
}

/// No handler on this component is optional.
///
/// `on_settings` was `Option<EventHandler<()>>` with the gear rendering
/// `disabled` when nothing was passed, a landing-order scaffold from a
/// merge that completed long ago: `main.rs` has passed a handler ever
/// since, so the `None` arm was unreachable and its only effect was to
/// make a dead control possible. An optional handler on a control that is
/// always drawn is a stub with a type signature.
#[test]
fn no_handler_on_this_component_is_optional() {
    assert!(
        !shipped_markup().contains("Option<EventHandler"),
        "an optional handler is back, which permits a control that renders \
         and cannot act"
    );
}

/// The Done shelf is bounded, and the cut is per bucket.
///
/// It was the one band in the sidebar with no cap: every session ever
/// finished in a bucket stayed a row, a comparator and a DOM node on every
/// paint once the shelf was open. Snoozed must NOT be cut — a parked row
/// comes back by itself, so that band drains on its own — and the Active
/// band has its own preview with its own rescue rule, so a cut applied to
/// the wrong band is as much a defect as no cut at all.
#[test]
fn only_the_done_shelf_is_cut_and_it_says_how_much_it_held_back() {
    assert_eq!(inbox::SETTLED_TAIL_LIMIT, 10);

    // Under the limit nothing is held back, so no affordance is drawn.
    assert_eq!(band_cut(Section::Settled, 10, false), (10, 0));
    assert_eq!(band_cut(Section::Settled, 0, false), (0, 0));
    // Over it, the remainder is exact: a band that hides rows without a
    // number is a band that has lost them.
    assert_eq!(band_cut(Section::Settled, 300, false), (10, 290));
    assert_eq!(band_cut(Section::Settled, 11, false), (10, 1));
    // Expanded shows everything and offers nothing.
    assert_eq!(band_cut(Section::Settled, 300, true), (300, 0));
    // The other two bands are never cut here, at any size.
    for section in [Section::Active, Section::Snoozed] {
        assert_eq!(
            band_cut(section, 300, false),
            (300, 0),
            "{section:?} was cut by the Done shelf's rule"
        );
    }

    let mut window = UiState::default().window;
    let a = GroupKey::Unfiled;
    let b = GroupKey::Folder(crate::state::FolderId(1));
    assert!(!window.settled_expanded(a));
    window.toggle_settled_tail(a);
    assert!(window.settled_expanded(a));
    assert!(
        !window.settled_expanded(b),
        "expanding one bucket's tail expanded another's"
    );
    window.toggle_settled_tail(a);
    assert!(!window.settled_expanded(a));

    // A filter forces every tail open, for the same reason it forces a
    // band open: those rows were asked for by name.
    window.filter = "boot".to_string();
    assert!(window.settled_expanded(b));
}

/// The row's SHAPE and the row's CLASS must agree, including under the
/// operator's "every row slim" switch.
///
/// They did not. The markup computed `card` from the section alone while
/// `row_class` fed `always_slim` into `row_variant`, so with the switch
/// thrown an Active row wore `rg-session--slim` and then rendered the
/// card's markup inside it: a box and its contents disagreeing, produced
/// by a control the operator had deliberately used. A settings toggle that
/// flips a class and breaks the element under it is worse than one that
/// does nothing.
#[test]
fn the_row_shape_and_the_row_class_never_disagree() {
    for section in [Section::Active, Section::Snoozed, Section::Settled] {
        for always_slim in [false, true] {
            assert_eq!(
                draws_card(section, always_slim),
                row_variant(section, always_slim) == "rg-session--card",
                "{section:?} with always_slim={always_slim} draws one shape \
                 and declares the other"
            );
        }
    }
    // And the switch actually does something, or the test above would
    // pass on a setting that is ignored.
    assert!(draws_card(Section::Active, false));
    assert!(!draws_card(Section::Active, true));
}

/// An empty bucket says why it is empty and how to fill it.
///
/// `bucket_by_folder` keeps an empty folder deliberately, because a folder
/// you have just made is exactly the one you are about to file into. The
/// sidebar then drew a header, a zero and nothing at all, with no hint
/// anywhere on screen that the way to fill it is the row context menu — so
/// the operator's first sight of named grouping was a bucket that looked
/// broken, and the four kinds of bucket cannot share one sentence because
/// they are empty for different reasons.
#[test]
fn an_empty_bucket_says_how_to_fill_it() {
    let folder = empty_bucket_hint(GroupKey::Folder(crate::state::FolderId(1)));
    assert!(
        folder.contains("Move to folder"),
        "an empty folder does not name the gesture that fills it: {folder}"
    );

    let hints = [
        empty_bucket_hint(GroupKey::Folder(crate::state::FolderId(1))),
        empty_bucket_hint(GroupKey::Unfiled),
        empty_bucket_hint(GroupKey::Project(vitrum_proto::ProjectId(1))),
    ];
    for hint in hints {
        assert!(!hint.is_empty());
        assert!(
            hint.ends_with('.'),
            "{hint:?} is not a sentence, and it sits where a row would"
        );
    }
    assert_ne!(
        hints[0], hints[1],
        "a folder and the unfiled remainder are empty for different \
         reasons and must not share a sentence"
    );
    assert_ne!(hints[1], hints[2]);
}

/// A session row must not animate itself into existence.
///
/// `.rg-session` carried `animation: rg-row-in`, a fade and a 2px slide. It
/// is attached to an element that exists at first paint, so every cold start
/// animated the whole visible list in, twenty rows at once, and the window
/// felt slow every time it was opened. It also ran on every expand of every
/// group. This is the one rule in the product that could put motion on an
/// element the operator did not ask to appear, so it gets its own guard
/// rather than relying on the generic one-shot check, which the old
/// declaration passed.
#[test]
fn no_row_animates_itself_into_existence() {
    let css = include_str!("../../../assets/sidebar.css");
    let (_, after) = css
        .split_once("\n.rg-session {")
        .expect("sidebar.css has no rule for the session row");
    let rule = after
        .split_once('}')
        .map(|(head, _)| head)
        .expect("unterminated rule");
    assert!(
        !rule.contains("animation:"),
        "the session row animates on appearance again, so every launch pays \
         for one animation per visible row: {rule}"
    );
    assert!(
        !css.contains("@keyframes rg-row-in"),
        "the row entrance keyframes are back and something will attach them"
    );
}

/// The list's three nesting levels are 32 / 16 / 8, a clean doubling.
///
/// Proximity is the only signal saying whether the thing above a row is its
/// shelf, its project group, or the next project. Those three distances had
/// drifted to 32, 24 and 8: a band boundary sat within one grid step of a
/// group boundary, so the scroller read as one flat column at three pitches
/// nobody could tell apart. Each level must stay twice the one below it.
#[test]
fn the_list_rhythm_doubles_at_every_level() {
    let spacing = include_str!("../../../assets/parts/10-spacing.css");
    let group = alias_px(spacing, "--rg-group-gap");
    let band = alias_px(spacing, "--rg-band-gap");
    let row = alias_px(spacing, "--rg-row-gap");
    assert_eq!(
        (group, band, row),
        (32.0, 16.0, 8.0),
        "the group / band / row ladder is no longer 32 / 16 / 8"
    );
    assert!(
        spacing.contains("margin-bottom: var(--rg-group-gap)"),
        "the project group stopped spending the token that names its gap, so \
         the ladder above proves nothing about what renders"
    );
    assert!(
        spacing.contains("margin: var(--rg-band-gap) 0 0"),
        "the section shelf stopped spending the token that names its gap"
    );
}

/// A token declared as `var(--other)`, resolved one level, in 1x pixels.
fn alias_px(css: &str, name: &str) -> f64 {
    let (_, rest) = css
        .split_once(&format!("{name}:"))
        .unwrap_or_else(|| panic!("no stylesheet declares {name}"));
    let value = rest
        .split(';')
        .next()
        .expect("a declaration ends in a semicolon")
        .trim();
    let target = value
        .strip_prefix("var(")
        .and_then(|v| v.strip_suffix(')'))
        .unwrap_or_else(|| panic!("{name} is {value}, which is not an alias"));
    token_px(css, target)
}
#[test]
fn virtual_slice_computation_and_overscan() {
    // Zero items -> full slice (empty)
    let empty_slice = VirtualSlice::compute(0, 0.0, 500.0, 50.0, 5);
    assert_eq!(empty_slice.start_index, 0);
    assert_eq!(empty_slice.end_index, 0);
    assert_eq!(empty_slice.top_spacer_px, 0.0);
    assert_eq!(empty_slice.bottom_spacer_px, 0.0);

    // 100 items, each 50px high. Viewport height 500px (fits 10 visible items).
    // Scroll top at 500px (first visible index = 10). Overscan = 5.
    let slice = VirtualSlice::compute(100, 500.0, 500.0, 50.0, 5);
    assert_eq!(slice.start_index, 5); // 10 - 5
    assert_eq!(slice.end_index, 25); // 10 + 10 + 5
    assert_eq!(slice.top_spacer_px, 250.0); // 5 * 50
    assert_eq!(slice.bottom_spacer_px, 3750.0); // (100 - 25) * 50

    // Top boundary overscan clamp
    let top_slice = VirtualSlice::compute(100, 50.0, 500.0, 50.0, 5);
    assert_eq!(top_slice.start_index, 0); // max(0, 1 - 5)
    assert_eq!(top_slice.top_spacer_px, 0.0);

    // Bottom boundary overscan clamp
    let bottom_slice = VirtualSlice::compute(100, 4300.0, 500.0, 50.0, 5);
    assert_eq!(bottom_slice.end_index, 100);
    assert_eq!(bottom_slice.bottom_spacer_px, 0.0);
}
