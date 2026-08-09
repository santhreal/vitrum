// The JavaScript half of the vitrum client.
//
// Runs once, inside a Dioxus eval, as `new AsyncFunction("dioxus", src)(dioxus)`.
// It never returns: the command loop at the bottom is what keeps the duplex
// channel to Rust open for the life of the window.
//
// Division of labour, which is the whole point of the file:
//
//   * There is no WebSocket here. It lives in `app/src/socket.rs` and owns
//     connect, reconnect, close codes, sequence continuity, the backlog splice
//     and the reassembly of a character split across two frames. This file
//     receives frames that are already decoded and already in order.
//   * Pane bytes arrive over a wry asset route as an `ArrayBuffer`, not over
//     the eval channel. That channel is JSON, JSON strings must be valid
//     UTF-8, and PTY output is arbitrary bytes.
//   * What is left is what only the DOM can do or only the DOM can see: the
//     terminal emulator itself, focus, the clipboard, the fit addon's
//     geometry, chords, and the keystrokes the grid captures. This file holds
//     no session id, no byte offset, no session list and no scrollback.
//
// Nothing here animates, polls, or sets an interval. Every wakeup is caused by
// a pane update, a DOM event, or a command from Rust.

const state = {
  term: null,
  // Set once a mount has failed, so the failure is reported once instead of
  // retried on every pane op. Declared here rather than assigned onto the
  // object later: one hidden class for the life of the window.
  termMountFailed: false,
  // Logical lines between the top visible line and the end of the buffer,
  // captured when a page-back is reported. The repaint adds history above, so
  // this is what puts the operator back on the line they were reading. It
  // stays on this side because counting wrapped rows needs the grid.
  keepLineFromEnd: 0,
};

function report(detail) {
  dioxus.send({ ev: "bad", detail: String(detail) });
}

// Resolve the computed style that a theme value should be read from.
//
// The `document.documentElement` default is WRONG for anything the theme
// touches, and that was a real bug: `data-theme` is set on `div.rg-app`
// (main.rs), not on `<html>`, so `[data-theme="light"]` never applied to the
// document element and every colour resolved to the `:root` dark value. The
// light palette declares `--rg-terminal-bg: #ffffff` and the terminal stayed at
// `#0b0b0d` in light mode, on a fresh launch as well as a live switch. Custom
// properties inherit, so reading from any element inside `.rg-app` resolves the
// theme correctly.
function styleOf(el) {
  return getComputedStyle(el || document.documentElement);
}

// Read one custom property off an already-resolved style.
//
// This takes a style rather than an element because resolving five colours off
// one node used to mean five `getComputedStyle` calls, and every one of them is
// a style resolution the engine has to be asked for. One resolve per read site.
function cssVar(name, fallback, style) {
  const v = style.getPropertyValue(name).trim();
  return v || fallback;
}

// The xterm theme for the "follow the app theme" case, resolved against the
// themed subtree in one style read.
function cssTheme(el) {
  const s = styleOf(el);
  return {
    background: cssVar("--rg-terminal-bg", "#0b0b0d", s),
    foreground: cssVar("--rg-terminal-fg", "#f1f3f7", s),
    cursor: cssVar("--rg-accent", "#4c6ef5", s),
    cursorAccent: cssVar("--rg-terminal-bg", "#0b0b0d", s),
    selectionBackground: cssVar("--rg-terminal-selection", "rgba(76,110,245,0.3)", s),
  };
}

// The theme to hand xterm, given the current settings push.
//
// A named palette arrives whole, all twenty colours, straight from the Rust
// table. `null` is the operator asking to follow the app theme, and only then
// is the stylesheet read. The two cases are kept apart deliberately: reading
// CSS for a named palette would mean Rust writing twenty custom properties
// onto the app root purely so JS could read them back, and the sixteen ANSI
// slots have no CSS consumer at all.
//
// A missing `theme` key is the same as `null`, because a push from an older
// call site must not blank the palette.
function termTheme(el, opts) {
  const o = opts || window.__vitrum_termOptions || {};
  const want = o.theme;
  const theme = want && typeof want === "object" ? want : cssTheme(el);
  if (!o.allowTransparency) return theme;
  // The one place a named palette and the inherit case meet, so the one place
  // the cell background can be cleared without writing the rule twice.
  //
  // Cleared rather than tinted: `.rg-terminal` already paints the pane at the
  // chosen alpha, and a cell that also carried it would blend the tint over
  // itself and come out close to opaque at the midpoint of the slider.
  //
  // A copy, because `theme` may be the caller's object and `themeChanged`
  // compares against the live one: mutating it in place would make the two
  // equal and the change would never be applied.
  return { ...theme, background: "rgba(0,0,0,0)" };
}

