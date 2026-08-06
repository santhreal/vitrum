# The goal

One sentence: **vitrum is the terminal you run twenty coding agents in, and it
has to be the best-looking and the fastest thing in its category at the same
time.**

Nothing here is aspirational. Every line is either measured today or has a
number attached that says when it is met.

---

## Definition of done, per screen

A screen is finished when all four hold, at 1.0x and 1.5x, at the 224px sidebar
floor, at the 3rem collapsed rail, and with real content loaded (a 60-character
title, a `wip/very-long-feature-branch`, a filter query filling the field):

1. **One grid.** Every spatial value a 4px multiple. Every left edge on one
   number. Constant vertical pitch inside a list.
2. **Zero overlap.** Not unlikely. Zero.
3. **No dead controls.** Anything that looks interactive is interactive.
4. **No invented data.** No fixtures, no demo sessions, no placeholder copy, and
   no confident answer where the platform cannot determine one.

Spacing and overlap are P0 functional bugs, not polish. Proximity is the only
signal telling a reader what belongs to what; break it and the interface is
unparseable, not merely ugly.

### Measured, on the running binary

Every condition above, driven with a real pointer on the WM-managed display
rather than reasoned about. Numbers are physical pixels unless marked CSS.

| condition | measurement | result |
|---|---|---|
| 1.0x row grid | pitch 76 x7, heights 68 | constant, 4px multiples |
| 1.5x row grid | pitch 114 x6, heights 102 | exactly 1.5x of 76/68 |
| 224px floor, 1.0x | seam 224 after dragging the splitter to x=60 | exact |
| 224px floor, 1.5x | seam 336 physical = 224.0 CSS | exact |
| 3rem collapsed rail | rail 48px, chip heights 32, pitch 40 | exact, 0 ink past the seam |
| 60-character title | truncates to "refactor the daemo..." at the floor | no overflow |
| `wip/very-long-feature-branch` | truncates to "very-lon..." at the floor | no overflow |
| filter query filling the field | "bash number" typed in full | no dropped characters |

**Settings states, not just defaults.** The audit above was run entirely at the
shipped defaults for a long time, and that was the biggest hole in it: two
defects were sitting in states a user reaches with one click. Every setting
that changes rendering has now been switched and measured.

| state | measurement | result |
|---|---|---|
| Density Comfortable | pitch 76, height 68, gap 8 | 4px |
| Density Compact | pitch 68, height 64, gap 4 | 4px, **was 66/66/0** |
| Text scale 125% | pitch 95, height 85 | exactly x1.25 of 76/68 |
| Dense rows (slim variant) | pitch 52, height 44, gap 8 | 4px |
| Branch, time, status word all off | pitch 76, height 68, gap 8 | 4px, icon remains as documented |
| Theme Light | terminal `#ffffff` | **was `#08080a`, could never be light** |

The two in bold were real and are fixed. Neither was reachable by looking
harder at the default state, and neither would have been found by reading the
code: Compact's tokens look plausible until you multiply them out, and the
theme bug is one wrong element in a `getComputedStyle` call.

Light theme was then checked on more than the one screen the fix was found on:
sidebar and rows, terminal, context menu, shortcuts overlay, new-session
dialog, settings sheet, and the error flash. All readable, no contrast
failures, and the chord-column fix holds there too. Light is where a colour
that was fine on dark disappears, so it needs the same per-surface pass the
default theme got.

The compound worst case was then checked directly, because states that each
pass alone can still collide together: **Compact density, at the 224px floor,
with "Approval"**, the longest status word, on a row that also carries a
branch and a timestamp. Seam 224, pitch 68, height 64, gap 4, and on the
densest text line the four groups sit at 32-55, 87-121, 133-135 and 148-189
with a minimum 12px between them and 17px of card padding to spare. Compact
tightens the insets and the floor is the narrowest the panel goes, so if
anything collided it would be here.

**Watch the splitter when you re-measure this.** An earlier note in this file
recorded the 1.5x floor as "seam 422". That is 281 CSS px, which is 22% of a
1280 CSS window: the DEFAULT width, not the floor. The drag had missed the
resizer and selected text in the terminal pane instead. The resizer's hit area
sits just INSIDE the seam, at x=419 for a 422 seam. Grab it there, and the
floor resolves to 336 physical.

The filter's promise is checked the same way. "Titles, commands, directories
and branches are all searched" is only honest if all four match, so each was
driven with a term the others cannot satisfy: `software` appears in cwd and in
no title, `main` appears in the branch and in no title, command or path. Both
matched every row. `zzzz` matches none and the sidebar says so by name while
the header still reads the true session count, which is the distinction
between a failed search and an empty server.

**Scanning Rust source for class names: anchor on a token, never on quote
pairing.** Two guards here read the markup out of `.rs` files, because the
class strings live in `rsx!` and have no runtime hook. The one that works,
`no_ui_module_emits_an_unpainted_class`, searches for the literal `class: "`
and reads to the next quote. The obvious alternative, a regex over all
double-quoted literals, is WRONG on this codebase and produced a confident
list of 180 "dead" classes including `rg-sidebar` and `rg-titlebar`. Pairing
quotes left to right desynchronises on the first literal it cannot match,
`"\u{00d7}"`, and on any `"` inside a comment, after which every "literal" is
the text BETWEEN strings. A second attempt that handled escapes still missed
`rg-badge`, which is emitted at `inbox.rs:527`.

The direction that matters is guarded: a class in the markup with no rule
renders unpainted and is caught. The reverse, a rule no markup emits, is dead
weight rather than a defect, and no scan here is trustworthy enough to delete
rules on. Anything that wants to do that needs a real parser, not a regex.

### The operator loop, driven through the UI

