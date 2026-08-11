use super::*;
use crate::testkit::{HOUR, NOW, project as a_project, row};
use vitrum_proto::HintState;

fn clock() -> Clock {
    Clock::utc(NOW)
}

fn policy() -> DispositionPolicy {
    DispositionPolicy::manual()
}

/// The five states must each map to a distinct modifier and word. Two
/// states sharing either collapses them on screen, and the whole point of
/// the pill is that a scan down twenty rows separates them.
///
/// There is no glyph column any more and the absence is the assertion:
/// the five characters this used to check were the "letter icons"
/// complaint, two of them literally `!` and `?`, and their painted widths
/// spanned 6.2x inside one fixed box. The mark is drawn from the modifier
/// by the stylesheet now, which is why the modifier being distinct is the
/// only thing left that can keep two states apart.
#[test]
fn every_status_has_its_own_modifier_and_word() {
    let modifiers: Vec<&str> = vitrum_model::ALL_STATUSES
        .into_iter()
        .map(status_modifier)
        .collect();
    let words: Vec<&str> = vitrum_model::ALL_STATUSES
        .into_iter()
        .map(|s| status_word(StateWord::of(s)))
        .collect();

    assert_eq!(
        modifiers,
        vec![
            "rg-pill--approval",
            "rg-pill--input",
            "rg-pill--working",
            "rg-pill--failed",
            "rg-pill--ready",
        ]
    );
    assert_eq!(
        words,
        vec!["Approval", "Input", "Working", "Failed", "Ready"]
    );
}

/// The whole vocabulary, pinned. Seven words name every state the sidebar
/// can report, and they have to stay distinct: two states sharing a word
/// collapses them on screen even though the model still separates them,
/// and the operator has no way to see that it happened.
#[test]
fn the_seven_state_words_are_distinct_and_short() {
    let all = [
        StateWord::Approval,
        StateWord::Input,
        StateWord::Working,
        StateWord::Failed,
        StateWord::Ready,
        StateWord::Woke,
        StateWord::Done,
    ];
    let words: Vec<&str> = all.into_iter().map(status_word).collect();
    assert_eq!(
        words,
        vec![
            "Approval", "Input", "Working", "Failed", "Ready", "Woke", "Done"
        ]
    );

    let mut unique = words.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        words.len(),
        "two states share a word: {words:?}"
    );

    // The slot they share with a close button is 32px at its floor. Nine
    // characters is the longest that still fits beside one at the 14rem
    // sidebar width, which is why "Needs approval" could not be reused.
    for word in &words {
        assert!(
            word.chars().count() <= 9,
            "{word:?} is too long for the row's one right-hand cell"
        );
    }
}

/// The two words the disposition owns must be the ones the disposition
/// badges and the Done shelf actually print. A second literal anywhere
/// would drift the moment one of the three is reworded.
#[test]
fn the_disposition_words_come_from_the_same_vocabulary() {
    let woken = row(1)
        .running()
        .waiting(Some(true))
        .snooze(NOW - 2 * HOUR, NOW - HOUR)
        .build();
    let badge = disposition_badge(&woken, clock(), policy()).expect("a woken row has a badge");
    assert_eq!(badge.text, status_word(StateWord::Woke));
    assert_eq!(badge.text, "Woke");

    assert_eq!(
        section_head(Section::Settled).0,
        status_word(StateWord::Done)
    );
    assert_eq!(section_head(Section::Settled).0, "Done");
}

