//! What the operator asked for, carried out.
//!
//! Menu items, launcher entries and keyboard actions all end here, so a
//! session starts the same way whichever surface asked for it.

use super::*;

/// Open a context menu at a pointer position, clamped inside the window.
///
/// A menu opened near the bottom right would otherwise draw off screen, and
/// the entries it hides are the destructive ones at the bottom.
pub(crate) fn open_menu(
    mut st: Signal<UiState>,
    window: &vitrum_dioxus_desktop::DesktopContext,
    (x, y, target): (f64, f64, SessionId),
) {
    let items = st.peek().menu_items(target, tick().model);
    // A right-click on a row whose session vanished has nothing to act on.
    if items.is_empty() {
        return;
    }
    let (vw, vh) = viewport(window);
    let h = ui::menu::menu_height(&items);
    let (x, y) = ui::menu::clamp(x, y, ui::menu::MENU_W, h, vw, vh);
    st.write().window.layer = Layer::Menu(MenuState { x, y, target });
}

/// Window size in CSS pixels, for clamping a context menu.
pub(crate) fn viewport(window: &vitrum_dioxus_desktop::DesktopContext) -> (f64, f64) {
    let scale = window.scale_factor();
    let size = window.inner_size();
    (size.width as f64 / scale, size.height as f64 / scale)
}

/// Sweep scrollback for what is in the search field.
///
/// Only the daemon can answer this. A client holds the focused viewport and
/// nothing else, so "which of my twenty agents hit an OOM" is one server-side
/// sweep here and impossible anywhere else.
///
/// Scoped to the sidebar selection when there is one. The wire has always
/// carried a session filter and the client always sent an empty list, so
/// selecting three rows and searching swept all twenty and buried the three.
/// One selected row is not a scope: that is where the cursor happens to be,
/// and narrowing to it would make the everyday case silently local.
pub(crate) fn run_search(bridge: Bridge, mut st: Signal<UiState>) {
    let (query, options, scope) = {
        let r = st.peek();
        let scope: Vec<SessionId> = if r.window.selection.len() > 1 {
            r.window.selection.iter().collect()
        } else {
            Vec::new()
        };
        (
            r.window.search.query.clone(),
            r.window.search.options,
            scope,
        )
    };
    // `request` owns the hit cap and the context width, because the summary
    // line quotes the cap back to the operator; a send site that picked its
    // own number would make that sentence name a value never sent. It also
    // refuses an all-whitespace pattern, which would otherwise match nearly
    // every line of every ring.
    let Some(msg) = ui::search::request(&query, options, scope.clone()) else {
        return;
    };
    {
        let mut w = st.write();
        w.window.search.searching = true;
        w.window.search.scope = scope;
    }
    bridge.msg(&msg);
}

/// Close whichever layer is open.
///
/// Onboarding and What's New both promise that closing them — Skip, Got it,
/// backdrop, or Escape — records that they were seen. The component handlers
/// honour that for clicks; Escape goes through this function, so the same
/// persistence has to live here or a sheet dismissed with Escape returns on
/// the next launch.
pub(crate) fn dismiss(mut st: Signal<UiState>) {
    let layer = st.peek().window.layer.clone();
    if !layer.is_open() {
        return;
    }
    let persist = dismiss_persists(&layer);
    {
        let mut w = st.write();
        if persist {
            let current = update::current_version();
            match layer {
                Layer::Onboarding => w.daemon.settings.finish_onboarding(&current),
                Layer::WhatsNew => w.daemon.settings.mark_seen(&current),
                _ => {}
            }
        }
        w.window.layer = Layer::None;
    }
    if persist {
        ui::settings::commit(&st.peek());
    }
}

/// Whether dismissing this layer must also write the "seen" bit.
///
/// Extracted so the Escape path and the click handlers can be tested against
/// the same rule without standing up a VirtualDom.
pub(crate) fn dismiss_persists(layer: &Layer) -> bool {
    matches!(layer, Layer::Onboarding | Layer::WhatsNew)
}

/// Open `next`, or close it if it is already the open layer.
pub(crate) fn toggle_layer(mut st: Signal<UiState>, next: Layer) {
    let mut w = st.write();
    w.window.layer = if w.window.layer == next {
        Layer::None
    } else {
        next
    };
}

