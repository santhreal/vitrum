//! Keeping the client and the daemon agreeing: attachment, backfill, deep
//! links, and reconnection.

use super::*;

/// Make the server's attachment match the focused session.
///
/// Every attach and detach in the program goes through here. Routing them
/// through one function is what makes a reconnect indistinguishable from a tab
/// click: both clear `attached` and let this reconcile.
pub(crate) fn reconcile(
    bridge: Bridge,
    st: Signal<UiState>,
    mut attached: Signal<Option<SessionId>>,
    opts: Options,
) {
    let want = st.peek().window.focused;
    let have = *attached.peek();
    if want == have {
        return;
    }

    if let Some(prev) = have
        && !opts.fixture
    {
        // Detach, never close. The child keeps running and the server keeps
        // filling its ring, so nothing is lost by looking away.
        bridge.msg(&ClientMsg::Detach { session: prev });
    }

    // Resets the grid and starts buffering live frames until the backfill
    // lands, so history and live output cannot interleave.
    bridge.cmd(BridgeCmd::Focus {
        session: want.map(|s| s.0),
    });

    if let Some(next) = want {
        if opts.fixture {
            if let Some(info) = st.peek().session(next) {
                bridge.cmd(BridgeCmd::Banner {
                    lines: fixture::transcript(info),
                });
            }
        } else {
            let (cols, rows, scrollback_lines, intent) = {
                let r = st.peek();
                (
                    r.window.cols,
                    r.window.rows,
                    r.daemon.settings.terminal.scrollback_lines,
                    r.window.history_intent,
                )
            };
            bridge.msg(&ClientMsg::Attach {
                session: next,
                cols,
                rows,
            });
            // Asked immediately after Attach. The server clamps `before_seq`
            // to its current head; anything the child emitted between the two
            // is spliced back out by byte offset.
            //
            // The budget is a function of the operator's scrollback setting
            // rather than a constant. The setting's caption promises that
            // raising it is how you see further back, and against a fixed
            // 64 KiB budget that was false for everything written before the
            // attach: the xterm buffer grew and not one extra byte arrived.
            //
            // A pending jump anchors the window on the hit instead of on the
            // head. Without that, activating a search result for a line
            // written an hour ago paints the last minute of output and the
            // hit is simply not in the buffer to scroll to.
            let before_seq = match intent {
                state::HistoryIntent::Jump(seq) => seq.saturating_add(wire::JUMP_TAIL_BYTES),
                _ => BEFORE_SEQ_HEAD,
            };
            bridge.msg(&ClientMsg::Scrollback {
                session: next,
                before_seq,
                max_bytes: backfill_max_bytes(scrollback_lines),
            });
        }
    }

    attached.set(want);
}

/// Ask the daemon for history older than what is painted, and repaint.
///
/// THE SHAPE, and why it is a repaint rather than a prepend: xterm.js has no
/// way to insert lines above its buffer. Everything else is a workaround that
/// gets the splice wrong somewhere. So a page-back re-requests a BIGGER window
/// ending at the same head, resets the grid and writes the whole thing again.
/// The daemon already holds the bytes, so nothing has to be retained on the
/// hot path to make this exact.
///
/// It is affordable because it is a deliberate gesture. The bridge sends this
/// once per arrival at the top of the buffer, never per wheel tick, and
/// [`wire::PAGE_CEILING_BYTES`] stops the window growing without bound.
pub(crate) fn page_back(bridge: Bridge, mut st: Signal<UiState>, session: SessionId) {
    let (history, scrollback_lines, focused) = {
        let r = st.peek();
        (
            r.window.history,
            r.daemon.settings.terminal.scrollback_lines,
            r.window.focused,
        )
    };
    // The operator may have moved on while the event was in flight. Repainting
    // another session's history into this grid is the one outcome worse than
    // not paging.
    if focused != Some(session) || history.session != Some(session) {
        return;
    }
    if !history.more {
        st.write().window.flash = Some(Flash::notice(
            "That is the whole history the daemon still holds for this session.",
        ));
        return;
    }
    let Some(max_bytes) = wire::page_back_max_bytes(history.span, scrollback_lines) else {
        st.write().window.flash = Some(Flash::notice(
            "This pane is holding as much history as it will. Search across \
             sessions to find older output.",
        ));
        return;
    };
    st.write().window.history_intent = state::HistoryIntent::PageBack;
    bridge.msg(&ClientMsg::Scrollback {
        session,
        before_seq: BEFORE_SEQ_HEAD,
        max_bytes,
    });
}