// Whether two xterm theme objects differ.
//
// Assigning `term.options.theme` forces a full repaint of the grid, and a
// settings push happens on every keystroke in the font field, so the cheap
// comparison is worth doing. Keys from both sides are walked: dropping a key
// (a named palette back to the app theme, which has no ANSI slots) is a change
// that a one-sided walk would miss.
function themeChanged(before, after) {
  const a = before || {};
  const b = after || {};
  for (const k of Object.keys(a)) if (a[k] !== b[k]) return true;
  for (const k of Object.keys(b)) if (a[k] !== b[k]) return true;
  return false;
}

// --------------------------------------------------------------------------
// Terminal
// --------------------------------------------------------------------------

// Resolve once #rg-term is in the DOM. A MutationObserver, not a poll: it is
// woken by the DOM mutation itself and disconnects on the first hit.
//
// This is also the moment the first real frame is up, so it is where the
// loading screen comes down. `#rg-term` is rendered by the app's root, so its
// arrival means the client has connected, the state has loaded and Dioxus has
// committed a tree — there is something to look at. Taking the screen down on
// the bridge's own start instead would uncover a document that is still empty.
function firstFrame() {
  if (window.__vitrum_bootDone) window.__vitrum_bootDone();
}

function waitForContainer() {
  const found = document.getElementById("rg-term");
  if (found) {
    firstFrame();
    return Promise.resolve(found);
  }
  return new Promise((resolve) => {
    const mo = new MutationObserver(() => {
      const el = document.getElementById("rg-term");
      if (el) {
        mo.disconnect();
        firstFrame();
        resolve(el);
      }
    });
    mo.observe(document.documentElement, { childList: true, subtree: true });
  });
}