/// An inferred status must be visibly marked, not merely tooltipped. On
/// Windows every live row takes this path, and a shell that renders an
/// inferred `Ready` identically to a proven one is claiming a certainty
/// the platform cannot give it.
///
/// The mark used to be a `~` glued to the front of the word, which read as
/// a rendering fault and stole width from the narrowest element on the
/// row. It is now `.rg-pill--inferred`, which draws a dotted rule under
/// the word and dims the icon, so this test watches the class and the
/// tooltip rather than the string.
#[test]
fn an_inferred_status_is_marked_by_its_class_and_its_tooltip_not_by_a_glyph() {
    let guessed = row(1)
        .running()
        .waiting(None)
        .idle_ms(vitrum_proto::IDLE_ATTENTION_MS)
        .unread(true)
        .build();
    let proven = row(2).running().waiting(Some(true)).build();
    let pill = Pill::of(&guessed);
    let proven = Pill::of(&proven);

    assert_eq!(pill.status, SidebarStatus::Ready);
    assert_eq!(pill.source, StatusSource::Idle);
    assert!(pill.source.is_inferred());
    assert_eq!(pill.class, "rg-pill rg-pill--ready rg-pill--inferred");
    assert!(
        pill.title.contains("cannot probe the child"),
        "tooltip must say the platform cannot be certain, got {:?}",
        pill.title
    );

    // The word itself is now identical to a proven Ready's. That is the
    // point: the hedge is typography, not punctuation. A row that still
    // printed a marker here would be the old behaviour surviving a
    // rename.
    assert_eq!(pill.word, "Ready");
    assert_eq!(pill.word, proven.word);
    assert_ne!(pill.class, proven.class);

    assert!(
        crate::shell::style::classes().contains(&"rg-pill--inferred"),
        "nothing styles the hedge, so an inferred row is indistinguishable"
    );
}

/// A status the kernel proved must NOT wear the uncertainty marker, or the
/// marker stops meaning anything and Linux inherits Windows' caveat.
#[test]
fn a_proven_status_carries_no_uncertainty_marker() {
    let row = row(1).running().waiting(Some(true)).build();
    let pill = Pill::of(&row);

    assert_eq!(pill.status, SidebarStatus::Ready);
    assert_eq!(pill.source, StatusSource::Waiting);
    assert!(!pill.source.is_inferred());
    assert_eq!(pill.word, "Ready");
    assert_eq!(pill.class, "rg-pill rg-pill--ready");
    assert!(
        pill.title.contains("blocked reading the terminal"),
        "tooltip must name what the OS observed, got {:?}",
        pill.title
    );
}

/// A declared label is the richest thing on the row and must reach the
/// tooltip. Showing "Needs approval" while the agent asked a specific
/// question throws away the only channel the hint protocol opens.
#[test]
fn a_declared_label_reaches_the_pill_tooltip() {
    let row = row(1)
        .running()
        .waiting(Some(true))
        .hint(HintState::Approval, Some("Force-push to main?"), NOW)
        .build();
    let pill = Pill::of(&row);

    assert_eq!(pill.status, SidebarStatus::Approval);
    assert_eq!(pill.source, StatusSource::Hint);
    assert_eq!(pill.word, "Approval");
    assert!(
        pill.title.contains("agent says: Force-push to main?"),
        "got {:?}",
        pill.title
    );
}

/// THE HEADLINE ROW. A Codex session parked on "Would you like to run the
/// following command?" titles itself `[ ! ] Action Required`, and the sidebar
/// used to render it `Ready` — the pane says the operator is blocked, the row
/// says nothing is needed.
///
/// It must now show the needs-approval pill AND the hedge, because the evidence
/// is a banner we matched rather than a declaration the agent addressed to us.
#[test]
fn a_codex_row_titled_action_required_shows_a_hedged_approval_pill() {
    let blocked = row(1)
        .running()
        .command("codex")
        .term_title("[ ! ] Action Required - codex")
        .waiting(Some(true))
        .build();
    let pill = Pill::of(&blocked);

    assert_eq!(pill.status, SidebarStatus::Approval);
    assert_eq!(pill.source, StatusSource::Title);
    assert_eq!(pill.word, "Approval");
    assert_eq!(pill.class, "rg-pill rg-pill--approval rg-pill--inferred");
    assert!(
        pill.title.contains("terminal title"),
        "the tooltip must say where this came from, got {:?}",
        pill.title
    );

    // The hedge has to be a style that exists, or "we marked it" is a claim
    // about a class nobody paints.
    assert!(crate::shell::style::classes().contains(&"rg-pill--inferred"));

    // A hinted approval on the same row is NOT hedged: the two channels must
    // stay visually distinguishable, or the hedge says nothing.
    let declared = row(2)
        .running()
        .command("codex")
        .term_title("[ ! ] Action Required - codex")
        .waiting(Some(true))
        .hint(HintState::Approval, Some("Run `git push --force`?"), NOW)
        .build();
    let declared = Pill::of(&declared);
    assert_eq!(declared.source, StatusSource::Hint);
    assert_eq!(declared.class, "rg-pill rg-pill--approval");
}

