//! What the operator asked for, carried out.
//!
//! Menu items, launcher entries and keyboard actions all end here, so a
//! session starts the same way whichever surface asked for it.

use super::*;

/// Close whichever layer is open.
///
/// Onboarding and What's New both promise that closing them, by Skip, Got it,
/// backdrop, or Escape, records that they were seen. The panels honour that
/// for clicks; Escape goes through this function, so the same persistence has
/// to live here or a sheet dismissed with Escape returns on the next launch.
pub(crate) fn dismiss(cx: &Rc<Ctx>) {
    let layer = cx.peek(|st| st.window.layer.clone());
    if !layer.is_open() {
        return;
    }
    let persist = dismiss_persists(&layer);
    cx.edit(|w| {
        if persist {
            let current = update::current_version();
            match layer {
                Layer::Onboarding => w.daemon.settings.finish_onboarding(&current),
                Layer::WhatsNew => w.daemon.settings.mark_seen(&current),
                _ => {}
            }
        }
        w.window.layer = Layer::None;
    });
    if persist {
        cx.peek(ui::settings::commit);
    }
}

/// Whether dismissing this layer must also write the "seen" bit.
///
/// Extracted so the Escape path and the click handlers can be tested against
/// the same rule without standing up a window.
pub(crate) fn dismiss_persists(layer: &Layer) -> bool {
    matches!(layer, Layer::Onboarding | Layer::WhatsNew)
}