function mount(el) {
  if (typeof Terminal !== "function") {
    report("xterm.js did not load; terminal pane is dead");
    return;
  }

  // Persisted preferences, if the settings module already pushed them. Read
  // here as well as in `applyTerm` so a restart with a saved font size mounts
  // at that size instead of mounting at the default and visibly reflowing.
  const pref = window.__vitrum_termOptions || {};

  // The face the stylesheet asks for, resolved once. `null` on the wire is the
  // Font control's "Stylesheet default" option, so the applier needs this
  // value too and must not pay another style resolution to get it. The token
  // is declared at `:root` and is not themed, so one read is the whole story.
  const stylesheetFont = cssVar("--rg-font-mono", "monospace", styleOf(el));

  const term = new Terminal({
    allowProposedApi: true,
    // A blinking cursor is a repeating timer, and a repeating timer is a
    // wakeup per tick for as long as the window is open. Idle CPU must be 0%.
    cursorBlink: false,
    cursorStyle: "block",
    // The server owns history. This is only the local viewport buffer that
    // makes the mouse wheel work between repaints, and there is exactly one
    // Terminal in the process, so it does not grow with the agent count.
    scrollback: typeof pref.scrollback === "number" ? pref.scrollback : 1000,
    fontFamily: pref.fontFamily || stylesheetFont,
    fontSize: typeof pref.fontSize === "number" ? pref.fontSize : 13,
    lineHeight: 1.2,
    letterSpacing: 0,
    drawBoldTextInBrightColors: false,
    // Off unless the profile asked for a see-through grid. It makes the
    // renderer blend every cell rather than fill a run of them, which is a
    // cost the opaque default has no reason to carry.
    allowTransparency: !!pref.allowTransparency,
    theme: termTheme(el, pref),
  });

  term.open(el);
  state.term = term;

  // Renderer choice starts from the value Rust injects into the document head
  // and is thereafter owned by the settings modal, which calls `applyTerm`.
  // The DOM renderer is the default for a measured reason: under WebKitGTK the
  // WebGL renderer costs a steady 0.244% idle CPU and about 80 MB more PSS,
  // for throughput nobody at twenty agents can consume.
  //
  // A lost context is reported, not swallowed: silently dropping to the DOM
  // renderer is a throughput cliff that would otherwise look like "the
  // terminal got slow for no reason".
  let webgl = null;

  function setRenderer(want) {
    if (want === "webgl" && !webgl) {
      // Compile the addon now. It is not part of the startup vendor load, so
      // on a DOM-renderer launch this is the first time its source is parsed.
      if (!loadWebgl()) {
        report("the WebGL renderer is unavailable in this build, staying on DOM");
        return;
      }
      try {
        webgl = new WebglAddon.WebglAddon();
        webgl.onContextLoss(() => {
          report("WebGL context lost; fell back to the DOM renderer");
          if (webgl) webgl.dispose();
          webgl = null;
        });
        term.loadAddon(webgl);
      } catch (e) {
        report(`WebGL renderer unavailable, using DOM renderer: ${e}`);
        webgl = null;
      }
    } else if (want !== "webgl" && webgl) {
      // `dispose` on a loaded addon detaches it and xterm falls back to the
      // DOM renderer in place, with no remount and no lost scroll position.
      webgl.dispose();
      webgl = null;
    }
  }

  setRenderer(
    pref.renderer || (window.__vitrum_renderer !== "dom" ? "webgl" : "dom"),
  );

  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit);

  // Reaching the top of the buffer asks the daemon for what came before it.
  //
  // `onScroll` is an xterm event, not a DOM wheel listener and not a poll: it
  // fires only when the viewport actually moves, so an idle window registers
  // nothing and wakes for nothing. That matters here because the whole product
  // is built on 0% idle CPU.
  //
  // No guards. Whether there is older history, whether a request is already
  // in flight, and which session this pane is showing are all Rust's to know
  // now, and were three pieces of session state this file had to hold to
  // answer. What is left is the gesture and the one measurement only this side
  // can take.
  term.onScroll((ydisp) => {
    if (ydisp !== 0) return;
    // Where the operator is reading, as a distance from the end of the buffer,
    // so the repaint can put them back on it. The top visible row may be the
    // middle of a wrapped line; counting logical starts from the bottom is the
    // measure that survives the reflow a wider window would cause.
    const buf = term.buffer.active;
    let back = 0;
    for (let y = buf.length - 1; y > buf.viewportY; y--) {
      const line = buf.getLine(y);
      if (line && !line.isWrapped) back++;
    }
    state.keepLineFromEnd = back;
    dioxus.send({ ev: "pageBack" });
  });

  // Geometry changes are driven by layout, not by a timer, and a fit only runs
  // when the box the grid has to fill has actually changed size.
  //
  // ResizeObserver delivers once on observe, by which time the synchronous fit
  // below has already run, so that first delivery used to re-measure the cell
  // grid and call `term.resize` again for a size nothing had changed. That is a
  // second forced layout on the one path where the operator is sitting waiting
  // for a terminal to appear. Measured offline: two fits to reach a sized grid
  // before, one after.
  let fitW = 0;
  let fitH = 0;

  function refit(why) {
    const w = el.clientWidth;
    const h = el.clientHeight;
    if (w === 0 || h === 0) return;
    fitW = w;
    fitH = h;
    try {
      fit.fit();
    } catch (e) {
      report(`${why}: ${e}`);
    }
  }

  const ro = new ResizeObserver(() => {
    // A ResizeObserver callback runs after layout, so these two reads are
    // cached values rather than a forced reflow.
    if (el.clientWidth === fitW && el.clientHeight === fitH) return;
    refit("fit failed");
  });
  ro.observe(el);

  // Fires only when cols/rows actually change, so this cannot feed back into
  // itself through a re-render.
  //
  // Only reported, never sent. Telling the daemon is Rust's, because only Rust
  // knows which session this pane is attached to, and a resize addressed to a
  // session the pane stopped showing while the event was in flight would
  // reflow somebody else's grid.
  term.onResize(({ cols, rows }) => {
    dioxus.send({ ev: "resize", cols, rows });
  });

  const encoder = new TextEncoder();
  term.onData((s) => {
    // A plain loop, not `Array.from`: that walks the typed array through the
    // iterator protocol and allocates a step result per byte. One keystroke
    // would not care. A paste is thousands of bytes and does.
    const u = encoder.encode(s);
    const data = new Array(u.length);
    for (let i = 0; i < u.length; i++) data[i] = u[i];
    dioxus.send({ ev: "input", data });
  });
  // Raw 8-bit responses (mouse reports, DEC replies) arrive here as a string of
  // code units 0..255, not as UTF-8 text.
  term.onBinary((s) => {
    const data = new Array(s.length);
    for (let i = 0; i < s.length; i++) data[i] = s.charCodeAt(i) & 0xff;
    dioxus.send({ ev: "input", data });
  });

  refit("initial fit failed");

  // Live reconfiguration, called by ui::settings whenever a Terminal setting
  // changes and once at startup with whatever was persisted. Every field is
  // optional so a partial push cannot clear the others, and a font or size
  // change is followed by a refit because the cell grid just changed size.
  window.__vitrum_applyTerm = (o) => {
    if (!o || !state.term) return;
    let geometry = false;
    if (typeof o.fontSize === "number" && o.fontSize !== term.options.fontSize) {
      term.options.fontSize = o.fontSize;
      geometry = true;
    }
    // `null` is the wire form of "Stylesheet default": `term_options_script`
    // emits a literal `fontFamily:null` for it, and a `typeof === "string"`
    // guard dropped it, so choosing that option after choosing a named face
    // did nothing at all until a restart. Anything else that is not a string
    // is absence, which must stay a no-op or a partial push would clear the
    // font.
    const family =
      o.fontFamily === null
        ? stylesheetFont
        : typeof o.fontFamily === "string"
          ? o.fontFamily
          : null;
    if (family !== null && family !== term.options.fontFamily) {
      term.options.fontFamily = family;
      geometry = true;
    }
    if (typeof o.scrollback === "number" && o.scrollback !== term.options.scrollback) {
      term.options.scrollback = o.scrollback;
    }
    // Before the theme below, because the theme is what carries the cleared
    // cell background: setting a transparent colour on a terminal that is
    // still refusing to composite one renders it as solid black.
    const wantsAlpha = !!o.allowTransparency;
    if (wantsAlpha !== term.options.allowTransparency) {
      term.options.allowTransparency = wantsAlpha;
    }
    if (typeof o.renderer === "string") setRenderer(o.renderer);
    // Theme is not one of `o`'s fields: it lives in CSS, and a settings push
    // is the only signal that anything changed. Re-read it here so a mounted
    // terminal picks the palette up.
    //
    // Read synchronously and deliberately. Deferring by one frame would be the
    // obvious way to wait for Dioxus to write the new `data-theme` onto
    // `.rg-app`, and the frame-scheduling call is banned outright by
    // `bootstrap_js_has_no_timers_or_animation`, which greps this file for the
    // name. A blanket ban is the only version of that rule that stays true.
    // The cost of reading early is that a live switch can lag one settings
    // push; the cost of a carve-out is the idle-CPU claim this product is
    // built on.
    // Compared field by field, not on background and foreground alone. A
    // named palette changes sixteen ANSI slots that those two do not witness,
    // so a shallow check would leave the grid on the previous palette's reds
    // and greens until something else happened to move the background.
    const next = termTheme(el, o);
    if (themeChanged(term.options.theme, next)) {
      term.options.theme = next;
    }
    if (geometry) {
      // The box has not changed, the CELL has, so this cannot go through the
      // size check the observer uses.
      refit("fit after a font change failed");
    }
  };
  window.__vitrum_applyTerm(window.__vitrum_termOptions || {});
}