/// The directory a new session should start in: the project asked for, else
/// the focused session's, else the first project's.
///
/// Seeding matters: the overwhelmingly common case is "another agent in the
/// thing I am already working on", and an empty field makes the user type a
/// path they can see on screen. One rule, shared by the one-click path and the
/// ranked list, so the two cannot seed differently.
pub(crate) fn seed_dir(r: &UiState, project: Option<ProjectId>) -> String {
    project
        .and_then(|p| r.daemon.projects.iter().find(|x| x.id == p))
        .map(|p| p.root.clone())
        .or_else(|| {
            r.window
                .focused
                .and_then(|f| r.session(f))
                .map(|s| s.cwd.clone())
        })
        .or_else(|| r.daemon.projects.first().map(|p| p.root.clone()))
        .unwrap_or_default()
}

/// Open the ranked launcher, seeded by [`seed_dir`].
pub(crate) fn open_new_session(mut st: Signal<UiState>, project: Option<ProjectId>) {
    let seed = {
        let r = st.peek();
        NewSessionSeed {
            project,
            cwd: seed_dir(&r, project),
        }
    };
    st.write().window.layer = Layer::NewSession(seed);
}

/// A launch this window has sent and the daemon has not confirmed yet.
#[derive(Clone, PartialEq)]
pub(crate) struct PendingLaunch {
    pub(crate) cwd: String,
    pub(crate) line: String,
    /// Every session that already existed when the request went out.
    ///
    /// The match is "a session with this command in this directory that was
    /// NOT here before", never "a session with this command in this
    /// directory": twenty agents in one repo means the second spelling focuses
    /// a row that has been running for an hour.
    pub(crate) before: Vec<SessionId>,
}

/// Send one launch, remember it, and say what started.
///
/// Every route to a new session funnels through here, so the history write,
/// the flash and the focus correlation cannot differ between the sidebar's
/// control, the ranked list and the context menu.
pub(crate) fn start_session(
    bridge: Bridge,
    mut st: Signal<UiState>,
    mut pending: Signal<Option<PendingLaunch>>,
    project: ProjectId,
    l: launch::Launch,
) {
    // A profile that cannot be written costs the operator their ranking next
    // time, never this session, so a failed write is logged and it goes ahead.
    if let Err(why) = launch::record_launch(&l.command, &l.args, &l.cwd, launch::now_ms()) {
        tracing::warn!("launch history not saved: {why}");
    }
    let word = ui::dialog::basename(&l.command).to_string();
    let (cols, rows, place, before) = {
        let r = st.peek();
        (
            r.window.cols,
            r.window.rows,
            ui::dialog::place_of(&r.daemon.projects, &l.cwd, &launch::user_home()),
            r.daemon
                .sessions
                .iter()
                .map(|s| s.info.id)
                .collect::<Vec<_>>(),
        )
    };
    pending.set(Some(PendingLaunch {
        cwd: l.cwd.clone(),
        line: launch::join_command(&l.command, &l.args),
        before,
    }));
    bridge.msg(&ClientMsg::CreateSession {
        project_id: project,
        cwd: l.cwd,
        command: l.command,
        args: l.args,
        cols,
        rows,
        title: l.title,
    });
    // One click now spawns a real child, so the strip names it and names the
    // way back. Ctrl+Shift+X is already aimed at it, because `claim_launch`
    // focuses the session the moment the daemon confirms it.
    st.write().window.flash = Some(Flash::notice(format!(
        "Started {word} in {place}. Ctrl+Shift+X stops it."
    )));
}

