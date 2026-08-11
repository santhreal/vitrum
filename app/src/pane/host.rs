//! The widget half of the pane.
//!
//! Everything in this file needs a toolkit, and nothing that does not need one
//! is here. It owns a `GtkDrawingArea`, the swapchain on that widget's own X
//! window, the input method context, the clipboard, the pointer, the wheel and
//! the frame clock. What each of those means is decided in the toolkit-free
//! modules beside it.
//!
//! # Where the pane sits
//!
//! Packed into the box the frame reserves for it, expanding and filling. Its
//! rectangle is therefore decided by the toolkit walking the widget tree, and
//! nothing computes it: a repaint of the titlebar, the sidebar or the bar
//! cannot move the terminal, because none of them is its parent.
//!
//! # The frame clock
//!
//! `add_tick_callback` fires once per compositor frame on the widget's own
//! frame clock. That is the only clock the pane has. Bytes arriving mark the
//! pacer and return; the tick decides whether the mark becomes a frame. A tick
//! that arrives while a frame is in flight is dropped rather than queued, so a
//! slow frame cannot turn into a growing backlog.
//!
//! # Reentrancy
//!
//! Every callback here runs on the GTK main thread, and every one of them
//! borrows the same `RefCell`. Two rules keep that sound and both are load
//! bearing: a borrow is never held across a call into GTK that can emit a
//! signal, and anything that has to call back into the shell is collected
//! while the borrow is held and dispatched after it is dropped.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use gtk::prelude::*;

use crate::WindowId;

use super::geometry::PaneRect;
use super::key::{Key, Named};
use super::mouse::{self, Action, Button, Position};
use super::pacing::{FrameLog, FrameStats, Pacer, Tick, WINDOW};
use super::paste;
use super::scroll::Viewport;
use super::select::{Mode as SelectMode, Point};
use super::session::PaneSession;
use super::surface::PaneSurface;
use super::theme::PaneTheme;
use super::{InputSink, PaneReport, ReportSink, keymode, theme_from};

/// How often the pointer is sampled while a drag is outside the pane.
///
/// Sixteen milliseconds is one frame at 60 Hz. Faster buys nothing, because
/// the scroll is not shown until the next frame anyway.
const AUTOSCROLL_MS: u32 = 16;