// --------------------------------------------------------------------------
// Shell chords
// --------------------------------------------------------------------------
//
// This file owns no chord of its own. The table is generated in Rust from
// keymap::CHORDS folded with the user's rebindings and injected as
// window.__vitrum_keymap, so a binding cannot exist in the webview without
// also existing in the shortcut overlay. A missing table means no chord fires,
// which is reported rather than silently tolerated.
//
// Mutable, because the settings modal rebinds chords without a restart. It is
// replaced wholesale rather than patched: a half-applied table would leave one
// action reachable by two chords and another by none.

let KEYMAP = Array.isArray(window.__vitrum_keymap) ? window.__vitrum_keymap : null;

// The keys the table can match, split by whether the chord needs Ctrl or Alt.
//
// Every keydown in the window runs through `chord`, including every keystroke
// typed at an agent, and the shipped table has 34 entries. These two sets turn
// the common case, a bare key with no modifier held, into one lookup instead of
// a scan of all of them. They are derived from the table rather than assuming
// what is in it.
//
// Built on the first keydown rather than at startup, and dropped rather than
// rebuilt when the table is replaced. Building them eagerly measured 4.3 us on
// the synchronous startup path, which is the one path where nothing is on
// screen yet; the first keypress is a long way after the first paint.
let PLAIN_KEYS = null;
let MOD_KEYS = null;