"No dead controls" was being checked by counting handlers: 375 class emissions
guarded, 80 interactive elements all wired. That proves a control is CONNECTED,
not that it DOES the thing. Every session in this audit had been created over
the wire, so the product's own controls had never been used. Driven for real:

| action | path | result |
|---|---|---|
| Create | dialog, Launch | session runs, cwd honoured, project derived from the directory |
| Rename | dialog, Rename | title changes in the row, the tab AND the titlebar |
| Snooze | context menu, In 1 hour | row leaves Active for a Snoozed band; flash reads "Snoozed 1 until 12:44" |
| Duplicate | context menu | new session, no dialog |
| Terminate | Ctrl+Shift+X | arms a named prompt, then kills the child |
| Copy path | context menu | the real X clipboard changes; flash names what was copied |
| Text scale | settings, Appearance | 125% moves row pitch 76 to 95 and height 68 to 85, both exactly x1.25 |

Four things worth keeping from that:

**"Scales the whole shell, not just type" is literally true, and now checked.**
That caption is the kind of claim that is usually half right, with the type
growing and the boxes staying put. Measured: at 125% the sidebar row pitch goes
76 to 95 and the row height 68 to 85, both exactly x1.25, and the settings
sheet, its nav, the titlebar and every inset grow with them. Divide back out
and the authored values are still 76 and 68, so the 4px grid holds in authored
units at a third scale beyond the 1.0x and 1.5x the objective names.

**The clipboard really is written, and the report is honest.** Checked against
the system selection, not the app's own word for it: a sentinel was placed on
the clipboard with `xclip`, Copy path was clicked, and the selection came back
51 bytes long and starting with `/`, which is exactly the session's absolute
cwd. The flash then names the string it copied. That matters because
`bootstrap.js` has two clipboard paths, the async API and an `execCommand`
fallback, and reports `ok` from whichever ran rather than assuming success.


**The terminate confirmation is right.** One press does not kill anything. It
puts "Terminate bash? Its child process is killed and there is no undo." in the
flash strip with Terminate and Dismiss, and a second press confirms. It is not
a modal, deliberately, because a fourth layer would compete for Escape with the
three that already exist. Sessions that have already exited skip the prompt, so
the prompt never becomes something people press through.

**Do not audit a control by cropping to the sidebar.** Ctrl+Shift+X was nearly
written up here as a dead shortcut, on a screenshot cropped to the session
list. The confirmation was on screen the whole time, in the flash strip above
the terminal. Look at the whole window before concluding nothing happened.

### One product question, found the same way

Every session in this audit was created over the wire. The first time the
**New session dialog** was actually driven end to end, it worked: the cwd
defaulted to `$HOME`, the command field offered `/bin/bash - login shell` from
a live PATH scan, Launch created the session, the daemon derived the project
name from the directory, and the row came up Ready. Typing in it runs
commands, and the prompt confirms the requested cwd was honoured.

It does NOT open the session it just created. You land on "No session
focused" with a new row in the sidebar, and have to click it.

That is not a violation of the four tests, which is why nothing here was
changed: Launch is interactive and does exactly what it says. `SessionCreated`
is broadcast to every window, so focusing on receipt would be wrong for the
other nineteen, and nothing currently records that THIS window is the one that
asked (`main.rs:2002-2014` sends and dismisses). `SPEC.md` states no
requirement either way.

But "Launch" that leaves you nowhere is worth a decision, and the fix is small:
correlate the request with the reply in the requesting window and open it.

---

## The numbers

| Axis | Today | Target | Status |
|---|---|---|---|
| Idle CPU, client | 0.0292% with twenty windows open | at or below 0.055% | **holds** |
| Idle CPU, competitor | 3.716% on an empty screen | beat it by 50x+ | holds at 127x |
| 20 windows open, sessions available | 917.1 MB | under 1 GB | **holds** |
| 20 windows each showing a session | **398.0 MB** | under 1 GB | **holds, by 626 MB** |
| One `SessionUpdated`, Rust model | **0.005 ms** one window, 0.105 ms across twenty | not the bottleneck | see below |
| One `SessionUpdated`, end to end | 16-20 ms client-side | under 16 ms | open, and NOT closable in Rust |
| Daemon at rest | zero syscalls, 20 sessions | unchanged | holds |
| Binary size | 9.6 MB | stays under 15 MB | holds, against a 151 MB competitor |

**The `SessionUpdated` row was misattributed for its whole life in this file.**
It read "16-20 ms client-side" next to a target of 16 ms, which invited every
reader to go looking in `state.rs`. Measured properly, twenty sessions over
eight repos with three unregistered directories and real paths on disk so
`realpath` takes the branch production takes, the Rust half of absorbing one
update was **0.051 ms**, which is 0.3% of the figure. It is now 0.005 ms after
the incremental derivation work, and `tree()` alone went 62.2 us to 5.8 us, but
that improvement is 0.3% of a number nobody can feel.

SPEC 9.4 already recorded where the time actually goes: **63% WebKit
style, layout and paint, 29% Dioxus VDOM diffing**, against 0.54 us for the
model. So this row cannot be closed from Rust at all, and an agent sent to
close it correctly reported that rather than optimising the 0.3% and claiming
the row. The lesson is the same one the memory section learned twice: a number
with no decomposition invites work on whichever part is easiest to reach.

### The 1 GB target, measured

This was the last failing criterion and it is now met with room to spare. The
number was wrong in this file four separate times before it was right, so the
history is kept below rather than tidied away.

Twenty windows, real launches on the real WM-managed display, twenty distinct
sessions, one per window, PSS across the whole process tree:

| scenario | PSS | web processes | under 1 GB |
|---|---|---|---|
| 20 windows each showing a session, **before** | 1101.0 MB | 20 | no, by 77 MB |
| 20 windows each showing a session, **now** | **398.0 MB** | **1** | **yes, by 626 MB** |