/// Open the session a `vitrum://session/N` handoff named, once the daemon has
/// confirmed it exists.
///
/// Retried after every bridge event rather than only on the first snapshot.
/// The link is known before the socket is even open, and a session the daemon
/// has not listed yet is the normal case, not the exception; giving up on the
/// first miss would silently discard the request.
///
/// A link naming a session that no longer exists never fires and never does
/// anything else either, which is the honest outcome: there is nothing to open
/// and inventing a substitute would put the user in a session they did not ask
/// for.
pub(crate) fn claim_link(
    bridge: Bridge,
    mut st: Signal<UiState>,
    attached: Signal<Option<SessionId>>,
    mut pending: Signal<Option<SessionId>>,
    opts: Options,
) {
    let Some(id) = *pending.peek() else {
        return;
    };
    if st.peek().row(id).is_none() {
        return;
    }
    pending.set(None);
    st.write().open(id, tick().now_ms);
    reconcile(bridge, st, attached, opts);
}

/// Open the session THIS window just asked the daemon to start.
///
/// Same shape and the same retry semantics as [`claim_link`], for the same
/// reason: the reply can arrive before, with, or several snapshots after the
/// request, and giving up on the first miss silently discards what the
/// operator asked for.
///
/// Matched on the directory and the command line rather than on an id, because
/// the protocol has no request id and the client does not choose the session
/// id. `before` is what keeps that from focusing the wrong row: the session
/// has to be one that did not exist when the request went out.
///
/// `SessionCreated` is broadcast to every window, so without a record of who
/// asked, a window cannot tell its own launch from another's and focusing on
/// receipt would yank nineteen operators into a session they did not start.
/// That is why Launch used to leave you on "No session focused" with a new row
/// in the sidebar.
pub(crate) fn claim_launch(
    bridge: Bridge,
    mut st: Signal<UiState>,
    attached: Signal<Option<SessionId>>,
    mut pending: Signal<Option<PendingLaunch>>,
    opts: Options,
) {
    let Some(want) = pending.peek().clone() else {
        return;
    };
    let found = st.peek().daemon.sessions.iter().find_map(|r| {
        let i = &r.info;
        (!want.before.contains(&i.id)
            && i.cwd == want.cwd
            && launch::join_command(&i.command, &i.args) == want.line)
            .then_some(i.id)
    });
    let Some(id) = found else {
        return;
    };
    pending.set(None);
    st.write().open(id, tick().now_ms);
    reconcile(bridge, st, attached, opts);
}

/// Raise a desktop notification for every session that just crossed a line
/// worth interrupting the operator for.
///
/// Edge-triggered, in [`ui::settings::notable_transitions`]: a session that was
/// already blocked stays quiet, so twenty agents do not produce twenty
/// notifications on every snapshot. A session absent from `before` is never
/// notable, which is what keeps a reconnect from emptying a whole day of
/// failures onto the desktop at once.
pub(crate) fn notify_transitions(st: Signal<UiState>, before: &[vitrum_model::SessionView]) {
    let read = st.peek();
    for notable in ui::settings::notable_transitions(before, &read.daemon.sessions) {
        let focused = read.window.focused == Some(notable.session);
        if ui::settings::should_notify(&read.daemon.settings.notifications, notable.kind, focused) {
            ui::settings::notify_now(&notable.notification());
        }
    }
}

