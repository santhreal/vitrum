//! Keeping the client and the daemon agreeing: attachment, backfill, deep
//! links, reconnection, and the one reducer every event goes through.

use super::*;

/// First reconnect delay, in milliseconds.
///
/// Not a preference. It is sized against how long a daemon takes to bind its
/// socket, and a shorter one turns the first two attempts into a busy loop
/// against a port nothing is listening on. The ceiling and the attempt count
/// are preferences and are read from the live settings bus.
pub(crate) const RECONNECT_BASE_MS: u64 = 250;

/// Make the server's attachment match the focused session.
///
/// Every attach and detach in the program goes through here. Routing them
/// through one function is what makes a reconnect indistinguishable from a tab
/// click: both clear `attached` and let this reconcile.
pub(crate) fn reconcile(cx: &Rc<Ctx>) {
    let want = cx.peek(|st| st.window.focused);
    let have = cx.attached.get();
    if want == have {
        return;
    }

    if let Some(prev) = have
        && !cx.opts.fixture
    {
        // Detach, never close. The child keeps running and the server keeps
        // filling its ring, so nothing is lost by looking away.
        cx.bridge.msg(&ClientMsg::Detach { session: prev });
    }

    // Resets the screen and starts holding live frames until the backfill
    // lands, so history and live output cannot interleave.
    cx.bridge.focus(want);

    if let Some(next) = want {
        if cx.opts.fixture {
            if let Some(lines) = cx.peek(|r| r.session(next).map(fixture::transcript)) {
                cx.bridge.banner(&lines);
            }
        } else {
            let (cols, rows, scrollback_lines, intent) = cx.peek(|r| {
                (
                    r.window.cols,
                    r.window.rows,
                    r.daemon.settings.terminal.scrollback_lines,
                    r.window.history_intent,
                )
            });
            cx.bridge.msg(&ClientMsg::Attach {
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
            // attach: the local buffer grew and not one extra byte arrived.
            //
            // A pending jump anchors the window on the hit instead of on the
            // head. Without that, activating a search result for a line
            // written an hour ago paints the last minute of output and the
            // hit is simply not in the buffer to scroll to.
            let before_seq = match intent {
                state::HistoryIntent::Jump(seq) => seq.saturating_add(wire::JUMP_TAIL_BYTES),
                _ => BEFORE_SEQ_HEAD,
            };
            cx.bridge.msg(&ClientMsg::Scrollback {
                session: next,
                before_seq,
                max_bytes: backfill_max_bytes(scrollback_lines),
            });
        }
    }

    cx.attached.set(want);
}

/// Ask the daemon for history older than what is painted, and repaint.
///
/// THE SHAPE, and why it is a repaint rather than a prepend: the terminal
/// engine keeps scrollback of what it has been fed and offers no way to splice
/// older bytes in above it. So a page-back re-requests a BIGGER window ending
/// at the same head, resets the screen and replays the whole span. The daemon
/// already holds the bytes, so nothing has to be retained on the hot path to
/// make this exact.
///
/// It is affordable because a granted page-back costs one request per arrival
/// at the top of the buffer, never one per wheel tick, and
/// [`wire::PAGE_CEILING_BYTES`] stops the window growing without bound.
///
/// A REFUSED page-back is the case that has to be counted separately. An
/// arrival at the top is not a click: a screen that is reset and repainted
/// arrives at the top again on its own, so a refusal that speaks every time it
/// is asked never stops speaking. [`plan_page_back`] holds that rule.
pub(crate) fn page_back(cx: &Rc<Ctx>, session: SessionId) {
    let (history, refused, scrollback_lines, focused) = cx.peek(|r| {
        (
            r.window.history,
            r.window.history_refused,
            r.daemon.settings.terminal.scrollback_lines,
            r.window.focused,
        )
    });
    // The operator may have moved on while the event was in flight. Repainting
    // another session's history into this pane is the one outcome worse than
    // not paging.
    if focused != Some(session) || history.session != Some(session) {
        return;
    }
    match plan_page_back(history, refused, scrollback_lines) {
        // Already said, about this same window. Saying it again is the loop.
        PageBackPlan::Silent => {}
        PageBackPlan::Refuse(text) => cx.edit(|st| record_refusal(&mut st.window, text)),
        PageBackPlan::Ask(max_bytes) => {
            cx.edit(|st| st.window.history_intent = state::HistoryIntent::PageBack);
            cx.bridge.msg(&ClientMsg::Scrollback {
                session,
                before_seq: BEFORE_SEQ_HEAD,
                max_bytes,
            });
            // Only now, and never before the request went out. Arming on the
            // gesture instead would leave the pane holding live output forever
            // for a request the plan declined to send.
            cx.bridge.arm_page_back();
        }
    }
}

/// Raised when the daemon has nothing older than what is already painted.
pub(crate) const NO_OLDER_HISTORY: &str =
    "That is the whole history the daemon still holds for this session.";

/// Raised when the pane itself is at the byte ceiling, whatever the daemon has.
pub(crate) const PANE_AT_CEILING: &str =
    "This pane is holding as much history as it will. Search across sessions \
     to find older output.";

/// What a page-back gesture should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageBackPlan {
    /// Refuse, and say nothing: this exact refusal has already been explained.
    Silent,
    /// Refuse, and explain it once.
    Refuse(&'static str),
    /// Ask the daemon for a window of this many bytes.
    Ask(u32),
}

/// Decide a page-back without touching signals, the pane or the socket.
///
/// Split out because the gesture is not a click. It is arrival at the top of
/// the buffer, which the pane reaches again every time its screen is reset and
/// repainted, and the notice strip itself used to cause exactly that. A
/// refusal that re-raises on every arrival is a strip that flickers on and off
/// under the operator with no way to make it stop, which is what shipped.
///
/// `refused` is the window the last refusal was about. While the painted
/// window is unchanged the answer cannot have changed either, so the refusal
/// stays silent; any new scrollback, any other session and any other span
/// makes it a different [`state::HistoryWindow`] and the notice is allowed to
/// speak once more.
#[must_use]
pub(crate) fn plan_page_back(
    history: state::HistoryWindow,
    refused: Option<state::HistoryWindow>,
    scrollback_lines: u32,
) -> PageBackPlan {
    let once = |text| {
        if refused == Some(history) {
            PageBackPlan::Silent
        } else {
            PageBackPlan::Refuse(text)
        }
    };
    if !history.more {
        return once(NO_OLDER_HISTORY);
    }
    match wire::page_back_max_bytes(history.span, scrollback_lines) {
        Some(max_bytes) => PageBackPlan::Ask(max_bytes),
        None => once(PANE_AT_CEILING),
    }
}

/// Put a refusal on screen, and remember that it has been put there.
///
/// The two halves are one step because separating them is the defect: a
/// notice raised without the record is raised again on the next arrival at
/// the top of the buffer, and a record written without the notice refuses in
/// silence the first time, which is a gesture that does nothing.
///
/// Dismissing clears the flash and deliberately leaves the record. The
/// operator has read the answer and said so; re-raising it on the next reflow
/// would make Dismiss a button that does not work.
pub(crate) fn record_refusal(window: &mut state::WindowState, text: &'static str) {
    window.flash = Some(Flash::notice(text));
    window.history_refused = Some(window.history);
}

/// A refusal the operator cannot act on is stated once, not once per reflow.
#[cfg(test)]
mod a_refusal_speaks_once;

/// What the socket and the pane owe each other, proven end to end.
#[cfg(test)]
mod what_a_dropped_daemon_costs;

/// What to present on a socket that just opened.
///
/// Pure, so the three token outcomes can be exercised without a socket, a
/// signal or a daemon. The distinction that matters is between the two
/// failures: a token nobody named is not an error, because a daemon from an
/// older release wants no token at all and gets to say so itself, while a
/// token that was named and could not be read is a refusal this client makes
/// before it sends anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Handshake {
    /// Say hello with this token.
    Present(String),
    /// Say hello with no token, and let the daemon answer. Carries the reason
    /// for the log.
    Anonymous(String),
    /// Send nothing at all, and fail closed with this sentence.
    Refuse(String),
}

/// Decide what a freshly opened socket presents.
#[must_use]
pub(crate) fn plan_handshake(token: cli::Token) -> Handshake {
    match token {
        cli::Token::Present(token) => Handshake::Present(token),
        // The daemon gets to answer. It knows the path it wrote, and an older
        // one wants no token at all.
        cli::Token::Unnamed(e) => Handshake::Anonymous(e.to_string()),
        cli::Token::Named(e) => Handshake::Refuse(e.to_string()),
    }
}

/// What a `Welcome` means for the connection it arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WelcomePlan {
    /// The handshake was accepted. Reset the backoff and subscribe.
    Subscribe,
    /// The handshake was refused, with a reason already recorded. Stop using
    /// this socket.
    HangUp,
}