**What changed is that all twenty windows now share one `WebKitWebProcess`.**
WebKitGTK gives every webview its own web process, and each one carries a
complete engine before our page contributes a byte: twenty of them were 599 MB
of the 1101. WebKit's own answer is
`webkit_web_view_new_with_related_view`, which builds a view inside an existing
view's process instead of starting another. wry already exposes it as
`WebViewBuilderExtUnix::with_related_view`, so this needed no patch to wry,
only for the vendored `dioxus-desktop` to hold a relation target. See
`LIVE_WEBKIT_VIEWS` in `vendor/src/webview.rs`.

Measured in isolation first, twenty views of our real page, thirteen
stylesheets and a live terminal with 400 lines in it:

| twenty views | total | web processes |
|---|---|---|
| independent, as upstream builds them | 1008.0 MB | 20 at 45.5 MB |
| related to one another | **403.4 MB** | **1 at 270.6 MB** |

The cost is shared fate: one web process dying takes every window with it
rather than one. That is the trade this API exists to make and the same bargain
every tabbed browser already makes.

**The relation target has to be alive, which the first version got wrong.** It
held one handle to the first view forever, reasoning that a GObject reference
keeps it valid. Measured: close the first window, open another, and WebKit
declines to reuse the dead view's process and starts a second. Twenty windows
came back 539.6 MB instead of 399.9, and left alone it leaks one process per
open-and-close generation. The fix keeps every view and picks the first whose
widget still has a parent. Verified across a generation: twenty windows one
process 395.6 MB, close three including the target 394.2 MB still one process,
open two more 396.3 MB still one process.

**Everything else still holds on the shared renderer**, which matters because
it is a different rendering topology, not just a smaller number. Re-verified on
the shared-process binary: card pitch 76, heights 68, gaps 8, all 4px; the
sidebar seam; all five status pills including Failed with its red rail and the
Done badge in row 2 column 2; PTY output arriving in a window that is not the
first; OSC 7373 driving a pill live; and idle CPU 0.0292% over 240 seconds with
**twenty** windows open rather than one, with zero memory drift across those
four minutes.

Re-measured at 1.5x on the shared-process binary, because a renderer change
invalidates a scaling result taken on the old one: pitch **114**, card height
**102**, gap **12**, exactly 76 / 68 / 8 multiplied by 1.5 with nothing
rounded. In a 1382 px window at 150% the sidebar lands on its **224 CSS floor**
(336 physical), so that capture is the 1.5x case and the floor case at once.

Two harness traps cost time here and are worth writing down. The scale steps
are `[80, 90, 100, 110, 125, 150, 175, 200]`, so one step down from 100% is
**110%, not 125%**, and an off-by-one in the keystroke count produced 84 / 75,
which is a real and correct 1.10x that looks like a failed 1.25x. And a probe
column fixed at x=30 reads the card **4 px short** at 150%, because the corner
radius scales too and the column now cuts the arc; the pitch is unaffected
since both edges move together. Confirm the dropdown reads the value you
intended before believing any measurement taken after it.

**What the earlier versions of this section got wrong**, kept because each
error had a cause worth remembering: "984.7 MB, MET" was measured with nothing
focused; "1059.2 MB" had fewer sessions than windows, so several windows showed
the same session; "1101.0 MB" was right for its workload but was presented as a
ceiling when the workload moves it between 1068 and 1154; and the conclusion
drawn from all of them, that only a Dioxus Native/Blitz migration could close
the gap, was **wrong**. The gap was closed by nine lines of vendored Rust
calling an API WebKit has shipped for years. The lesson is not about WebKit. It
is that "the platform's floor" was asserted three times about a number that had
a platform-supported answer nobody had looked for.

### The shared WebContext: implemented, proven, and not enough

`vendor/` carries a patched `dioxus-desktop` 0.7.10. Upstream builds a fresh
`WebContext` per webview, and on Linux each one starts its own
`WebKitNetworkProcess`. Twenty windows ran twenty of them.

Two problems had to be solved together, which is why upstream has not:

1. wry registers custom protocols PER CONTEXT, so `dioxus://` can be registered
   once. A second builder on a shared context panics with
   `DuplicateCustomProtocol`.
2. The single surviving handler must still serve each webview its OWN page,
   because `protocol::module_loader` inlines that webview's edit-queue path and
   server key into its index.html, and `__events` routes DOM events into one
   specific VirtualDom. A naive shared handler wires window 2 to window 1.

The fix: `wry::WebViewId` is `&str` and `WebViewBuilder::with_id` sets it, so
every webview gets a known id and registers its route BEFORE it is built. There
is no window in which a request arrives for an unknown id, which is what makes
it sound rather than a race that usually wins.

**Proven independent, not assumed.** Two windows on two interactive shells:
`echo WINDOW_ONE_AAA` in one and `echo WINDOW_TWO_BBB` in the other, each
rendering only its own output, each tab showing its own session. That is the
exact cross-talk this design could have caused.

Worth **71 MB** at twenty windows: 1132.6 to 1061.7 (three runs, spread 2.5).
Less than the 161 MB the network processes cost, because PSS re-attributes
shared pages onto the web processes once the context is common.

### Levers measured and rejected

Recorded so nobody spends the afternoon again:

- **Scrollback does cost, and the entry that used to sit here was wrong twice
  over.** It first said `scrollback: 0` measured 1206.1 MB against 1132.6, "a
  73 MB regression", which was confound. It was then replaced with "not a
  lever in either direction", on a harness run at 0, 200 and 1000 that came
  back 39.19 / 39.14 / 39.20. That was **also** confound, in the opposite
  direction: xterm allocates buffer lines as they are written, the harness had
  written three, so all three runs measured an empty buffer and the setting
  never applied. Measured properly, twenty windows go 1068.1 MB with nothing
  printed, 1101.0 with ordinary startup output, and 1153.6 with 400 lines
  each. Roughly 10 KB per line per window. It stays at 1000 because at the
  shipped workload the buffers are far from full, so capping it buys almost
  nothing and costs the operator their scroll history.