pub(crate) fn on_bridge_event(
    ev: BridgeEvent,
    bridge: Bridge,
    mut st: Signal<UiState>,
    mut attached: Signal<Option<SessionId>>,
    opts: Options,
    pending_terminate: Signal<Vec<SessionId>>,
    pending_open: Signal<Option<PendingLaunch>>,
    mut reconnect: Signal<u32>,
) {
    match ev {
        BridgeEvent::Server { msg } => {
            // The server starts a per-connection status watcher for every
            // session named in a List snapshot, so a session we never listed
            // never pushes SessionUpdated or Exited and its sidebar row looks
            // alive forever. List is idempotent and free, so it is sent again
            // here rather than relying on how Hello and List interleaved with
            // the handshake.
            let welcome = matches!(msg, vitrum_proto::ServerMsg::Welcome { .. });
            // A session appearing or ending changes the watched set, and the
            // daemon republishes only when the CONTESTED set changes. Per
            // session counts and the degraded list move without it, so a new
            // session's row would keep another session's numbers until two
            // agents next collided. The one-shot re-reads the report without
            // touching the subscription.
            let membership = matches!(
                msg,
                vitrum_proto::ServerMsg::SessionCreated(_) | vitrum_proto::ServerMsg::Exited { .. }
            );
            // Snapshotted only when at least one notification switch is on
            // AND the message can actually move a session. A transition is a
            // difference between two session lists, so a scrollback chunk, a
            // search answer or a collision report can never produce one, and
            // cloning twenty `SessionView`s (four strings and a vector each)
            // to diff a list against itself was the whole cost of the feature
            // on the busiest messages the daemon sends.
            let moves_sessions = matches!(
                msg,
                vitrum_proto::ServerMsg::Sessions { .. }
                    | vitrum_proto::ServerMsg::SessionCreated(_)
                    | vitrum_proto::ServerMsg::SessionUpdated(_)
                    | vitrum_proto::ServerMsg::Exited { .. }
            );
            let before = moves_sessions
                .then(|| {
                    let read = st.peek();
                    let want = &read.daemon.settings.notifications;
                    (want.finished || want.needs_approval || want.failed)
                        .then(|| read.daemon.sessions.clone())
                })
                .flatten();
            let now = tick();
            let reaction = st.write().apply(msg, now.now_ms);
            if let Some(before) = before {
                notify_transitions(st, &before);
            }
            // The one number the dock, taskbar or launcher entry shows. Every
            // session the daemon holds, not this window's workspace: there is
            // one badge per process and the sessions it is most worth
            // reporting are the ones nobody has on screen.
            badge::publish(st.peek().daemon.attention_total(now.model));
            if welcome && !opts.fixture && matches!(st.peek().daemon.conn, ConnState::Live { .. }) {
                bridge.msg(&ClientMsg::List);
                // Subscribe, on every connect.
                //
                // The daemon holds no watcher, no thread and no watch
                // descriptors until a client asks, which is what keeps a
                // headless daemon at zero. But a window IS a client, and an
                // operator running two agents in one checkout wants to be
                // told they are overwriting each other whether or not they
                // found a setting first. Two agents silently clobbering one
                // file is work already lost by the time anybody notices, so
                // the default has to be on and the option unnecessary.
                //
                // Measured cost on a 65-directory checkout with twenty
                // sessions: 64 inotify watches, 612 KiB, no measurable CPU
                // while nothing writes.
                bridge.msg(&ClientMsg::WatchCollisions { enabled: true });
            }
            if membership && !opts.fixture && st.peek().daemon.collisions.watching {
                bridge.msg(&ClientMsg::Collisions);
            }
            match reaction {
                Reaction::None => {}
                Reaction::Backfill {
                    session,
                    from_seq,
                    resume_seq,
                    bytes,
                    jump_seq,
                    keep_view,
                    more,
                } => bridge.cmd(BridgeCmd::Backfill {
                    session: session.0,
                    from_seq: from_seq.to_string(),
                    resume_seq: resume_seq.to_string(),
                    bytes,
                    jump_seq: jump_seq.map(|s| s.to_string()),
                    keep_view,
                    more,
                }),
                Reaction::Refill { .. } => {
                    // Full detach and re-attach. Splicing across a reported gap
                    // is exactly what the byte-offset seq exists to prevent.
                    attached.set(None);
                }
            }
            reconcile(bridge, st, attached, opts);
        }

        BridgeEvent::Conn { state, detail } => match state {
            ConnEvent::Open => {
                st.write().daemon.conn = ConnState::Connecting;
                bridge.msg(&ClientMsg::Hello {
                    protocol: PROTOCOL_VERSION,
                });
                bridge.msg(&ClientMsg::List);
                // A fresh socket knows nothing about our previous attachment.
                attached.set(None);
                // The link came back, so the schedule starts from zero next
                // time. Without this a laptop that drops once an hour reaches
                // the cap by lunchtime and then waits a minute to recover from
                // a blip that lasted a second.
                reconnect.set(0);
                reconcile(bridge, st, attached, opts);
            }
            ConnEvent::Closed | ConnEvent::Error => {
                st.write().daemon.conn = ConnState::Failed {
                    detail: detail.unwrap_or_else(|| "connection lost".to_string()),
                };
                attached.set(None);
                schedule_reconnect(bridge, st, reconnect, opts);
            }
        },

        BridgeEvent::Resize { cols, rows } => {
            // Guarded: writing an unchanged value would still mark the signal
            // dirty and repaint the whole shell on every layout pass.
            let changed = {
                let r = st.peek();
                r.window.cols != cols || r.window.rows != rows
            };
            if changed {
                let mut w = st.write();
                w.window.cols = cols;
                w.window.rows = rows;
            }
        }

        BridgeEvent::PageBack { session } => page_back(bridge, st, SessionId(session)),

        // Through the custom dispatcher, not straight to `on_key`. It
        // consults the operator's own bindings first, so a chord they rebound
        // shadows the built-in one, and it reports an unknown chord itself.
        BridgeEvent::Key { action } => {
            dispatch_key(
                &action,
                bridge,
                st,
                attached,
                opts,
                pending_terminate,
                pending_open,
            );
        }

        BridgeEvent::Copied { ok, text } => {
            st.write().window.flash = Some(if ok {
                Flash::notice(format!("Copied {text}"))
            } else {
                Flash::error(format!("Could not copy {text} to the clipboard"))
            });
        }

        BridgeEvent::Bad { detail } => {
            tracing::warn!("bridge: {detail}");
            st.write().window.flash = Some(Flash::error(detail));
        }
    }
}

