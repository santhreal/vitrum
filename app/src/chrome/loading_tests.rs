//! The loading screen: what the document head sets up, and what the script
//! does with it.
//!
//! Two halves, tested two ways. What Rust emits into the head is asserted here
//! directly. What the script then does with a clock is asserted by running it,
//! because the whole feature is a timing decision and a timing decision has no
//! honest static proof: "the source contains a setTimeout" says nothing about
//! whether a fast start renders anything.

use super::*;

/// The options a launch with no arguments produces. `Options` has no
/// `Default`, deliberately: every field is a documented launch decision.
fn bare_options() -> Options {
    Options::parse(Vec::<String>::new()).expect("no arguments always parses")
}

/// The loading screen must draw the mark and nothing else.
///
/// The head is what the webview parses before anything else exists, so the
/// mark has to be in it rather than fetched. Fetching it would resolve after
/// the thing it was drawn for is over.
#[test]
fn the_head_carries_the_mark_and_the_delay() {
    let head = document_head(bare_options());
    assert!(
        head.contains("window.__vitrum_bootDelayMs=400"),
        "the head does not carry the loading delay"
    );
    assert!(
        head.contains("M12 42 L48 84 L84 42"),
        "the head does not carry the mark's pavilion, so the loading screen has \
         nothing to draw"
    );
    assert!(
        head.contains("window.__vitrum_bootDone"),
        "the head does not expose the dismissal the bridge calls"
    );
}

/// The delay in the document must be the constant, not a number typed twice.
///
/// Two copies of a timing value drift, and the one that drifts is always the
/// one nobody is testing.
#[test]
fn the_delay_in_the_document_is_the_constant() {
    let head = document_head(bare_options());
    assert!(
        head.contains(&format!("window.__vitrum_bootDelayMs={LOADING_SCREEN_DELAY_MS}")),
        "the emitted delay is not LOADING_SCREEN_DELAY_MS"
    );
}

/// Nothing about the loading screen may be pre-rendered into the document.
///
/// The screen must be absent for a fast start, and "absent" means no element,
/// not an element with `display:none`. A hidden element is still a node the
/// engine lays out and still a thing that can be revealed by a stylesheet
/// loading a frame late, which is the flash this feature exists to prevent.
#[test]
fn the_head_pre_renders_no_overlay() {
    let head = document_head(bare_options());
    assert!(
        !head.contains("<div"),
        "the document head renders an element; the loading screen must be created \
         by script only, after the delay"
    );
}

/// The loading screen must not animate.
///
/// An animation is a wakeup per frame for as long as it is on screen, and idle
/// cost is this product's competitive claim. A spinner during startup is the
/// easiest place in the whole application to forget that.
#[test]
fn the_loading_screen_does_not_animate() {
    let head = document_head(bare_options());
    for banned in ["@keyframes", "animation:", "transition:", "requestAnimationFrame"] {
        assert!(
            !LOADING_JS.contains(banned) && !head.contains(&format!("#vitrum-boot{banned}")),
            "the loading screen uses {banned}"
        );
    }
    assert!(!LOADING_JS.contains("setInterval"), "the loading screen polls");
}

/// The bridge must take the screen down when the first frame lands.
///
/// `#rg-term` appearing is the app's root having committed a tree. The bridge
/// is the only code that observes it, so it is the only code that can end the
/// loading screen; nothing else in the process knows the difference between an
/// empty document and a rendered one.
#[test]
fn the_bridge_dismisses_on_the_first_frame() {
    let js = crate::BOOTSTRAP_JS;
    assert!(
        js.contains("window.__vitrum_bootDone()"),
        "bootstrap.js no longer dismisses the loading screen"
    );
    let container = js
        .split_once("function waitForContainer()")
        .expect("bootstrap.js has no waitForContainer")
        .1;
    let body = &container[..container.find("\n}").unwrap_or(container.len())];
    assert_eq!(
        body.matches("firstFrame()").count(),
        2,
        "both paths out of waitForContainer must report the first frame: the \
         container already being present and the observer seeing it arrive"
    );
}

// --------------------------------------------------------------------------
// The script, run against a clock
// --------------------------------------------------------------------------

/// The harness: a fake document, a fake window and a clock that only moves
/// when it is told to.
///
/// Deliberately not a DOM library. The script touches five methods, and a
/// hand-written stand-in makes "was an element ever created" observable, which
/// is the exact question the fast-start case asks and which a real DOM cannot
/// answer after the fact.
const HARNESS: &str = r#"
const fs = require("fs");
const src = fs.readFileSync(process.argv[2], "utf8");

function makeEnv(delayMs, mark) {
  let now = 0;
  let nextId = 1;
  const timers = new Map();
  let created = 0;

  function element() {
    return {
      children: [],
      parent: null,
      id: "",
      innerHTML: "",
      attrs: {},
      setAttribute(k, v) { this.attrs[k] = v; },
      appendChild(child) { child.parent = this; this.children.push(child); },
      remove() {
        if (!this.parent) return;
        this.parent.children = this.parent.children.filter((c) => c !== this);
        this.parent = null;
      },
    };
  }

  const body = element();
  const document = {
    body,
    documentElement: element(),
    createElement() { created += 1; return element(); },
  };
  const win = { __vitrum_bootDelayMs: delayMs, __vitrum_bootMark: mark, document };
  win.window = win;

  const setTimeout = (fn, ms) => { const id = nextId++; timers.set(id, { fn, at: now + ms }); return id; };
  const clearTimeout = (id) => { timers.delete(id); };
  const advance = (ms) => {
    now += ms;
    for (const [id, t] of [...timers]) {
      if (t.at <= now) { timers.delete(id); t.fn(); }
    }
  };

  new Function("window", "document", "setTimeout", "clearTimeout", src)(
    win, document, setTimeout, clearTimeout);

  return { win, body, advance, elements: () => created };
}