/// The banner belongs to the agent that writes it. The same title on a session
/// running anything else leaves the row exactly as it was, because a global
/// string match would put "Needs approval" on any program that titles itself
/// that way.
#[test]
fn the_action_required_banner_only_speaks_for_codex() {
    for command in ["claude", "gemini", "opencode", "veyyon", "bash", "make"] {
        let other = row(1)
            .running()
            .command(command)
            .term_title("[ ! ] Action Required")
            .waiting(Some(true))
            .build();
        let pill = Pill::of(&other);
        assert_eq!(
            (pill.status, pill.source),
            (SidebarStatus::Ready, StatusSource::Waiting),
            "{command} wore another agent's banner"
        );
        assert_eq!(pill.class, "rg-pill rg-pill--ready");
    }
}

/// The Woke badge must carry a one-shot pulse class and nothing looping.
/// The inbox sort is static, so a woken row reappears exactly where it was
/// and the badge is the only signal that it came back.
#[test]
fn a_woken_row_gets_a_pulsing_woke_badge_and_a_snoozed_one_gets_a_countdown() {
    let woken = row(1)
        .running()
        .waiting(Some(true))
        .snooze(NOW - 2 * HOUR, NOW - HOUR)
        .build();
    let badge = disposition_badge(&woken, clock(), policy()).expect("woken row needs a badge");
    assert_eq!(badge.text, "Woke");
    assert_eq!(badge.class, "rg-badge rg-badge--woke rg-badge--pulse");

    let parked = row(2)
        .running()
        .waiting(Some(false))
        .snooze(NOW - HOUR, NOW + 2 * HOUR)
        .build();
    let badge = disposition_badge(&parked, clock(), policy()).expect("parked row needs a badge");
    assert_eq!(badge.text, "2h");
    assert_eq!(badge.class, "rg-badge rg-badge--snoozed");
}

/// A plain inbox row and a drained row both get no disposition badge. A
/// badge on every row under the Done head is a badge on none of them.
#[test]
fn plain_and_settled_rows_carry_no_disposition_badge() {
    let plain = row(1).running().waiting(Some(false)).build();
    assert_eq!(disposition_badge(&plain, clock(), policy()), None);

    let drained = row(2)
        .exited(Some(0))
        .last_activity_ms(NOW - HOUR)
        .visited(NOW)
        .build();
    assert_eq!(drained.disposition(clock(), policy()), Disposition::Settled);
    assert_eq!(disposition_badge(&drained, clock(), policy()), None);
}

/// Unseen completion is its own badge. A row can be unread without having
/// finished, and finished without being unread, and the sidebar exists to
/// find the second kind.
#[test]
fn unseen_completion_is_a_separate_badge_from_unread() {
    let finished_unseen = row(1)
        .exited(Some(0))
        .last_activity_ms(NOW - 60_000)
        .unread(true)
        .build();
    assert!(completion_badge(&finished_unseen).is_some());

    let noisy_but_running = row(2).running().waiting(Some(false)).unread(true).build();
    assert!(
        completion_badge(&noisy_but_running).is_none(),
        "unread output from a working agent is not a completion"
    );

    let finished_and_seen = row(3)
        .exited(Some(0))
        .last_activity_ms(NOW - 60_000)
        .visited(NOW)
        .build();
    assert!(completion_badge(&finished_and_seen).is_none());
}

/// Refusals must name their reason. A snooze entry that greys out with no
/// explanation teaches the operator nothing and reads as a bug.
#[test]
fn refusals_explain_themselves() {
    let blocked = row(1)
        .running()
        .waiting(Some(true))
        .hint(HintState::Approval, None, NOW)
        .build();
    assert!(snooze_refusal(&blocked).is_some_and(|r| r.contains("blocked on you")));
    assert!(settle_refusal(&blocked).is_some_and(|r| r.contains("answer it first")));

    let working = row(2).running().waiting(Some(false)).build();
    assert_eq!(
        snooze_refusal(&working),
        None,
        "a running session is snoozable; snooze hides a row, it does not stop an agent"
    );
    assert!(settle_refusal(&working).is_some_and(|r| r.contains("still working")));

    let resting = row(3).running().waiting(Some(true)).build();
    assert_eq!(snooze_refusal(&resting), None);
    assert_eq!(settle_refusal(&resting), None);
}