- `WEBKIT_USE_SINGLE_WEB_PROCESS` is ignored by modern WebKitGTK: measured, no
  change.
- Window size at launch does not matter: 0.2 MB between 800x600 and 1382x800.
  **Resizing after launch does, and in the wrong direction:** twenty windows
  taken from 1382x800 down to 1000x640 measured 73.7 MB *worse*, because
  WebKit does not return what a reflow allocated.
- **Turning off the WebKit features we never use saves nothing.** wry forces
  `set_enable_webgl(true)`, `set_enable_webaudio(true)` and
  `set_enable_page_cache(true)` on every webview it builds, with no way to opt
  out (`wry-0.53.5/src/webkitgtk/mod.rs:411-450`). vitrum uses the DOM
  renderer, plays no audio, and is a single page that never navigates, so all
  three read like free wins. Measured in the harness with those plus
  `html5_database`, `html5_local_storage`, `offline_web_application_cache` and
  `smooth_scrolling` all disabled: **39.38 and 39.41 MB per web process
  against 39.47 and 39.39 for the defaults.** The gap between the two modes is
  smaller than the spread within either. WebKit allocates these lazily, so
  enabling a feature costs nothing until something uses it. Do not patch the
  vendored wry for this.
- **Sharing `DaemonState` between windows is not a lever, however wrong it
  looks.** `state.rs` says it outright: a `Signal<UiState>` belongs to exactly
  one VirtualDom, so each window holds a WHOLE `UiState` and its own socket.
  Twenty copies of the session list reads like a twenty-fold waste and is the
  obvious thing to go after in the client's 118 MB. Do the arithmetic first:
  `SessionInfo` is a handful of short strings, so twenty sessions is single-
  digit kilobytes and twenty windows of it is about 120 KB. It is four orders
  of magnitude away from the 77 MB. The host's 5.9 MB per window is the
  VirtualDom, the tao/wry window, and the socket, not the model.
- The WebGL addon ships only when WebGL is the renderer, worth 1.3 MB. Kept
  because 100 KB of dead JavaScript per window is wrong regardless, not because
  it is a lever.
- **Minifying the CSS whitespace is not worth its risk.** `strip_css` already
  removes comments, which took the inlined sheets from 361,734 raw bytes to
  110,123 and was measured at 1.4 MB per window. Collapsing the remaining
  whitespace and the spaces around `{}:;,>` gets to 93,815 bytes, only 14.8%
  further. At the ratio the comment strip actually achieved, roughly 8.5 bytes
  of process memory per source byte, that is about 0.14 MB per window and
  under 3 MB at twenty; a generous linear projection still only reaches 6 MB.
  Against a 77 MB gap that buys nothing, and a whitespace minifier running
  over thirteen stylesheets is a real chance of a subtle selector or `calc()`
  break for it. Comments were the win here; whitespace is not.
- **The WebGL renderer is heavier, not lighter.** 1082.6 MB against 1059.2 for
  DOM at twenty focused windows. A canvas instead of thousands of DOM nodes
  sounded like it should win; it does not, and it would also cost idle CPU,
  which is the one axis this product cannot spend.
- **Releasing the terminal when the window loses focus makes it WORSE.**
  Built end to end and measured, not reasoned about. tao's
  `WindowEvent::Focused` was forwarded into the page from the patched
  `dioxus-desktop` (a webview under tao never fires its own DOM `blur`/`focus`
  for the toplevel, so the obvious `window.addEventListener("blur")` reports
  nothing at all: verified, zero events). On blur the client detached and
  called `term.dispose()`; on focus `reconcile` re-attached and re-requested
  the backfill. Correctness was fine, and a blur/refocus round trip returned
  the full scrollback intact.

  It measured **1117.5 MB**, higher than every baseline in the table above,
  with per-WebProcess at 50.6 MB. Be precise about the comparison: that run
  cycled a couple of sessions across the twenty windows rather than giving
  each its own, so it is not a controlled pair with the 1101.0 MB figure, and
  the "58 MB regression" first written here overstated it. What is not in
  doubt is the direction and the verdict. The change was predicted to SAVE
  about 112 MB by leaving nineteen of twenty windows without a terminal, and
  it came in ABOVE every measured baseline instead.

  **`dispose()` does not return memory to the OS.** WebKit's heap does not
  shrink, so building a terminal and then destroying it costs strictly more
  than building it once. The gap between "never built" and "built" is only
  available by never building, which is what the existing build-on-demand
  already does.

  The whole change was reverted, including the vendor hook. Do not retry it in
  another shape: deferring the disposal behind a timer cannot help, because the
  cost is the allocation, not the lifetime, and it would spend the idle-CPU
  budget as well.

### What is left, after the disposal lever was disproved

The plan recorded here used to be "a background window disposes its terminal
and rebuilds it on focus, freeing about 9 MB for nineteen of twenty windows",
deferred only because it looked like a product tradeoff. It was then built and
measured, and the premise was simply false: disposal does not give the memory
back. The tradeoff never had to be decided, because there was nothing to buy.

That removes disposal as a lever. It does not, as this file went on to claim
for several revisions, mean the rest is WebKit's.

**"It is WebKit's floor" was wrong, and this file said it for several
revisions.** The claim rested on reading 961 MB of WebProcess PSS as though it
were WebKit's own cost. It is not: that number is WebKit's cost WITH our page
already loaded into all twenty processes. Nobody had measured a bare one.

So it was measured. `/tmp/webkit-floor` is twenty tao windows, each with one
wry webview (the same wry 0.53.5 and tao 0.34.8 this app pins) showing a blank
document, PSS across the tree:

| what each web process holds | MB each | x20 |
|---|---|---|
| blank document, no application at all | 29.94 | 599 |
| + our 13 stylesheets, 113,562 bytes inlined | 32.07 | 641 |
| + the shell's DOM, `bootstrap.js` and the Dioxus runtime | 34.19 | 684 |
| + a live xterm.js terminal | 48.2 | 964 |

Read down that column. **WebKit is 599 MB of the 1101. The other 502 MB is
ours**, and the single largest item in it is the terminal at 14.0 MB per
window, or 280 MB across twenty. Our CSS is 2.1 MB per window and the whole
rest of the shell is another 2.1.

That reframes the 77 MB: it is not carved out of an immovable platform floor,
it comes out of the 280 MB the terminal costs across twenty windows.

**And the terminal's cost is mostly not its contents, but the number moves
with the workload.** Twenty windows were run with every session on
`sleep 600`, which prints nothing, so every terminal was open, attached and
sized with an empty buffer. Then twice more with real output:

| twenty windows, each showing its own session | total | per web process |
|---|---|---|
| session printed nothing at all | **1068.1 MB** | 46.47 |
| ordinary shell startup, a handful of lines | **1101.0 MB** | 48.2 |
| 400 lines of output each | **1153.6 MB** | 50.55 |

Two things follow. An entirely empty terminal still leaves us **44 MB over**,
so emptying the buffer cannot close the gap and what costs the memory is that
the terminal exists. But 1101.0 is **not a ceiling**: a busy agent measured
1153.6, and the headline number should be read as one workload, not a bound.

This also corrects an earlier entry in this file. It said scrollback was "not
a lever in either direction" on the strength of a harness run at 0, 200 and
1000 lines that came back 39.19 / 39.14 / 39.20. That test was **confounded**:
xterm allocates its buffer lazily as lines arrive, and the harness had written
three lines, so all three runs measured the same empty buffer and the setting
never came into play. The table above is the honest version. Scrollback does
cost, roughly 10 KB per line per window, but capping it below what an agent
actually prints is what would have to be given up, and at the shipped workload
the buffers are nowhere near full, so it still buys nothing against the 77 MB.

**Making the windows smaller makes it worse.** The twenty windows above open
at 1382x800. Resized down to 1000x640 and left to settle for 45 seconds, the
same twenty processes measured **1227.3 MB, up 73.7 MB**. WebKit does not hand
back what a reflow allocated, which is the same behaviour that made
`dispose()` on blur measure worse than doing nothing. Window geometry is not a
memory lever, and anyone re-measuring should size the windows once at launch
rather than resizing into position.

**Where that 280 MB actually goes, measured in the same harness.** The first
guess written here was "compiling xterm.js into twenty JavaScript heaps". That
guess was wrong too, and cheap to disprove. Each stage below is the same
twenty-window run with one thing added:

| each web process holds | MB each | delta |
|---|---|---|
| blank document | 30.14 | |
| + xterm.js and the fit addon compiled, no Terminal | 31.23 | **+1.09** |
| + `new Terminal(...)`, constructed but never opened | 35.18 | **+3.95** |
| + `.open()`, `.fit()` and a few lines written, 1300x700 | 39.10 | **+3.92** |

Compiling the library is 1.09 MB. It is not the problem. **The Terminal
INSTANCE is, at about 8 MB**, split roughly evenly between constructing the
object graph and opening it onto the DOM.

And that 8 MB does not respond to configuration. Two knobs were tested
directly, because both had been guessed at before:

- **Scrollback is real but small at the shipped workload.** The 0 / 200 / 1000
  harness run that produced 39.19 / 39.14 / 39.20 measured an empty buffer,
  because xterm allocates lines as they arrive and only three had been
  written. In the real app the buffer content is worth 33 MB across twenty
  windows at ordinary output and 85 MB at 400 lines each. Keep it at 1000.
- **Terminal size barely matters.** A 200x120 terminal measured 38.59 against
  39.10 for 1300x700. Half a megabyte between a postage stamp and a full pane.

So the per-instance cost is xterm.js's fixed object graph, and it is the same
whatever we set. There is no configuration left to find, and nobody should
spend another afternoon looking.

That leaves exactly one shape of fix for the terminal itself: **fewer terminal
instances**. That is still true and still unsolved. It is no longer the thing
standing between this build and the target, because the 599 MB next to it
turned out not to be a floor at all. See below.

`vitrum-grid` is NOT a component that can be wired into the client as it
stands. Its own header says so: "The vitrum client renders terminals with
xterm.js inside a webview today. The plan is to move the client to Dioxus
Native, which paints through Blitz, and Blitz has no JavaScript engine, so
xterm.js cannot come along." It is a `wgpu` renderer for a host that hands it
a device. There is no such host in a WebKitGTK webview, so "wire up the crate
we already have" was never an option and should not be written here again.

The Dioxus Native / Blitz migration remains a real plan with a real payoff,
and this codebase is still prepared for it: every stylesheet is authored
Blitz-safe (no `position: fixed`, no `:has()`, no `@container`, no nesting, no
`color-mix()`, no `oklch()`, recorded as SPEC 14.14), `15-rows.css` documents a
flex fallback for its one grid, and `vitrum-grid` is the terminal renderer for
that path. **But it is no longer required to meet the memory target, and this
file was wrong to say it was.**

### The claim that twenty web processes are unavoidable was false

This section used to end: "One web process per webview is not negotiable.
Twenty webviews will cost 599 MB whatever we do." That was asserted three
times, and it was wrong.

What is true is narrower. `WEBKIT_PROCESS_MODEL_SHARED_SECONDARY_PROCESS` is
genuinely dead: WebKitGTK 2.52.3 still exports the enum, but the shipped
`libwebkit2gtk-4.1.so.0` carries its own diagnostic string,