/// Decide what to do with the connection a `Welcome` was folded into.
///
/// Reads the state the fold produced rather than the message, because the
/// protocol comparison and the sentence that names the corrective action both
/// live in `DaemonState::apply`, and a second place that decides whether a
/// daemon is usable is a second answer.
#[must_use]
pub(crate) fn plan_welcome(conn: &ConnState) -> WelcomePlan {
    match conn {
        ConnState::Live { .. } => WelcomePlan::Subscribe,
        // Includes `Connecting`, which after a `Welcome` means the fold did
        // not accept it. Continuing to talk on a socket the daemon is about to
        // close is a window that looks connected and answers nothing.
        _ => WelcomePlan::HangUp,
    }
}

/// The reason to record for a socket that closed, or `None` to keep the one
/// already on the state.
///
/// A reason already recorded wins. The daemon says why it is refusing and then
/// closes, and the close carries no reason at all, so overwriting replaced
/// "restart vitrum-server, and that ends every session it holds" with "the
/// connection dropped". The operator then had a red banner naming a symptom
/// and no action.
#[must_use]
pub(crate) fn plan_close(conn: &ConnState, detail: Option<String>) -> Option<String> {
    if matches!(conn, ConnState::Failed { .. }) {
        return None;
    }
    // A close frame can carry no reason at all, and a banner that says nothing
    // is a banner an operator cannot act on.
    Some(detail.unwrap_or_else(|| "connection lost".to_string()))
}