/// How long to wait before reconnect attempt `n`, in milliseconds.
///
/// Doubling from a quarter second to a ceiling of thirty. The early attempts
/// are what recover a blip without the operator noticing; the ceiling is what
/// stops a machine that has been asleep for a week from having spent the night
/// dialling. Returns `None` once the schedule is exhausted, which is a real
/// answer: the window then says the daemon is gone and offers Retry, rather
/// than reconnecting silently forever.
#[must_use]
pub(crate) fn reconnect_delay_ms(attempt: u32) -> Option<u64> {
    (attempt < RECONNECT_ATTEMPTS).then(|| {
        RECONNECT_BASE_MS
            .saturating_mul(1 << attempt.min(7))
            .min(RECONNECT_MAX_MS)
    })
}

/// Try the daemon again, later.
///
/// This is the one automatic reconnect in the program, and it is here because
/// a window may be pointed at a daemon across a network now: a laptop that
/// closes its lid must not need a click to come back. It is a SCHEDULE, not a
/// loop. Each attempt is one `sleep` that fires once; a connected window has
/// none outstanding, so the idle cost this program is built around is
/// unchanged at rest.
///
/// The schedule ends. When it does the window keeps saying the connection
/// failed and the Retry button is still the way back, which is what it was
/// before any of this existed.
pub(crate) fn schedule_reconnect(
    bridge: Bridge,
    st: Signal<UiState>,
    mut reconnect: Signal<u32>,
    opts: Options,
) {
    // A window that never talks to a daemon has nothing to reconnect to.
    if opts.fixture {
        return;
    }
    let attempt = *reconnect.peek();
    let Some(delay) = reconnect_delay_ms(attempt) else {
        return;
    };
    reconnect.set(attempt + 1);
    spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        // The operator may have reconnected by hand, or pointed this window at
        // another daemon, while this was asleep. Re-read rather than acting on
        // what was true when the timer was set.
        //
        // The URL is RESOLVED from the setting, not taken from `opts.server`.
        // THE BUG: this dialled the command line while the manual Retry button
        // dialled the setting, so a window pointed at another daemon through
        // Settings silently returned to the wrong one on the first blip.
        if matches!(st.peek().daemon.conn, ConnState::Failed { .. }) {
            let url = st
                .peek()
                .daemon
                .settings
                .resolved_daemon_url(opts.server)
                .to_string();
            bridge.cmd(BridgeCmd::Connect { url });
        }
    });
}