function indexKeymap() {
  PLAIN_KEYS = new Set();
  MOD_KEYS = new Set();
  for (const c of KEYMAP) {
    if (typeof c.key !== "string") continue;
    if (c.ctrl || c.alt) MOD_KEYS.add(c.key);
    else PLAIN_KEYS.add(c.key);
  }
}

window.__vitrum_applyKeymap = (table) => {
  KEYMAP = Array.isArray(table) ? table : null;
  PLAIN_KEYS = null;
  MOD_KEYS = null;
};

// True when the event is headed for a text entry. xterm.js reads keys through
// a hidden textarea, so this covers the terminal as well as the filter field.
function inTextField(e) {
  const t = e.target;
  if (!t) return false;
  return t.tagName === "INPUT" || t.tagName === "TEXTAREA";
}

// True when the terminal grid has focus. Chords scoped notTerminal belong to
// the agent while it is being typed at: Ctrl+K there is kill-to-end-of-line.
function inTerminal(e) {
  const t = e.target;
  return !!t && !!t.closest && !!t.closest("#rg-term");
}

// True when a transient layer is open. The layer is a real element, so this is
// one DOM query on keydown and nothing at all while no layer exists.
function layerOpen() {
  return document.querySelector(".rg-layer") !== null;
}

// True when focus is on a sidebar row. Plain arrow keys belong to the agent
// everywhere else, so traversal can only claim them once the operator has
// actually moved into the list (Ctrl+Shift+E, or a click on a row).
function inSessionList(e) {
  const t = e.target;
  return !!t && !!t.closest && !!t.closest(".rg-session");
}

function scopeAllows(scope, e) {
  switch (scope) {
    case "global":
      return true;
    case "notTerminal":
      return !inTerminal(e);
    case "notTextInput":
      return !inTextField(e);
    case "layerOnly":
      return layerOpen();
    case "sessionList":
      return inSessionList(e) && !layerOpen();
    default:
      report(`unknown chord scope ${scope}`);
      return false;
  }
}

// The key a chord is matched on, with the top digit row unshifted.
//
// `KeyboardEvent.key` for Ctrl+Shift+1 on a US layout is `!`, not `1`, so a
// binding stored as `1` never matches the keystroke it is named after. That is
// a shortcut the settings panel displays, the overlay explains, and the product
// never fires -- and digits are the most natural thing to bind a saved command
// to, so it took out exactly the bindings people would make first.
//
// `code` is the physical key and is unaffected by Shift or by the layout, so a
// top-row digit is taken from there. Everything else comes from `key`, because
// `code` for a letter is `KeyK` rather than `k` and because a chord bound to a
// letter is already layout-dependent in the operator's head.
//
// This is the same rule `ui/dialog.rs::chord_of` applies. It had to exist in
// both places because the dialog matches its own keydown while the shared
// table is matched here, and for a while only the dialog had it.
function chordKey(e) {
  const code = typeof e.code === "string" ? e.code : "";
  if (code.length === 6 && code.startsWith("Digit")) {
    const d = code[5];
    if (d >= "0" && d <= "9") return d;
  }
  return e.key.toLowerCase();
}