> `WEBKIT_PROCESS_MODEL_SHARED_SECONDARY_PROCESS is deprecated and has no effect`

and `webkitgtk-6.0` no longer exposes `set_process_model`. That is also why
`WEBKIT_USE_SINGLE_WEB_PROCESS` measured no change: it was not being ignored by
accident.

The mistake was concluding from a dead *global knob* that the *capability* was
gone. It is not. WebKit replaced the process model with a per-view relation:
`webkit_web_view_new_with_related_view` puts a new view in an existing view's
process, one pair at a time, which is exactly what a browser does for tabs of
the same site. It is not deprecated, it is the supported path, and wry has
exposed it the whole time as `WebViewBuilderExtUnix::with_related_view`.

Twenty windows now run in **one** web process and the application measures
398.0 MB. The 599 MB was never a floor; it was twenty copies of a thing WebKit
is happy to share when asked properly.

The general lesson, which cost this project several sessions: a removed
configuration switch is evidence that the switch is gone, not that the
behaviour is unreachable. Before writing "the platform will not do X" a third
time, search the platform's API for X.

The same harness also confirms the shared `WebContext` was worth what it
claimed. Unshared, twenty webviews spawn **twenty** network processes costing
164.4 MB; vitrum runs one, at 15.1 MB. That change is already in and is worth
149 MB.

It was 1263.9 MB when this file first recorded it, and fifteen windows was all
that fit. Three changes moved it, each measured as a controlled pair rather
than estimated, because the first estimate was wrong by a factor of twenty:

| change | per window | at twenty |
|---|---|---|
| Terminal built on demand, not at startup | 4.15 MB | 82 MB |
| xterm.js and its addons shipped unparsed | 5.0 MB | 105 MB |
| CSS comments stripped before inlining | 1.4 MB | 37 MB |

- **The terminal is built on first use.** Every window used to construct an
  xterm instance, a fit addon and a renderer whether or not a session was ever
  focused, and most windows in a twenty-window session show a sidebar and an
  empty pane. `bootstrap.js` now mounts on the first `focus`, `backfill` or
  `banner`.
- **The vendored bundles ship as `type="text/plain"` and are evaluated on
  demand.** The cost was never the bytes, it was COMPILING 390 KB of
  JavaScript in every window. `loadVendor` evaluates them once and clears the
  element text, which releases the source as well.
- **CSS comments are stripped before inlining.** 70% of the 410 KB in every
  webview was comment: 409,530 bytes becomes 122,894. The comments are
  load-bearing for the SOURCE and worth nothing to the engine, which discards
  them during parse.

The WebGL addon is also emitted only when WebGL is the renderer, which saved
1.3 MB: not a lever, but 100 KB of dead JavaScript in every window was wrong
regardless.

What remains is WebKit's floor: **35.7 MB per web process**, down from 47.4 but
still fixed, and there is one per window. Sharing a single `WebContext` would
recover a further 9.1 MB per window, but it needs an upstream `dioxus-desktop`
change; see `Cargo.toml` for why a local patch is unsafe. It is no longer
needed to make the target.

Levers tried that do NOT work: `WEBKIT_USE_SINGLE_WEB_PROCESS` is ignored by
modern WebKitGTK, and a smaller window does not help (0.2 MB between 800x600
and 1382x800).

Idle CPU is the axis this product unambiguously wins: **0.0333% measured over
180 s** on the final binary, against a 0.055% budget and a competitor's 3.716%
on an empty screen. The deferred loading above improved it from 0.0500%.
Nothing ships that costs it. Zero infinite animation, ever.

## The settings audit

The premise put to this audit was that the settings sheet might be decorative.
It is not. All 47 settings and controls were traced from the control site
through persistence to a non-test read site that changes rendered markup, an
injected script, a persisted document, or a bridge command:

| class | count |
|---|---|
| WIRED | 44 |
| PERSISTED-ONLY | 0 |
| DEAD | 0 |
| MISSING | 0 |
| read-only diagnostics, not settings | 2 |
| machine field (format version) | 1 |

Every field of `Settings`, `TerminalPrefs`, `NotifyPrefs`, `KeyboardPrefs`,
`DispositionPolicy` and `SectionVisibility` has a control. There is no hidden
knob and nothing is saved that nothing reads.

**The premise failed, and seven specific defects took its place.** Four are
fixed; the evidence for each is the file that now behaves differently.

**A control that rendered and did nothing, which is the thing this project
bans outright.** `Notifier::set_activation_handler` was never called anywhere
in `app/`, yet `Notification::dbus_args` shipped `actions: ["default", "Show"]`
and passed them into the `Notify` call, so **every Linux notification drew a
"Show" button that could not do anything**. Worse than unrouted: on Linux the
`ActionInvoked` match rule is installed only by `set_activation_handler`, so
the click signal was never even subscribed to. The settings caption told the
operator that clicking a notification focuses the session through the
`vitrum://session/<id>` deep link, which was false. The deep-link parser and
the in-app handoff were both real and tested the whole time; only this half was
missing. Now `activate_session` posts to the same mailbox a second launch uses,
so a click lands on exactly the code a browser link lands on, and the Linux
backend route-gates its actions so an unrouted build advertises no button at
all rather than a lying one.

**Scrollback could not do what its caption said.** The caption promised that
raising the setting was the only way to see further back. The backfill was a
hard-coded 64 KiB, so choosing "100,000 lines" grew the local xterm buffer a
hundredfold and retrieved **not one extra byte** of pre-attach history. The
budget is now `backfill_max_bytes(lines)`, and the four offered steps produce
64,000 / 320,000 / 1,280,000 / 2,097,152 bytes, strictly increasing, clamped at
both ends and capped well under the daemon's own per-session ring.

