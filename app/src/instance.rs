//! One process owns the app. Everything else hands its arguments over and
//! exits.

use super::*;

/// A queue one producer posts to and one or more tasks drain.
///
/// Hand-rolled, because there is no async channel in reach that both the GTK
/// main loop and a foreign backend thread may touch. The contract is narrow enough to be worth the fifty lines.
/// Several consumers may wait on the same queue, each item is handed to
/// exactly one of them, and a consumer going away passes its place to another
/// without dropping a queued item, because the queue outlives every consumer.
///
/// Two callers use it, for the two things this process has to hear from a
/// thread it does not own: single-instance handoffs, and the result of
/// starting the daemon.
pub(crate) struct Mailbox<T> {
    pub(crate) inner: Mutex<MailboxInner<T>>,
}

pub(crate) struct MailboxInner<T> {
    pub(crate) queue: VecDeque<T>,
    pub(crate) waiting: Vec<Waker>,
}

/// Handoffs that ask this process to show something: a later launch's deep
/// link, and a click on a desktop notification. Written by threads this process
/// does not own, read by whichever window's task gets there first.
pub(crate) static ACTIVATIONS: Mailbox<Activation> = Mailbox::new();

/// Open the session a notification was about.
///
/// Installed on the process-wide notifier by [`ui::settings`], and called on the
/// backend's own listener thread: on Linux the thread parked on `ActionInvoked`.
/// It may therefore touch no widget and no state, which is why it does one
/// thing. Posting is the same cross-thread handoff `guard.listen` already uses
/// for a second launch, and `Mailbox::post` wakes its wakers outside the lock,
/// which is what makes a foreign thread safe here.
///
/// A click then lands on exactly the code a `vitrum://session/<id>` link from a
/// browser lands on: one window takes it, opens on that session, and
/// `claim_link` holds the request until the daemon confirms the session exists.
///
/// Before this existed the Notifications panel told the operator that clicking
/// a notification focuses the session, and every Linux notification rendered a
/// "Show" button, while `set_activation_handler` was never called from `app/`
/// at all. The button was a control that rendered and did nothing.
pub(crate) fn activate_session(session: SessionId) {
    ACTIVATIONS.post(Activation::Open(DeepLink::Session(session)));
}

impl<T> Mailbox<T> {
    pub(crate) const fn new() -> Self {
        Self {
            inner: Mutex::new(MailboxInner {
                queue: VecDeque::new(),
                waiting: Vec::new(),
            }),
        }
    }

    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, MailboxInner<T>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queue an item and wake everyone waiting. Called from a worker thread,
    /// never on the UI thread.
    pub(crate) fn post(&self, item: T) {
        let wake = {
            let mut inner = self.lock();
            inner.queue.push_back(item);
            std::mem::take(&mut inner.waiting)
        };
        // Woken outside the lock: a waker that resumes its task inline would
        // otherwise re-enter `poll` and deadlock on a lock this thread holds.
        for waker in wake {
            waker.wake();
        }
    }

    /// Wait for the next item nobody else has taken.
    pub(crate) fn next(&self) -> NextItem<'_, T> {
        NextItem { mailbox: self }
    }
}

/// The future [`Mailbox::next`] returns.
pub(crate) struct NextItem<'a, T> {
    pub(crate) mailbox: &'a Mailbox<T>,
}

impl<T> Future for NextItem<'_, T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut inner = self.mailbox.lock();
        if let Some(item) = inner.queue.pop_front() {
            return Poll::Ready(item);
        }
        // Checked rather than pushed blindly: a task polled twice without an
        // intervening post would otherwise leave a stale waker per poll, and
        // the vector would grow for the life of the process.
        if !inner.waiting.iter().any(|w| w.will_wake(cx.waker())) {
            inner.waiting.push(cx.waker().clone());
        }
        Poll::Pending
    }
}

/// Run a blocking job on its own thread and await the answer.
///
/// Used for exactly one thing: probing the daemon port and, if nothing is
/// there, starting it. That takes microseconds on a warm machine and up to
/// three seconds on a cold one, and three seconds of a frozen window while the
/// event loop waits on a `connect` is not something to ship. The thread exits
/// as soon as the job returns, so nothing is left running.
pub(crate) async fn off_thread<T, F>(job: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let slot = Arc::new(Mailbox::new());
    let worker = Arc::clone(&slot);
    std::thread::spawn(move || worker.post(job()));
    slot.next().await
}

/// Which side of the single-instance race this process is on.
pub(crate) enum Instance {
    /// This process owns the slot.
    ///
    /// Held and never read, which is the point: the claim lasts exactly as
    /// long as this value does, and dropping it hands the slot to the next
    /// launch. `dead_code` cannot see a lifetime as a use.
    First(#[allow(dead_code)] InstanceGuard),
    /// Another process owns it and has been handed what this launch wanted.
    /// There is nothing left to do but exit.
    Second,
    /// The mechanism could not run at all. Carry on as an ordinary process:
    /// an unwritable runtime directory is a reason to lose window sharing, not
    /// a reason to lose the application.
    Alone,
}

/// Claim the single-instance slot, or hand `activation` to whoever holds it.
pub(crate) fn claim_instance(activation: &Activation) -> Instance {
    let paths = match AppPaths::for_current_platform() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("no application directories, running standalone: {e}");
            return Instance::Alone;
        }
    };
    if let Err(e) = paths.create_all() {
        tracing::warn!("cannot create application directories, running standalone: {e}");
        return Instance::Alone;
    }
    match single_instance::acquire(
        &paths.instance_lock_file(),
        &paths.instance_socket_path(),
        activation,
    ) {
        Ok(Acquisition::Primary(guard)) => {
            primary_role(guard, |g| g.listen(Arc::new(|a| ACTIVATIONS.post(a))))
        }
        Ok(Acquisition::HandedOff) => Instance::Second,
        Err(e) => {
            tracing::warn!("single instance unavailable, running standalone: {e}");
            Instance::Alone
        }
    }
}

/// Turn a won claim into a role, failing open if the listener will not start.
///
/// Holding the slot without serving it is the worst of both outcomes: the lock
/// keeps turning every later launch into a handoff, and there is nothing on
/// the other end to open a window, so the operator types `vitrum` a second
/// time and gets nothing at all. Dropping the guard costs window sharing and
/// keeps the window, which is the same trade [`Instance::Alone`] already
/// makes for an unwritable runtime directory.
///
/// `listen` is a parameter so the rule can be exercised without a listener
/// that refuses to start on demand.
pub(crate) fn primary_role(
    guard: InstanceGuard,
    listen: impl FnOnce(&InstanceGuard) -> Result<(), single_instance::SingleInstanceError>,
) -> Instance {
    match listen(&guard) {
        Ok(()) => Instance::First(guard),
        Err(e) => {
            tracing::warn!("activation listener did not start, running standalone: {e}");
            drop(guard);
            Instance::Alone
        }
    }
}