// Match one keydown against the injected table. First match wins; Rust proves
// no two entries can match the same event.
function chord(e) {
  if (!KEYMAP) return null;
  // Meta is never part of a binding. Cmd+Tab is the macOS application
  // switcher and never reaches us, so the primary modifier is Ctrl on every
  // platform and a held Cmd means the chord is not ours.
  if (e.metaKey) return null;
  if (PLAIN_KEYS === null) indexKeymap();
  const key = chordKey(e);
  // The partition is exact, which is what makes this a lookup and not a guess:
  // a chord with neither Ctrl nor Alt can only match an event with neither
  // held, and a chord with either can only match an event with that one held.
  if (!(e.ctrlKey || e.altKey ? MOD_KEYS : PLAIN_KEYS).has(key)) return null;
  for (const c of KEYMAP) {
    if (c.key !== key) continue;
    if (c.ctrl !== e.ctrlKey || c.alt !== e.altKey) continue;
    if (c.shift === "on" && !e.shiftKey) continue;
    if (c.shift === "off" && e.shiftKey) continue;
    if (!scopeAllows(c.scope, e)) continue;
    return c.action;
  }
  return null;
}

// Capture phase on window, so a chord is claimed before xterm's textarea
// listener sees it. Without capture, Ctrl+Tab would insert a tab character.
window.addEventListener(
  "keydown",
  (e) => {
    const action = chord(e);
    if (!action) return;
    e.preventDefault();
    e.stopPropagation();
    dioxus.send({ ev: "key", action });
  },
  true,
);

// Move DOM focus, and bring the element into view. Done here rather than in
// Rust because focus and scrolling are DOM operations the virtual DOM has no
// handle for. Keyboard traversal that moved focus to a row thirty rows below
// the fold, without scrolling, would be indistinguishable from doing nothing.
function focusDom(selector) {
  let el;
  try {
    el = document.querySelector(selector);
  } catch (e) {
    report(`bad focus selector ${selector}: ${e}`);
    return;
  }
  if (!el) return;
  el.focus({ preventScroll: true });
  if (el.select) el.select();
  if (el.scrollIntoView) el.scrollIntoView({ block: "nearest", inline: "nearest" });
}

// Put text on the clipboard and report the outcome. A webview can refuse the
// async clipboard API outside a secure context, so the synchronous
// execCommand path is a real fallback rather than a formality, and a refusal
// by both is reported instead of being swallowed into a false "Copied".
function copyText(text) {
  const fallback = () => {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "absolute";
    ta.style.left = "-9999px";
    document.body.appendChild(ta);
    ta.select();
    let ok = false;
    try {
      ok = document.execCommand("copy");
    } catch (e) {
      ok = false;
    }
    document.body.removeChild(ta);
    dioxus.send({ ev: "copied", ok, text });
  };
  if (navigator.clipboard && navigator.clipboard.writeText) {
    navigator.clipboard.writeText(text).then(
      () => dioxus.send({ ev: "copied", ok: true, text }),
      fallback,
    );
    return;
  }
  fallback();
}

// --------------------------------------------------------------------------
// The pane route
// --------------------------------------------------------------------------
//
// The WebSocket used to be here. It is now `app/src/socket.rs`, which owns
// connect, reconnect, the close-code vocabulary, sequence continuity, the
// backlog splice and the reassembly of a character split across two frames.
// What is left on this side is a pull: the bytes Rust has already decided to
// paint, in the order it decided to paint them.
//
// The first path segment is what `dioxus-desktop`'s `desktop_handler` matches
// a registered asset handler on, and it must stay equal to
// `socket::PANE_ROUTE`.
const PANE_ROUTE = "/vitrum-pane";

// --------------------------------------------------------------------------
// Applying pane operations
// --------------------------------------------------------------------------
//
// Everything that touches the grid arrives here, in one stream, in the order
// Rust emitted it. There is no second channel into the terminal on purpose: a
// reset that overtook the write it was meant to precede would clear the
// repaint it was clearing FOR.

// `[tag:u8][len:u32 LE][payload]`, repeated. Mirrors `socket::encode_ops`.
const OP_WRITE = 0;
const OP_RESET = 1;
const OP_SCROLL_FROM_END = 2;
const OP_SCROLL_KEEP = 3;
const OP_HEADER_LEN = 5;
// The buffer row that starts the logical line `back` lines from the last one.
//
// Walks bottom-up counting rows that are NOT continuations, because one
// logical line occupies several rows once it wraps and a row index is what
// `scrollToLine` takes. Returns 0 when the line has been trimmed away, which
// puts the operator at the top of what survives rather than somewhere
// arbitrary.
function rowOfLineFromEnd(term, back) {
  const buf = term.buffer.active;
  let seen = 0;
  for (let y = buf.length - 1; y >= 0; y--) {
    const line = buf.getLine(y);
    if (!line || line.isWrapped) continue;
    if (seen === back) return y;
    seen++;
  }
  return 0;
}