/// Fire one saved preset from its own chord, with no dialog.
///
/// The point of a preset shortcut: one keystroke starts a named command in a
/// named directory. Anything that stops it says so out loud rather than
/// opening a dialog the operator did not ask for, because a chord that
/// sometimes launches and sometimes opens a form is a chord nobody can build
/// a habit around.
///
/// The preset is looked up by id at fire time, not captured when the chord was
/// bound: editing a preset's command must change what its chord does, and
/// deleting it must make the chord say so instead of running a stale copy.
pub(crate) fn launch_preset(
    bridge: Bridge,
    mut st: Signal<UiState>,
    pending: Signal<Option<PendingLaunch>>,
    id: u64,
) {
    if !st.peek().server_ready() {
        st.write().window.flash = Some(Flash::notice(
            "Starting a session needs the daemon; this window is not connected.",
        ));
        return;
    }
    let store = launch::load_launch_store();
    let Some(preset) = store.presets.iter().find(|p| p.id == id).cloned() else {
        // The chord outlived the preset. Saying so beats silence, which is
        // indistinguishable from a keyboard that dropped the keystroke.
        st.write().window.flash = Some(Flash::notice(
            "That shortcut's saved command no longer exists.",
        ));
        return;
    };
    let here = seed_dir(&st.peek(), None);
    match launch::preset_launch(&preset, &here) {
        Ok(l) => {
            let pid = launch::resolve_project(&st.peek().daemon.projects, &l.cwd).0;
            start_session(bridge, st, pending, pid, l);
        }
        // A pinned directory that has been deleted, or a command that is not on
        // PATH. Named, because "nothing happened" is the worst answer.
        Err(why) => {
            st.write().window.flash = Some(Flash::notice(format!("{}: {why}", preset.label)));
        }
    }
}

/// One click, no layer: start the top-ranked launch, or open the list and let
/// it say why it will not guess.
///
/// This is what makes a new session one click. Every route used to cost two,
/// the first of which only opened a form asking three questions with known
/// answers.
pub(crate) fn launch_now(
    bridge: Bridge,
    mut st: Signal<UiState>,
    pending: Signal<Option<PendingLaunch>>,
    project: Option<ProjectId>,
) {
    if !st.peek().server_ready() {
        st.write().window.flash = Some(Flash::notice(
            "Starting a session needs the daemon; this window is not connected.",
        ));
        return;
    }
    let here = seed_dir(&st.peek(), project);
    let decided = ui::dialog::primary_launch(&st.peek(), &here);
    match decided {
        ui::dialog::Primary::Ready(l) => {
            let pid = launch::resolve_project(&st.peek().daemon.projects, &l.cwd).0;
            start_session(bridge, st, pending, pid, l);
        }
        // Never a guess. The launcher recomputes the same decision and renders
        // the same sentence, so there is one copy of it and it cannot go stale.
        ui::dialog::Primary::Choose(_) => open_new_session(st, project),
    }
}

/// Start a second session with `id`'s command, directory and title.
///
/// Same everything, new PTY. Round-tripped through `launch::duplicate_of`
/// rather than copied field by field, so a session whose checkout was deleted
/// after it started says so here instead of failing at spawn three seconds
/// later.
pub(crate) fn duplicate_session(bridge: Bridge, mut st: Signal<UiState>, id: SessionId) {
    let made = st.peek().session(id).map(launch::duplicate_of);
    let l = match made {
        Some(Ok(l)) => l,
        Some(Err(why)) => {
            st.write().window.flash = Some(Flash::notice(why));
            return;
        }
        // The row vanished between the right-click and the pick.
        None => return,
    };
    if !st.peek().server_ready() {
        st.write().window.flash = Some(Flash::notice(
            "Duplicating needs the daemon; this window is not connected.",
        ));
        return;
    }
    let (project, cols, rows) = {
        let r = st.peek();
        (
            launch::resolve_project(&r.daemon.projects, &l.cwd).0,
            r.window.cols,
            r.window.rows,
        )
    };
    bridge.msg(&ClientMsg::CreateSession {
        project_id: project,
        cwd: l.cwd,
        command: l.command,
        args: l.args,
        cols,
        rows,
        title: l.title,
    });
}

