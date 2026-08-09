//! Development fixture, reachable only via the `--fixture` flag.
//!
//! This exists so the shell can be looked at and driven before the session
//! server is running. It is never a fallback: a failed connection sets
//! [`crate::state::ConnState::Failed`] and says so in the sidebar. Fixture mode
//! never opens a socket at all, and paints a banner that says the data is fake.

use vitrum_model::{SessionView, SettleOverride, Snooze};
use vitrum_proto::{
    AgentHint, Attention, HintState, ProjectId, ProjectInfo, SessionId, SessionInfo, SessionStatus,
};

/// Projects the fixture presents.
pub fn projects() -> Vec<ProjectInfo> {
    vec![
        ProjectInfo {
            id: ProjectId(1),
            name: "vitrum".into(),
            root: "/src/vitrum".into(),
        },
        ProjectInfo {
            id: ProjectId(2),
            name: "kernel-notes".into(),
            root: "/src/kernel-notes".into(),
        },
        ProjectInfo {
            id: ProjectId(3),
            name: "scratch".into(),
            root: "/tmp/scratch".into(),
        },
        ProjectInfo {
            id: ProjectId(4),
            name: "fleet".into(),
            root: "/src/fleet".into(),
        },
    ]
}

/// Sessions the fixture presents.
///
/// Spread across every status, every disposition, every relative-time branch
/// and both status provenances, so one screenshot of fixture mode exercises
/// the whole sidebar rather than the easy half of it. Anything the model can
/// derive is set up here as INPUT, never as the answer: the fixture supplies
/// hints, exits, snoozes and visit stamps, and `vitrum-model` decides what each
/// row is.
pub fn sessions(now_ms: u64) -> Vec<SessionView> {
    let s = |id: u64,
             project: u64,
             title: &str,
             command: &str,
             args: &[&str],
             status: SessionStatus,
             age_ms: u64,
             branch: Option<&str>,
             unread: bool| {
        let failed = matches!(
            status,
            SessionStatus::Exited { code: None } | SessionStatus::Exited { code: Some(1..) }
        ) || matches!(status, SessionStatus::Exited { code: Some(c) } if c < 0);
        SessionInfo {
            id: SessionId(id),
            project_id: ProjectId(project),
            title: title.into(),
            cwd: match project {
                1 => "/src/vitrum".into(),
                2 => "/src/kernel-notes".into(),
                _ => "/tmp/scratch".into(),
            },
            command: command.into(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
            status: status.clone(),
            created_at_ms: now_ms.saturating_sub(age_ms * 3),
            last_activity_ms: now_ms.saturating_sub(age_ms),
            cols: 120,
            rows: 40,
            git_branch: branch.map(str::to_string),
            unread,
            attention: Attention {
                bell: false,
                // `idle_ms` counts silence the operator has not seen, which is
                // exactly what `unread` means: output arrived and nobody looked.
                idle_ms: if unread { age_ms } else { 0 },
                failed,
                // Most fixture rows are on a platform that answered the
                // foreground probe, so they carry a proven `Some(_)`. The two
                // rows below that keep `None` are the Windows case, and they
                // are what makes the inferred pill visible in a screenshot.
                waiting: match status {
                    SessionStatus::Running => Some(false),
                    SessionStatus::Starting => Some(false),
                    SessionStatus::Exited { .. } => None,
                },
            },
            hint: None,
        }
    };

    let mut out = vec![
        s(
            1,
            1,
            "claude - session server",
            "claude",
            &["--resume"],
            SessionStatus::Running,
            3_000,
            Some("main"),
            false,
        ),
        s(
            2,
            1,
            "codex - proto review",
            "codex",
            &[],
            SessionStatus::Running,
            132_000,
            Some("main"),
            true,
        ),
        s(
            3,
            1,
            "gemini - very long session title that must ellipsise cleanly",
            "gemini",
            &["-i"],
            SessionStatus::Starting,
            2_000,
            Some("wip/sidebar"),
            false,
        ),
        s(
            4,
            1,
            "codex - flake triage",
            "codex",
            &["--full-auto"],
            SessionStatus::Exited { code: Some(0) },
            5_400_000,
            Some("main"),
            false,
        ),
        s(
            5,
            2,
            "opencode - tracing",
            "opencode",
            &[],
            SessionStatus::Exited { code: Some(101) },
            7_200_000,
            Some("perf/ftrace"),
            false,
        ),
        s(
            6,
            2,
            "claude - trace review",
            "claude",
            &["--resume"],
            SessionStatus::Running,
            48_000,
            Some("perf/ftrace"),
            true,
        ),
        s(
            7,
            2,
            "veyyon - smaps rollup",
            "veyyon",
            &[],
            SessionStatus::Exited { code: None },
            259_200_000,
            None,
            false,
        ),
        s(
            8,
            3,
            "gemini - notebook port",
            "gemini",
            &[],
            SessionStatus::Running,
            15_000,
            None,
            false,
        ),
    ];

    // Twelve more to reach the stated load case of twenty concurrent agents.
    // The sidebar and the tab strip behave differently at 8 and at 20, and a
    // fixture that only ever shows 8 would not exercise either.
    let fleet = [
        ("claude", SessionStatus::Running, 4_000u64, false),
        ("claude", SessionStatus::Running, 21_000, false),
        ("codex", SessionStatus::Running, 9_000, false),
        ("codex", SessionStatus::Starting, 1_000, false),
        ("gemini", SessionStatus::Running, 64_000, true),
        (
            "gemini",
            SessionStatus::Exited { code: Some(0) },
            900_000,
            false,
        ),
        ("opencode", SessionStatus::Running, 37_000, true),
        (
            "opencode",
            SessionStatus::Exited { code: Some(2) },
            480_000,
            false,
        ),
        ("veyyon", SessionStatus::Running, 12_000, false),
        ("veyyon", SessionStatus::Running, 155_000, true),
        ("claude", SessionStatus::Running, 6_000, false),
        (
            "codex",
            SessionStatus::Exited { code: Some(2) },
            1_800_000,
            false,
        ),
    ];
    for (i, (cmd, status, age, unread)) in fleet.into_iter().enumerate() {
        let id = 9 + i as u64;
        out.push(s(
            id,
            4,
            &format!("{cmd} - worker {:02}", i + 1),
            cmd,
            &[],
            status,
            age,
            Some(if i % 3 == 0 { "main" } else { "fleet/shard" }),
            unread,
        ));
    }

    // Two sessions that rang the bell. Set explicitly rather than derived,
    // because BEL is a thing the child did, not a thing the projection implies.
    for id in [SessionId(6), SessionId(11)] {
        if let Some(belled) = out.iter_mut().find(|s| s.id == id) {
            belled.attention.bell = true;
        }
    }

    // Declared states. Only an agent can know it is asking a question, so
    // Approval and Input exist nowhere else, and the fixture has to declare
    // them or two of the five pills are unreachable on screen.
    hint(
        &mut out,
        1,
        HintState::Approval,
        Some("Force-push wip/sidebar to origin?"),
        now_ms - 12_000,
    );
    hint(
        &mut out,
        6,
        HintState::Input,
        Some("Which crate should the fix land in?"),
        now_ms - 40_000,
    );
    hint(&mut out, 9, HintState::Working, None, now_ms - 4_000);

    // Two live rows on a platform that cannot answer the foreground probe.
    // This is the Windows case and it is the whole reason the pill carries a
    // provenance: these two must wear `rg-pill--inferred`, which draws the
    // status word with a dotted underline and dims the icon, so "we could not
    // tell" can never be read as "Ready" even in a screenshot.
    for id in [SessionId(2), SessionId(13)] {
        if let Some(row) = out.iter_mut().find(|s| s.id == id) {
            row.attention.waiting = None;
            row.attention.idle_ms = vitrum_proto::IDLE_ATTENTION_MS + 5_000;
            row.unread = true;
        }
    }

    let mut rows: Vec<SessionView> = out.into_iter().map(SessionView::new).collect();

    // Client-local decisions the daemon knows nothing about. Every one of them
    // is an input to the model, not a state the fixture asserts.
    for row in &mut rows {
        match row.id().0 {
            // Parked for two hours: shows a live countdown in the Snoozed band.
            10 => row.snooze = Some(park(now_ms, now_ms + 2 * HOUR_MS)),
            // Parked until tomorrow: a different countdown unit on the badge.
            // A LIVE row, deliberately. Parking one that has already exited
            // makes it raise its hand immediately, because its exit is newer
            // than the snooze, and it comes back as Woke instead.
            15 => row.snooze = Some(park(now_ms - HOUR_MS, now_ms + 19 * HOUR_MS)),
            // Snooze already elapsed and never looked at since: Woke.
            16 => row.snooze = Some(park(now_ms - 3 * HOUR_MS, now_ms - HOUR_MS)),
            // Explicitly drained by the operator even though it is only Ready.
            8 => {
                row.settle_override = Some(SettleOverride::Settled);
                row.last_visited_ms = Some(now_ms - 60_000);
            }
            // Looked at recently, so it carries no unseen-completion badge and
            // proves the badge is about being unseen rather than about being
            // finished.
            4 => row.last_visited_ms = Some(now_ms - 30_000),
            _ => {}
        }
    }
    rows
}

const HOUR_MS: u64 = 3_600_000;

fn park(snoozed_at_ms: u64, wake_at_ms: u64) -> Snooze {
    Snooze {
        snoozed_at_ms,
        wake_at_ms,
    }
}

/// Attach a declared state to one fixture session.
fn hint(
    out: &mut [SessionInfo],
    id: u64,
    state: HintState,
    label: Option<&str>,
    received_at_ms: u64,
) {
    if let Some(row) = out.iter_mut().find(|s| s.id == SessionId(id)) {
        row.hint = Some(AgentHint {
            state,
            label: label.map(str::to_string),
            received_at_ms,
        });
    }
}

/// Terminal content painted when a fixture session gains focus.
///
/// Written as literal terminal lines so the pane on screen is a real xterm.js
/// grid running a real VT parser, not a div pretending to be one. The SGR
/// escapes are there to prove the parser is live.
pub fn transcript(info: &SessionInfo) -> Vec<String> {
    let SessionId(id) = info.id;
    let argv = if info.args.is_empty() {
        info.command.clone()
    } else {
        format!("{} {}", info.command, info.args.join(" "))
    };
    let mut lines = vec![
        "\u{1b}[1;38;5;68m  vitrum\u{1b}[0m \u{1b}[2m- fixture mode, no session server attached\u{1b}[0m".into(),
        String::new(),
        format!("\u{1b}[2msession\u{1b}[0m  {id}  \u{1b}[2mtitle\u{1b}[0m  {}", info.title),
        format!("\u{1b}[2mcwd\u{1b}[0m      {}", info.cwd),
        format!("\u{1b}[2margv\u{1b}[0m     {argv}"),
        format!("\u{1b}[2mgeometry\u{1b}[0m {}x{}", info.cols, info.rows),
        format!(
            "\u{1b}[2mbranch\u{1b}[0m   {}",
            info.git_branch.as_deref().unwrap_or("-")
        ),
        String::new(),
    ];
    // What follows is an agent at work, because that is what this pane holds
    // in the product and therefore what a screenshot of it has to show. It
    // replaced a card describing the renderer and two colour ramps: accurate,
    // but it made the pane read as a test harness, and the pane is the one
    // place a reader looks to find out what vitrum runs.
    //
    // The SGR escapes still prove the VT parser is live; they are just doing
    // it inside a diff now, where colour means something, rather than in a
    // ramp labelled with its own colour depth.
    match &info.status {
        SessionStatus::Running => lines.extend([
            "\u{1b}[38;5;68m*\u{1b}[0m \u{1b}[2mread\u{1b}[0m  app/src/session/registry.rs".into(),
            "\u{1b}[38;5;68m*\u{1b}[0m \u{1b}[2mread\u{1b}[0m  app/src/session/reaper.rs".into(),
            String::new(),
            "  The reaper closes the pty before the registry drops its handle,".into(),
            "  so a reattach arriving in that window sees a closed descriptor".into(),
            "  and reports the session as dead. Holding the guard across the".into(),
            "  close removes the window entirely.".into(),
            String::new(),
            "\u{1b}[2m~\u{1b}[0m app/src/session/registry.rs".into(),
            "  \u{1b}[31m- let handle = self.handles.remove(&id);\u{1b}[0m".into(),
            "  \u{1b}[32m+ let handle = self.handles.get(&id).cloned();\u{1b}[0m".into(),
            String::new(),
            "\u{1b}[33m?\u{1b}[0m apply this edit\u{1b}[2m   y yes   n skip   e explain\u{1b}[0m"
                .into(),
            String::new(),
            "\u{1b}[7m \u{1b}[0m".into(),
        ]),
        SessionStatus::Starting => lines.push(
            "\u{1b}[33m*\u{1b}[0m starting  \u{1b}[2mspawned, waiting for the first token\u{1b}[0m"
                .into(),
        ),
        SessionStatus::Exited { code: Some(c) } if *c == 0 => lines.extend([
            "  Renamed the guard and updated the four call sites that took it".into(),
            "  by value. Nothing else referenced the old name.".into(),
            String::new(),
            "\u{1b}[90m*\u{1b}[0m exited 0  \u{1b}[2mthe agent finished and left the branch clean\u{1b}[0m".into(),
        ]),
        SessionStatus::Exited { code: Some(c) } => lines.extend([
            "  Stopped: the change needs a decision I should not make alone.".into(),
            "  Two callers disagree about who owns the handle after a reattach.".into(),
            String::new(),
            format!("\u{1b}[31m*\u{1b}[0m exited {c} \u{1b}[2mthe agent gave up and said why\u{1b}[0m"),
        ]),
        SessionStatus::Exited { code: None } => lines.push(
            "\u{1b}[31m*\u{1b}[0m signalled \u{1b}[2mthe agent was stopped from outside\u{1b}[0m"
                .into(),
        ),
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::UiState;
    use vitrum_model::{
        AgentKind, Clock, Disposition, DispositionPolicy, SidebarStatus, StatusSource,
    };

    const NOW: u64 = 1_772_580_600_000;

    fn clock() -> Clock {
        Clock::utc(NOW)
    }

    fn policy() -> DispositionPolicy {
        DispositionPolicy::default()
    }

    fn loaded() -> UiState {
        let mut st = UiState::default();
        st.daemon.projects = projects();
        st.daemon.sessions = sessions(NOW);
        st
    }

    /// Every fixture session must belong to a fixture project. An orphan here
    /// would put a row in the "no project" bucket and misrepresent what the
    /// grouped sidebar looks like against a real server.
    #[test]
    fn every_fixture_session_has_a_project() {
        let ids: Vec<ProjectId> = projects().iter().map(|p| p.id).collect();
        for row in sessions(NOW) {
            assert!(
                ids.contains(&row.project_id()),
                "session {:?} points at missing project {:?}",
                row.id(),
                row.project_id()
            );
        }
    }

    /// Session ids must be unique. A duplicate id makes two tabs collapse into
    /// one and the focus logic pick an arbitrary row, which would look like a
    /// bug in the tab code rather than in the fixture.
    #[test]
    fn fixture_session_ids_are_unique() {
        let mut ids: Vec<u64> = sessions(NOW).iter().map(|row| row.id().0).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate fixture session id");
    }

    /// ALL FIVE sidebar states must be reachable in fixture mode. Approval and
    /// Input exist only when an agent declares them, so a fixture with no
    /// hints leaves two of the five pills unreachable on screen and the
    /// screenshot proves nothing about them.
    #[test]
    fn fixture_covers_every_one_of_the_five_sidebar_states() {
        let states: Vec<SidebarStatus> = sessions(NOW).iter().map(|row| row.status()).collect();
        for want in vitrum_model::ALL_STATUSES {
            assert!(
                states.contains(&want),
                "fixture never produces {:?}; that pill cannot be screenshotted",
                want
            );
        }
    }

    /// The fixture must include rows whose status the platform could not prove,
    /// because that is the Windows case and the only way to see the inferred
    /// treatment without a Windows box.
    #[test]
    fn fixture_includes_rows_whose_status_is_only_inferred() {
        let inferred: Vec<u64> = sessions(NOW)
            .iter()
            .filter(|row| row.info.status.is_live() && row.resolve_status().source.is_inferred())
            .map(|row| row.id().0)
            .collect();
        assert_eq!(
            inferred,
            vec![2, 13],
            "exactly the two rows set up as unprobeable must resolve as inferred"
        );
        for id in &inferred {
            let row = sessions(NOW).into_iter().find(|r| r.id().0 == *id).unwrap();
            let pill = crate::inbox::Pill::of(&row);
            assert!(
                pill.class.contains("rg-pill--inferred"),
                "row {id} resolves as inferred but its pill does not wear the hedge class: {:?}",
                pill.class
            );
        }
    }

    /// The fixture must exercise every disposition. Snoozed and Woke are the
    /// two the model added and the two with no other way to reach them for a
    /// screenshot.
    #[test]
    fn fixture_covers_every_disposition() {
        let seen: Vec<Disposition> = sessions(NOW)
            .iter()
            .map(|row| row.disposition(clock(), policy()))
            .collect();
        for want in [
            Disposition::Active,
            Disposition::Woke,
            Disposition::Snoozed,
            Disposition::Settled,
        ] {
            assert!(seen.contains(&want), "fixture never produces {want:?}");
        }
    }

    /// The declared hints must actually resolve to the states they declare,
    /// and their labels must survive. A hint that resolves to something else is
    /// a fixture that lies about what the UI will show.
    #[test]
    fn declared_hints_resolve_to_their_declared_state() {
        let rows = sessions(NOW);
        let find = |id: u64| rows.iter().find(|r| r.id().0 == id).unwrap();

        let approval = find(1);
        assert_eq!(approval.status(), SidebarStatus::Approval);
        assert_eq!(approval.resolve_status().source, StatusSource::Hint);
        assert_eq!(
            approval.hint_label(),
            Some("Force-push wip/sidebar to origin?")
        );

        assert_eq!(find(6).status(), SidebarStatus::Input);
        assert_eq!(find(9).status(), SidebarStatus::Working);
    }

    /// A blocked row must not be snoozable, and the fixture must contain one,
    /// or the disabled-because-blocked case cannot be demonstrated.
    #[test]
    fn the_fixture_contains_a_row_that_refuses_to_be_snoozed() {
        let blocked: Vec<u64> = sessions(NOW)
            .iter()
            .filter(|row| !row.can_snooze())
            .map(|row| row.id().0)
            .collect();
        assert_eq!(blocked, vec![1, 6]);
    }

    /// Activity times must be in the past relative to the `now` handed in, and
    /// creation must precede activity. A fixture with a future timestamp would
    /// make every row read "now" and hide the relative-time rendering.
    #[test]
    fn fixture_timestamps_are_in_the_past_and_ordered() {
        for row in sessions(NOW) {
            let info = &row.info;
            assert!(
                info.last_activity_ms < NOW,
                "{} is not in the past",
                info.title
            );
            assert!(
                info.created_at_ms <= info.last_activity_ms,
                "{} was created after its last activity",
                info.title
            );
        }
    }

    /// The fixture must produce more than one relative-time bucket, otherwise
    /// the screenshot proves nothing about the timestamp code.
    #[test]
    fn fixture_spans_several_relative_time_buckets() {
        // Rendered against the same instant the fixture was built from. Using
        // the wall clock here would date every row to 2026 and collapse them
        // all into one bucket, which is the bug this test would then miss.
        let fmt = vitrum_fmt::TimeFormat::new(vitrum_fmt::Timestamp::from_millis(NOW as i64), 0);
        let labels: std::collections::BTreeSet<String> = sessions(NOW)
            .iter()
            .map(|row| crate::clock::age(fmt, row.info.last_activity_ms))
            .collect();
        assert!(
            labels.len() >= 4,
            "fixture produced only {labels:?}, expected at least 4 distinct ages"
        );
    }

    /// Loading the fixture into the real state must produce exactly the four
    /// project groups with no orphan bucket, and must hit the stated load case
    /// of twenty concurrent agents. A fixture that only ever shows eight would
    /// not exercise the tab strip's eviction or the sidebar at scale, which is
    /// where both are actually hard.
    #[test]
    fn fixture_is_the_twenty_agent_load_case() {
        let st = loaded();
        assert_eq!(st.daemon.sessions.len(), 20);

        let g = st.tree(clock());
        assert_eq!(g.len(), 4);
        assert!(g.iter().all(|x| x.project.is_some()));
        assert_eq!(
            g.iter().map(|x| x.len()).collect::<Vec<_>>(),
            vec![4, 3, 1, 12]
        );
    }

    /// Every project must roll up, and at least one must have something urgent
    /// in it, so a collapsed header has a non-empty indicator to show.
    #[test]
    fn every_project_rolls_up_and_one_of_them_is_urgent() {
        let st = loaded();
        let g = st.tree(clock());
        let indicators: Vec<Option<SidebarStatus>> = g
            .iter()
            .map(|group| group.bands.rollup.as_ref().and_then(|r| r.indicator))
            .collect();
        assert_eq!(indicators.len(), 4);
        assert!(
            indicators.contains(&Some(SidebarStatus::Approval)),
            "no collapsed header would show an approval indicator: {indicators:?}"
        );
        assert!(
            indicators.contains(&None),
            "a project whose sessions are all drained must show NO indicator rather \
             than a grey one, or it wears a permanent dot for work that is over"
        );
        assert_eq!(
            indicators.iter().filter(|i| i.is_some()).count(),
            3,
            "{indicators:?}"
        );
    }

    /// The fixture must fill all three bands, or the Snoozed head never
    /// appears on screen.
    #[test]
    fn the_fixture_fills_all_three_bands() {
        let st = loaded();
        let g = st.tree(clock());
        let snoozed: usize = g.iter().map(|x| x.bands.snoozed.len()).sum();
        let settled: usize = g.iter().map(|x| x.bands.settled.len()).sum();
        let active: usize = g
            .iter()
            .map(|x| x.bands.active.len() + x.bands.hidden.len())
            .sum();
        assert_eq!(snoozed, 2, "two rows are parked");
        assert!(settled >= 2, "got {settled} settled rows");
        assert_eq!(active + snoozed + settled, 20);
    }

    /// Opening every fixture session must leave the strip at the cap while the
    /// sidebar still lists all twenty. This is the whole point of the MRU
    /// strip: twenty tabs is unreadable, twenty sidebar rows is not.
    #[test]
    fn opening_every_session_caps_the_strip_but_not_the_sidebar() {
        let mut st = loaded();
        let ids: Vec<_> = st.daemon.sessions.iter().map(|row| row.id()).collect();
        for id in &ids {
            st.open(*id, NOW);
        }
        assert_eq!(st.window.tabs.len(), crate::state::MAX_TABS);
        assert_eq!(st.window.focused, ids.last().copied());
        assert_eq!(
            st.tree(clock()).iter().map(|g| g.len()).sum::<usize>(),
            20,
            "eviction from the strip must never remove a session from the sidebar"
        );
    }

    /// A transcript must name its own session, so a screenshot of the terminal
    /// pane can be checked against the row that was clicked.
    #[test]
    fn transcript_names_its_session() {
        let rows = sessions(NOW);
        let text = transcript(&rows[1].info).join("\n");
        assert!(
            text.contains("codex - proto review"),
            "missing title:\n{text}"
        );
        assert!(text.contains("/src/vitrum"), "missing cwd:\n{text}");
        assert!(text.contains("120x40"), "missing geometry:\n{text}");
    }

    /// A transcript must never contain a bare newline. xterm.js does not move
    /// the cursor to column zero on LF unless `convertEol` is on, which it is
    /// not, so a bare LF would produce a staircase down the screen.
    #[test]
    fn transcript_lines_contain_no_bare_newlines() {
        for row in sessions(NOW) {
            for line in transcript(&row.info) {
                assert!(
                    !line.contains('\n') || line.contains("\r\n"),
                    "bare LF in {:?}",
                    line
                );
            }
        }
    }

    /// Every SGR escape opened in a transcript must be closed with a reset, or
    /// the colour bleeds into the rest of the grid and into the next session's
    /// repaint.
    #[test]
    fn transcript_resets_every_colour_it_sets() {
        for row in sessions(NOW) {
            for line in transcript(&row.info) {
                let opens = line.matches('\u{1b}').count();
                let resets = line.matches("\u{1b}[0m").count();
                assert!(
                    opens == 0 || resets > 0,
                    "line sets SGR without resetting: {line:?}"
                );
            }
        }
    }

    /// WHY: this fixture is where every published screenshot comes from, so a
    /// shell in it becomes a shell on the front page. The demo assets deleted
    /// alongside this test showed `bash`, `cargo watch`, `python3` and two
    /// `make` sessions, and each of those was a row this file defined. Banning
    /// the pictures in AGENTS.md only removes the symptom; while the fixture
    /// can still produce a shell row, the next screenshot puts one back.
    ///
    /// The class closed here is the whole sidebar, not the five rows that were
    /// wrong: the kind is resolved through the same [`AgentKind::of`] the tab
    /// strip paints with, so a session whose command is a shell, a build tool,
    /// an interpreter or anything else this build cannot name fails. Adding a
    /// row is therefore red until its command is a recognised agent.
    ///
    /// What this does NOT catch: a real agent given a shell-shaped title, and
    /// a screenshot taken from something other than the fixture. The first is
    /// covered below; the second is covered by [`crate::tests::assets`], which
    /// refuses to let an image sit in the tree without a written claim about
    /// which agents are on screen.
    #[test]
    fn no_fixture_session_runs_a_shell_or_an_unrecognised_command() {
        for row in sessions(NOW) {
            let kind = AgentKind::of(&row.info.command);
            assert!(
                !matches!(kind, AgentKind::Shell | AgentKind::Unknown),
                "session {:?} runs {:?}, which resolves to {kind:?}; the sidebar \
                 in every screenshot is built from this list",
                row.info.title,
                row.info.command
            );
        }
    }

    /// A row's title is what a reader actually sees, and it is set separately
    /// from the command, so a `claude` session titled `bash` would pass the
    /// check above and still sell a terminal multiplexer in the screenshot.
    /// The argv is checked with it because the focused pane prints it.
    ///
    /// The boundary, stated because it was measured rather than assumed: this
    /// catches text that NAMES a shell or a build tool, so `["run", "cargo",
    /// "test"]` and `["-c", "bash -lc make"]` both fail. An argv like
    /// `["watch", "-x", "test"]` passes, and that is the intended answer, not
    /// a gap: attached to a recognised agent it renders as `codex watch -x
    /// test`, which is an agent invocation. Widening this to generic words
    /// would fail honest agent flags and buy nothing.
    #[test]
    fn no_fixture_title_or_argv_reads_as_a_shell_or_a_build_tool() {
        for row in sessions(NOW) {
            let argv = row.info.args.join(" ");
            for text in [row.info.title.as_str(), argv.as_str()] {
                let lowered = text.to_ascii_lowercase();
                for banned in [
                    "bash", "zsh", "fish", "shell", "cargo", "git ", "make", "npm", "docker",
                    "htop", "python",
                ] {
                    assert!(
                        !lowered.contains(banned),
                        "{text:?} names {banned:?}, and a sidebar row reading like \
                         a shell command is what puts vitrum in tmux's category"
                    );
                }
            }
        }
    }

    /// A focused pane paints a transcript, and a `$` prompt with a block cursor
    /// is the single most recognisable shell tell there is. The pane may say it
    /// is a fixture, describe the session and prove the VT parser runs; it may
    /// not imitate a shell waiting for a command.
    #[test]
    fn no_transcript_paints_a_shell_prompt() {
        for row in sessions(NOW) {
            for line in transcript(&row.info) {
                let bare = strip_sgr(&line);
                assert!(
                    !bare.contains(" $ ") && !bare.trim_end().ends_with(" $"),
                    "transcript for {:?} paints a prompt: {bare:?}",
                    row.info.title
                );
            }
        }
    }

    /// Drop SGR escapes so a prompt cannot hide behind colour codes.
    fn strip_sgr(line: &str) -> String {
        let mut out = String::with_capacity(line.len());
        let mut rest = line;
        while let Some(at) = rest.find('\u{1b}') {
            out.push_str(&rest[..at]);
            rest = match rest[at..].find('m') {
                Some(end) => &rest[at + end + 1..],
                None => "",
            };
        }
        out.push_str(rest);
        out
    }
}