// Put the viewport on the logical line `back` lines from the end.
//
// `write("", cb)` is how xterm says "once the parser queue has drained". The
// parser runs asynchronously, so the buffer is not final the instant a write
// returns and scrolling before it drains lands on the wrong row. It is the
// documented callback, not a timer.
//
// `lift` is a third of a screen of context above the line, for a search hit
// that would otherwise sit on the very top edge with nothing after it visible.
function scrollToLineFromEnd(term, back, lift) {
  term.write("", () => {
    const row = rowOfLineFromEnd(term, back);
    term.scrollToLine(Math.max(0, row - (lift ? Math.floor(term.rows / 3) : 0)));
  });
}

// Apply one batch of pane operations.
//
// The bytes are the socket's own, carried here as an ArrayBuffer and never
// decoded: a multi-byte character split across two frames was made whole in
// Rust and nothing on this path can split it again.
function applyOps(buf) {
  if (!ensureTerm()) return;
  const term = state.term;
  const dv = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  let at = 0;
  while (at + OP_HEADER_LEN <= buf.length) {
    const tag = buf[at];
    const len = dv.getUint32(at + 1, true);
    const from = at + OP_HEADER_LEN;
    const end = from + len;
    if (end > buf.length) {
      report(`pane op ${tag} claims ${len} bytes and only ${buf.length - from} are here`);
      return;
    }
    at = end;
    switch (tag) {
      case OP_WRITE:
        if (len) term.write(buf.subarray(from, end));
        break;
      case OP_RESET:
        // Full reset, not clear: the previous session may have left the grid
        // in alternate-screen mode, with a scroll region set, or with SGR
        // state pending, and any of those would corrupt the incoming repaint.
        term.reset();
        break;
      case OP_SCROLL_FROM_END:
        scrollToLineFromEnd(term, dv.getUint32(from, true), true);
        break;
      case OP_SCROLL_KEEP:
        // The operator was reading a line and asked for what came before it.
        // The repaint added history ABOVE, so the same content is now further
        // down. The distance was captured when the gesture was reported,
        // because only this side can count wrapped rows.
        scrollToLineFromEnd(term, state.keepLineFromEnd || 0, false);
        break;
      default:
        report(`unknown pane op ${tag}`);
        return;
    }
  }
  if (at !== buf.length) {
    report(`pane op stream ended with ${buf.length - at} bytes of header`);
  }
}

// Pull pane bytes from Rust, forever.
//
// A long poll over a wry asset route, not a timer and not a poll loop: the
// request parks in `socket::PaneQueue` until there is something to say, and a
// new one opens the instant it is answered. An idle window has exactly one
// request outstanding and wakes for nothing, which is the same idle cost the
// WebSocket had when it lived in this file.
//
// The route rather than the eval channel because that channel is JSON, JSON
// strings must be valid UTF-8, and PTY output is arbitrary bytes: it could
// only cross base64-encoded, which is a 4/3 size tax plus an `atob` and a
// per-byte copy on the hottest path in the product.
async function panePump() {
  await ready;
  for (;;) {
    let buf;
    try {
      const res = await fetch(PANE_ROUTE);
      buf = new Uint8Array(await res.arrayBuffer());
    } catch (e) {
      // The route is how the pane is painted at all, so losing it is not
      // something to retry quietly in a loop that would spin.
      report(`the pane route stopped answering, so this pane is now blind: ${e}`);
      return;
    }
    // An empty body is a superseded request, which happens when a page is
    // replaced. Ask again rather than treating it as an end.
    if (buf.length === 0) continue;
    try {
      applyOps(buf);
    } catch (e) {
      report(`pane update failed: ${e}`);
    }
  }
}