/// Reopen the socket after a failure.
///
/// Goes through the same daemon check as startup. The Retry button existed
/// before the daemon was ever started automatically, and on a machine with no
/// daemon it was a button that could not work no matter how many times it was
/// pressed. Now it can: if the reason for the failure was that nothing was
/// listening, retrying starts the thing that should have been.
pub(crate) fn retry(
    bridge: Bridge,
    mut st: Signal<UiState>,
    mut attached: Signal<Option<SessionId>>,
    opts: Options,
) {
    let url = {
        let mut w = st.write();
        w.daemon.conn = ConnState::Connecting;
        w.window.layer = Layer::None;
        w.daemon
            .settings
            .resolved_daemon_url(opts.server)
            .to_string()
    };
    attached.set(None);
    spawn(async move {
        start_daemon_then_connect(bridge, st, &url, opts).await;
    });
}

/// Make sure the daemon exists, then point the bridge at it.
///
/// Connect-first is the whole design: [`launch::ensure_daemon`] probes the
/// port before it considers spawning, so a daemon somebody else started, or
/// one left over from a previous run holding twenty live agents, is reused
/// rather than duplicated. The probe runs off the UI thread because a cold
/// start can take a second or two and a frozen window is not an acceptable
/// way to spend it.
///
/// A failure here is a named failure. "The session daemon vitrum-server is not
/// installed. Looked beside this program at ..., then on PATH. Start it
/// yourself with: vitrum-server" is a sentence a person can act on;
/// "disconnected" is not.
pub(crate) async fn start_daemon_then_connect(
    bridge: Bridge,
    mut st: Signal<UiState>,
    url: &str,
    opts: Options,
) {
    let probe_url = url.to_string();
    let autostart = opts.autostart;
    let outcome = off_thread(move || launch::ensure_daemon(&probe_url, autostart)).await;

    match &outcome {
        launch::Autostart::Started { pid, path } => {
            tracing::info!("started {} as pid {pid}", path.display());
        }
        launch::Autostart::AlreadyRunning => tracing::info!("reusing the running daemon at {url}"),
        other => tracing::warn!("daemon autostart: {other:?}"),
    }

    if outcome.connectable() {
        bridge.cmd(BridgeCmd::Connect {
            url: url.to_string(),
        });
    } else if let Some(detail) = outcome.failure() {
        st.write().daemon.conn = ConnState::Failed { detail };
    }
}

/// Terminate a session's child and drop it from the daemon.
///
/// Unconditional. Every caller reaches it through [`request_terminate`], which
/// is where the confirmation lives.
pub(crate) fn terminate(bridge: Bridge, mut st: Signal<UiState>, id: SessionId, opts: Options) {
    if opts.fixture {
        let mut w = st.write();
        w.close_tab(id);
        w.daemon.sessions.retain(|row| row.id() != id);
        return;
    }
    // The daemon answers with an `Exited` delta rather than a fresh snapshot
    // (`vitrum-server` conn.rs, the `ClientMsg::Close` arm): killing the child
    // makes the PTY report EOF and every connected window learns from that one
    // delta. Closing the tab here as well keeps the strip from lagging a round
    // trip behind the click.
    bridge.msg(&ClientMsg::Close { session: id });
    st.write().close_tab(id);
}

/// Is this press the answer to the prompt already on screen?
///
/// `None` means nothing is armed for these rows, so ask. `Some(retire)` means
/// go ahead, and `retire` says whether the prompt is still the message on the
/// strip and so must come down with it.
///
/// Pure, and separate from the signals, because the retiring is the part that
/// was missing: an error flash never expires by design, so a prompt that had
/// already been answered kept asking about a session that no longer existed,
/// which reads as an armed prompt rather than a spent one. Comparing the text
/// keeps a newer message about something else on screen.
pub(crate) fn answers_prompt(
    armed: &[SessionId],
    live: &[SessionId],
    flash: Option<&Flash>,
    prompt: &str,
) -> Option<bool> {
    if armed != live {
        return None;
    }
    Some(flash.is_some_and(|f| f.text == prompt))
}