/// The jump key's predicate must be narrower than "not working", or in a
/// twenty-agent list it matches almost everything and the first press
/// lands somewhere useless.
#[test]
fn the_attention_queue_holds_only_rows_the_operator_is_blocking() {
    let approval = row(1)
        .running()
        .waiting(Some(true))
        .hint(HintState::Approval, None, NOW)
        .build();
    let failed = row(2).exited(Some(1)).visited(NOW).build();
    let finished_unseen = row(3)
        .exited(Some(0))
        .last_activity_ms(NOW - 60_000)
        .unread(true)
        .build();
    let ready_and_seen = row(4).running().waiting(Some(true)).visited(NOW).build();
    let working = row(5).running().waiting(Some(false)).build();

    assert!(wants_operator(&approval, clock(), policy()));
    assert!(wants_operator(&failed, clock(), policy()));
    assert!(wants_operator(&finished_unseen, clock(), policy()));
    assert!(
        !wants_operator(&ready_and_seen, clock(), policy()),
        "a Ready row you already looked at is finished business"
    );
    assert!(!wants_operator(&working, clock(), policy()));
}

/// A parked row must stay off the queue even when it would otherwise
/// qualify, or the jump key undoes the operator's decision to park it.
#[test]
fn a_parked_row_stays_off_the_attention_queue() {
    let parked = row(1)
        .exited(Some(1))
        .last_activity_ms(NOW - 2 * HOUR)
        .snooze(NOW - HOUR, NOW + HOUR)
        .visited(NOW - HOUR)
        .build();
    assert_eq!(parked.disposition(clock(), policy()), Disposition::Snoozed);
    assert!(!wants_operator(&parked, clock(), policy()));
}

/// Rollup chips must come back most urgent first and drop the zeroes. A
/// collapsed header has one line; four zeroes on it is four lines of noise
/// where the space is tightest.
#[test]
fn rollup_chips_are_urgency_ordered_and_omit_empty_states() {
    let project = a_project(7, "fleet");
    let rows = [row(1).project(7).running().waiting(Some(false)).build(),
        row(2).project(7).running().waiting(Some(false)).build(),
        row(3)
            .project(7)
            .running()
            .waiting(Some(true))
            .hint(HintState::Approval, None, NOW)
            .build(),
        row(4)
            .project(7)
            .exited(Some(1))
            .last_activity_ms(NOW - 60_000)
            .unread(true)
            .build()];
    let borrowed: Vec<&SessionView> = rows.iter().collect();
    let group = build_group(
        ProjectId(7),
        Some(&project),
        borrowed,
        None,
        false,
        clock(),
        policy(),
        PREVIEW_LIMIT,
    );
    let rollup = group.rollup.expect("every bucket rolls up");

    assert_eq!(rollup.indicator, Some(SidebarStatus::Approval));
    assert_eq!(
        rollup_chips(&rollup),
        vec![
            (SidebarStatus::Approval, 1),
            (SidebarStatus::Failed, 1),
            (SidebarStatus::Working, 2),
        ]
    );
    assert_eq!(
        rollup_title(&rollup),
        "1 needs approval, 1 failed, 2 working"
    );
}

/// Over the preview limit, the focused row must stay on screen wherever it
/// sorts. A row you are looking at vanishing behind "show all" is the
/// exact bug `preview_sessions` exists to stop.
#[test]
fn the_preview_cut_never_hides_the_focused_row() {
    let project = a_project(1, "vitrum");
    // Newest first, so id 12 sorts to the top and id 1 to the bottom.
    let rows: Vec<SessionView> = (1..=12)
        .map(|id| {
            row(id)
                .project(1)
                .running()
                .waiting(Some(false))
                .created_at_ms(1_000 * id)
                .build()
        })
        .collect();
    let borrowed: Vec<&SessionView> = rows.iter().collect();

    let cut = build_group(
        ProjectId(1),
        Some(&project),
        borrowed.clone(),
        Some(SessionId(1)),
        false,
        clock(),
        policy(),
        PREVIEW_LIMIT,
    );
    assert_eq!(cut.active.len(), PREVIEW_LIMIT + 1);
    assert_eq!(cut.hidden.len(), 12 - PREVIEW_LIMIT - 1);
    assert!(
        cut.active.iter().any(|row| row.id() == SessionId(1)),
        "the focused row must be rescued from the cut"
    );
    assert_eq!(
        cut.active.last().map(|row| row.id()),
        Some(SessionId(1)),
        "the rescued row keeps its place in the order rather than jumping"
    );

    let whole = build_group(
        ProjectId(1),
        Some(&project),
        borrowed,
        None,
        true,
        clock(),
        policy(),
        PREVIEW_LIMIT,
    );
    assert_eq!(whole.active.len(), 12);
    assert!(whole.hidden.is_empty());
}

