//! Captions in this sheet against what the product actually does.
//!
//! Its own module because it is a coherence suite, not a settings-logic one:
//! every test here reads the shipped source of the files that implement the
//! behaviour and asserts that a sentence shown to an operator is true of the
//! code beside it. Source scanning rather than a runtime assertion because
//! neither behaviour has a hook a unit test can reach: one is a scroll
//! gesture on the pane, the other is a D-Bus click on a live desktop.

use super::{Notification, SessionId};

/// Source with `//` and `///` lines removed.
///
/// A caption check must not read the comment that explains the caption.
/// The first version of the test below failed on its own doc comment,
/// which quoted the false sentence it was written to forbid.
fn code_only(src: &str) -> String {
    src.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The shipped half of this file: code only, every test module cut off.
///
/// A guard that counts call sites has to stop where the assertions begin.
/// Counting `backend.notify(` over the whole file finds the needle in this
/// module too and reports two delivery paths where there is one.
///
/// The cut is asserted, not assumed. `split_once("#[cfg(test)]")` truncates
/// at the FIRST such attribute, so one early `#[cfg(test)] mod testkit;`
/// collapses the scan to a few lines. Every guard below then fails on an
/// `expect` and blames a rename that never happened, which is a wrong
/// diagnosis rather than a false pass, and still an hour of somebody's day.
fn shipped() -> String {
    let src = code_only(include_str!("../settings.rs"));
    let out = match src.split_once("#[cfg(test)]") {
        Some((before, _)) => before.to_string(),
        None => src,
    };
    assert!(
        out.contains("fn NotificationsPanel") && out.contains("static NOTIFIER"),
        "the shipped scan collapsed to {} bytes: an early #[cfg(test)] item now \
         precedes the code these guards read, so cut on the test module's own \
         opener instead",
        out.len()
    );
    out
}

/// The Scrollback caption may claim only what `wire::backfill_max_bytes`
/// delivers.
///
/// **This caption has now been wrong twice.** Version one told the operator
/// the daemon "serves it on demand ... before a request", describing a fetch
/// no code makes. Version two deleted those two phrases and went on saying
/// that raising the number was the only way to see further back, which was
/// false for a different reason: the backfill was a hard-coded 64 KiB, so
/// "100,000 lines" grew the local buffer a hundredfold and retrieved not one
/// extra byte of pre-attach history. The guard shipped with version two
/// asserted only that the two retired phrases were absent, so version two's
/// own different overstatement walked straight past it.
///
/// Hence a relationship, not a vocabulary check. The caption is allowed to
/// say that raising the setting is how you see further back only while every
/// offered step really does ask the daemon for a strictly larger budget, and
/// the ceiling it quotes has to be the ceiling the code enforces.
///
/// The third version of this caption is the first one written against a
/// client that can page. "Nothing fetches more later" was true and is now
/// false, so the guard is inverted: it requires the second send site and
/// the scroll handler to exist, and requires the caption to describe them
/// with the ceiling that actually stops them.
#[test]
fn the_scrollback_caption_matches_what_the_client_actually_fetches() {
    let settings = code_only(include_str!("../settings.rs"));
    let main_src = crate::testkit::shell();
    let main_src = main_src.as_str();


    let at = settings
        .find("label: \"Scrollback\"")
        .expect("the Scrollback row was renamed");
    let caption = &settings[at..(at + 900).min(settings.len())];

    for stale in [
        "on demand",
        "before a request",
        "Nothing fetches more later",
    ] {
        assert!(
            !caption.contains(stale),
            "the Scrollback caption still says {stale:?}, which no longer \
             describes what the client does"
        );
    }

    let mut budgets: Vec<u32> = super::SCROLLBACK_STEPS
        .iter()
        .map(|(lines, _)| crate::wire::backfill_max_bytes(*lines))
        .collect();
    budgets.sort_unstable();
    budgets.dedup();
    assert_eq!(
        budgets.len(),
        super::SCROLLBACK_STEPS.len(),
        "the caption says this number sizes the first request, but the {} \
         offered steps produce only {} distinct budgets, so at least two of \
         them fetch the same history",
        super::SCROLLBACK_STEPS.len(),
        budgets.len()
    );

    assert!(
        caption.contains("stopping at 2 MiB"),
        "the caption must name the per-request ceiling; a budget that \
         silently stops growing is exactly how this caption went wrong the \
         second time"
    );
    assert_eq!(
        crate::wire::BACKFILL_CEILING_BYTES,
        2 * 1024 * 1024,
        "the ceiling moved and the caption still says 2 MiB"
    );

    // Paging must be REAL, not described. Both halves are load-bearing:
    // the caption tells the operator to scroll to the top, so a missing
    // handler makes it an instruction that does nothing, and a handler
    // with no second request site makes it a scroll that fetches nothing.
    assert!(
        caption.contains("Scroll to the top"),
        "the caption no longer tells the operator how to see older history"
    );
    let sites = main_src.matches("ClientMsg::Scrollback").count();
    assert!(
        sites >= 2,
        "the client asks for scrollback from {sites} place(s); paging back \
         needs its own request, so the caption's instruction to scroll to \
         the top fetches nothing"
    );
    assert!(
        main_src.contains("ClientEvent::PageBack"),
        "nothing in the shell handles the pane's arrival at the top, so the \
         caption's instruction to scroll up there fetches nothing"
    );
    assert!(
        main_src.contains("page_back(bridge, st, session)"),
        "the shell hears the pane reach the top and does not ask for a page"
    );
    assert!(
        caption.contains("8 MiB"),
        "the caption must name the ceiling paging stops at; an operator \
         who scrolls into a notice they were never warned about is the \
         same defect in a new place"
    );
    assert_eq!(
        crate::wire::PAGE_CEILING_BYTES,
        8 * 1024 * 1024,
        "the paging ceiling moved and the caption still says 8 MiB"
    );
}

/// Locks out: a notification that advertises a click this build cannot
/// service.
///
/// `Notification::dbus_args` has always offered two clickable keys,
/// `default` for the body and `Show` for a button, and nothing in `app/`
/// ever called `set_activation_handler`. On Linux that is worse than an
/// unrouted click: the backend subscribes to `ActionInvoked` only when a
/// handler is installed, so every notification this product has ever raised
/// rendered a `Show` button with nothing behind it. The three facts that
/// keep it serviced are asserted here because none of them can be observed
/// without a live notification daemon: the payload really does advertise
/// actions, the one process-wide notifier installs the route in its own
/// initialiser so no delivery path can skip it, and the route lands in the
/// activation queue that already carries a browser's deep link.
#[test]
fn every_advertised_notification_action_has_somewhere_to_go() {
    let settings = shipped();
    let main_src = crate::testkit::shell();
    let main_src = main_src.as_str();

    let args = Notification::needs_approval(SessionId(4), "agent", "run rm -rf?").dbus_args(0);
    assert_eq!(
        args.actions,
        vec!["default".to_string(), "Show".to_string()],
        "the payload no longer advertises the two keys this guard is about"
    );

    // Scoped to the initialiser's own body, not to "somewhere between the
    // static and the next item". An install sitting in a helper nobody calls
    // satisfies the looser range and is exactly this ticket's defect class:
    // present, correct, never reached. Hunted for and confirmed to escape
    // before this was tightened.
    let at = settings
        .find("static NOTIFIER")
        .expect("the process-wide notifier was renamed");
    let open = settings[at..]
        .find("LazyLock::new(|| {")
        .expect("the notifier is no longer built by a closure that can install the route");
    let body = &settings[at + open..];
    let end = body
        .find("\n});")
        .expect("the notifier's initialiser is no longer a closure");
    assert!(
        body[..end].contains("set_activation_handler(Arc::new(crate::activate_session))"),
        "the notifier's initialiser no longer installs the click route, so the \
         first notification is again a button with nothing behind it"
    );

    let deliveries = settings.matches("backend.notify(").count();
    assert_eq!(
        deliveries, 1,
        "notifications are delivered from {deliveries} places; every one of \
         them has to force NOTIFIER, or a notification can be raised before \
         the click route exists"
    );
    let at = settings
        .find("pub fn notify_now")
        .expect("the delivery path was renamed");
    let deliver = &settings[at..(at + 400).min(settings.len())];
    assert!(
        deliver.contains("NOTIFIER.as_ref()"),
        "notify_now no longer goes through the notifier that installs the route"
    );

    assert!(
        main_src.contains("fn activate_session(session: SessionId)"),
        "main.rs no longer defines the click route the notifier installs"
    );
    assert!(
        main_src.contains("ACTIVATIONS.post(Activation::Open(DeepLink::Session(session)))"),
        "a notification click no longer reaches the activation queue, so it \
         resolves to nothing again"
    );
}

/// Locks out: the Notifications note promising a click behaviour the build
/// does not have.
///
/// It used to say a click "focuses the session it is about", which was false
/// twice over: nothing installed a handler, so a click did nothing at all,
/// and the handoff a click now takes opens a window rather than moving the
/// one you are in.
///
/// Two-sided on purpose. It fails if the note claims focus again, and it
/// fails if the activation consumer stops opening a window, because then the
/// note would be understating what a click does.
#[test]
fn the_notification_note_matches_what_a_click_actually_does() {
    let settings = shipped();
    let main_src = crate::testkit::shell();
    let main_src = main_src.as_str();

    let at = settings
        .find("fn NotificationsPanel")
        .expect("the Notifications panel was renamed");
    let rest = &settings[at..];
    let end = rest.find("#[component]").unwrap_or(rest.len());
    let panel = &rest[..end];

    // The note's exact content, whitespace collapsed so the literal may be
    // reflowed and may not be reworded. A phrase blacklist is what let this
    // file's scrollback caption ship wrong a SECOND time: the next wrong
    // version used different words. Hunted for and confirmed: a note that
    // keeps the true sentence and adds "brings the session to the front of
    // the window you are already in" passed the blacklist.
    let at = panel
        .find("class: \"rg-sheet__note\"")
        .expect("the Notifications note was restructured");
    let note = &panel[at..];
    let end = note
        .find("\n        }")
        .expect("the note div is no longer closed at its own indentation");
    let squashed: String = note[..end]
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\\')
        .collect();
    let want: String = concat!(
        "class: \"rg-sheet__note\",",
        "\"Clicking a notification opens the session it is about in a new window, ",
        "through the same vitrum://session/<id> handoff a link from a browser takes. ",
        "The window you are in is left where it is.\""
    )
    .chars()
    .filter(|c| !c.is_whitespace())
    .collect();
    assert_eq!(
        squashed, want,
        "the Notifications note says something other than what a click does: \
         a click posts Activation::Open into the queue and the consumer calls \
         open_window, so it opens a session in a NEW window and leaves the \
         current one alone. Change the code first, then this string, then this \
         expectation, in that order"
    );

    let at = main_src
        .find("ACTIVATIONS.next()")
        .expect("nothing consumes the activation queue any more");
    let consumer = &main_src[at..(at + 200).min(main_src.len())];
    assert!(
        consumer.contains("open_window("),
        "an activation no longer opens a window; the Notifications note has \
         to say what a click does now"
    );
}

/// Locks out: either guard above rotting into a no-op.
///
/// This file has already shipped a caption guard that passed while its
/// caption was false, because it asserted only that two retired phrases were
/// absent and the next wrong version used different words. A source scan is
/// only worth its line count if it fails on the regression it names.
///
/// The last two cases are ESCAPES: mutations that the first version of these
/// guards passed. They are here because proving a guard catches the
/// regression you thought of is confirmation, and only hunting for one it
/// misses is a test.
#[test]
fn each_guard_fails_on_the_regression_it_names() {
    let settings = shipped();

    /// The install guard's own logic, so a mutant can be run through it.
    fn installs_in_the_initialiser(src: &str) -> bool {
        let Some(at) = src.find("static NOTIFIER") else {
            return false;
        };
        let Some(open) = src[at..].find("LazyLock::new(|| {") else {
            return false;
        };
        let body = &src[at + open..];
        let Some(end) = body.find("\n});") else {
            return false;
        };
        body[..end].contains("set_activation_handler(Arc::new(crate::activate_session))")
    }

    /// The note guard's own logic, likewise.
    fn note_of(src: &str) -> Option<String> {
        let at = src.find("fn NotificationsPanel")?;
        let rest = &src[at..];
        let end = rest.find("#[component]").unwrap_or(rest.len());
        let panel = &rest[..end];
        let at = panel.find("class: \"rg-sheet__note\"")?;
        let note = &panel[at..];
        let end = note.find("\n        }")?;
        Some(
            note[..end]
                .chars()
                .filter(|c| !c.is_whitespace() && *c != '\\')
                .collect(),
        )
    }

    let real_note = note_of(&settings).expect("the shipped note");
    assert!(
        installs_in_the_initialiser(&settings),
        "the real file must pass"
    );

    // The install is deleted.
    let broken = settings.replace(
        "set_activation_handler(Arc::new(crate::activate_session))",
        "",
    );
    assert!(
        !installs_in_the_initialiser(&broken),
        "the install guard would pass with the install deleted"
    );

    // A second delivery path appears that does not force NOTIFIER.
    let broken = format!("{settings}\nfn other() {{ backend.notify(x); }}\n");
    assert_ne!(
        broken.matches("backend.notify(").count(),
        1,
        "the single-delivery-path guard would pass with two send sites"
    );

    // ESCAPE, now closed: the install moves into a helper nobody calls, and
    // stays textually between the static and the next item. The first version
    // of the guard searched that whole range and passed. Present, correct,
    // never reached, which is the defect this ticket exists to fix.
    let broken = settings
        .replace(
            "if let Err(why) = backend.set_activation_handler(Arc::new(crate::activate_session)) {",
            "if false {",
        )
        .replace(
            "pub fn notify_support",
            "fn install_that_nobody_calls(backend: &dyn Notifier) {\n    let _ = \
             backend.set_activation_handler(Arc::new(crate::activate_session));\n}\n\npub fn \
             notify_support",
        );
    assert!(
        broken.contains("set_activation_handler(Arc::new(crate::activate_session))"),
        "the mutation did not apply: the needle has to survive somewhere unreachable"
    );
    assert!(
        !installs_in_the_initialiser(&broken),
        "the install guard passes when the route is installed only by a \
         function nobody calls"
    );

    // ESCAPE, now closed: the note KEEPS the true sentence and adds a false
    // one in different words. A phrase blacklist passed this, which is
    // exactly how the scrollback caption in this file shipped wrong twice.
    let broken = settings.replace(
        "The window you are \\\n             in is left where it is.",
        "It also brings the session to the front of the window you are \\\n             \
         already in.",
    );
    let mutant_note = note_of(&broken).expect("the doctored note");
    assert_ne!(
        mutant_note, real_note,
        "the note guard passes when a second, false sentence is added beside \
         the true one"
    );
    assert!(
        mutant_note.contains("inanewwindow"),
        "the mutation has to keep the true sentence, or it proves nothing \
         about a blacklist"
    );
}
