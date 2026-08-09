// The loading screen: the mark, centred, until the first real frame.
//
// The window used to paint nothing between the moment it appeared and the
// moment the client had connected and rendered. On a warm start that is
// imperceptible; on a cold one, or against a daemon that has to be launched
// first, it is a rectangle of flat colour with no indication that anything is
// happening.
//
// Two rules shape this file.
//
// It must never flash. A screen that appears for 80 milliseconds and vanishes
// is worse than no screen at all, so nothing is inserted into the document
// until `__vitrum_bootDelayMs` has passed. A start that beats the delay
// renders no overlay, creates no element and touches no style.
//
// It must not animate. There is no spinner, no pulse and no fade. A repeating
// animation is a wakeup per frame for as long as it is on screen, and this
// product's claim is that it does no work it was not asked to do. The mark is
// static.
//
// The timer lives here, in the document head, and not in `bootstrap.js`: the
// bridge is forbidden a `setTimeout` outright, and it could not own this one
// anyway. The bridge is evaluated after the app has mounted, which is the
// event this file is waiting for.
(function () {
  const DELAY_MS = window.__vitrum_bootDelayMs;
  const MARK = window.__vitrum_bootMark;

  // The overlay element, or null while nothing is on screen. Null both before
  // the delay elapses and after the first frame removes it, because in both
  // cases there is nothing in the document.
  let overlay = null;
  // The pending one-shot, or null once it has fired or been cancelled. One
  // timer for the life of the window; it is never rearmed.
  let timer = setTimeout(show, DELAY_MS);

  function show() {
    timer = null;
    if (overlay) return;
    const host = document.body || document.documentElement;
    if (!host) return;
    overlay = document.createElement("div");
    overlay.id = "vitrum-boot";
    overlay.setAttribute("role", "presentation");
    overlay.innerHTML = MARK;
    host.appendChild(overlay);
  }

  // The first real frame is up. Cancel the pending reveal and take down
  // whatever is on screen.
  //
  // Idempotent, because the bridge may reach a rendered document more than
  // once: a second window in the same process evaluates its own copy of this
  // file, and a reconnect re-enters the path that calls it.
  function done() {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    if (overlay) {
      overlay.remove();
      overlay = null;
    }
  }

  window.__vitrum_bootDone = done;
  // Read by the tests, which need to tell "never shown" from "shown and
  // removed" and cannot do it from the document alone.
  window.__vitrum_bootState = function () {
    return { pending: timer !== null, showing: overlay !== null };
  };
})();