/// The three bands must come out of the model, each with its own sort. The
/// inbox is a work queue and the settled pile is history; one comparator
/// for both puts an hour-old corpse above a streaming agent.
#[test]
fn groups_split_into_three_bands_each_with_its_own_order() {
    let project = a_project(1, "vitrum");
    let rows = [row(1)
            .project(1)
            .running()
            .waiting(Some(false))
            .created_at_ms(1_000)
            .build(),
        row(2)
            .project(1)
            .running()
            .waiting(Some(false))
            .created_at_ms(9_000)
            .build(),
        row(3)
            .project(1)
            .running()
            .waiting(Some(false))
            .snooze(NOW - HOUR, NOW + 3 * HOUR)
            .build(),
        row(4)
            .project(1)
            .running()
            .waiting(Some(false))
            .snooze(NOW - HOUR, NOW + HOUR)
            .build(),
        row(5)
            .project(1)
            .exited(Some(0))
            .created_at_ms(NOW - 20 * HOUR)
            .last_activity_ms(NOW - 10 * HOUR)
            .visited(NOW)
            .build(),
        row(6)
            .project(1)
            .exited(Some(0))
            .created_at_ms(NOW - 20 * HOUR)
            .last_activity_ms(NOW - HOUR)
            .visited(NOW)
            .build()];
    let borrowed: Vec<&SessionView> = rows.iter().collect();
    let group = build_group(
        ProjectId(1),
        Some(&project),
        borrowed,
        None,
        false,
        clock(),
        policy(),
        PREVIEW_LIMIT,
    );

    assert_eq!(
        group.active.iter().map(|r| r.id().0).collect::<Vec<_>>(),
        vec![2, 1],
        "the inbox is creation order, newest first"
    );
    assert_eq!(
        group.snoozed.iter().map(|r| r.id().0).collect::<Vec<_>>(),
        vec![4, 3],
        "parked rows sort by soonest wake, which is the only useful question about them"
    );
    assert_eq!(
        group.settled.iter().map(|r| r.id().0).collect::<Vec<_>>(),
        vec![6, 5],
        "history sorts most recently ended first"
    );
    assert_eq!(group.len(), 6);
}

/// Every band names itself from one place. Whether the Active caption is
/// DRAWN is the markup's call, but its words have to come from here or the
/// three shelves end up with three vocabularies and the operator learns
/// that "Active" and "Done" are not the same kind of label.
#[test]
fn every_band_has_a_caption_and_a_sentence_saying_what_is_in_it() {
    for section in [Section::Active, Section::Snoozed, Section::Settled] {
        let (head, hint) = section_head(section);
        assert!(!head.is_empty(), "{section:?} has no caption");
        assert!(
            hint.len() > head.len(),
            "{section:?}'s tooltip {hint:?} adds nothing to its caption"
        );
    }
    assert_eq!(section_head(Section::Active).0, "Active");
    assert_eq!(section_head(Section::Snoozed).0, "Snoozed");
    assert_eq!(section_head(Section::Settled).0, "Done");
}

/// An empty project's rollup must say so rather than rendering an empty
/// string into the header tooltip.
#[test]
fn an_empty_rollup_says_there_are_no_sessions() {
    assert_eq!(
        rollup_title(&ProjectRollup::empty(ProjectId(3))),
        "No sessions"
    );
}