/// Open the session a `vitrum://session/N` handoff named, once the daemon has
/// confirmed it exists.
///
/// Retried after every event rather than only on the first snapshot. The link
/// is known before the socket is even open, and a session the daemon has not
/// listed yet is the normal case, not the exception; giving up on the first
/// miss would silently discard the request.
///
/// A link naming a session that no longer exists never fires and never does
/// anything else either, which is the honest outcome: there is nothing to open
/// and inventing a substitute would put the user in a session they did not ask
/// for.
pub(crate) fn claim_link(cx: &Rc<Ctx>) {
    let Some(id) = cx.pending_link.get() else {
        return;
    };
    if cx.peek(|st| st.row(id).is_none()) {
        return;
    }
    cx.pending_link.set(None);
    cx.edit(|st| st.open(id, tick().now_ms));
    reconcile(cx);
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
pub(crate) fn claim_launch(cx: &Rc<Ctx>) {
    let Some(want) = cx.pending_open.borrow().clone() else {
        return;
    };
    let found = cx.peek(|st| {
        st.daemon.sessions.iter().find_map(|r| {
            let i = &r.info;
            (!want.before.contains(&i.id)
                && i.cwd == want.cwd
                && launch::join_command(&i.command, &i.args) == want.line)
                .then_some(i.id)
        })
    });
    let Some(id) = found else {
        return;
    };
    *cx.pending_open.borrow_mut() = None;
    cx.edit(|st| st.open(id, tick().now_ms));
    reconcile(cx);
}

/// Raise a desktop notification for every session that just crossed a line
/// worth interrupting the operator for.
///
/// Edge-triggered, in [`ui::settings::notable_transitions`]: a session that was
/// already blocked stays quiet, so twenty agents do not produce twenty
/// notifications on every snapshot. A session absent from `before` is never
/// notable, which is what keeps a reconnect from emptying a whole day of
/// failures onto the desktop at once.
pub(crate) fn notify_transitions(cx: &Rc<Ctx>, before: &[vitrum_model::SessionView]) {
    cx.peek(|read| {
        for notable in ui::settings::notable_transitions(before, &read.daemon.sessions) {
            let focused = read.window.focused == Some(notable.session);
            if ui::settings::should_notify(
                &read.daemon.settings.notifications,
                notable.kind,
                focused,
            ) {
                ui::settings::notify_now(&notable.notification());
            }
        }
    });
}

/// Show whatever the pane needs the operator told.
///
/// [`socket::PaneStream`] raises a notice rather than logging or panicking
/// when the byte stream disagrees with itself: a gap in the offsets, history
/// evicted before it could be painted, a backfill abandoned under pressure.
/// Each one means what is on screen is not the whole transcript, and an
/// operator reading an agent's output has to know that.
///
/// Only the FIRST is shown when several arrive at once. A flash is one line
/// and the second notice would replace the first before it could be read; the
/// rest go to the log, where a bug report can find them.
pub(crate) fn flush_notices(cx: &Rc<Ctx>) {
    let notices = cx.bridge.notices();
    let Some(first) = notices.first() else {
        return;
    };
    for extra in &notices[1..] {
        tracing::warn!("pane: {extra}");
    }
    cx.flash(Flash::error(first.clone()));
}

/// Everything the session socket has to say, in the vocabulary the rest of the
/// client already reacts to.
///
/// Output is the one case that never becomes a [`ClientEvent`]. It goes to the
/// pane state machine and from there to the terminal engine, so the hot path
/// does not touch UI state, does not mark a signal dirty and does not cause a
/// paint. Everything else is forwarded, because what a `Welcome` means to the
/// client cannot depend on which part of the process observed it.
pub(crate) fn on_socket_event(cx: &Rc<Ctx>, ev: socket::SocketEvent) {
    match ev {
        socket::SocketEvent::Output(frame) => {
            cx.bridge.output(frame);
            flush_notices(cx);
        }
        socket::SocketEvent::Server(msg) => on_client_event(cx, ClientEvent::Server { msg: *msg }),
        socket::SocketEvent::Open => on_client_event(
            cx,
            ClientEvent::Conn {
                state: ConnEvent::Open,
                detail: None,
            },
        ),
        socket::SocketEvent::Closed(detail) => on_client_event(
            cx,
            ClientEvent::Conn {
                state: ConnEvent::Closed,
                detail: Some(detail),
            },
        ),
        socket::SocketEvent::Error(detail) => on_client_event(
            cx,
            ClientEvent::Conn {
                state: ConnEvent::Error,
                detail: Some(detail),
            },
        ),
        socket::SocketEvent::Bad(detail) => on_client_event(cx, ClientEvent::Bad { detail }),
    }
}

/// The one reducer. Everything that can move the client's state arrives here.
pub(crate) fn on_client_event(cx: &Rc<Ctx>, ev: ClientEvent) {
    match ev {
        ClientEvent::Server { msg } => {
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
            // cloning twenty `SessionView`s to diff a list against itself was
            // the whole cost of the feature on the busiest messages the daemon
            // sends.
            let moves_sessions = matches!(
                msg,
                vitrum_proto::ServerMsg::Sessions { .. }
                    | vitrum_proto::ServerMsg::SessionCreated(_)
                    | vitrum_proto::ServerMsg::SessionUpdated(_)
                    | vitrum_proto::ServerMsg::Exited { .. }
            );
            let before = moves_sessions
                .then(|| {
                    cx.peek(|read| {
                        let want = &read.daemon.settings.notifications;
                        (want.finished || want.needs_approval || want.failed)
                            .then(|| read.daemon.sessions.clone())
                    })
                })
                .flatten();
            let now = tick();
            let reaction = cx.edit(|st| st.apply(msg, now.now_ms));
            if let Some(before) = before {
                notify_transitions(cx, &before);
            }
            // The one number the dock, taskbar or launcher entry shows. Every
            // session the daemon holds, not this window's workspace: there is
            // one badge per process and the sessions it is most worth
            // reporting are the ones nobody has on screen.
            badge::publish(cx.peek(|st| st.daemon.attention_total(now.model)));
            if welcome && !cx.opts.fixture {
                let plan = cx.peek(|st| plan_welcome(&st.daemon.conn));
                match plan {
                    WelcomePlan::Subscribe => {
                        // The schedule starts from zero again, here and
                        // nowhere else.
                        //
                        // It used to reset when the SOCKET opened, which is
                        // not the same event: a daemon that refuses the
                        // handshake accepts the socket first and closes it
                        // after saying why. The reset therefore fired on every
                        // attempt, the backoff never grew, and a client facing
                        // a permanent refusal reconnected about four times a
                        // second forever. Measured against a daemon one
                        // release behind: 75 attempts in 20 seconds, each one
                        // a refusal written to the log. Resetting on the
                        // accepted handshake keeps the blip case that this
                        // exists for, since a dropped link that comes back
                        // does reach Welcome.
                        cx.reconnect.set(0);
                        cx.bridge.msg(&ClientMsg::List);
                        // Subscribe, on every connect.
                        //
                        // The daemon holds no watcher, no thread and no watch
                        // descriptors until a client asks, which is what keeps
                        // a headless daemon at zero. But a window IS a client,
                        // and an operator running two agents in one checkout
                        // wants to be told they are overwriting each other
                        // whether or not they found a setting first.
                        //
                        // Measured cost on a 65-directory checkout with twenty
                        // sessions: 64 inotify watches, 612 KiB, no measurable
                        // CPU while nothing writes.
                        cx.bridge
                            .msg(&ClientMsg::WatchCollisions { enabled: true });
                    }
                    WelcomePlan::HangUp => {
                        // A refused handshake. The daemon named the reason and
                        // the corrective action in `ConnState::failed`, and
                        // this client will not send another message on this
                        // socket: holding it open is a window that looks
                        // connected and answers nothing. Hanging up also means
                        // the close that follows is ours, so it cannot
                        // overwrite the reason.
                        cx.bridge.hang_up();
                        cx.attached.set(None);
                        schedule_reconnect(cx);
                        return;
                    }
                }
            }
            if membership && !cx.opts.fixture && cx.peek(|st| st.daemon.collisions.watching) {
                cx.bridge.msg(&ClientMsg::Collisions);
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
                } => {
                    // `more` never leaves this process: whether the daemon
                    // holds older bytes is a fact about the daemon, and the
                    // only thing that reads it is `page_back` above. It is
                    // already recorded on `window.history` by `apply`.
                    let _ = more;
                    cx.bridge
                        .backfill(session, from_seq, resume_seq, bytes, jump_seq, keep_view);
                    flush_notices(cx);
                }
                Reaction::Refill { .. } => {
                    // Full detach and re-attach. Splicing across a reported gap
                    // is exactly what the byte-offset seq exists to prevent.
                    cx.attached.set(None);
                }
            }
            reconcile(cx);
        }

        ClientEvent::Conn { state, detail } => match state {
            ConnEvent::Open => {
                // The token is resolved here, on the open socket, and not at
                // startup. The daemon writes a new one every time it starts,
                // so a reconnect after a daemon restart needs the token that
                // daemon wrote, not the one that was on disk when this window
                // opened.
                let token = match plan_handshake(cli::resolve_token(cx.opts)) {
                    Handshake::Present(token) => token,
                    Handshake::Anonymous(why) => {
                        tracing::info!(
                            "no token to present ({why}); the daemon will say what it wants"
                        );
                        String::new()
                    }
                    Handshake::Refuse(detail) => {
                        // Nothing is sent. The daemon refuses every message
                        // before a hello, so holding the socket open would be
                        // a window that looks connected and answers nothing.
                        cx.bridge.hang_up();
                        cx.edit(|st| st.daemon.conn = ConnState::failed(detail));
                        schedule_reconnect(cx);
                        return;
                    }
                };
                cx.edit(|st| st.daemon.conn = ConnState::Connecting);
                cx.bridge.msg(&ClientMsg::Hello {
                    protocol: PROTOCOL_VERSION,
                    token,
                });
                cx.bridge.msg(&ClientMsg::List);
                // A fresh socket knows nothing about our previous attachment.
                cx.attached.set(None);
                reconcile(cx);
            }
            ConnEvent::Closed | ConnEvent::Error => {
                let reason = cx.peek(|st| plan_close(&st.daemon.conn, detail));
                if let Some(reason) = reason {
                    cx.edit(|st| st.daemon.conn = ConnState::failed(reason));
                }
                cx.attached.set(None);
                schedule_reconnect(cx);
            }
        },

        ClientEvent::Resize { cols, rows } => {
            // Guarded: writing an unchanged value would still repaint the
            // whole shell on every layout pass.
            let changed = cx.peek(|r| r.window.cols != cols || r.window.rows != rows);
            if changed {
                cx.edit(|w| {
                    w.window.cols = cols;
                    w.window.rows = rows;
                });
            }
            // Telling the daemon is this side's job. Only this side knows
            // which session the pane is attached to, and a resize addressed to
            // a session the pane stopped showing is a real way to reflow
            // somebody else's grid.
            let focused = cx.peek(|st| st.window.focused);
            if let Some(session) = focused
                && !cx.opts.fixture
            {
                cx.bridge.msg(&ClientMsg::Resize {
                    session,
                    cols,
                    rows,
                });
            }
        }

        // Bytes the pane captured: a keystroke, a paste, or a raw 8-bit reply.
        // Addressed here for the same reason the resize above is.
        ClientEvent::Input { data } => {
            let focused = cx.peek(|st| st.window.focused);
            if let Some(session) = focused
                && !cx.opts.fixture
            {
                cx.bridge.msg(&ClientMsg::Input { session, data });
            }
        }

        // Unguarded on the pane's side by design: whether there is more
        // history, and whether a request is already in flight, are both known
        // here and nowhere else.
        ClientEvent::PageBack => {
            let focused = cx.peek(|st| st.window.focused);
            if let Some(session) = focused {
                page_back(cx, session);
            }
        }

        // Already resolved against the live table by whichever surface took
        // the press, so there is nothing to match here and nothing that can
        // fail.
        ClientEvent::Key { action } => on_key(cx, action),

        // The operator's own binding, looked up again against this window's
        // profile at the moment it runs.
        ClientEvent::CustomKey { chord } => dispatch_custom(cx, &chord),

        ClientEvent::Copied { ok, text } => {
            cx.flash(if ok {
                Flash::notice(format!("Copied {text}"))
            } else {
                Flash::error(format!("Could not copy {text} to the clipboard"))
            });
        }

        // A control-plane message a panel built for itself: the launcher's
        // Start, the confirmation's Close, the search sweep. It goes through
        // the reducer rather than through the socket because this is where a
        // fixture window is kept off the wire, and a panel holding the socket
        // would make a fixture dial a daemon.
        ClientEvent::Msg { msg } => {
            if !cx.opts.fixture {
                cx.bridge.msg(&msg);
            }
        }

        // A panel changed the strip and cannot know whether that moved the
        // attachment. This is where the answer lives.
        ClientEvent::Reconcile => reconcile(cx),

        ClientEvent::Clipboard { text } => cx.bridge.clipboard(text),

        ClientEvent::Start { project, launch } => {
            crate::actions::start_session(cx, project, launch);
            reconcile(cx);
        }

        ClientEvent::Duplicate { session } => {
            crate::actions::duplicate_session(cx, session);
            reconcile(cx);
        }

        ClientEvent::Terminate { targets } => {
            crate::actions::request_terminate(cx, &targets);
            reconcile(cx);
        }

        // Same path as startup, so a machine with no daemon gets one started
        // rather than a button that can never work.
        ClientEvent::Retry => crate::actions::retry(cx),

        // No reconcile: the launch is not confirmed yet. `start_session`
        // records it as pending and the arrival of the new session is what
        // moves the attachment.
        ClientEvent::LaunchNow { project } => crate::actions::launch_now(cx, project),

        ClientEvent::Redial { url } => {
            // The attachment goes with the socket. A session id minted by the
            // old daemon means nothing to the new one, so holding it would
            // address the next keystroke at a session that does not exist.
            cx.bridge.connect(url);
            cx.attached.set(None);
        }

        ClientEvent::Bad { detail } => {
            tracing::warn!("client: {detail}");
            cx.flash(Flash::error(detail));
        }
    }
}