/// How many allocations the pane's widget has been given.
///
/// A counter and not a log, because the claim it defends is a number: the
/// pane resizes when its own allocation changes and at no other time. A
/// repaint of the sidebar, the titlebar, the bar or a dialog must leave this
/// standing still, and a drag of the divider must move it.
static ALLOCATIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many of those allocations changed the grid the child is writing into.
///
/// Strictly fewer than [`ALLOCATIONS`]: an allocation whose pixels divide
/// into the same cell count costs nothing beyond a frame, which is why a
/// one-pixel drag does not resize a pty.
static RESIZES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Allocations the pane's widget has been given in this process.
pub(crate) fn allocations() -> usize {
    ALLOCATIONS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Grid resizes the pane has performed in this process.
pub(crate) fn resizes() -> usize {
    RESIZES.load(std::sync::atomic::Ordering::Relaxed)
}

thread_local! {
    /// Every pane in this process, by the ordinal of the window it lives in.
    ///
    /// Thread-local because GTK is single-threaded and every one of these is
    /// created, used and dropped on the main loop. Keyed by the ordinal
    /// rather than by a toolkit window handle because the ordinal is the
    /// identity the rest of the program already uses for a window: geometry
    /// is remembered under it, the tray checks it, and it survives the
    /// toolkit underneath the window being replaced.
    static PANES: RefCell<HashMap<WindowId, PaneHost>> = RefCell::new(HashMap::new());
}

/// A handle on one window's pane.
///
/// Cheap to clone: the state is behind one reference count, and every clone
/// names the same widget.
#[derive(Clone)]
pub(crate) struct PaneHost {
    inner: Rc<RefCell<Inner>>,
}

/// Everything one pane owns.
struct Inner {
    /// The window this pane lives in, for the chord table's benefit.
    window: WindowId,
    area: gtk::DrawingArea,
    /// The emulator, the grid and the overlay.
    session: PaneSession,
    /// The swapchain, once the widget is realized.
    surface: Option<PaneSurface>,
    /// Why there is no surface, so the pane can say it rather than stay blank.
    fault: Option<String>,
    pacer: Pacer,
    log: FrameLog,
    /// Where the shell last put the pane, in device pixels.
    rect: PaneRect,
    /// Device pixels per logical pixel.
    scale: i32,
    input: InputSink,
    report: Option<ReportSink>,
    /// The drag in progress, if the pointer is down and the child is not
    /// tracking it.
    drag: Option<Drag>,
    /// Where inside the scrollbar thumb the pointer grabbed it, in device
    /// pixels, while a thumb drag is live.
    thumb: Option<u32>,
    /// The autoscroll timer, live only while a drag is outside the pane.
    autoscroll: Option<glib::SourceId>,
    /// Whether the first frame has been reported to the boot timeline.
    first_paint: bool,
}

/// A pointer drag the pane owns.
struct Drag {
    /// The mode the selection is growing in, so a sample that cannot change
    /// the selection does not repaint every span it covers.
    mode: SelectMode,
    /// The cell the drag last reached.
    point: Point,
    /// The last pointer position, in widget pixels, for the autoscroll timer.
    at: (i32, i32),
}

// ---------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------

/// Give the window numbered `ordinal` a pane, inside `parent`.
///
/// Runs while the frame is being built and before the window is shown, so the
/// pane is parsing and holding a grid for the whole interval the panels are
/// still being mounted.
///
/// `parent` is the box the frame reserves for the terminal. The area is packed
/// to expand and fill, so its rectangle comes from the toolkit's allocation
/// and from nothing else.
///
/// # Errors
///
/// The emulator refused its first size.
pub(crate) fn install_in(
    parent: &gtk::Box,
    ordinal: WindowId,
    input: InputSink,
) -> Result<PaneHost> {
    let area = gtk::DrawingArea::new();
    area.set_can_focus(true);
    area.add_events(
        gdk::EventMask::BUTTON_PRESS_MASK
            | gdk::EventMask::BUTTON_RELEASE_MASK
            | gdk::EventMask::POINTER_MOTION_MASK
            | gdk::EventMask::SCROLL_MASK
            | gdk::EventMask::SMOOTH_SCROLL_MASK
            | gdk::EventMask::KEY_PRESS_MASK
            | gdk::EventMask::KEY_RELEASE_MASK
            | gdk::EventMask::FOCUS_CHANGE_MASK
            | gdk::EventMask::ENTER_NOTIFY_MASK
            | gdk::EventMask::LEAVE_NOTIFY_MASK,
    );
    parent.pack_start(&area, true, true, 0);
    area.show();

    let theme = theme_from(&crate::state::live::pane_settings());
    let scale = area.scale_factor().max(1);
    // A size before the toolkit has allocated anything. It is replaced by the
    // first allocation, and it exists so the emulator is never zero by zero: a
    // child started against a zero grid writes into nothing and its first
    // screen is lost.
    let session = PaneSession::new(80, 24, (8, 16), theme)
        .map_err(|e| anyhow!("start the pane's terminal: {e}"))?;

    let host = PaneHost {
        inner: Rc::new(RefCell::new(Inner {
            window: ordinal,
            area: area.clone(),
            session,
            surface: None,
            fault: None,
            pacer: Pacer::default(),
            log: FrameLog::new(),
            rect: PaneRect::EMPTY,
            scale,
            input,
            report: None,
            drag: None,
            thumb: None,
            autoscroll: None,
            first_paint: false,
        })),
    };

    host.wire(&area);
    PANES.with(|m| m.borrow_mut().insert(ordinal, host.clone()));
    Ok(host)
}

impl PaneHost {
    /// The pane in `window`, if it has one.
    pub(crate) fn for_window(window: WindowId) -> Option<Self> {
        PANES.with(|m| m.borrow().get(&window).cloned())
    }

    /// Forget the pane in `window`, because the window is going away.
    pub(crate) fn forget(window: WindowId) {
        PANES.with(|m| m.borrow_mut().remove(&window));
    }

    /// The socket's end of the pane.
    ///
    /// Valid immediately, before the widget is realized and before there is a
    /// GPU. Bytes that arrive that early are parsed into the grid and painted
    /// by the first frame, which is why a window that connects fast does not
    /// lose the first screen.
    pub(crate) fn sink(&self) -> Rc<RefCell<dyn crate::socket::PaneSink>> {
        Rc::new(RefCell::new(Sink {
            inner: Rc::clone(&self.inner),
        }))
    }

    /// Where the pane sends what only the shell can act on.
    pub(crate) fn on_report(&self, report: ReportSink) {
        self.inner.borrow_mut().report = Some(report);
    }

    /// Adopt a new theme while the window is open.
    pub(crate) fn set_theme(&self, theme: PaneTheme) {
        let mut inner = self.inner.borrow_mut();
        inner.apply_theme(theme);
    }

    /// What the frame clock has been doing, and what its frames cost.
    pub(crate) fn frame_summary(&self) -> FrameStats {
        let inner = self.inner.borrow();
        FrameStats::of(&inner.pacer, &inner.log)
    }

    /// Connect every signal the pane answers.
    fn wire(&self, area: &gtk::DrawingArea) {
        let im = gtk::IMMulticontext::new();

        // Realize: the X window exists from here, and so can a swapchain.
        {
            let this = self.clone();
            let im = im.clone();
            area.connect_realize(move |area| {
                im.set_client_window(area.window().as_ref());
                this.realize(area);
            });
        }

        // GTK must not paint under the swapchain. Returning `Stop` without a
        // surface would leave the operator looking at whatever was in that
        // framebuffer, so the fallback paints the pane's own background and
        // the reason it has no GPU.
        {
            let this = self.clone();
            area.connect_draw(move |area, cr| this.draw_fallback(area, cr));
        }

        {
            let this = self.clone();
            area.connect_size_allocate(move |_, alloc| this.allocated(alloc));
        }

        // The frame clock. One tick per compositor frame, for as long as the
        // widget is on screen.
        {
            let this = self.clone();
            area.add_tick_callback(move |_, _| {
                this.tick();
                glib::ControlFlow::Continue
            });
        }

        // Keys. The input method sees every press first, because a press that
        // is part of a composition is not a keystroke.
        {
            let this = self.clone();
            let im = im.clone();
            area.connect_key_press_event(move |_, ev| {
                if im.filter_keypress(ev) {
                    return glib::Propagation::Stop;
                }
                this.key(ev)
            });
        }
        {
            let im = im.clone();
            area.connect_key_release_event(move |_, ev| {
                if im.filter_keypress(ev) {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
        }

        // Committed text is the only thing a composition sends. The preedit is
        // drawn and never transmitted, which is what stops a half-composed
        // character reaching a program as three keystrokes.
        {
            let this = self.clone();
            im.connect_commit(move |_, text| this.commit(text));
        }
        {
            let this = self.clone();
            im.connect_preedit_changed(move |ctx| {
                let (text, _attrs, _cursor) = ctx.preedit_string();
                this.preedit(&text);
            });
        }
        {
            let this = self.clone();
            im.connect_preedit_end(move |_| this.preedit(""));
        }

        {
            let im = im.clone();
            area.connect_focus_in_event(move |area, _| {
                im.focus_in();
                area.queue_draw();
                glib::Propagation::Proceed
            });
        }
        {
            let this = self.clone();
            let im = im.clone();
            area.connect_focus_out_event(move |_, _| {
                im.focus_out();
                // An uncommitted composition abandoned by a focus change must
                // not stay painted in a pane nobody is typing into.
                this.preedit("");
                glib::Propagation::Proceed
            });
        }

        {
            let this = self.clone();
            area.connect_button_press_event(move |area, ev| {
                area.grab_focus();
                this.button(ev, true)
            });
        }
        {
            let this = self.clone();
            area.connect_button_release_event(move |_, ev| this.button(ev, false));
        }
        {
            let this = self.clone();
            area.connect_motion_notify_event(move |_, ev| this.motion(ev));
        }
        {
            let this = self.clone();
            area.connect_scroll_event(move |_, ev| this.scroll(ev));
        }

        // Settings arrive on a bus that is not the GTK thread. The callback
        // must not touch a widget, so it hops onto the main loop and reads the
        // snapshot there.
        let subscription = crate::state::live::subscribe_pane(|_| {
            glib::idle_add_once(|| {
                let now = crate::state::live::pane_settings();
                let theme = theme_from(&now);
                PANES.with(|m| {
                    for host in m.borrow().values() {
                        host.set_theme(theme.clone());
                    }
                });
            });
        });
        // Held by the widget, so it lives exactly as long as the pane does and
        // a publish after the window closes finds nothing to call.
        unsafe {
            area.set_data("vitrum-pane-settings", subscription);
        }
    }
}

// ---------------------------------------------------------------------------
// Callbacks
// ---------------------------------------------------------------------------

impl PaneHost {
    /// Build the swapchain, now that there is a window to present to.
    fn realize(&self, area: &gtk::DrawingArea) {
        let (font, present) = {
            let inner = self.inner.borrow();
            let scale = f64::from(inner.scale.max(1));
            let theme = inner.session.theme();
            (theme.font_config(scale), theme.present)
        };

        // The borrow is dropped across `attach`, which runs a GPU handshake
        // and pumps the main loop.
        let attached = {
            let mut inner = self.inner.borrow_mut();
            let Inner { session, .. } = &mut *inner;
            PaneSurface::attach(area, font, present, session.grid_mut())
        };

        let mut inner = self.inner.borrow_mut();
        match attached {
            Ok(surface) => {
                let cell = surface.cell_size();
                let (cols, rows) = surface.cells_for(surface.size().0, surface.size().1);
                if let Err(e) = inner.session.resize(cols, rows, cell) {
                    tracing::error!("the pane's terminal refused {cols}x{rows}: {e}");
                }
                inner.surface = Some(surface);
                inner.fault = None;
                inner.pacer.mark();
                drop(inner);
                self.report(PaneReport::Resize { cols, rows });
            }
            Err(e) => {
                // Named, drawn and logged. A pane that cannot reach a GPU is a
                // pane the operator has to be told about, and the one thing it
                // may not do is show a black rectangle and say nothing.
                let why = format!("{e:#}");
                tracing::error!("no GPU surface for this pane: {why}");
                inner.fault = Some(why);
                drop(inner);
                area.queue_draw();
            }
        }
    }

    /// Paint when there is no swapchain.
    ///
    /// With a surface this returns `Stop` and draws nothing: the X window
    /// under the widget belongs to the GPU and a themed background drawn over
    /// it on every expose flickers.
    fn draw_fallback(&self, area: &gtk::DrawingArea, cr: &gtk::cairo::Context) -> glib::Propagation {
        let (fault, bg) = {
            let inner = self.inner.borrow();
            if inner.surface.is_some() {
                return glib::Propagation::Stop;
            }
            (
                inner.fault.clone(),
                inner.session.theme().background_with_opacity(),
            )
        };
        let f = |v: u8| f64::from(v) / 255.0;
        cr.set_source_rgba(f(bg.r), f(bg.g), f(bg.b), f(bg.a));
        let _ = cr.paint();
        if let Some(why) = fault {
            let fg = self.inner.borrow().session.theme().palette.foreground;
            cr.set_source_rgb(f(fg.r), f(fg.g), f(fg.b));
            cr.select_font_face(
                "monospace",
                gtk::cairo::FontSlant::Normal,
                gtk::cairo::FontWeight::Normal,
            );
            cr.set_font_size(13.0 * f64::from(area.scale_factor().max(1)));
            let mut y = 24.0;
            for line in wrap(&why, 72) {
                cr.move_to(12.0, y);
                let _ = cr.show_text(&line);
                y += 18.0;
            }
        }
        glib::Propagation::Stop
    }

    /// Follow the widget to a new size.
    ///
    /// The ONLY way the pane changes size. There is no second entry point
    /// that takes a rectangle from elsewhere: a rectangle computed from the
    /// window size and the sidebar width, and pushed here whenever anything
    /// on screen re-rendered, is what made the terminal move under the
    /// operator while something else painted.
    fn allocated(&self, alloc: &gtk::Allocation) {
        ALLOCATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let resized = {
            let mut inner = self.inner.borrow_mut();
            let scale = inner.scale.max(1) as u32;
            let px = (
                (alloc.width().max(1) as u32) * scale,
                (alloc.height().max(1) as u32) * scale,
            );
            // What the pane is, in device pixels, for everything that reasons
            // about the surface rather than the widget: the scrollbar thumb,
            // the selection, and the slack under the last full row. Read from
            // the allocation because the allocation is the only truth about
            // where the pane is.
            inner.rect = PaneRect {
                x: alloc.x().max(0) * scale as i32,
                y: alloc.y().max(0) * scale as i32,
                width: px.0,
                height: px.1,
            };
            let Some(surface) = inner.surface.as_mut() else {
                return;
            };
            let (cols, rows) = surface.resize(px.0, px.1);
            let cell = surface.cell_size();
            if (cols, rows) == (inner.session.grid().cols(), inner.session.grid().rows()) {
                inner.pacer.mark();
                return;
            }
            if let Err(e) = inner.session.resize(cols, rows, cell) {
                tracing::error!("the pane's terminal refused {cols}x{rows}: {e}");
                return;
            }
            inner.pacer.mark();
            RESIZES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // Both counts, on the one line that means the pty was resized.
            // The claim they defend is a comparison over time, so a reader
            // needs the pair at the moment it moved rather than a total at
            // the end: a repaint of any other surface must leave these
            // standing still and only a change of the pane's own allocation
            // may move them.
            tracing::debug!(
                allocations = allocations(),
                resizes = resizes(),
                "pane resized to {cols}x{rows}"
            );
            (cols, rows)
        };
        self.report(PaneReport::Resize {
            cols: resized.0,
            rows: resized.1,
        });
    }

    /// One compositor frame.
    fn tick(&self) {
        let started = Instant::now();
        let mut reports = Vec::new();
        let (drawn, spent) = {
            let mut inner = self.inner.borrow_mut();
            match inner.pacer.tick() {
                Tick::Idle | Tick::Backpressure => return,
                Tick::Draw => {}
            }
            let drawn = inner.frame(&mut reports);
            // The clock stops here and not after the reports below. Handing a
            // resize to the shell is work this frame caused and not work the
            // frame did, and charging it to the frame time makes the number
            // move whenever the shell is slow, which is the one thing it must
            // not do.
            (drawn, started.elapsed())
        };
        // The borrow is gone before anything can re-enter.
        for report in reports {
            self.report(report);
        }
        let report_stats = {
            let mut inner = self.inner.borrow_mut();
            match drawn {
                Ok(painted) => {
                    inner.pacer.presented();
                    if painted {
                        inner.log.record(spent);
                        if !inner.first_paint {
                            inner.first_paint = true;
                            crate::boot::mark("pane.first-paint");
                        }
                    }
                    painted && inner.log.count() % WINDOW as u64 == 0
                }
                Err(e) => {
                    inner.pacer.failed();
                    tracing::warn!("pane frame dropped: {e:#}");
                    false
                }
            }
        };
        // One line per window of frames. The percentiles are the whole of
        // this pane's latency claim, and a number nothing can read is not
        // evidence; a window is thirty seconds at 60 Hz.
        if report_stats {
            let s = self.frame_summary();
            tracing::debug!(
                drawn = s.drawn,
                skipped = s.skipped,
                idle = s.idle,
                recorded = s.recorded,
                p50_us = s.p50.as_micros(),
                p95_us = s.p95.as_micros(),
                p99_us = s.p99.as_micros(),
                worst_us = s.worst.as_micros(),
                behind = s.behind,
                "pane frame times"
            );
        }
    }

    /// A key the input method did not take.
    fn key(&self, ev: &gdk::EventKey) -> glib::Propagation {
        let Some((key, mods)) = super::surface::decode_event(ev) else {
            return glib::Propagation::Proceed;
        };
        // The shell gets first refusal. A chord it claims must not also reach
        // the child, or one press opens a session and types an escape
        // sequence into the session that was already there.
        let window = self.inner.borrow().window;
        if crate::keys::claim_in_pane(window, key, super::surface::digit_of(ev), mods) {
            return glib::Propagation::Stop;
        }

        // The pane's own chords. None of them leaves the pane, so none of
        // them is in the shell's table, and all three are what a terminal in
        // this class binds: the unshifted forms belong to the child.
        if mods.ctrl && mods.shift && !mods.alt {
            match key {
                Key::Char('c' | 'C') => {
                    self.copy();
                    return glib::Propagation::Stop;
                }
                Key::Char('v' | 'V') => {
                    self.paste_clipboard();
                    return glib::Propagation::Stop;
                }
                Key::Char('g' | 'G') if !self.inner.borrow().session.find_is_open() => {
                    let mut inner = self.inner.borrow_mut();
                    inner.session.find_open();
                    inner.pacer.mark();
                    return glib::Propagation::Stop;
                }
                _ => {}
            }
        }

        // With the find open the pane owns the keyboard. The operator is
        // typing a query, and a query that also reached the child would run
        // as a command the moment they pressed Enter.
        if self.inner.borrow().session.find_is_open() {
            return self.find_key(key, mods);
        }

        // Shifted paging keys are the pane's, not the child's. That is the
        // convention every terminal in this class follows, and it is the only
        // way to read history with the keyboard while a full-screen program
        // is using the unshifted keys for its own paging.
        if mods.shift && !mods.ctrl && !mods.alt {
            let mut reports = Vec::new();
            {
                let mut inner = self.inner.borrow_mut();
                let moved = match key {
                    Key::Named(Named::PageUp) => inner.session.scroll(|v| v.by_pages(-1)),
                    Key::Named(Named::PageDown) => inner.session.scroll(|v| v.by_pages(1)),
                    Key::Named(Named::Home) => inner.session.scroll(Viewport::to_top),
                    Key::Named(Named::End) => inner.session.scroll(Viewport::to_bottom),
                    _ => return self.send(ev),
                };
                if moved {
                    inner.pacer.mark();
                }
                // Arrival at the top asks for more history whether or not the
                // keystroke moved anything: holding Shift+Page Up against the
                // top is the same gesture as holding the wheel there.
                inner.viewport_moved(&mut reports);
            }
            for report in reports {
                self.report(report);
            }
            return glib::Propagation::Stop;
        }
        self.send(ev)
    }

    /// A key while the find is open.
    ///
    /// Enter steps forward and Shift+Enter steps back, which is what the
    /// find bar in every editor the operator already uses does. Escape
    /// closes. Everything else that produces a character edits the query,
    /// and the scan is redone on each edit because a find that only runs on
    /// Enter makes the operator type blind.
    fn find_key(&self, key: Key, mods: super::key::Mods) -> glib::Propagation {
        let mut reports = Vec::new();
        {
            let mut inner = self.inner.borrow_mut();
            let current = inner.session.find_input().unwrap_or_default().to_owned();
            let edited = match key {
                Key::Named(Named::Escape) => {
                    inner.session.find_clear();
                    inner.pacer.mark();
                    return glib::Propagation::Stop;
                }
                Key::Named(Named::Enter | Named::KeypadEnter) => {
                    inner.session.find_step(!mods.shift);
                    None
                }
                Key::Char('g' | 'G') if mods.ctrl && mods.shift => {
                    inner.session.find_step(true);
                    None
                }
                Key::Named(Named::Backspace) => {
                    let mut text = current;
                    text.pop();
                    Some(text)
                }
                Key::Char(c) if !mods.ctrl && !mods.alt && !c.is_control() => {
                    let mut text = current;
                    text.push(c);
                    Some(text)
                }
                _ => return glib::Propagation::Stop,
            };
            if let Some(text) = edited
                && let Err(e) = inner.session.find_type(&text)
            {
                tracing::error!("the pane could not search its own scrollback: {e}");
            }
            inner.pacer.mark();
            inner.viewport_moved(&mut reports);
        }
        for report in reports {
            self.report(report);
        }
        glib::Propagation::Stop
    }

    /// Encode a keystroke and hand it to the child.
    ///
    /// The event goes to [`super::surface`] rather than to the encoder,
    /// because translating a toolkit event is that module's whole job and
    /// the host has no business knowing what a keyval is.
    fn send(&self, ev: &gdk::EventKey) -> glib::Propagation {
        let Some(mut bytes) = super::surface::encode_event(ev) else {
            return glib::Propagation::Proceed;
        };
        let mut reports = Vec::new();
        {
            let mut inner = self.inner.borrow_mut();
            // Typing puts the reader back at the live edge. A keystroke that
            // lands somewhere the operator cannot see is a keystroke they will
            // send twice.
            if inner.session.scroll_to_bottom() {
                inner.pacer.mark();
                inner.viewport_moved(&mut reports);
            }
            // and drops the selection, because the cells it covers are about
            // to hold something else and a highlight over them is a highlight
            // the operator cannot aim at.
            if inner.session.select_clear() {
                inner.pacer.mark();
            }
            keymode::for_cursor_mode(&mut bytes, inner.session.application_cursor());
            (inner.input)(&bytes);
        }
        for report in reports {
            self.report(report);
        }
        glib::Propagation::Stop
    }

    /// Text an input method finished composing.
    fn commit(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let mut inner = self.inner.borrow_mut();
        inner.session.set_preedit("");
        let _ = inner.session.scroll_to_bottom();
        (inner.input)(text.as_bytes());
        inner.pacer.mark();
    }

    /// The composition in progress, which is drawn and never sent.
    fn preedit(&self, text: &str) {
        let mut inner = self.inner.borrow_mut();
        if inner.session.set_preedit(text) {
            inner.pacer.mark();
        }
    }

    /// A pointer button.
    fn button(&self, ev: &gdk::EventButton, press: bool) -> glib::Propagation {
        let mods = mods_of(ev.state());
        let Some(button) = button_of(ev.button()) else {
            return glib::Propagation::Proceed;
        };
        let mut reports = Vec::new();
        {
            let mut inner = self.inner.borrow_mut();
            let at = inner.position(ev.position());
            let modes = inner.session.modes();

            if modes.child_owns_pointer(mods) {
                let action = if press { Action::Press } else { Action::Release };
                if let Some(bytes) = mouse::report(modes, action, button, mods, at) {
                    (inner.input)(&bytes);
                }
                return glib::Propagation::Stop;
            }

            if !press {
                inner.thumb = None;
            }

            if press && button == Button::Left {
                // The scrollbar first. The thumb is drawn over the last
                // column, so a press inside it is a grab and not the start of
                // a selection of whatever text is under it.
                if let Some(grab) = inner.thumb_grab(at) {
                    inner.thumb = Some(grab);
                    inner.drag = None;
                    return glib::Propagation::Stop;
                }
                let point = inner.point(at);
                // Shift extends what is already there, in the mode it was
                // made in. Shift-clicking after a double click grows the word
                // selection by whole words, which is what double-clicking
                // asked for and what restarting in character mode would undo.
                let mode = match (mods.shift, inner.session.selection_mode()) {
                    (true, Some(mode)) => {
                        inner.session.select_drag(point);
                        mode
                    }
                    _ => {
                        let mode = SelectMode::for_click_count(click_count(ev));
                        inner.session.select_start(point, mode);
                        mode
                    }
                };
                inner.drag = Some(Drag {
                    mode,
                    point,
                    at: (ev.position().0 as i32, ev.position().1 as i32),
                });
                inner.pacer.mark();
            } else if press && button == Button::Middle {
                // The X convention: middle click pastes the primary selection.
                drop(inner);
                self.paste_from(gdk::SELECTION_PRIMARY);
                return glib::Propagation::Stop;
            } else if !press && button == Button::Left {
                inner.drag = None;
                inner.stop_autoscroll();
                if let Some(text) = inner.session.selection_text() {
                    // PRIMARY only. Taking CLIPBOARD on every drag would throw
                    // away whatever the operator copied a moment ago.
                    gtk::Clipboard::get(&gdk::SELECTION_PRIMARY).set_text(&text);
                }
            } else if press && button == Button::Right {
                if let Some(text) = inner.session.selection_text() {
                    reports.push(PaneReport::Copied { ok: true, text });
                }
            }
        }
        for report in reports {
            self.report(report);
        }
        glib::Propagation::Stop
    }

    /// The pointer moved.
    fn motion(&self, ev: &gdk::EventMotion) -> glib::Propagation {
        let mods = mods_of(ev.state());
        let held = held_button(ev.state());
        let mut reports = Vec::new();
        {
            let mut inner = self.inner.borrow_mut();
            let at = inner.position(ev.position());
            let modes = inner.session.modes();

            if modes.child_owns_pointer(mods) {
                if let Some(bytes) = mouse::report(
                    modes,
                    Action::Motion { held },
                    held.unwrap_or(Button::Left),
                    mods,
                    at,
                ) {
                    (inner.input)(&bytes);
                }
                return glib::Propagation::Stop;
            }

            if let Some(grab) = inner.thumb {
                let track = inner.track_px();
                let want = inner
                    .session
                    .viewport()
                    .offset_for_thumb(at.py.saturating_sub(grab), track);
                if inner.session.scroll_to_offset(want) {
                    inner.pacer.mark();
                    inner.viewport_moved(&mut reports);
                }
                drop(inner);
                for report in reports {
                    self.report(report);
                }
                return glib::Propagation::Stop;
            }

            if inner.drag.is_some() {
                let point = inner.point(at);
                let moved = inner
                    .drag
                    .as_ref()
                    .is_some_and(|d| drag_changes(d.mode, d.point, point));
                if moved {
                    inner.session.select_drag(point);
                    inner.pacer.mark();
                }
                if let Some(drag) = inner.drag.as_mut() {
                    drag.at = (ev.position().0 as i32, ev.position().1 as i32);
                    drag.point = point;
                }
                let height = inner.area.allocated_height();
                let y = ev.position().1 as i32;
                let rows = super::select::autoscroll_rows(
                    y,
                    height,
                    inner.session.viewport().page_rows() as u16,
                );
                if rows == 0 {
                    inner.stop_autoscroll();
                } else {
                    drop(inner);
                    self.start_autoscroll();
                }
            }
        }
        glib::Propagation::Stop
    }

    /// The wheel.
    fn scroll(&self, ev: &gdk::EventScroll) -> glib::Propagation {
        let mods = mods_of(ev.state());
        let Some(notch) = wheel_of(ev) else {
            return glib::Propagation::Proceed;
        };
        let mut reports = Vec::new();
        {
            let mut inner = self.inner.borrow_mut();
            let at = inner.position(ev.position());
            let modes = inner.session.modes();

            if modes.child_owns_pointer(mods) {
                if let Some(bytes) = mouse::report(modes, Action::Press, notch, mods, at) {
                    (inner.input)(&bytes);
                }
                return glib::Propagation::Stop;
            }

            // A horizontal notch the child did not ask for does nothing. The
            // pane's history is a column of rows with nowhere to go sideways,
            // and turning a tilt into a vertical scroll moves the text the
            // operator was reading in a direction they did not push.
            let up = match notch {
                Button::WheelUp => true,
                Button::WheelDown => false,
                _ => return glib::Propagation::Stop,
            };

            // On the alternate screen with 1007 set, the wheel is arrow keys:
            // the program has no scrollback and the pane's own history is not
            // what the operator is looking at.
            let lines = inner.session.theme().scroll_lines_per_notch;
            let app_cursor = inner.session.application_cursor();
            let alt = mouse::alt_scroll(modes, up, lines, app_cursor);
            if !alt.is_empty() {
                (inner.input)(&alt);
                return glib::Propagation::Stop;
            }

            let notches = if up { -1 } else { 1 };
            let moved = inner.session.scroll(|v| v.by_notches(notches, lines));
            if moved {
                inner.pacer.mark();
                inner.viewport_moved(&mut reports);
            } else if up {
                // Already at the top and the operator kept scrolling. That is
                // the gesture that asks for older history; the shell decides
                // whether there is any.
                inner.request_backfill(&mut reports);
            }
        }
        for report in reports {
            self.report(report);
        }
        glib::Propagation::Stop
    }

    /// Keep scrolling while a drag is held outside the pane.
    fn start_autoscroll(&self) {
        if self.inner.borrow().autoscroll.is_some() {
            return;
        }
        let this = self.clone();
        let id = glib::timeout_add_local(Duration::from_millis(u64::from(AUTOSCROLL_MS)), move || {
            let mut reports = Vec::new();
            let keep = {
                let mut inner = this.inner.borrow_mut();
                let Some(drag) = inner.drag.as_ref() else {
                    inner.autoscroll = None;
                    return glib::ControlFlow::Break;
                };
                let (x, y) = drag.at;
                let mode = drag.mode;
                let was = drag.point;
                let height = inner.area.allocated_height();
                let rows = super::select::autoscroll_rows(
                    y,
                    height,
                    inner.session.viewport().page_rows() as u16,
                );
                if rows == 0 {
                    inner.autoscroll = None;
                    return glib::ControlFlow::Break;
                }
                if inner.session.scroll(|v| v.by_lines(i64::from(rows))) {
                    inner.pacer.mark();
                    inner.viewport_moved(&mut reports);
                }
                // The drag continues to the row the pointer is over now that
                // the view has moved under it. The scroll changed the row, so
                // this sample always has somewhere new to go, but the mode
                // still decides whether the column matters.
                let at = inner.position((f64::from(x), f64::from(y)));
                let point = inner.point(at);
                if drag_changes(mode, was, point) {
                    inner.session.select_drag(point);
                }
                if let Some(drag) = inner.drag.as_mut() {
                    drag.point = point;
                }
                true
            };
            for report in reports {
                this.report(report);
            }
            if keep {
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
        self.inner.borrow_mut().autoscroll = Some(id);
    }

    /// Copy the selection to the system clipboard.
    pub(crate) fn copy(&self) {
        let text = self.inner.borrow_mut().session.selection_text();
        let Some(text) = text else {
            self.report(PaneReport::Copied {
                ok: false,
                text: String::new(),
            });
            return;
        };
        gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_text(&text);
        self.report(PaneReport::Copied { ok: true, text });
    }

    /// Paste the system clipboard into the child.
    pub(crate) fn paste_clipboard(&self) {
        self.paste_from(gdk::SELECTION_CLIPBOARD);
    }

    /// Paste one named selection.
    fn paste_from(&self, selection: gdk::Atom) {
        let this = self.clone();
        gtk::Clipboard::get(&selection).request_text(move |_, text| {
            let Some(text) = text else {
                return;
            };
            let mut inner = this.inner.borrow_mut();
            let bracketed = inner.session.bracketed_paste();
            let bytes = paste::frame(&text, bracketed);
            let _ = inner.session.scroll_to_bottom();
            (inner.input)(&bytes);
            inner.pacer.mark();
        });
    }

    /// Send one report to the shell, with no borrow held.
    fn report(&self, report: PaneReport) {
        let sink = {
            let inner = self.inner.borrow();
            inner.report.is_some()
        };
        if !sink {
            return;
        }
        let inner = self.inner.borrow();
        if let Some(f) = inner.report.as_ref() {
            f(report);
        }
    }
}

impl Inner {
    /// Draw one frame.
    fn frame(&mut self, reports: &mut Vec<PaneReport>) -> Result<bool> {
        self.session
            .sync()
            .map_err(|e| anyhow!("project the terminal onto the grid: {e}"))?;
        self.drain_replies();
        self.viewport_moved(reports);

        let Some(surface) = self.surface.as_mut() else {
            return Ok(false);
        };
        surface
            .present(self.session.grid_mut())
            .context("present the pane")
    }

    /// Hand the child every answer the emulator owes it.
    fn drain_replies(&mut self) {
        let mut out = Vec::new();
        self.session.drain_pty_write(&mut out);
        if !out.is_empty() {
            (self.input)(&out);
        }
    }

    /// Notice arrival at the oldest retained row.
    fn viewport_moved(&mut self, reports: &mut Vec<PaneReport>) {
        if self.session.viewport().at_top() {
            self.request_backfill(reports);
        }
    }

    /// Ask the shell for older history.
    ///
    /// Unguarded on purpose. Whether more exists, and whether a request is
    /// already in flight, are the shell's to know; a second guard here would
    /// be a second thing to keep in step with the first.
    fn request_backfill(&mut self, reports: &mut Vec<PaneReport>) {
        reports.push(PaneReport::PageBack);
    }

    /// Adopt a theme and rebuild whatever it changed.
    ///
    /// The session holds the one copy. Clamping happens before the
    /// comparison, because the session stores the clamped form and an
    /// out-of-range size would otherwise read as a change on every publish.
    fn apply_theme(&mut self, theme: PaneTheme) {
        let theme = theme.clamped();
        let (font_changed, present_changed) = {
            let old = self.session.theme();
            if theme == *old {
                return;
            }
            (
                theme.families != old.families
                    || theme.size_pt != old.size_pt
                    || theme.line_height_pct != old.line_height_pct
                    || theme.cell_width_pct != old.cell_width_pct,
                theme.present != old.present,
            )
        };

        if let Err(e) = self.session.set_theme(theme) {
            tracing::error!("the pane's terminal refused a colour: {e}");
        }
        if present_changed {
            let present = self.session.theme().present;
            if let Some(surface) = self.surface.as_mut() {
                surface.set_present(present);
            }
        }
        if font_changed {
            let scale = f64::from(self.scale.max(1));
            let font = self.session.theme().font_config(scale);
            let Self {
                session, surface, ..
            } = self;
            if let Some(surface) = surface.as_mut() {
                match surface.set_font(font) {
                    Ok((cols, rows)) => {
                        let cell = surface.cell_size();
                        if let Err(e) = session.resize(cols, rows, cell) {
                            tracing::error!("the pane's terminal refused {cols}x{rows}: {e}");
                        }
                    }
                    Err(e) => tracing::error!("the pane kept its old font: {e:#}"),
                }
            }
        }
        self.pacer.mark();
    }

    /// One cell in device pixels, or a plausible one before there is a GPU.
    fn cell_px(&self) -> (u32, u32) {
        self.surface
            .as_ref()
            .map_or((8, 16), super::surface::PaneSurface::cell_size)
    }

    /// The scrollbar's track, in device pixels.
    ///
    /// The height the cells cover, not the height of the box. The slack at
    /// the bottom is pixels no row occupies, and measuring the track through
    /// it puts the thumb up to a cell away from the row it names, at the
    /// bottom of the track, which is where the operator drags it.
    fn track_px(&self) -> u32 {
        let cell = self.cell_px();
        self.rect.height.saturating_sub(self.rect.slack(cell).1)
    }

    /// Where inside the thumb `at` grabbed it, if it grabbed it at all.
    ///
    /// `None` for a press anywhere else, including the last column while the
    /// viewport is live: there is no thumb drawn there, and treating the
    /// column as a permanent gutter would take the operator's last column of
    /// text away from every click.
    fn thumb_grab(&self, at: Position) -> Option<u32> {
        if self.session.viewport().is_live() {
            return None;
        }
        let cell = self.cell_px();
        let gutter =
            u32::from(self.session.grid().cols().saturating_sub(1)) * cell.0.max(1);
        if at.px < gutter {
            return None;
        }
        let (top, len) = self.session.viewport().thumb(self.track_px())?;
        (at.py >= top && at.py < top + len).then(|| at.py - top)
    }

    /// Stop the autoscroll timer, if one is running.
    fn stop_autoscroll(&mut self) {
        if let Some(id) = self.autoscroll.take() {
            id.remove();
        }
    }

    /// A widget-space pointer position as the grid sees it.
    fn position(&self, at: (f64, f64)) -> Position {
        let scale = f64::from(self.scale.max(1));
        let cell = self.cell_px();
        let px = (at.0.max(0.0) * scale) as u32;
        let py = (at.1.max(0.0) * scale) as u32;
        let cols = self.session.grid().cols();
        let rows = self.session.grid().rows();
        Position {
            col: ((px / cell.0.max(1)) as u16).min(cols.saturating_sub(1)),
            row: ((py / cell.1.max(1)) as u16).min(rows.saturating_sub(1)),
            px,
            py,
        }
    }

    /// A grid position as an absolute point in the session's history.
    fn point(&self, at: Position) -> Point {
        Point {
            row: self.session.viewport().top_row() + usize::from(at.row),
            col: at.col,
        }
    }
}

// ---------------------------------------------------------------------------
// The socket's end
// ---------------------------------------------------------------------------

/// What [`crate::socket`] writes into.
struct Sink {
    inner: Rc<RefCell<Inner>>,
}

impl crate::socket::PaneSink for Sink {
    fn reset(&mut self) {
        let mut inner = self.inner.borrow_mut();
        inner.session.reset();
        inner.pacer.mark();
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut inner = self.inner.borrow_mut();
        // Feed and return. Nothing is projected and nothing is drawn: sixty
        // reads in one wakeup are sixty feeds and one frame.
        inner.session.feed(bytes);
        inner.pacer.mark();
    }

    fn scroll_from_end(&mut self, lines: u32) {
        let mut inner = self.inner.borrow_mut();
        let rows = inner.session.grid().rows();
        inner.session.scroll(|v: &mut Viewport| {
            v.to_bottom();
            v.by_lines(-i64::from(lines).saturating_add(i64::from(rows) / 2));
        });
        inner.pacer.mark();
    }

    fn keep_view(&mut self) {
        let mut inner = self.inner.borrow_mut();
        inner.pacer.mark();
    }

    fn flush(&mut self) {
        // Nothing. The frame clock is the only clock; a batch ending is not a
        // reason to draw, because the compositor has not asked for a frame
        // yet and drawing now would be a frame nobody shows.
    }
}

// ---------------------------------------------------------------------------
// Toolkit translation
// ---------------------------------------------------------------------------

/// gdk modifier state as the encoders read it.
fn mods_of(state: gdk::ModifierType) -> mouse::Mods {
    mouse::Mods {
        shift: state.contains(gdk::ModifierType::SHIFT_MASK),
        alt: state.contains(gdk::ModifierType::MOD1_MASK),
        ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
    }
}

/// A gdk button number as a button.
///
/// Buttons 8 through 11 are the side buttons. A mouse that has them sends
/// them at the pane like any other, and a child that asked for mouse
/// reporting expects them in the second bank rather than not at all.
const fn button_of(n: u32) -> Option<Button> {
    match n {
        1 => Some(Button::Left),
        2 => Some(Button::Middle),
        3 => Some(Button::Right),
        8..=11 => Some(Button::Extra {
            index: (n - 8) as u8,
        }),
        _ => None,
    }
}

/// The wheel notch a scroll event carries.
fn wheel_of(ev: &gdk::EventScroll) -> Option<Button> {
    match ev.direction() {
        gdk::ScrollDirection::Up => Some(Button::WheelUp),
        gdk::ScrollDirection::Down => Some(Button::WheelDown),
        gdk::ScrollDirection::Left => Some(Button::WheelLeft),
        gdk::ScrollDirection::Right => Some(Button::WheelRight),
        gdk::ScrollDirection::Smooth => {
            let (dx, dy) = ev.delta();
            smooth_notch(dx, dy)
        }
        _ => None,
    }
}

/// The notch a smooth scroll delta names.
///
/// One axis, the larger one. A trackpad reports both at once and a swipe that
/// is mostly vertical carries a pixel of horizontal drift with it; reporting
/// both would send the child a tilt on every scroll it never made. Ties go to
/// the vertical axis, because a wheel that reports equal deltas is a wheel
/// with no horizontal axis at all.
fn smooth_notch(dx: f64, dy: f64) -> Option<Button> {
    if dx == 0.0 && dy == 0.0 {
        return None;
    }
    Some(if dy.abs() >= dx.abs() {
        if dy < 0.0 {
            Button::WheelUp
        } else {
            Button::WheelDown
        }
    } else if dx < 0.0 {
        Button::WheelLeft
    } else {
        Button::WheelRight
    })
}

/// Whether a pointer sample can change the selection it is dragging.
///
/// Motion arrives at pixel resolution and a cell is several pixels across, so
/// most samples land on the cell the last one did. Growing the selection for
/// one of those re-lays every span it covers, and a line selection cannot
/// change at all until the pointer leaves the row it is on.
const fn drag_changes(mode: SelectMode, from: Point, to: Point) -> bool {
    if from.row != to.row {
        return true;
    }
    !matches!(mode, SelectMode::Line) && from.col != to.col
}

/// The button held during a motion event, if any.
fn held_button(state: gdk::ModifierType) -> Option<Button> {
    if state.contains(gdk::ModifierType::BUTTON1_MASK) {
        Some(Button::Left)
    } else if state.contains(gdk::ModifierType::BUTTON2_MASK) {
        Some(Button::Middle)
    } else if state.contains(gdk::ModifierType::BUTTON3_MASK) {
        Some(Button::Right)
    } else {
        None
    }
}

/// How many clicks this press is part of.
///
/// GTK reports a double click as a third event after two single ones. Reading
/// the event type rather than counting means the pane never has to hold a
/// timer to decide what a click was.
fn click_count(ev: &gdk::EventButton) -> u32 {
    match ev.event_type() {
        gdk::EventType::DoubleButtonPress => 2,
        gdk::EventType::TripleButtonPress => 3,
        _ => 1,
    }
}

/// Break a diagnostic into lines a fallback paint can show.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            out.push(core::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WHY: a diagnostic painted as one line runs off the side of the pane and
    /// the operator reads half a sentence that stops mid-word.
    #[test]
    fn a_diagnostic_is_broken_into_lines_that_fit() {
        let text = "the pane needs the X11 GDK backend and this session is Wayland. \
                    Start vitrum with GDK_BACKEND=x11 to run it through XWayland.";
        let lines = wrap(text, 40);
        assert!(lines.len() > 1, "nothing was wrapped");
        for line in &lines {
            assert!(line.chars().count() <= 40 || !line.contains(' '), "{line}");
        }
        // Every word survives, in order.
        assert_eq!(
            lines.join(" ").split_whitespace().collect::<Vec<_>>(),
            text.split_whitespace().collect::<Vec<_>>()
        );
    }

    /// WHY: a click count read from a counter needs a timer and gets the third
    /// click wrong. Read from the event type it is exact, and the selection
    /// mode it chooses is what the operator sees.
    #[test]
    fn a_button_number_maps_to_exactly_one_button() {
        assert_eq!(button_of(1), Some(Button::Left));
        assert_eq!(button_of(2), Some(Button::Middle));
        assert_eq!(button_of(3), Some(Button::Right));
        // Back and forward are the second bank, not left clicks, and not
        // nothing: a child that asked for mouse reporting reads them.
        assert_eq!(button_of(8), Some(Button::Extra { index: 0 }));
        assert_eq!(button_of(9), Some(Button::Extra { index: 1 }));
        assert_eq!(button_of(11), Some(Button::Extra { index: 3 }));
        assert_eq!(button_of(12), None);
        assert_eq!(button_of(0), None);
    }

    /// WHY: a trackpad reports both axes on every sample, so a vertical swipe
    /// with a pixel of drift would send the child a horizontal notch it never
    /// made, and a program bound to a tilt would act on it.
    #[test]
    fn a_smooth_scroll_reports_one_axis_and_it_is_the_larger_one() {
        assert_eq!(smooth_notch(0.0, -1.0), Some(Button::WheelUp));
        assert_eq!(smooth_notch(0.0, 1.0), Some(Button::WheelDown));
        assert_eq!(smooth_notch(-1.0, 0.0), Some(Button::WheelLeft));
        assert_eq!(smooth_notch(1.0, 0.0), Some(Button::WheelRight));
        // Drift on the other axis does not change the gesture.
        assert_eq!(smooth_notch(0.2, -3.0), Some(Button::WheelUp));
        assert_eq!(smooth_notch(-3.0, 0.2), Some(Button::WheelLeft));
        // A tie is vertical, and no movement is no notch.
        assert_eq!(smooth_notch(1.0, 1.0), Some(Button::WheelDown));
        assert_eq!(smooth_notch(0.0, 0.0), None);
    }

    /// WHY: a pointer sample that cannot change the selection still costs a
    /// full re-lay of every span it covers, and a line drag across a wide row
    /// produces one of those per pixel.
    #[test]
    fn a_sample_that_cannot_change_the_selection_is_not_one() {
        let a = Point { row: 4, col: 10 };
        let b = Point { row: 4, col: 11 };
        let c = Point { row: 5, col: 10 };

        assert!(!drag_changes(SelectMode::Character, a, a));
        assert!(drag_changes(SelectMode::Character, a, b));
        assert!(drag_changes(SelectMode::Character, a, c));

        // A line selection covers whole rows, so only the row matters.
        assert!(!drag_changes(SelectMode::Line, a, b));
        assert!(drag_changes(SelectMode::Line, a, c));
    }
}