const MARK = "<svg id='m'></svg>";
const out = {};

// A slow start: the delay elapses, the mark appears, the first frame removes it.
{
  const e = makeEnv(400, MARK);
  out.slowBeforeDelay = e.body.children.length;
  e.advance(399);
  out.slowJustBeforeDelay = e.body.children.length;
  e.advance(2);
  out.slowAfterDelay = e.body.children.length;
  out.slowMarkup = e.body.children.length ? e.body.children[0].innerHTML : null;
  out.slowId = e.body.children.length ? e.body.children[0].id : null;
  e.win.__vitrum_bootDone();
  out.slowAfterFirstFrame = e.body.children.length;
  out.slowState = e.win.__vitrum_bootState();
}

// A fast start: the first frame beats the delay, so nothing is ever created.
{
  const e = makeEnv(400, MARK);
  e.advance(200);
  e.win.__vitrum_bootDone();
  out.fastAtDismiss = e.body.children.length;
  e.advance(10000);
  out.fastLater = e.body.children.length;
  out.fastElementsCreated = e.elements();
  out.fastState = e.win.__vitrum_bootState();
}

// Dismissing twice, and dismissing after the screen is already down, must be
// safe: a second window and a reconnect both re-enter that path.
{
  const e = makeEnv(400, MARK);
  e.advance(500);
  e.win.__vitrum_bootDone();
  e.win.__vitrum_bootDone();
  e.advance(500);
  out.doubleDismiss = e.body.children.length;
}

console.log(JSON.stringify(out));
"#;

/// Run the harness against `loading.js` and return its JSON, or `None` when
/// there is no node on this machine.
fn run_harness() -> Option<serde_json::Value> {
    if std::process::Command::new("node").arg("--version").output().is_err() {
        eprintln!(
            "skipping the loading-screen behaviour cases: no `node` on PATH. The \
             static cases in this file still ran."
        );
        return None;
    }
    let dir = std::env::temp_dir().join(format!("vitrum-loading-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("stage the harness");
    let script = dir.join("harness.js");
    let source = dir.join("loading.js");
    std::fs::write(&script, HARNESS).expect("write the harness");
    std::fs::write(&source, LOADING_JS).expect("write the script under test");

    let out = std::process::Command::new("node")
        .arg(&script)
        .arg(&source)
        .output()
        .expect("run node");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        out.status.success(),
        "the harness failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(serde_json::from_slice(&out.stdout).expect("the harness printed JSON"))
}

/// A slow start must show the mark, and the first frame must take it away.
///
/// The defect this closes is the one the feature was written for: a window
/// that paints nothing at all until the client has connected. It also closes
/// the opposite one, a loading screen that is shown and then left on top of
/// the running application because nothing ever removes it.
#[test]
fn a_slow_start_shows_the_mark_and_the_first_frame_removes_it() {
    let Some(out) = run_harness() else { return };
    assert_eq!(out["slowBeforeDelay"], 0, "something was in the document before the delay");
    assert_eq!(out["slowJustBeforeDelay"], 0, "the screen appeared one millisecond early");
    assert_eq!(out["slowAfterDelay"], 1, "the delay elapsed and no screen appeared");
    assert_eq!(out["slowId"], "vitrum-boot", "the overlay is not the styled element");
    assert_eq!(out["slowMarkup"], "<svg id='m'></svg>", "the overlay does not carry the mark");
    assert_eq!(out["slowAfterFirstFrame"], 0, "the loading screen survived the first frame");
    assert_eq!(out["slowState"]["showing"], false);
    assert_eq!(out["slowState"]["pending"], false, "a timer was left armed");
}

/// A start faster than the delay must render nothing at all.
///
/// Not hidden, not removed a frame later: never created. A logo that appears
/// for 80 milliseconds and vanishes is a flash, and it is worse than the blank
/// window it replaced because it draws the eye to a surface that is already
/// gone by the time it lands there.
#[test]
fn a_start_faster_than_the_delay_renders_nothing() {
    let Some(out) = run_harness() else { return };
    assert_eq!(out["fastAtDismiss"], 0, "the screen was up before the delay elapsed");
    assert_eq!(out["fastLater"], 0, "the screen appeared after the first frame had landed");
    assert_eq!(
        out["fastElementsCreated"], 0,
        "an element was created for a start that beat the delay; the screen must \
         never be built at all"
    );
    assert_eq!(out["fastState"]["pending"], false, "the timer was left armed after dismissal");
}

/// Dismissing more than once must be safe.
///
/// A second window in the process evaluates its own copy of the script, and a
/// reconnect re-enters the path that calls the dismissal. Neither may resurrect
/// the screen or throw inside the bridge's command loop.
#[test]
fn dismissing_twice_is_safe() {
    let Some(out) = run_harness() else { return };
    assert_eq!(out["doubleDismiss"], 0, "the loading screen came back");
}