/// A parked row spends its one right-hand cell on when it comes back, and
/// every other row spends it on something else. Printing the last-activity
/// age there instead would answer a question nobody asked about a row the
/// operator has explicitly deferred.
#[test]
fn only_a_snoozed_row_shows_a_return_ticket() {
    let parked = row(1)
        .running()
        .waiting(Some(false))
        .snooze(NOW - HOUR, NOW + 2 * HOUR)
        .build();
    let label = parked_label(&parked, clock(), policy()).expect("a parked row has a ticket");
    assert_eq!(label.class, "rg-pill rg-pill--snoozed");
    assert_eq!(
        label.icon, None,
        "the return ticket must carry no glyph: the countdown and the \
         snooze hue are the message, and a mark in front of \"2h\" is a \
         second thing to look at saying what the first already said"
    );
    assert_eq!(label.text, "2h");
    assert!(
        label.title.starts_with("Parked until "),
        "got {:?}",
        label.title
    );

    let woken = row(2)
        .running()
        .waiting(Some(true))
        .snooze(NOW - 2 * HOUR, NOW - HOUR)
        .build();
    assert_eq!(parked_label(&woken, clock(), policy()), None);

    let settled = row(3).exited(Some(0)).build();
    assert_eq!(parked_label(&settled, clock(), policy()), None);

    let live = row(4).running().waiting(Some(false)).build();
    assert_eq!(parked_label(&live, clock(), policy()), None);
}

/// Three ways for the sidebar to be empty, three different sentences.
///
/// The bug: ONE string answered all three. "Projects appear here as soon
/// as a session runs in one" was shown to an operator with twenty sessions
/// filed in another workspace, and to an operator who had switched every
/// band off in `Settings > Workspaces`. Both of those are features working
/// exactly as designed, and in both the only sentence on screen said the
/// daemon had nothing — so a blank second workspace and a hidden band both
/// read as "the sidebar lost my sessions", and two working features were
/// reported as missing on the strength of one string.
///
/// Each of the two new sentences has to name its CAUSE and the way back,
/// or it is the same defect with more words.
#[test]
fn an_empty_sidebar_names_its_cause_instead_of_blaming_the_daemon() {
    let nothing = Empty::of(Census {
        total: 0,
        in_workspace: 0,
        admitted: 0,
    });
    assert_eq!(nothing, Empty::NoSessions);
    let (title, hint) = nothing.words("Default");
    assert_eq!(
        title, "",
        "the honest first-run state takes no heading: the terminal pane \
         already carries that one and two surfaces must not both shout"
    );
    assert_eq!(
        hint,
        "Projects appear here as soon as a session runs in one."
    );

    let elsewhere = Empty::of(Census {
        total: 12,
        in_workspace: 0,
        admitted: 0,
    });
    assert_eq!(elsewhere, Empty::ElsewhereFiled { elsewhere: 12 });
    let (title, hint) = elsewhere.words("Review");
    assert_eq!(title, "Review is empty");
    assert!(
        hint.starts_with("12 sessions are in other workspaces."),
        "the sentence does not say where the sessions went: {hint}"
    );
    assert!(
        hint.contains("Move to workspace"),
        "the sentence does not name the way back: {hint}"
    );

    let hidden = Empty::of(Census {
        total: 12,
        in_workspace: 5,
        admitted: 0,
    });
    assert_eq!(hidden, Empty::BandsHidden { hidden: 5 });
    let (title, hint) = hidden.words("Review");
    assert_eq!(title, "Every row in Review is hidden");
    assert!(
        hint.contains("Settings \u{203a} Workspaces"),
        "the sentence does not name the surface that hid them: {hint}"
    );

    // The WIDER cut wins. A session that never reached the band filter
    // must not send the operator to the band toggles looking for it.
    assert_eq!(
        Empty::of(Census {
            total: 9,
            in_workspace: 0,
            admitted: 0,
        }),
        Empty::ElsewhereFiled { elsewhere: 9 }
    );

    // One session reads as one session, in both new sentences.
    let (_, one_elsewhere) = Empty::of(Census {
        total: 1,
        in_workspace: 0,
        admitted: 0,
    })
    .words("Default");
    assert!(
        one_elsewhere.starts_with("1 session is in other workspaces."),
        "{one_elsewhere}"
    );
    let (_, one_hidden) = Empty::of(Census {
        total: 1,
        in_workspace: 1,
        admitted: 0,
    })
    .words("Default");
    assert!(
        one_hidden.starts_with("1 session is filed here"),
        "{one_hidden}"
    );
    assert!(one_hidden.contains("showing none of it"), "{one_hidden}");
}