/// How long to wait before reconnect attempt `n`, in milliseconds.
///
/// Doubling from a quarter second to the ceiling the operator set. The early
/// attempts are what recover a blip without the operator noticing; the ceiling
/// is what stops a machine that has been asleep for a week from having spent
/// the night dialling. Returns `None` once the schedule is exhausted, which is
/// a real answer: the window then says the daemon is gone and offers Retry,
/// rather than reconnecting silently forever.
///
/// Both bounds come from [`crate::state::live::shell_settings`], which has
/// already clamped them, so nothing here re-checks a range. Read per attempt
/// rather than captured once, because an operator who lengthens the ceiling
/// during an outage means it for the outage they are watching.
#[must_use]
pub(crate) fn reconnect_delay_ms(attempt: u32) -> Option<u64> {
    let live = crate::state::live::shell_settings();
    (attempt < live.reconnect_attempts).then(|| {
        RECONNECT_BASE_MS
            .saturating_mul(1 << attempt.min(7))
            .min(u64::from(live.reconnect_max_ms))
    })
}

/// Try the daemon again, later.
///
/// This is the one automatic reconnect in the program, and it is here because
/// a window may be pointed at a daemon across a network: a laptop that closes
/// its lid must not need a click to come back. It is a SCHEDULE, not a loop.
/// Each attempt is one `sleep` that fires once; a connected window has none
/// outstanding, so the idle cost this program is built around is unchanged at
/// rest.
///
/// The schedule ends. When it does the window keeps saying the connection
/// failed and the Retry button is still the way back.
pub(crate) fn schedule_reconnect(cx: &Rc<Ctx>) {
    // A window that never talks to a daemon has nothing to reconnect to.
    if cx.opts.fixture {
        return;
    }
    let attempt = cx.reconnect.get();
    let Some(delay) = reconnect_delay_ms(attempt) else {
        return;
    };
    cx.reconnect.set(attempt + 1);
    let cx = Rc::clone(cx);
    glib::MainContext::default().spawn_local(async move {
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        // The operator may have reconnected by hand, or pointed this window at
        // another daemon, while this was asleep. Re-read rather than acting on
        // what was true when the timer was set.
        //
        // The URL is RESOLVED from the setting, not taken from `opts.server`.
        // THE BUG: this dialled the command line while the manual Retry button
        // dialled the setting, so a window pointed at another daemon through
        // Settings silently returned to the wrong one on the first blip.
        let url = cx.peek(|st| {
            matches!(st.daemon.conn, ConnState::Failed { .. }).then(|| {
                st.daemon
                    .settings
                    .resolved_daemon_url(cx.opts.server)
                    .to_string()
            })
        });
        if let Some(url) = url {
            cx.bridge.connect(url);
        }
    });
}