This caption has now been wrong **twice**, and the reason is worth keeping. The
guard written after the first correction asserted only that two retired phrases
were absent, so the second, differently-worded overstatement walked straight
past it. The replacement asserts the *relationship*: the caption may claim that
raising the setting shows more history only while every offered step really
does ask for a strictly larger budget, and the ceiling it quotes is the ceiling
the code enforces. **A guard that checks vocabulary cannot protect a claim
about behaviour.**

**Three things were persisted only by luck.** `sidebar_collapsed` and the whole
tab strip live in `WindowSnapshot`, which `save_prefs` writes and
`restore_window` reads, but not one of the five collapse toggles and not one
tab operation ever called `commit`, and the exit hooks wrote only
`windows.json`. Filing a session into another workspace or folder had the same
shape: the context menu mutated persisted state and only set a flash. Each
survived a restart when some unrelated control happened to commit afterwards,
and was silently discarded when nothing did. They now write at the two moments
geometry already uses, and immediately on a move.

The audit's closing observation is the sharpest thing in it: the round-trip
suite covered 16 settings, and the four fields it never pushed through
`encode_ui_state`/`parse_ui_state` were **exactly** the four with the defect.
That is not a coincidence, it is the mechanism.

**The landmine nobody had stepped on yet.** Adding one field to a persisted
struct nearly wiped every operator's arrangement. `WindowSnapshot` and
`Persisted` carry `serde(rename_all)` but NOT container-level `default`, and
they are the only two persisted structs that do not: `Settings`,
`TerminalPrefs`, `NotifyPrefs`, `KeyboardPrefs` and `Strip` all have it. So a
new REQUIRED field on `WindowSnapshot` makes every existing `ui.json` fail to
deserialize, `parse_ui_state` turns that into `UiStateLoad::Corrupt`,
`load_prefs` answers with defaults, and `restore_daemon` then writes those
defaults over the live state. Every workspace, every folder, every session
placement and every window layout, discarded on the first launch after an
upgrade, by one missing attribute.

It was caught before it was written, by an agent auditing something else who
read the derive rather than assuming it matched its five siblings. The fix is
`#[serde(default)]` on the CONTAINER for both, not on the new field, because
fixing the field leaves the mine armed for whoever adds the next one. **A
persisted struct without a container default is a one-field fuse.**

The test that protects it has to round-trip a REAL pre-upgrade document and
assert the arrangement survives with actual values. Asserting that parsing
returned `Ok` proves nothing here: the failure mode is a successful parse of
defaults.

### A guard must read its subject, not a proxy for it

This is the single most repeated defect in the project, and it took three
independent instances in two subsystems before anybody named it. A check
asserts something that CANNOT falsify the thing it is guarding, so the thing
ships wrong and the check stays green.

| the guard asserted | its actual subject | how it stayed green while the subject was false |
|---|---|---|
| the strings "on demand" and "before a request" are absent from the caption | the caption is true of the code | a third, differently worded overstatement walked straight past it, so the same caption shipped wrong TWICE |
| `probe_count == 0` after ten ticks | output re-arms the settle timer | a 150 ms scheduler stall flips the count without the subject being false |
| `--rg-row-gap:0rem` is present | rows are on the 4px grid | it asserted the defect, so fixing the grid would have failed the test |

The rule: **a guard is load-bearing only if it reads the subject and would fail
on a plausible wrong version of it.** Absence-of-string, count-is-zero and
`!is_empty()` all pass on the wrong answer.

Two things in this codebase are the shape to copy. The rewritten scrollback
guard asserts a RELATIONSHIP: every offered step must produce a strictly larger
backfill budget, and the ceiling the caption quotes must equal the constant the
code enforces. And the tab-width test reads all six lengths out of the
stylesheets that own them and closes to the pixel, so retuning any one of them
fails loudly instead of silently eating the session title.

**Mutation is the only way to find out which of your tests are decoration**,
and there is a sharp distinction inside it that took nine agents most of a day
to name: *proving a guard catches the mutation you thought of is confirmation.
Hunting for one it misses is a test.* Seven agents ran the first kind, reported
their suites as proven, then ran the second kind and found real holes in work
they had just certified. Not one hole would have been found by reading.

The escapes, because the shapes recur and each one looked fine on the page:

| what escaped | why the suite could not see it |
|---|---|
| `css.contains(".rg-foo")` | satisfied by `.rg-foo2`, so renaming a rule out from under live markup stayed green while the element rendered unstyled |
| `BOOTSTRAP_JS.contains("theme: termTheme(el)")` | satisfied by a MENTION, so commenting the read out and hardcoding the dark palette passes the guard written to stop exactly that |
| a length-conditioned bug at exactly ten bytes | no fixture fed ten bytes down that path |
| `.take(1)` on a render loop | every fixture held one hit in one session, so presence passed where COUNTING was needed |
| a coordinate shrunk WITHIN its box | structurally valid, distinct, unfilled, correctly classed, and visibly the wrong size |
| `wrapping_mul` for `saturating_mul` | the probe was `u32::MAX`, which wraps to a value that clamps identically; only `1 << 26` discriminates |
| `ActiveOrder::Urgency` for `Static` | the fixture gave every row one urgency to isolate the variable, which removed the only signal that could tell the comparators apart |
| an install moved into a function nobody calls | the guard searched a line RANGE, so a helper in that range satisfied it. This is the ticket's own defect, reappearing inside the guard written to prevent it |

Three generalisations fall out, and all three describe checks that read as
behavioural:

- **An assertion whose operands are all constants is a tautology in a test's
  clothes.** It passes every mutation of the code it names.
- **A fixture that holds everything constant to isolate one variable can hold
  constant the very thing the assertion needs to discriminate on.**
- **Presence is not counting.** `.take(1)` survives any check that asks whether
  a thing rendered rather than how many did.