/// The live turn duration appears ONLY while a turn is genuinely running.
///
/// Both halves are the bug. The feature was built three times and never
/// connected — `SessionView::working_elapsed_ms`, `format_duration_label`
/// and the `.rg-pill__aux` rule all existed with nothing joining them — so
/// the row never showed the one number that separates an agent stuck for
/// forty minutes from one that started ten seconds ago. And the absence at
/// rest matters just as much: a duration on a row that is not working puts
/// a changing element on every row of a quiet list, which is the clutter
/// this sidebar exists to avoid.
#[test]
fn the_turn_duration_shows_only_while_a_turn_is_running() {
    let live = row(1)
        .running()
        .waiting(Some(false))
        .hint(HintState::Working, None, NOW - 9_000)
        .build();
    assert_eq!(live.status(), SidebarStatus::Working);
    assert_eq!(working_aux(&live, clock()).as_deref(), Some("9s"));

    // Working, but the agent never declared when the stretch began. A PTY
    // cannot answer that, so there is no honest number and nothing is
    // printed rather than session age dressed up as turn duration.
    let unhinted = row(2).running().waiting(Some(false)).build();
    assert_eq!(unhinted.status(), SidebarStatus::Working);
    assert_eq!(working_aux(&unhinted, clock()), None);

    for at_rest in [
        row(3).running().waiting(Some(true)).build(),
        row(4).exited(Some(0)).build(),
        row(5).exited(Some(1)).build(),
    ] {
        assert_eq!(
            working_aux(&at_rest, clock()),
            None,
            "a row that is not working printed a turn duration: {:?}",
            at_rest.status()
        );
    }
}

/// The preview cut must lose nothing and reorder nothing.
///
/// `build_group` used to throw away the `visible` half of the
/// [`PreviewSplit`] it had just asked for and then rediscover the same
/// partition with two `hidden.contains(..)` scans over every active row:
/// O(n*h) twice, three allocations, per bucket, per paint. The single
/// cursored `retain` that replaced it is only correct while
/// `preview_sessions` emits both halves in the caller's order, so this
/// pins the OUTCOME rather than the method. A cursor that drifts silently
/// leaves rows in `active` that belong behind the affordance, which shows
/// more than the limit and never offers to show the rest.
#[test]
fn the_preview_cut_loses_nothing_and_reorders_nothing() {
    let rows: Vec<SessionView> = (1..=12)
        .map(|n| row(n).running().waiting(Some(true)).build())
        .collect();
    let refs: Vec<&SessionView> = rows.iter().collect();
    let build = |focused, expanded| {
        build_group(
            ProjectId(1),
            None,
            refs.clone(),
            focused,
            expanded,
            clock(),
            policy(),
            PREVIEW_LIMIT,
        )
    };

    let cut = build(None, false);
    assert_eq!(cut.active.len(), PREVIEW_LIMIT);
    assert_eq!(cut.hidden.len(), 12 - PREVIEW_LIMIT);

    let mut seen: Vec<u64> = cut
        .active
        .iter()
        .chain(cut.hidden.iter())
        .map(|r| r.id().0)
        .collect();
    let banded = seen.clone();
    seen.sort_unstable();
    assert_eq!(seen, (1..=12).collect::<Vec<u64>>(), "the cut lost a row");

    // The two halves concatenate back into the band's own sorted order,
    // which is the invariant the cursor rests on.
    let whole: Vec<u64> = build(None, true).active.iter().map(|r| r.id().0).collect();
    assert_eq!(banded, whole, "the cut reordered the band");
    assert!(build(None, true).hidden.is_empty());

    // A focused row past the cut is rescued into view and taken out of
    // hidden, in one pass, without disturbing anything else.
    let late = SessionId(whole[11]);
    let rescued = build(Some(late), false);
    assert!(rescued.active.iter().any(|r| r.id() == late));
    assert!(!rescued.hidden.iter().any(|r| r.id() == late));
    assert_eq!(rescued.active.len(), PREVIEW_LIMIT + 1);
    assert_eq!(rescued.hidden.len(), 12 - PREVIEW_LIMIT - 1);
}
