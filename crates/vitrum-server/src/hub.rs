//! Daemon-wide state every connection shares, and the registry event bus.
//!
//! The split here is the whole point. Registry events (a session appeared,
//! changed, or exited) are low volume, small, and describe state that belongs to
//! the daemon, so every connected client must see them. Output frames are a
//! firehose that belongs to whoever is looking at that pane, so they stay
//! per-attachment. Conflating the two either leaves a second window's sidebar
//! silently stale or sprays twenty agents' output at every window.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use vitrum_core::SessionManager;
use vitrum_proto::{ServerMsg, SessionId, SessionStatus};
use tokio::sync::broadcast;

use crate::overlap::{OverlapService, Publish, live_sessions};
use crate::projects::ProjectRegistry;

/// Registry events buffered per connection.
///
/// Generous because these are deltas: one small frame per real state change, not
/// a snapshot. A client would have to miss hundreds of distinct changes to lap
/// this, and if it does it is told to resynchronise from a full snapshot rather
/// than left with a hole.
const EVENT_QUEUE: usize = 256;

/// One registry event, already serialized.
///
/// The bus used to carry `ServerMsg`, which made a broadcast to N windows cost N
/// deep clones of the message and N serde traversals producing identical bytes.
/// A session list of twenty entries is twenty `SessionInfo` and their strings
/// cloned per window. Serializing once at the source turns that into one
/// traversal plus one atomic increment per window.
pub type Event = Arc<str>;

/// Sessions, projects, and the event bus, shared by every connection.
pub struct Hub {
    pub manager: Arc<SessionManager>,
    pub projects: ProjectRegistry,
    /// Same-file collision detection, daemon-wide and off until a client
    /// asks for it.
    ///
    /// Here rather than on a connection because a collision is between two
    /// SESSIONS, which belong to the daemon. Two windows watching the same
    /// sessions must be told the same thing, and a per-connection watcher
    /// would spend one inotify watch set per window for one answer.
    ///
    /// Holds nothing at all until subscribed: no thread, no watcher, no watch
    /// descriptor. See `overlap.rs`.
    pub overlap: Arc<OverlapService>,
    events: broadcast::Sender<Event>,
    /// Sessions that already have a status watcher, so one is spawned per
    /// session rather than per session per connection.
    watched: Mutex<HashSet<SessionId>>,
}

impl Hub {
    pub fn new(manager: Arc<SessionManager>) -> Arc<Self> {
        let (events, _) = broadcast::channel(EVENT_QUEUE);
        Arc::new(Self {
            manager,
            projects: ProjectRegistry::default(),
            overlap: OverlapService::new(),
            events,
            watched: Mutex::new(HashSet::new()),
        })
    }

    /// A callback that puts a message on this hub's bus.
    ///
    /// The overlap watcher publishes from its own thread, which has no
    /// runtime and must not hold a `Hub`: an `Arc` there would keep the
    /// daemon's whole state alive for as long as the watcher, and the cycle
    /// would leak both. A `Weak` upgrade per message costs an atomic and
    /// breaks it.
    pub fn publisher(self: &Arc<Self>) -> Publish {
        let hub = Arc::downgrade(self);
        Arc::new(move |msg| {
            if let Some(hub) = hub.upgrade() {
                hub.publish(msg);
            }
        })
    }