/// Terminate, or ask first.
///
/// `Settings::confirm_terminate` defaults to on, and it has to mean something:
/// terminating kills a real child process and there is no undo. The prompt is
/// the flash strip rather than a modal, because a modal for this would be a
/// fourth layer competing for Escape with the three that already exist.
///
/// Sessions that have already exited are dropped without a prompt. Asking an
/// operator to confirm the death of a process that ended ten minutes ago is
/// how a prompt trains people to press through it without reading, which is
/// precisely how it stops protecting the live ones.
pub(crate) fn request_terminate(
    bridge: Bridge,
    mut st: Signal<UiState>,
    targets: &[SessionId],
    opts: Options,
    mut pending: Signal<Vec<SessionId>>,
) {
    if targets.is_empty() {
        return;
    }
    let (ask, confirmed): (bool, Vec<SessionId>) = {
        let read = st.peek();
        let live: Vec<SessionId> = targets
            .iter()
            .copied()
            .filter(|id| read.session(*id).is_some_and(|s| s.status.is_live()))
            .collect();
        (
            read.daemon.settings.confirm_terminate && !live.is_empty(),
            live,
        )
    };

    if !ask {
        pending.set(Vec::new());
        for id in targets {
            terminate(bridge, st, *id, opts);
        }
        return;
    }

    let text = match confirmed.len() {
        1 => {
            let title = st
                .peek()
                .session(confirmed[0])
                .map(|s| s.title.clone())
                .unwrap_or_else(|| "this session".to_string());
            format!("Terminate {title}? Its child process is killed and there is no undo.")
        }
        n => format!(
            "Terminate {n} sessions? Their child processes are killed and there is no undo."
        ),
    };

    let decision = {
        let read = st.peek();
        answers_prompt(
            &pending.peek(),
            &confirmed,
            read.window.flash.as_ref(),
            &text,
        )
    };
    match decision {
        None => {
            pending.set(confirmed);
            st.write().window.flash = Some(Flash::error(text));
            return;
        }
        Some(retire) => {
            pending.set(Vec::new());
            if retire {
                st.write().window.flash = None;
            }
        }
    }
    for id in targets {
        terminate(bridge, st, *id, opts);
    }
}