/// Compile the terminal bundles, which the document ships unparsed.
///
/// `main.rs` emits xterm.js and its addons as `type="text/plain"` so the
/// engine stores the source without compiling it. Compiling 390 KB of
/// JavaScript costs 5.0 MB of WebProcess memory per window, measured over
/// twenty windows, and a window that never focuses a session never needs it.
///
/// Order matters: the addons extend `Terminal`, so xterm goes first. Each
/// element's text is cleared once evaluated, which releases the source string
/// as well as deferring the compile.
///
/// Indirect eval, so the bundles land in global scope exactly as a real
/// `<script>` would have put them. They are our own vendored files, inlined by
/// the binary at compile time; there is no path by which this reaches anything
/// the operator or a session supplied.
function loadVendor() {
  if (typeof Terminal === "function") return true;
  for (const id of ["rg-vendor-xterm", "rg-vendor-fit"]) {
    const el = document.getElementById(id);
    if (!el || !el.textContent) continue;
    try {
      (0, eval)(el.textContent);
    } catch (e) {
      report(`${id} failed to load: ${e}`);
      return false;
    }
    el.textContent = "";
  }
  return typeof Terminal === "function";
}

// Evaluate the WebGL addon, once, the first time the operator asks for it.
//
// This is deliberately NOT part of `loadVendor`. The addon is 100 KB and the
// DOM renderer is the default, so parsing it in every window that ever shows a
// terminal is 100 KB of compile work per window that most operators never use.
// It is also not left out of the document, which is what the previous build
// did: the script was emitted only when `--renderer webgl` was on the command
// line, so the Terminal settings row could not work at all without a flag the
// row never mentions. The source now always ships as unparsed `text/plain` and
// is compiled here on demand, so the setting alone is enough.
function loadWebgl() {
  if (typeof WebglAddon === "object" || typeof WebglAddon === "function") return true;
  const el = document.getElementById("rg-vendor-webgl");
  if (!el || !el.textContent) return false;
  try {
    (0, eval)(el.textContent);
  } catch (e) {
    report(`the WebGL renderer failed to load: ${e}`);
    return false;
  }
  el.textContent = "";
  return typeof WebglAddon !== "undefined";
}

/// Build the terminal if it does not exist yet.
///
/// Returns whether one is available afterwards, so a caller can skip work
/// rather than write into nothing. Mount failure is reported once by `mount`
/// itself; this only has to avoid retrying it on every command.
function ensureTerm() {
  if (state.term) return true;
  if (state.termMountFailed) return false;
  const el = document.getElementById("rg-term");
  if (!el) return false;
  if (!loadVendor()) {
    state.termMountFailed = true;
    return false;
  }
  try {
    mount(el);
  } catch (e) {
    state.termMountFailed = true;
    report(`terminal mount failed: ${e}`);
    return false;
  }
  return !!state.term;
}

function handle(cmd) {
  switch (cmd.cmd) {
    case "focusDom":
      focusDom(cmd.selector);
      break;
    case "clipboard":
      copyText(cmd.text);
      break;
    default:
      report(`unknown bridge command ${cmd.cmd}`);
  }
}

// --------------------------------------------------------------------------
// Command loop. Never returns: returning would close the channel to Rust.
// --------------------------------------------------------------------------

// The container exists from the first paint, but a WINDOW WITH NO SESSION
// FOCUSED does not need a terminal in it. Building one eagerly cost 4.15 MB of
// WebProcess memory per window, measured as a controlled pair over twenty
// windows: 45.0 MB each with the Terminal constructed against 40.8 MB without,
// 81.8 MB across the set. Most windows in a twenty-window session are showing
// a sidebar and an empty pane, and were each paying for an xterm instance,
// a fit addon and a renderer that nothing had written to.
//
// So we wait for the container and then stop. `ensureTerm` builds the terminal
// the first time a pane op actually needs one.
const ready = waitForContainer();

// Nothing on the eval channel waits for the container any more: `focusDom`
// takes a selector that may name anything in the document, and `clipboard`
// touches no element at all. The pane pump is the one thing that needs a
// terminal, and it awaits `ready` itself.
panePump();

for (;;) {
  const cmd = await dioxus.recv();
  try {
    handle(cmd);
  } catch (e) {
    report(`bridge command ${cmd && cmd.cmd} failed: ${e}`);
  }
}