    /// Reconcile the collision watcher against the sessions that are live now.
    ///
    /// A no-op while nothing is watching, so every caller can invoke it
    /// unconditionally. Off the runtime, because establishing a watch walks
    /// the session's tree and spends one `inotify_add_watch` per directory,
    /// and a four-thousand-directory checkout is milliseconds of syscalls
    /// that would otherwise stall every PTY coalescer sharing this runtime.
    ///
    /// Nothing here is timed. It is called from the two places that already
    /// publish a registry delta: a session appearing, and one going away.
    pub fn sync_overlap(self: &Arc<Self>) {
        if !self.overlap.is_watching() {
            return;
        }
        let hub = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let live = live_sessions(&hub.manager);
            hub.overlap.sync(&live);
        });
    }

    /// Receive every registry event from now on.
    ///
    /// Only a connection past the handshake should hold one of these: a client
    /// that has not agreed on a protocol version cannot be sent typed state.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Announce a registry change to every connected client.
    pub fn publish(&self, msg: ServerMsg) {
        // Once, here, rather than once per window in every event pump.
        let text = match serde_json::to_string(&msg) {
            Ok(text) => text,
            Err(e) => {
                tracing::error!(error = %e, "could not serialize a registry event");
                return;
            }
        };
        // Err means nobody is connected, which is not a failure: the daemon runs
        // without a GUI by design.
        let _ = self.events.send(Event::from(text));
    }

    /// Every project that still has a session in it.
    ///
    /// The ONLY way to read the project list. It is built from the session
    /// manager on every call, so a folder whose sessions have all gone cannot
    /// be reported by any path, whether or not that path remembered to
    /// reclaim anything. See [`crate::projects::ProjectRegistry`].
    pub fn projects_now(&self) -> Vec<vitrum_proto::ProjectInfo> {
        let live: std::collections::HashSet<vitrum_proto::ProjectId> = self
            .manager
            .list()
            .into_iter()
            .map(|s| s.project_id)
            .collect();
        self.projects.live(&live)
    }

    /// Send the current project list to every window.
    pub fn publish_projects(&self) {
        self.publish(ServerMsg::Projects {
            projects: self.projects_now(),
        });
    }

    /// Record a project and announce it if the project set actually changed.
    pub fn ensure_project(&self, id: vitrum_proto::ProjectId, root: &str) {
        if self.projects.ensure(id, root) {
            self.publish_projects();
        }
    }

    /// Start the single watcher for `session`, if it has none.
    ///
    /// Idempotent, so every path that learns about a session can call it: the
    /// connection that created it, and any connection that lists a manager which
    /// already had sessions in it.
    pub fn watch(self: &Arc<Self>, session: SessionId) {
        // An exited session never changes again, and its scrollback is retained,
        // so every `List` reaches it. Watching one parks a task holding two watch
        // receivers and an `Arc<Hub>` for the life of the daemon, and the exit it
        // would report was already published by the watcher that saw it happen.
        if self
            .manager
            .info(session)
            .is_some_and(|info| !info.status.is_live())
        {
            return;
        }
        {
            let mut watched = self.watched.lock().unwrap_or_else(|e| e.into_inner());
            if !watched.insert(session) {
                return;
            }
        }
        let started = self
            .manager
            .subscribe_status(session)
            .zip(self.manager.subscribe_observations(session));
        let Some((status, observations)) = started else {
            self.watched
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&session);
            return;
        };
        let hub = Arc::clone(self);
        tokio::spawn(async move { hub.watch_loop(session, status, observations).await });
    }

    /// How many sessions have a status watcher right now.
    ///
    /// Exists for one test, and that test earns it: a watcher on a session that
    /// can never change again is a task parked for the life of the daemon, and
    /// nothing about that is visible to the compiler or to the protocol.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn watcher_count(&self) -> usize {
        self.watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Forward one session's changes onto the bus until it ends.
    ///
    /// Two channels, because two very different things change: the child's
    /// lifecycle, and what the daemon has since observed about it. The second
    /// one is why a sidebar learns that an agent went quiet and blocked on its
    /// prompt, or declared a hint, or that another window's attach changed the
    /// geometry, without anyone polling for it.
    ///
    /// Only the lifecycle channel ends this loop, and it is polled first. Both
    /// channels die together when the session is dropped, so an unbiased select
    /// that treated either closure as the end would race: half the time it
    /// would notice the observation channel shutting and return before
    /// delivering the exit, and the client would never learn the session was
    /// gone.
    async fn watch_loop(
        self: Arc<Self>,
        session: SessionId,
        status: tokio::sync::watch::Receiver<SessionStatus>,
        observations: tokio::sync::watch::Receiver<u64>,
    ) {
        self.watch_until_exit(session, status, observations).await;
        self.watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&session);
    }

    /// The watch itself, so the registration is released on every exit path.
    pub(crate) async fn watch_until_exit(
        &self,
        session: SessionId,
        mut status: tokio::sync::watch::Receiver<SessionStatus>,
        mut observations: tokio::sync::watch::Receiver<u64>,
    ) {
        // Read the current status before waiting on a change. `watch` checked
        // that the session was live, but the child can die between that check
        // and this subscription, and a value that changed before you subscribed
        // never fires `changed()`. Waiting first would park this task forever
        // and no client would ever be told the session ended.
        if let SessionStatus::Exited { code } = status.borrow_and_update().clone() {
            if let Some(info) = self.manager.info(session) {
                self.publish(ServerMsg::SessionUpdated(info));
            }
            self.publish(ServerMsg::Exited { session, code });
            return;
        }
        let mut observing = true;
        loop {
            let exited = tokio::select! {
                biased;
                changed = status.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let current = status.borrow_and_update().clone();
                    // A closed session is already out of the registry, so there
                    // is no projection left to send, but its exit still matters
                    // to everyone.
                    if let Some(info) = self.manager.info(session) {
                        self.publish(ServerMsg::SessionUpdated(info));
                    }
                    match current {
                        SessionStatus::Exited { code } => Some(code),
                        _ => None,
                    }
                }
                changed = observations.changed(), if observing => {
                    if changed.is_err() {
                        observing = false;
                        continue;
                    }
                    observations.borrow_and_update();
                    if let Some(info) = self.manager.info(session) {
                        self.publish(ServerMsg::SessionUpdated(info));
                    }
                    None
                }
            };
            if let Some(code) = exited {
                self.publish(ServerMsg::Exited { session, code });
                break;
            }
        }
    }
}