/// Perform one context-menu entry.
///
/// Every entry acts on [`UiState::menu_targets`], which is the whole selection
/// when the right-click landed inside one and the single row otherwise. The
/// tab-strip entries are the exception: a tab menu is about one tab, and
/// "close tabs to the right" has no plural reading.
pub(crate) fn on_menu_action(
    action: MenuAction,
    menu: MenuState,
    bridge: Bridge,
    mut st: Signal<UiState>,
    attached: Signal<Option<SessionId>>,
    opts: Options,
    pending_terminate: Signal<Vec<SessionId>>,
) {
    let id = menu.target;
    let tick = tick();
    let targets = st.peek().menu_targets(menu.target, tick.model);
    dismiss(st);
    match action {
        // A caption is rendered disabled, so this arm is only reachable if the
        // markup ever stops disabling it. Doing nothing is the correct answer
        // either way.
        MenuAction::SnoozeHeader => return,
        MenuAction::Focus => st.write().open(id, tick.now_ms),
        MenuAction::CloseTab => st.write().close_tab(id),
        MenuAction::CloseOthers => st.write().close_other_tabs(id),
        MenuAction::Snooze(preset_id) => {
            let preset = st
                .peek()
                .snooze_presets(tick.model)
                .into_iter()
                .find(|p| p.id == preset_id);
            // The preset list is time-dependent: "this evening" disappears
            // once evening is under an hour away. A pick for a preset that is
            // no longer offered says so rather than parking the row until an
            // instant nobody chose.
            let Some(preset) = preset else {
                st.write().window.flash = Some(Flash::notice(
                    "That snooze time has passed. Open the menu again for current options.",
                ));
                return;
            };
            let parked = st.write().snooze(&targets, preset.wake_at_ms, tick.now_ms);
            let when = vitrum_model::wake_description(preset.wake_at_ms, tick.model);
            let text = if parked == targets.len() {
                format!("Snoozed {parked} until {when}")
            } else {
                format!(
                    "Snoozed {parked} of {} until {when}; the rest are blocked on you",
                    targets.len()
                )
            };
            st.write().window.flash = Some(Flash::notice(text));
        }
        MenuAction::Wake => st.write().wake(&targets, tick.now_ms),
        MenuAction::Settle => {
            let drained = st.write().settle(&targets, tick.now_ms);
            if drained < targets.len() {
                st.write().window.flash = Some(Flash::notice(format!(
                    "Settled {drained} of {}; the rest are still working or blocked on you",
                    targets.len()
                )));
            }
        }
        MenuAction::Unsettle => st.write().unsettle(&targets),
        MenuAction::MarkRead => st.write().mark_seen(&targets, tick.now_ms),
        MenuAction::MarkUnread => st.write().mark_unseen(&targets),
        MenuAction::Rename => {
            let title = st.peek().session(id).map(|s| s.title.clone());
            // A rename dialog for a session that vanished between the
            // right-click and the pick would send a title for an id the daemon
            // no longer has.
            if let Some(title) = title {
                st.write().window.layer = Layer::Rename(RenameSeed { session: id, title });
            }
            return;
        }
        MenuAction::CopyPath => copy(bridge, st, id, |s| s.cwd.clone()),
        MenuAction::CopyBranch => {
            copy(bridge, st, id, |s| s.git_branch.clone().unwrap_or_default())
        }
        MenuAction::CopyCommand => copy(bridge, st, id, |s| {
            let mut line = s.command.clone();
            for a in &s.args {
                line.push(' ');
                line.push_str(a);
            }
            line
        }),
        MenuAction::NewSessionHere => {
            let project = st.peek().session(id).map(|s| s.project_id);
            open_new_session(st, project);
            return;
        }
        // Falls through to the reconcile at the end of this function, unlike
        // NewSessionHere above which returns: duplicating changes the session
        // list, and the strip has to be reconciled against it. Same helper as
        // Ctrl+Shift+D, so the pointer and the keyboard cannot drift.
        MenuAction::Duplicate => duplicate_session(bridge, st, id),
        // Captions, rendered disabled. Reachable only if the markup ever stops
        // disabling them, and doing nothing is the right answer either way.
        MenuAction::MoveToWorkspaceHeader | MenuAction::MoveToFolderHeader => return,
        MenuAction::MoveToWorkspace(workspace) => {
            let outcome = st
                .write()
                .move_to_workspace(&targets, workspace, tick.now_ms);
            report_move(st, outcome, targets.len(), "workspace");
            // Filing is a deliberate act on a durable arrangement, so it is
            // written now rather than at the next window event. The Workspaces
            // tab already commits every edit it makes; this menu is the path
            // that actually files sessions and it did not, so a move survived a
            // restart only if something else committed afterwards.
            save_window_state(st);
            return;
        }
        MenuAction::MoveToFolder(folder) => {
            let outcome = st.write().move_to_folder(&targets, folder);
            report_move(st, outcome, targets.len(), "folder");
            save_window_state(st);
            return;
        }
        MenuAction::Terminate => {
            request_terminate(bridge, st, &targets, opts, pending_terminate);
        }
    }
    reconcile(bridge, st, attached, opts);
}

/// Say what a move actually did.
///
/// A move of five rows that placed three is not a success, and reporting it as
/// one is how a user discovers two sessions missing an hour later. The count
/// is compared against what was asked for and the shortfall is named.
pub(crate) fn report_move(
    mut st: Signal<UiState>,
    outcome: Result<usize, state::WorkspaceError>,
    asked: usize,
    what: &str,
) {
    st.write().window.flash = Some(match outcome {
        Ok(moved) if moved == asked => Flash::notice(format!("Moved {moved} to another {what}")),
        Ok(moved) => Flash::notice(format!(
            "Moved {moved} of {asked} to another {what}; the rest are already there"
        )),
        Err(e) => Flash::error(format!("Could not move to that {what}: {e}")),
    });
}

/// Put one field of a session on the clipboard.
///
/// The outcome is reported by the bridge rather than assumed here. A webview
/// can refuse a clipboard write, and "Copied" for a copy that did not happen
/// is a lie the user discovers only when they paste.
pub(crate) fn copy(
    bridge: Bridge,
    st: Signal<UiState>,
    id: SessionId,
    pick: impl Fn(&vitrum_proto::SessionInfo) -> String,
) {
    let text = st.peek().session(id).map(pick).unwrap_or_default();
    if text.is_empty() {
        return;
    }
    bridge.cmd(BridgeCmd::Clipboard { text });
}