Four techniques were worked out for doing this without touching the shared
tree, which matters because one agent mutated a live file and gave everyone
else three minutes of phantom failures in a file they did not own: a `#[path]`
scratch crate for anything needing the real module graph; generated-copy-per-
path for before/after pairs; in-memory string mutation for pure `include_str!`
guards, which writes nothing at any instant; and snapshot-the-function for a
pure `fn` whose callers you do not want to compile.

**Still open**: the "Stylesheet default" font option is inert on a live switch,
because the script emits a literal `fontFamily:null` and the applier guards
with `typeof === "string"` while `typeof null === "object"`, so one of eight
options behaves differently from the other seven and only until a restart.
`apply_live` runs `document::eval` in the calling window, so windows 2..N keep
the old terminal font, size, scrollback, renderer and keybindings while the
shared `Settings` updates every window's markup. And `sidebar_width` is
restored from `ui.json` while the flag deciding whether the operator chose it
is read from `windows.json`, so one number lives in two files and the flag
reads the one that is not authoritative.

---

## The standing loop

Build, launch on the real WM-managed `:1`, capture, **look at the image**,
measure the pixels, fix, capture again.

Two things that have already produced false conclusions here:

- The headless harness reports a **1018 CSS px document** whatever size the X
  window is. Trustworthy for colour and band boundaries. Useless for geometry.
- **A test count is not progress.** This project reached 1,692 passing tests
  while the running binary could not do most of what was asked. The question is
  never "do its tests pass", it is "does the running binary reach it".
- **Three flakes, all the same mistake.** Each was found by running the whole
  suite repeatedly rather than the test just touched, and each turned out to
  be a test asserting a PROXY for its subject instead of the subject:
  - `vitrum-overlap::live_watch` counted inotify events after a rename
    barrier. A rename inside one watched directory emits MOVED_FROM and
    MOVED_TO, and whether one or both have landed is genuinely timing
    dependent, so the barrier must have no opinion about the count.
  - `vitrum-server::geometry::a_disconnecting_window_releases_its_constraint`
    waited for "a line ending in CRLF". The child prints its size on SIGWINCH
    too, so the stale `20 90` from the earlier resize satisfied that, and the
    assertion ran before the restored size ever arrived. It now waits for the
    content it is about, `50 200`.
  - `vitrum::launch::no_autostart_leaves_a_dead_port_alone` ASSUMED its
    precondition. It needs a port with nothing listening, and no number can be
    reserved in that state: the instant the probe socket closes, the port is
    available to everything. Two attempts at a "safe" range failed because the
    race is not about which numbers are used. It now establishes the
    precondition and moves on when a port is taken, bounded at 16 attempts.

  If a test here is flaky, the question to ask first is not "how long should
  it wait" but "what is it actually asserting on".

---

## What is not built

Named by the user, never started, zero lines of code:

- Lazy-loaded in-terminal browser, opencode-style
- Plugin system with a public API, cmux-style
- Macros, including vim-style

Built and unreachable, which counts as not built:

- ~~File-collision detection (`vitrum-overlap`, 9,665 lines, no caller)~~
  **Closed 2026-08-04.** The 9,665-line crate was deleted and rebuilt at about a
  tenth the size inside the daemon (`crates/vitrum-server/src/overlap.rs`),
  wired to a marker on every contested sidebar row, and verified with two live
  agents writing one file. The rebuild found why the original could never have
  worked: it scanned for an open descriptor at `CLOSE_WRITE`, which is after
  the writer closed it by definition. Attribution now happens at `OPEN`.
- `vitrum-grid` and `vitrum-replay`: 15,382 orphan lines. `vitrum-grid` is a
  workspace member and compiles; it is unreachable **by design**, because it
  needs a `wgpu::Device` that WebKitGTK cannot provide. `vitrum-replay` is
  outside the workspace: its library compiles, its tests no longer do. See
  SPEC 12.2 for the measurement and the decision it needs.

**Two of these were closed, and both were closed by wiring rather than by
writing anything new**, which is the shape of this whole defect:

- **Cross-session scrollback search is now reachable.** The daemon had answered
  it correctly the entire time: a regex sweep across every retained buffer
  returns right offsets, context lines and an honest byte count, verified over
  the wire. The client received `ServerMsg::SearchResults` and threw it on the
  floor at `state.rs`, under a comment claiming the answer was "routed by the
  search overlay, which holds its own results signal". There was no overlay and
  no signal; the comment described a design nobody built. Ctrl+Shift+F opens it
  now, `Scope::Global`, so it works from inside a terminal pane. That chord was
  previously a second way to focus the sidebar filter, which Ctrl+K already
  does and is documented for.
- **Notifications now route their click.** `set_activation_handler` was never
  called from `app/`, while `dbus_args` shipped a `Show` action, so every Linux
  notification drew a button that could not do anything. It lands on the same
  code a `vitrum://session/<id>` link lands on.

Neither needed new capability. Both needed somebody to connect two halves that
were each finished and tested in isolation, and in both cases a comment in the
code asserted the connection already existed.

`vitrum-grid` is unreachable BY DESIGN, not by neglect, and should not be
deleted as dead weight. Its header states it plainly: the client draws
terminals with xterm.js in a webview today, the plan is Dioxus Native painting
through Blitz, Blitz has no JavaScript engine, and this crate is the
replacement renderer for that path. It cannot be wired into the current client
because a WebKitGTK webview has no `wgpu::Device` to hand it. It is the one
piece of the memory fix that is already written.

`SPEC.md` carries all 120 requirements with status and file:line evidence.

---

## The rule that protects all of it

**File ownership is absolute.** Two agents editing one file destroyed hours of
work twice in one day, and silently restored a defect the user had already
rejected. One file, one author. If you need a change elsewhere, ask the owner.
