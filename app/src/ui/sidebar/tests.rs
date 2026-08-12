use super::*;
use crate::state::UiState;
use crate::testkit::{HOUR, NOW, row};
use vitrum_model::{Clock, SidebarStatus};
use vitrum_proto::{Attention, HintState, IDLE_ATTENTION_MS, SessionStatus};

fn clock() -> Clock {
    Clock::utc(NOW)
}





/// A failed row says one thing about its turn.
///
/// The status pill and the completion badge sit on the same line of a card.
/// A crashed session that nobody had looked at satisfied both, so the row
/// drew a red "Failed" and a green "Done" together and neither was readable
/// as the answer. Every other status keeps the badge, because "finished while
/// you were not looking" is still news on those.
///
/// The loop is driven by `ALL_STATUSES`, so a sixth status arrives here as a
/// decision rather than as silence.
#[test]
fn only_a_failed_row_suppresses_the_completion_badge() {
    let badge = || {
        Some(crate::inbox::Badge {
            class: "rg-badge rg-badge--done".to_string(),
            icon: Some("\u{2605}"),
            text: "Done".to_string(),
            title: "Finished while you were not looking".to_string(),
        })
    };
    for status in vitrum_model::ALL_STATUSES {
        let shown = completion_shown(status, badge());
        if status == SidebarStatus::Failed {
            assert!(shown.is_none(), "a failed row still draws a Done badge");
        } else {
            assert_eq!(shown, badge(), "{status:?} lost its completion badge");
        }
    }

    // No badge in means no badge out, whatever the status. The gate must not
    // manufacture one for a row that has not finished unseen.
    for status in vitrum_model::ALL_STATUSES {
        assert!(completion_shown(status, None).is_none());
    }
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



/// Row ids must be unique per session and match what the bridge is asked
/// to scroll into view. A mismatch makes keyboard traversal move focus off
/// screen with no scroll, which is indistinguishable from doing nothing.
#[test]
fn row_ids_are_derived_from_the_session_id() {
    assert_eq!(row_id(SessionId(0)), "rg-row-0");
    assert_eq!(row_id(SessionId(4321)), "rg-row-4321");
    assert_ne!(row_id(SessionId(1)), row_id(SessionId(2)));
}

/// Defect class: two names for one state on one row.
///
/// A healthy running session's tooltip must not claim anything about
/// blocking when the daemon reported `waiting: Some(false)`, and it must
/// use the pill's word. The row used to say "running" 8px under a pill
/// reading "Working", which reads as two facts.
#[test]
fn a_working_session_tooltip_states_only_facts() {
    let r = row(4)
        .title("review auth")
        .cwd("/src/vitrum")
        .waiting(Some(false))
        .build();
    assert_eq!(
        row_tooltip(&r, "/home/u", &Pill::of(&r)),
        "review auth\n/src/vitrum\nshell \u{2022} Working\nthe OS reports it running, not blocked on the terminal\nRight-click for more"
    );
}

/// When the daemon cannot answer the blocking question, the row says the
/// state is a guess. Windows has no equivalent of the Linux and macOS
/// foreground-process probe, and a row that omitted the sentence would let
/// a Windows user read "Working" as "not blocked".
#[test]
fn an_unknowable_platform_says_it_cannot_tell() {
    let r = row(4)
        .title("review auth")
        .cwd("/src/vitrum")
        .waiting(None)
        .build();
    assert_eq!(
        row_tooltip(&r, "/home/u", &Pill::of(&r)),
        "review auth\n/src/vitrum\nshell \u{2022} Working\nthis platform cannot probe the child, so this is a guess from recent output and may be wrong\nRight-click for more"
    );
}

/// An observed block must name what was observed, not guess why. "Blocked
/// reading the terminal" is a syscall fact; "waiting for approval" would be
/// an inference only the agent can make.
#[test]
fn an_observed_block_names_the_observation() {
    let r = row(4)
        .title("review auth")
        .cwd("/src/vitrum")
        .waiting(Some(true))
        .build();
    assert_eq!(
        row_tooltip(&r, "/home/u", &Pill::of(&r)),
        "review auth\n/src/vitrum\nshell \u{2022} Ready\nthe OS reports it blocked reading the terminal\nblocked reading input - needs you\nRight-click for more"
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
        row_tooltip(&r, "/home/u", &Pill::of(&r)),
        "review auth\n/src/vitrum\nshell \u{2022} Ready\nthe child process exited\nRight-click for more"
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
    assert!(!row_tooltip(&r, "/home/u", &Pill::of(&r)).contains("rm -rf build"));
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


/// The search chip must name a chord the bridge actually claims. A chip
/// promising a shortcut that does nothing is worse than no chip.
#[test]
fn search_chip_names_a_real_chord() {
    let has = crate::keymap::CHORDS
        .iter()
        .any(|c| c.action == crate::keymap::KeyAction::FocusSearch && c.rendered() == "Ctrl+K");
    assert!(has, "no Ctrl+K chord is bound to the filter field");
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

    // One owner for the cut. It used to be duplicated between this module and
    // `WindowState`, and a duplicate of an arithmetic rule is a pair that
    // agrees until one of them is edited.
    let window = UiState::default().window;
    let key = GroupKey::Unfiled;
    // Under the limit nothing is held back, so no affordance is drawn.
    assert_eq!(window.band_cut(key, Section::Settled, 10, inbox::SETTLED_TAIL_LIMIT), (10, 0));
    assert_eq!(window.band_cut(key, Section::Settled, 0, inbox::SETTLED_TAIL_LIMIT), (0, 0));
    // Over it, the remainder is exact: a band that hides rows without a
    // number is a band that has lost them.
    assert_eq!(window.band_cut(key, Section::Settled, 300, inbox::SETTLED_TAIL_LIMIT), (10, 290));
    assert_eq!(window.band_cut(key, Section::Settled, 11, inbox::SETTLED_TAIL_LIMIT), (10, 1));
    // Expanded shows everything and offers nothing.
    let mut open = UiState::default().window;
    open.toggle_settled_tail(key);
    assert_eq!(open.band_cut(key, Section::Settled, 300, inbox::SETTLED_TAIL_LIMIT), (300, 0));
    // The other two bands are never cut here, at any size.
    for section in [Section::Active, Section::Snoozed] {
        assert_eq!(
            window.band_cut(key, section, 300, inbox::SETTLED_TAIL_LIMIT),
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





/// A row at its project's own directory, on a branch, draws no path.
///
/// The group header directly above the row already stands for it, and the
/// branch beside it is carrying the line. This is the common row, and the
/// whole reason the element can be on by default: the list does not change at
/// all for anyone whose sessions sit at their project roots inside a
/// repository.
#[test]
fn a_row_at_the_project_root_on_a_branch_draws_no_working_directory() {
    assert_eq!(
        place_label("/src/vitrum", "/src/vitrum", "/home/mk", true),
        ""
    );
}

/// A row at the project root with no branch draws its directory anyway.
///
/// WHY: the header carries a project NAME, not a path, so the silence that
/// arm was designed around is only readable while the branch beside it is
/// speaking. An agent started in a home directory hits both blanks at once —
/// the client mints a project for the launch directory, so the row is at its
/// root, and a home directory is not a repository, so there is no branch —
/// and the row went out with an empty context line saying nothing about where
/// its agent was working.
///
/// What this does NOT catch: whether the header is a useful label, or whether
/// a home directory should have become a project at all.
#[test]
fn a_row_at_the_project_root_with_no_branch_still_says_where_it_is() {
    assert_eq!(
        place_label("/home/mk", "/home/mk", "/home/mk", false),
        "~",
        "an agent in a home directory the client made a project of must still \
         say it is there"
    );
    assert_eq!(
        place_label("/src/notes", "/src/notes", "/home/mk", false),
        "/src/notes",
        "a project root outside any repository must still name itself"
    );
}

/// A row inside the project draws only the part below the root.
#[test]
fn a_row_inside_the_project_draws_the_remainder() {
    assert_eq!(
        place_label(
            "/src/vitrum/crates/vitrum-fmt",
            "/src/vitrum",
            "/home/mk",
            true
        ),
        "crates/vitrum-fmt"
    );
    assert_eq!(
        place_label("/src/vitrum/app", "/src/vitrum", "/home/mk", true),
        "app"
    );
}

/// A worktree beside the project draws the whole path, shortened against
/// home.
///
/// The case the element exists for. The row's branch already differs from
/// every other row in the group, and before this there was nothing on it
/// saying the files were somewhere else entirely.
#[test]
fn a_worktree_beside_the_project_draws_its_own_path() {
    assert_eq!(
        place_label("/home/mk/worktrees/topic", "/src/vitrum", "/home/mk", true),
        "~/worktrees/topic"
    );
}

/// A long remainder is elided in the middle, keeping the leaf.
///
/// The leaf is the part that answers "which crate is this agent in". Cutting
/// the tail would throw away the answer and keep the preamble.
#[test]
fn a_long_remainder_keeps_its_leaf() {
    let label = place_label(
        "/src/vitrum/crates/vitrum-core/src/session",
        "/src/vitrum",
        "/home/mk",
        true,
    );
    assert!(
        label.ends_with("session"),
        "the leaf was cut instead of the middle: {label}"
    );
    assert!(
        vitrum_fmt::text::display_width(&label) <= PLACE_COLUMNS,
        "the label overran its column budget: {label}"
    );
}

/// A session in the Unfiled bucket, which has no root, draws its own path.
///
/// There is no header above it saying where it is, so the row is the only
/// place the directory can appear, whether or not it is in a repository.
#[test]
fn an_unfiled_row_draws_its_own_path() {
    assert_eq!(
        place_label("/home/mk/scratch", "", "/home/mk", true),
        "~/scratch"
    );
    assert_eq!(
        place_label("/home/mk/scratch", "", "/home/mk", false),
        "~/scratch"
    );
}

/// A fixed reading of the clock, so a relative time in a fold is stable.
fn at() -> crate::Tick {
    let fmt = crate::clock::render_clock(NOW as i64, 0);
    crate::Tick {
        model: crate::inbox::model_clock(fmt),
        now_ms: NOW,
        fmt,
    }
}

/// The panel folded from `st`, with nothing the window resolves left over.
fn folded(st: &UiState) -> fold::Fold {
    fold::panel(st, at(), &fold::Context::default())
}

/// Every combination of the four things that hide a row, against the number
/// printed over the list.
///
/// # The class this closes
///
/// The count and the list are two answers to "which rows are on screen" and
/// they were resolved by two different pieces of code. Four things hide a
/// row: a collapsed bucket, a collapsed band, the Active preview cut, and the
/// Done shelf's tail cut. Any one of them applied to the list and not to the
/// count leaves the toolbar offering to jump to a row nobody can see, and the
/// jump then lands focus on a row off screen.
///
/// The Done shelf is the case that was actually broken. Its tail cut lived in
/// this module and was applied when the rows were drawn, while the count
/// walked the bands with no cut at all, so a bucket with more than
/// [`inbox::SETTLED_TAIL_LIMIT`] finished sessions counted every one of them.
/// `WindowState::band_cut` is now the single owner and `visible_rows_of`
/// applies it. This test goes red against the code before that change, at the
/// two cases below with a Settled band of thirty.
///
/// What it does not catch: a row that is seated and counted but painted
/// outside the scrolled viewport. That is a scroll position, not a fold.
#[test]
fn the_attention_count_and_the_seated_rows_agree_at_every_combination() {
    let key = GroupKey::Project(vitrum_proto::ProjectId(1));
    // Thirty that ended badly: a Settled band three times the tail limit,
    // every row of it on the attention queue, because a failure is the one
    // thing on the Done shelf that still wants an answer.
    let settled: Vec<_> = (1..=30)
        .map(|id| {
            row(id)
                .exited(Some(1))
                .hint(HintState::Approval, Some("approve this write?"), NOW - 60_000)
                .build()
        })
        .collect();
    // Enough live rows blocked on an answer to be cut by the Active preview as
    // well, so the two cuts are exercised together and not one at a time.
    let active: Vec<_> = (100..=130)
        .map(|id| {
            row(id)
                .running()
                .waiting(Some(true))
                .hint(HintState::Approval, Some("approve this write?"), NOW - 60_000)
                .build()
        })
        .collect();

    let mut both = settled.clone();
    both.extend(active.clone());

    let cases: Vec<(&str, Vec<_>)> = vec![
        ("a Settled band over the tail limit", settled),
        ("an Active band over the preview cut", active),
        ("both bands over their cuts", both),
    ];

    // A guard that only ever compared nought with nought would pass against
    // any cut at all.
    let mut ever_counted = 0usize;
    for (what, sessions) in cases {
        for collapsed_bucket in [false, true] {
            for open_tail in [false, true] {
                for collapsed_band in [false, true] {
                    let mut st = UiState::default();
                    st.daemon.projects = vec![crate::testkit::project(1, "vitrum")];
                    st.daemon.sessions = sessions.clone();
                    if collapsed_bucket {
                        st.window.collapsed.insert(key);
                    }
                    if open_tail {
                        st.window.toggle_settled_tail(key);
                    }
                    if collapsed_band {
                        st.toggle_section(key, Section::Settled);
                    }

                    let panel = folded(&st);
                    let seated = panel.visible_ids();
                    // The queue's own predicate, over the rows the panel
                    // actually seats. Re-deriving "is this waiting" here would
                    // test the re-derivation.
                    let policy = st.daemon.policy();
                    let on_queue = seated
                        .iter()
                        .filter_map(|id| st.row(*id))
                        .filter(|row| inbox::wants_operator(row, at().model, policy))
                        .count();
                    assert_eq!(
                        panel.attention, on_queue,
                        "{what}: the toolbar counts {} waiting and the list seats {on_queue} \
                         (bucket collapsed {collapsed_bucket}, tail open {open_tail}, \
                         band collapsed {collapsed_band})",
                        panel.attention
                    );
                    ever_counted = ever_counted.max(panel.attention);
                }
            }
        }
    }
    assert!(
        ever_counted > 0,
        "no arrangement in this table put a single row on the attention queue"
    );
}

/// A jump to a row past the Done shelf's tail brings it on screen.
///
/// # The class this closes
///
/// Focus moving to a row that is not drawn is the worst version of the count
/// disagreeing with the list: the operator presses the chord, the pane
/// switches, and the sidebar shows no sign of which session they are now
/// looking at. `WindowState::reveal` has to defeat every hider between the
/// row and the top of the panel, and the Done shelf's tail was the one it did
/// not know about. It now inserts into `settled_expanded`, and this test goes
/// red against the code before that change.
///
/// What it does not catch: a reveal that opens the right containers and still
/// leaves the row below the fold of the scrolled viewport.
#[test]
fn a_jump_past_the_settled_tail_puts_the_row_on_screen() {
    let key = GroupKey::Project(vitrum_proto::ProjectId(1));
    let mut st = UiState::default();
    st.daemon.projects = vec![crate::testkit::project(1, "vitrum")];
    // Thirty finished sessions, so the deep end of the shelf is well past the
    // tail limit, and the bucket and the band both shut on top of it.
    st.daemon.sessions = (1..=30)
        .map(|id| {
            row(id)
                .exited(Some(1))
                .hint(HintState::Approval, Some("approve this write?"), NOW - 60_000)
                .build()
        })
        .collect();
    st.window.collapsed.insert(key);
    st.toggle_section(key, Section::Settled);

    let target = folded(&st)
        .rows
        .is_empty()
        .then_some(SessionId(30))
        .expect("a collapsed bucket should seat nothing to begin with");

    st.reveal(target, at().model);
    let seated = folded(&st).visible_ids();
    assert!(
        seated.contains(&target),
        "the jump target is still off screen: {} rows seated",
        seated.len()
    );
}

/// Every label in `node` whose class contains `class`, with the width it holds.
fn reserved_labels(node: &super::tree::Node, class: &str, out: &mut Vec<(String, u16)>) {
    if node.class.split_whitespace().any(|c| c == class) {
        out.push((node.text.clone(), node.chars));
    }
    for child in &node.children {
        reserved_labels(child, class, out);
    }
}

/// A row that changes state must not change the width of anything on it.
///
/// # The class this closes
///
/// The status word runs from five characters to eight. The pill is the last
/// element on line one and the title before it is the element that grows, so a
/// row moving from `Ready` to `Approval` widened the pill, shrank the title's
/// box, and re-elided the title at a new character. The operator sees a row
/// that reflows every time an agent changes what it is doing, which is exactly
/// when they are reading it.
///
/// The width is derived from the vocabulary at run time, so a longer word
/// added later widens the reservation instead of reintroducing the reflow.
///
/// What it does not catch: a word wider than the reservation would still fit
/// and still push, because `set_width_chars` is a minimum. Nothing can produce
/// one while the width comes from the vocabulary itself.
#[test]
fn the_status_pill_holds_one_width_across_every_state() {
    let reserve = crate::inbox::state_word_chars();
    for word in crate::inbox::ALL_STATE_WORDS {
        let said = crate::inbox::status_word(word);
        assert!(
            said.chars().count() as u16 <= reserve,
            "{said:?} is wider than the reservation of {reserve}"
        );
    }

    let mut seen = 0usize;
    for status in vitrum_model::ALL_STATUSES {
        let mut st = UiState::default();
        st.daemon.projects = vec![crate::testkit::project(1, "vitrum")];
        // Fresh and unseen, so every one of them lands in the open inbox band
        // rather than on a shelf this test would then have to unfold.
        let base = || row(1).last_activity_ms(NOW - 1_000).unread(true);
        st.daemon.sessions = vec![match status {
            SidebarStatus::Approval => base()
                .running()
                .hint(HintState::Approval, Some("approve this write?"), NOW - 1_000)
                .build(),
            SidebarStatus::Input => base()
                .running()
                .hint(HintState::Input, Some("which file?"), NOW - 1_000)
                .build(),
            SidebarStatus::Working => base().running().waiting(Some(false)).build(),
            SidebarStatus::Ready => base().running().waiting(Some(true)).build(),
            SidebarStatus::Failed => base().exited(Some(1)).build(),
        }];

        let fold = folded(&st);
        let row = fold.rows.first().expect("one row was seated");
        let mut words = Vec::new();
        reserved_labels(&row.node, "rg-pill__word", &mut words);
        assert!(!words.is_empty(), "{status:?} folded no status word");
        for (said, chars) in words {
            assert_eq!(
                chars, reserve,
                "the pill saying {said:?} for {status:?} holds {chars} characters, \
                 not the {reserve} every state has to fit in"
            );
            seen += 1;
        }
    }
    assert_eq!(seen, vitrum_model::ALL_STATUSES.len(), "a state folded twice");
}

/// The counter beside the status word ticks without moving the row.
///
/// # The class this closes
///
/// `format_duration_label` is two characters at nine seconds and three at ten,
/// so an agent that has just started widens its own pill once a second for the
/// first minute of every turn. The reservation covers the whole range in which
/// the number moves at that rate; past an hour the label changes at most once
/// every ten minutes and is documented as widening then.
#[test]
fn the_live_counter_reserves_every_label_it_ticks_through() {
    for seconds in 0..3_600u64 {
        let label = vitrum_model::format_duration_label(seconds * 1_000);
        assert!(
            label.chars().count() as u16 <= super::fold::AUX_CHARS,
            "{label:?} at {seconds}s overruns the {} characters the pill reserves",
            super::fold::AUX_CHARS
        );
    }
}

/// The row's right-hand slot is one width whatever it is saying.
///
/// # The class this closes
///
/// The slot holds a timestamp or a snooze ticket, and the tail line's branch
/// is the element that grows into what the slot does not take. `just now`
/// becoming `4m ago` moved the branch's ellipsis. Every relative label the
/// clock produces below the point where it switches to a calendar date is
/// checked, because that is the range in which the value still changes.
#[test]
fn the_slot_reserves_every_relative_label() {
    let fmt = crate::clock::render_clock(NOW as i64, 0);
    let mut ladder: Vec<u64> = (0..600).collect();
    ladder.extend((0..48).map(|h| h * 3_600));
    ladder.extend((0..7).map(|d| d * 86_400));
    for secs in ladder {
        let label = crate::clock::age(fmt, NOW.saturating_sub(secs * 1_000));
        assert!(
            label.chars().count() as u16 <= super::fold::SLOT_CHARS,
            "{label:?} at {secs}s overruns the {} characters the slot reserves",
            super::fold::SLOT_CHARS
        );
    }
}