/// Open `next`, or close it if it is already the open layer.
pub(crate) fn toggle_layer(cx: &Rc<Ctx>, next: Layer) {
    cx.edit(|w| {
        w.window.layer = if w.window.layer == next {
            Layer::None
        } else {
            next
        };
    });
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
pub(crate) fn open_new_session(cx: &Rc<Ctx>, project: Option<ProjectId>) {
    let seed = cx.peek(|r| NewSessionSeed {
        project,
        cwd: seed_dir(r, project),
    });
    cx.edit(|w| w.window.layer = Layer::NewSession(seed));
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
pub(crate) fn start_session(cx: &Rc<Ctx>, project: ProjectId, l: launch::Launch) {
    // A profile that cannot be written costs the operator their ranking next
    // time, never this session, so a failed write is logged and it goes ahead.
    let prefs = cx.peek(|r| r.daemon.settings.launcher);
    if let Err(why) = launch::record_launch(&l.command, &l.args, &l.cwd, launch::now_ms(), prefs) {
        tracing::warn!("launch history not saved: {why}");
    }
    let word = ui::dialog::basename(&l.command).to_string();
    let (cols, rows, place, before) = cx.peek(|r| {
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
    });
    *cx.pending_open.borrow_mut() = Some(PendingLaunch {
        cwd: l.cwd.clone(),
        line: launch::join_command(&l.command, &l.args),
        before,
    });
    cx.bridge.msg(&ClientMsg::CreateSession {
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
    cx.flash(Flash::notice(format!(
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
pub(crate) fn launch_preset(cx: &Rc<Ctx>, id: u64) {
    if !cx.peek(UiState::server_ready) {
        cx.flash(Flash::notice(
            "Starting a session needs the daemon; this window is not connected.",
        ));
        return;
    }
    let store = launch::load_launch_store();
    let Some(preset) = store.presets.iter().find(|p| p.id == id).cloned() else {
        // The chord outlived the preset. Saying so beats silence, which is
        // indistinguishable from a keyboard that dropped the keystroke.
        cx.flash(Flash::notice(
            "That shortcut's saved command no longer exists.",
        ));
        return;
    };
    let here = cx.peek(|r| seed_dir(r, None));
    match launch::preset_launch(&preset, &here) {
        Ok(l) => {
            let pid = cx.peek(|r| launch::resolve_project(&r.daemon.projects, &l.cwd).0);
            start_session(cx, pid, l);
        }
        // A pinned directory that has been deleted, or a command that is not on
        // PATH. Named, because "nothing happened" is the worst answer.
        Err(why) => cx.flash(Flash::notice(format!("{}: {why}", preset.label))),
    }
}

/// One click, no layer: start the top-ranked launch, or open the list and let
/// it say why it will not guess.
///
/// This is what makes a new session one click. Every route used to cost two,
/// the first of which only opened a form asking three questions with known
/// answers.
pub(crate) fn launch_now(cx: &Rc<Ctx>, project: Option<ProjectId>) {
    if !cx.peek(UiState::server_ready) {
        cx.flash(Flash::notice(
            "Starting a session needs the daemon; this window is not connected.",
        ));
        return;
    }
    let here = cx.peek(|r| seed_dir(r, project));
    let decided = cx.peek(|r| ui::dialog::primary_launch(r, &here));
    match decided {
        ui::dialog::Primary::Ready(l) => {
            let pid = cx.peek(|r| launch::resolve_project(&r.daemon.projects, &l.cwd).0);
            start_session(cx, pid, l);
        }
        // Never a guess. The launcher recomputes the same decision and renders
        // the same sentence, so there is one copy of it and it cannot go stale.
        ui::dialog::Primary::Choose(_) => open_new_session(cx, project),
    }
}

/// Start a second session with `id`'s command, directory and title.
///
/// Same everything, new PTY. Round-tripped through `launch::duplicate_of`
/// rather than copied field by field, so a session whose checkout was deleted
/// after it started says so here instead of failing at spawn three seconds
/// later.
pub(crate) fn duplicate_session(cx: &Rc<Ctx>, id: SessionId) {
    let made = cx.peek(|r| r.session(id).map(launch::duplicate_of));
    let l = match made {
        Some(Ok(l)) => l,
        Some(Err(why)) => {
            cx.flash(Flash::notice(why));
            return;
        }
        // The row vanished between the right-click and the pick.
        None => return,
    };
    if !cx.peek(UiState::server_ready) {
        cx.flash(Flash::notice(
            "Duplicating needs the daemon; this window is not connected.",
        ));
        return;
    }
    let (project, cols, rows) = cx.peek(|r| {
        (
            launch::resolve_project(&r.daemon.projects, &l.cwd).0,
            r.window.cols,
            r.window.rows,
        )
    });
    cx.bridge.msg(&ClientMsg::CreateSession {
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
pub(crate) fn retry(cx: &Rc<Ctx>) {
    let url = cx.edit(|w| {
        w.daemon.conn = ConnState::Connecting;
        w.window.layer = Layer::None;
        w.daemon
            .settings
            .resolved_daemon_url(cx.opts.server)
            .to_string()
    });
    cx.attached.set(None);
    let cx = Rc::clone(cx);
    glib::MainContext::default().spawn_local(async move {
        start_daemon_then_connect(&cx, &url).await;
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
pub(crate) async fn start_daemon_then_connect(cx: &Rc<Ctx>, url: &str) {
    let probe_url = url.to_string();
    let autostart = cx.opts.autostart;
    let outcome = off_thread(move || launch::ensure_daemon(&probe_url, autostart)).await;

    match &outcome {
        launch::Autostart::Started { pid, path } => {
            tracing::info!("started {} as pid {pid}", path.display());
        }
        launch::Autostart::AlreadyRunning => tracing::info!("reusing the running daemon at {url}"),
        other => tracing::warn!("daemon autostart: {other:?}"),
    }

    if outcome.connectable() {
        cx.bridge.connect(url.to_string());
    } else if let Some(detail) = outcome.failure() {
        cx.edit(|w| w.daemon.conn = ConnState::failed(detail));
    }
}

/// Terminate a session's child and drop it from the daemon.
///
/// Unconditional. Every caller reaches it through [`request_terminate`], which
/// is where the confirmation lives.
pub(crate) fn terminate(cx: &Rc<Ctx>, id: SessionId) {
    if cx.opts.fixture {
        cx.edit(|w| {
            w.close_tab(id);
            w.daemon.sessions.retain(|row| row.id() != id);
        });
        return;
    }
    // The daemon answers with an `Exited` delta rather than a fresh snapshot
    // (`vitrum-server` conn.rs, the `ClientMsg::Close` arm): killing the child
    // makes the PTY report EOF and every connected window learns from that one
    // delta. Closing the tab here as well keeps the strip from lagging a round
    // trip behind the click.
    cx.bridge.msg(&ClientMsg::Close { session: id });
    cx.edit(|w| w.close_tab(id));
}

/// Is this press the answer to the prompt already on screen?
///
/// `None` means nothing is armed for these rows, so ask. `Some(retire)` means
/// go ahead, and `retire` says whether the prompt is still the message on the
/// strip and so must come down with it.
///
/// Pure, and separate from the state, because the retiring is the part that
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
pub(crate) fn request_terminate(cx: &Rc<Ctx>, targets: &[SessionId]) {
    if targets.is_empty() {
        return;
    }
    let (ask, confirmed): (bool, Vec<SessionId>) = cx.peek(|read| {
        let live: Vec<SessionId> = targets
            .iter()
            .copied()
            .filter(|id| read.session(*id).is_some_and(|s| s.status.is_live()))
            .collect();
        (
            read.daemon.settings.confirm_terminate && !live.is_empty(),
            live,
        )
    });

    if !ask {
        cx.edit(|w| w.window.armed_terminate.clear());
        for id in targets {
            terminate(cx, *id);
        }
        return;
    }

    let text = match confirmed.len() {
        1 => {
            let title = cx
                .peek(|r| r.session(confirmed[0]).map(|s| s.title.clone()))
                .unwrap_or_else(|| "this session".to_string());
            format!("Terminate {title}? Its child process is killed and there is no undo.")
        }
        n => format!(
            "Terminate {n} sessions? Their child processes are killed and there is no undo."
        ),
    };

    let decision = cx.peek(|read| {
        answers_prompt(
            &read.window.armed_terminate,
            &confirmed,
            read.window.flash.as_ref(),
            &text,
        )
    });
    match decision {
        None => {
            cx.edit(|w| {
                w.window.armed_terminate = confirmed;
                w.window.flash = Some(Flash::error(text));
            });
            return;
        }
        Some(retire) => cx.edit(|w| {
            w.window.armed_terminate.clear();
            if retire {
                w.window.flash = None;
            }
        }),
    }
    for id in targets {
        terminate(cx, *id);
    }
}
