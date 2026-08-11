//! The launcher and the rename field, as presented surfaces.
//!
//! Every rule this surface is built on lives in the module above and is
//! unchanged: two fields, WHERE then WHAT; ranked rows that are already there
//! before a key is pressed; Ctrl+digit on the first nine; Tab completes rather
//! than commits; the place chip only when a row would run somewhere other than
//! the `in` field says. Those are all decided by pure functions there, and
//! this file does nothing except call them and put the answer on widgets.
//!
//! # What the open path is still not allowed to do
//!
//! Nothing that can block. [`launch::detected_agents`] is five `PATH` lookups
//! and [`launch::list_dirs`] is a `read_dir` that a wedged network mount can
//! hold in the kernel for as long as it likes, so both go through
//! [`super::off_thread`] and land through the main context when they land. The
//! open path is one profile read and two environment reads, exactly as before.
//!
//! # Why so little is rebuilt
//!
//! A surface that rebuilds its whole row list on every keystroke is the
//! flicker this wave exists to remove. The rows, the completions and the two
//! bands each remember what they last drew and are rebuilt only when that
//! answer changes, which for typing further into one directory is never.
//!
//! # Keys
//!
//! Escape and every shell chord stay with key dispatch. What is handled here
//! is what only exists while this surface is on screen and only inside its two
//! fields: the arrows, Tab, Enter, Ctrl+digit and Ctrl+S. A saved preset's
//! chord is NOT handled here, because [`crate::keymap::KeyAction::LaunchPreset`]
//! fires it from anywhere and a second copy on this surface would be a binding
//! with two implementations.

use std::cell::{Cell, RefCell};
use std::path::MAIN_SEPARATOR;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use vitrum_proto::SessionId;

use super::{
    DIROPT, DIROPT_ON, DIR_MAX, Attempt, Intent, Pick, RowView, ROWS_MAX, attempt, completion,
    intents, is_dir_search, key_of, listed, looks_like_path, no_row_reason, note, recent_dirs,
    shorten_home, typed_intent, view,
};
use crate::launch::{self, Detected, Launch, LaunchStore};
use crate::shell::{Dialog, Shell};
use crate::state::{Layer, NewSessionSeed, RenameSeed};
use crate::ui::sheet::{self, Sheet};
use crate::ui::{glyph, presets, recents};
use crate::wire::ClientEvent;

/// Start `l` and close whatever surface asked for it.
///
/// The one exit from every launch control in this module and in the two bands
/// beside it. The project is resolved here and the launch is sent as one
/// event, because [`crate::actions::start_session`] is where the history
/// write, the flash and the focus correlation happen and a second launch path
/// would have to remember all three.
pub(crate) fn go(shell: &Shell, l: Launch) {
    let project = shell.peek(|st| launch::resolve_project(&st.daemon.projects, &l.cwd).0);
    shell.send(ClientEvent::Start {
        project,
        launch: l,
    });
    shell.update(|st| st.window.layer = Layer::None);
    shell.dismiss();
}

/// What one directory scan has answered, and for which directory.
///
/// The key is kept beside the answer so a scan that finishes after the
/// operator has typed further cannot overwrite a newer one. Without it, a slow
/// mount answering late replaces the completions for a directory nobody is
/// looking at any more.
#[derive(Default)]
struct Scanned {
    key: RefCell<String>,
    list: RefCell<Vec<String>>,
}

/// Which field a scan is completing for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    /// The `in` field, which is always completing a directory.
    Where,
    /// The `run` field, which completes a directory only while what is typed
    /// is a path.
    What,
}

/// The launcher.
pub(crate) struct Launcher {
    shell: Shell,
    frame: Rc<Sheet>,
    /// The profile, read once when the surface opened. It IS the ranking, so
    /// it is not deferred, and it is written back when a preset is saved.
    store: RefCell<LaunchStore>,
    home: String,
    opened_ms: u64,
    /// The `PATH` answer, empty until the thread returns.
    detected: RefCell<Vec<Detected>>,
    /// The resolved directory a launch would use, which is not the text in the
    /// `in` field: that text can carry the trailing separator that means "show
    /// me what is inside".
    here: RefCell<String>,
    /// Every launch worth offering from `here`. Recomputed when the place or
    /// the profile moves, never per keystroke.
    all: RefCell<Vec<Intent>>,
    picks: RefCell<Vec<Pick>>,
    hi: Cell<usize>,
    dir_hi: Cell<usize>,
    /// Whether the highlighted row has already been refused once, so taking it
    /// again runs it anyway.
    armed: Cell<bool>,
    said: RefCell<Option<String>>,

    where_scan: Scanned,
    what_scan: Scanned,

    dir_field: gtk::Entry,
    dir_list: gtk::Box,
    query: gtk::Entry,
    save: gtk::Button,
    bands: gtk::Box,
    rows: gtk::Box,
    note: gtk::Label,

    /// What is currently on screen, so nothing is rebuilt for an answer that
    /// has not changed.
    drawn_dirs: RefCell<Vec<String>>,
    drawn_rows: RefCell<Vec<RowView>>,
    drawn_bands: Cell<bool>,
    handle: RefCell<Weak<Self>>,
}

/// Build the launcher, seeded from wherever it was opened.
pub(crate) fn build(shell: &Shell, seed: &NewSessionSeed) -> Rc<Launcher> {
    let store = launch::load_launch_store();
    let home = launch::user_home();
    let here = launch::seed_cwd(&seed.cwd, &store, &home);

    let panel = sheet::column("rg-sheet__panel");
    panel.pack_start(&sheet::head(shell, "Start a session"), false, false, 0);

    // WHERE, then WHAT. Two fields, each labelled, each holding one thing.
    //
    // Each field holds exactly one line. The inset is on the line and the
    // rule under it is on the field, so the rule spans the whole sheet while
    // the controls stay inside the margin; a second child would put the rule
    // under whichever child happened to be last.
    let where_field = sheet::column("rg-launch__field");
    let where_line = sheet::row("rg-launch__line");
    where_line.pack_start(&sheet::label("rg-launch__label", "in"), false, false, 0);
    let dir_field = gtk::Entry::new();
    dir_field.style_context().add_class("rg-launch__dir");
    dir_field.set_placeholder_text(Some("Directory"));
    dir_field.set_text(&shorten_home(&here, &home));
    dir_field.set_hexpand(true);
    where_line.pack_start(&dir_field, true, true, 0);
    where_field.pack_start(&where_line, false, false, 0);
    panel.pack_start(&where_field, false, false, 0);

    // The completions sit outside the field, because they are an answer about
    // it rather than part of it: inside, the field's rule would be drawn under
    // the list instead of under the control it belongs to.
    let dir_list = sheet::column("rg-launch__dirs");
    panel.pack_start(&dir_list, false, false, 0);

    let what_field = sheet::column("rg-launch__field");
    let line = sheet::row("rg-launch__line");
    line.pack_start(&sheet::label("rg-launch__label", "run"), false, false, 0);
    let query = gtk::Entry::new();
    query.style_context().add_class("rg-launch__query");
    query.set_placeholder_text(Some("Command, or an agent name"));
    query.set_hexpand(true);
    line.pack_start(&query, true, true, 0);
    // Saving lives where the command was typed. The chord still works and is
    // named in the tooltip, so this control teaches it instead of replacing
    // it.
    let save = gtk::Button::with_label("Save");
    save.style_context().add_class("rg-launch__save");
    save.set_tooltip_text(Some("Save this command as a preset (Ctrl+S)"));
    save.set_sensitive(false);
    line.pack_end(&save, false, false, 0);
    what_field.pack_start(&line, false, false, 0);
    panel.pack_start(&what_field, false, false, 0);

    let bands = sheet::column("rg-launch__bands");
    panel.pack_start(&bands, false, false, 0);

    let rows = sheet::column("rg-launch__list");
    panel.pack_start(&rows, true, true, 0);

    let note = sheet::label("rg-launch__note", "");
    note.set_no_show_all(true);
    panel.pack_start(&note, false, false, 0);

    let frame = Sheet::new(sheet::LAUNCHER, sheet::LIST, &panel);
    let me = Rc::new(Launcher {
        shell: shell.clone(),
        frame,
        store: RefCell::new(store),
        home,
        opened_ms: launch::now_ms(),
        detected: RefCell::new(Vec::new()),
        here: RefCell::new(here),
        all: RefCell::new(Vec::new()),
        picks: RefCell::new(Vec::new()),
        hi: Cell::new(0),
        dir_hi: Cell::new(0),
        armed: Cell::new(false),
        said: RefCell::new(None),
        where_scan: Scanned::default(),
        what_scan: Scanned::default(),
        dir_field: dir_field.clone(),
        dir_list,
        query: query.clone(),
        save: save.clone(),
        bands,
        rows,
        note,
        drawn_dirs: RefCell::new(Vec::new()),
        drawn_rows: RefCell::new(Vec::new()),
        drawn_bands: Cell::new(false),
        handle: RefCell::new(Weak::new()),
    });
    *me.handle.borrow_mut() = Rc::downgrade(&me);

    dir_field.connect_changed({
        let me = Rc::downgrade(&me);
        move |_| {
            if let Some(me) = me.upgrade() {
                me.where_typed();
            }
        }
    });
    dir_field.connect_key_press_event({
        let me = Rc::downgrade(&me);
        move |_, event| match me.upgrade() {
            Some(me) => me.key(Field::Where, event),
            None => glib::Propagation::Proceed,
        }
    });

    query.connect_changed({
        let me = Rc::downgrade(&me);
        move |_| {
            if let Some(me) = me.upgrade() {
                me.what_typed();
            }
        }
    });
    query.connect_key_press_event({
        let me = Rc::downgrade(&me);
        move |_, event| match me.upgrade() {
            Some(me) => me.key(Field::What, event),
            None => glib::Propagation::Proceed,
        }
    });
    // The caret goes to the command field as the sheet appears. Done on map
    // rather than by whoever handled the chord, because the widget does not
    // exist at that point and a focus issued there lands on whatever had it
    // before, which inside a live pane means the query is typed at the child
    // process.
    query.connect_map(|entry| entry.grab_focus());

    save.connect_clicked({
        let me = Rc::downgrade(&me);
        move |_| {
            if let Some(me) = me.upgrade() {
                me.save_typed();
            }
        }
    });

    me.rebuild_intents();
    me.rescan(Field::Where);
    me.refresh();
    me.find_agents();
    me
}

impl Launcher {
    /// The `in` field changed. The place moves with it, as it is typed.
    fn where_typed(self: &Rc<Self>) {
        let typed = self.dir_field.text().to_string();
        // `~` is what the operator types and what every recent is offered as,
        // so it has to be accepted back: storing the literal string would
        // spawn the session in a directory called `~`.
        let full = launch::expand_home(&typed, &self.home);
        *self.here.borrow_mut() = launch::tidy_dir(&full);
        self.dir_hi.set(0);
        *self.said.borrow_mut() = None;
        self.rebuild_intents();
        self.rescan(Field::Where);
        self.refresh();
    }

    /// The `run` field changed.
    fn what_typed(self: &Rc<Self>) {
        self.hi.set(0);
        self.armed.set(false);
        *self.said.borrow_mut() = None;
        self.rescan(Field::What);
        self.refresh();
    }

    /// Rank every launch worth offering from the current place.
    ///
    /// The daemon is read through `peek`. Subscribing here would rebuild every
    /// row whenever a session streams output, which on a busy daemon is twenty
    /// times a second under the operator's hand.
    fn rebuild_intents(&self) {
        let store = self.store.borrow();
        let detected = self.detected.borrow();
        let here = self.here.borrow();
        let rows = self.shell.peek(|st| {
            intents(st, &store, &detected, &here, &self.home, self.opened_ms)
        });
        *self.all.borrow_mut() = rows;
    }

    /// Walk `PATH` on a thread and fold the answer in when it lands.
    ///
    /// The agents band sits BELOW the recents precisely so this can arrive
    /// late without moving the row the highlight is already on.
    fn find_agents(self: &Rc<Self>) {
        let me = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let found = super::off_thread(launch::detected_agents).await;
            let Some(me) = me.upgrade() else { return };
            *me.detected.borrow_mut() = found;
            me.rebuild_intents();
            me.refresh();
        });
    }

    /// Scan whatever directory `field` is currently completing against.
    ///
    /// Keyed on the directory rather than on the text, so typing further into
    /// one folder spawns no thread and runs no syscall.
    fn rescan(self: &Rc<Self>, field: Field) {
        let (slot, dir) = match field {
            Field::Where => {
                let typed = self.dir_field.text().to_string();
                (
                    &self.where_scan,
                    launch::split_dir_input(&typed, &self.home).0,
                )
            }
            Field::What => {
                let typed = self.query.text().to_string();
                let dir = if looks_like_path(&typed) {
                    launch::split_dir_input(&typed, &self.home).0
                } else {
                    String::new()
                };
                (&self.what_scan, dir)
            }
        };
        if *slot.key.borrow() == dir {
            return;
        }
        *slot.key.borrow_mut() = dir.clone();
        if dir.is_empty() {
            slot.list.borrow_mut().clear();
            return;
        }

        let me = Rc::downgrade(self);
        let wanted = dir.clone();
        glib::MainContext::default().spawn_local(async move {
            let found = super::off_thread(move || launch::list_dirs(&dir)).await;
            let Some(me) = me.upgrade() else { return };
            let slot = match field {
                Field::Where => &me.where_scan,
                Field::What => &me.what_scan,
            };
            // The operator has typed on. This answer is for a directory
            // nobody is looking at.
            if *slot.key.borrow() != wanted {
                return;
            }
            *slot.list.borrow_mut() = found;
            me.refresh();
        });
    }

    /// Completions for the `in` field, or where this operator has worked
    /// before when there is nothing to complete against.
    fn dir_picks(&self) -> Vec<String> {
        let typed = self.dir_field.text().to_string();
        // Nothing typed is not "no answer": it is the moment the operator has
        // said least, and where they last worked is the most useful thing on
        // screen. Completing an empty field against the filesystem would offer
        // the root's children, which is never where anybody is going.
        if typed.trim().is_empty() {
            let last = self.store.borrow().last_cwd.clone();
            return self.shell.peek(|st| recent_dirs(st, &last));
        }
        let fragment = launch::split_dir_input(&typed, &self.home).1;
        let hits = launch::filter_dirs(&self.where_scan.list.borrow(), &fragment, DIR_MAX);
        // An exact, complete directory name is not a suggestion: offering
        // `software/` while `software/` is what the field already says makes
        // Tab a no-op and the list a mirror of the input.
        let whole = launch::tidy_dir(&launch::expand_home(&typed, &self.home));
        if hits.len() == 1 && launch::tidy_dir(&hits[0]) == whole {
            return Vec::new();
        }
        hits
    }

    /// The rows the list would draw for what is typed.
    fn row_picks(&self) -> Vec<Pick> {
        let text = self.query.text().to_string();
        if is_dir_search(&text) {
            let fragment = launch::split_dir_input(&text, &self.home).1;
            return launch::filter_dirs(&self.what_scan.list.borrow(), &fragment, DIR_MAX)
                .into_iter()
                .map(Pick::Cd)
                .collect();
        }
        let rows = self.all.borrow();
        let mut out: Vec<Pick> = listed(&rows, &text)
            .into_iter()
            .map(|i| Pick::Go(rows[i].clone()))
            .collect();
        let extra = self.shell.peek(|st| {
            typed_intent(&rows, st, &self.here.borrow(), &text, &self.home)
        });
        if let Some(extra) = extra {
            if out.len() >= ROWS_MAX {
                out.truncate(ROWS_MAX - 1);
            }
            out.push(Pick::Go(extra));
        }
        out
    }

    /// Put the current answer on screen, rebuilding only what changed.
    fn refresh(self: &Rc<Self>) {
        let dirs = self.dir_picks();
        if *self.drawn_dirs.borrow() != dirs {
            self.draw_dirs(&dirs);
            *self.drawn_dirs.borrow_mut() = dirs.clone();
        } else {
            self.mark_dir_highlight(dirs.len());
        }

        let picks = self.row_picks();
        let here_now = launch::tidy_dir(&self.here.borrow());
        let views: Vec<RowView> = picks.iter().map(|p| view(p, &self.home)).collect();
        *self.picks.borrow_mut() = picks;
        if *self.drawn_rows.borrow() != views {
            self.draw_rows(&views, &here_now);
            *self.drawn_rows.borrow_mut() = views.clone();
        } else {
            self.mark_row_highlight(views.len());
        }

        let text = self.query.text().to_string();
        let empty = text.is_empty();
        if self.drawn_bands.get() != empty || self.bands.children().is_empty() && empty {
            self.draw_bands(empty, &here_now);
            self.drawn_bands.set(empty);
        }
        self.save.set_sensitive(!text.trim().is_empty());

        let said = self.said.borrow().clone();
        match note(said.as_deref(), views.len(), &text) {
            Some(line) => {
                self.note.set_text(&line);
                self.note.set_visible(true);
            }
            None => self.note.set_visible(false),
        }
    }

    /// The `in` field's completion list.
    fn draw_dirs(self: &Rc<Self>, dirs: &[String]) {
        for child in self.dir_list.children() {
            self.dir_list.remove(&child);
        }
        for (i, full) in dirs.iter().enumerate() {
            let option = gtk::Button::with_label(launch::leaf(full));
            option.set_tooltip_text(Some(full));
            self.dir_list.pack_start(&option, false, false, 0);
            let me = Rc::downgrade(self);
            let full = full.clone();
            option.connect_clicked(move |_| {
                if let Some(me) = me.upgrade() {
                    me.dir_hi.set(i);
                    me.take_dir(&full);
                }
            });
        }
        self.mark_dir_highlight(dirs.len());
        self.dir_list.show_all();
    }

    /// Say which completion is highlighted, without rebuilding the list.
    ///
    /// The clamp is what keeps a stale highlight from naming a row a newer,
    /// shorter list no longer has.
    fn mark_dir_highlight(&self, count: usize) {
        let cur = self.dir_hi.get().min(count.saturating_sub(1));
        for (i, child) in self.dir_list.children().iter().enumerate() {
            let class = if i == cur { DIROPT_ON } else { DIROPT };
            sheet::set_classes(&child.style_context(), class);
        }
    }

    /// The ranked rows.
    fn draw_rows(self: &Rc<Self>, views: &[RowView], here_now: &str) {
        for child in self.rows.children() {
            self.rows.remove(&child);
        }
        let presets = self.store.borrow().presets.clone();
        for (i, v) in views.iter().enumerate() {
            let body = sheet::row("rg-launch__row-body");
            // A reserved slot, never a conditional element. A row whose
            // Ctrl+digit a saved preset already owns draws no digit, and if
            // the slot collapsed with it the rows either side would sit on two
            // different left edges.
            body.pack_start(
                &sheet::label("rg-launch__key", &key_of(&presets, i)),
                false,
                false,
                0,
            );
            match &v.mark {
                Some(mark) => body.pack_start(
                    &glyph::mark(mark.stroke, mark.fill, "rg-launch__agent"),
                    false,
                    false,
                    0,
                ),
                // A directory row. The box is held so the two kinds of row
                // share one text column; dropping it would step every path row
                // left of every agent row.
                None => {
                    let gap = sheet::row("rg-launch__agent");
                    gap.set_size_request(crate::shell::style::rem(1.0).round() as i32, -1);
                    body.pack_start(&gap, false, false, 0);
                }
            }
            let text = sheet::label("rg-launch__text", &v.text);
            text.set_hexpand(true);
            body.pack_start(&text, true, true, 0);
            // A place chip only when the row would run somewhere OTHER than
            // the `in` field says. The field already states the common case;
            // repeating it on every row is the same number twice.
            if let Some((place, full)) = &v.place
                && launch::tidy_dir(full) != here_now
            {
                let chip = sheet::label("rg-launch__place", place);
                chip.set_tooltip_text(Some(full));
                body.pack_end(&chip, false, false, 0);
            }
            if let Some(branch) = &v.branch {
                body.pack_end(&sheet::label("rg-launch__branch", branch), false, false, 0);
            }

            let button = gtk::Button::new();
            button.add(&body);
            button.set_tooltip_text(Some(&v.tip));
            let me = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(me) = me.upgrade() {
                    me.take(i);
                }
            });
            self.rows.pack_start(&button, false, false, 0);
        }
        self.mark_row_highlight(views.len());
        self.rows.show_all();
    }

    /// Say which row is highlighted, without rebuilding the list.
    fn mark_row_highlight(&self, count: usize) {
        let cur = if count == 0 {
            0
        } else {
            self.hi.get().min(count - 1)
        };
        for (i, child) in self.rows.children().iter().enumerate() {
            let class = if i == cur {
                "rg-launch__row rg-launch__row--on"
            } else {
                "rg-launch__row"
            };
            sheet::set_classes(&child.style_context(), class);
        }
    }

    /// The saved presets and the recents, shown only with an empty query.
    ///
    /// Once the operator is typing, the ranked list is the answer and a second
    /// list beside it is noise.
    fn draw_bands(&self, show: bool, here_now: &str) {
        for child in self.bands.children() {
            self.bands.remove(&child);
        }
        if !show {
            return;
        }
        let store = self.store.borrow();
        if let Some(band) = presets::native::band(&self.shell, &store.presets, here_now) {
            self.bands.pack_start(&band, false, false, 0);
        }
        let projects = self.shell.peek(|st| st.daemon.projects.clone());
        self.bands.pack_start(
            &recents::native::band(&self.shell, launch::recents(&store), &projects, &self.home),
            false,
            false,
            0,
        );
        self.bands.show_all();
    }

    /// Make `full` the place, and offer what is inside it.
    ///
    /// The separator is what makes the next Tab offer the contents rather than
    /// re-offer the folder, exactly as a shell does.
    fn take_dir(self: &Rc<Self>, full: &str) {
        let mut next = shorten_home(full, &self.home);
        next.push(MAIN_SEPARATOR);
        self.dir_field.set_text(&next);
        self.dir_field.set_position(-1);
    }

    /// Take row `i`: launch it, or make it the place.
    fn take(self: &Rc<Self>, i: usize) {
        let pick = match self.picks.borrow().get(i) {
            Some(p) => p.clone(),
            None => return,
        };
        match pick {
            Pick::Cd(path) => {
                *self.here.borrow_mut() = path;
                self.query.set_text("");
                self.hi.set(0);
                self.armed.set(false);
                *self.said.borrow_mut() = None;
                self.rebuild_intents();
                self.refresh();
            }
            Pick::Go(intent) => match attempt(&intent, self.armed.get()) {
                // Sent, not recorded. The reducer is the single place a launch
                // leaves this client, so recording it here as well would count
                // one launch twice and skew the ranking this surface is built
                // on.
                Attempt::Go(l) => go(&self.shell, l),
                Attempt::Warn(why) => {
                    self.armed.set(true);
                    *self.said.borrow_mut() =
                        Some(format!("{why} Take it again to run it anyway."));
                    self.refresh();
                }
                Attempt::Refuse(why) => {
                    self.armed.set(false);
                    *self.said.borrow_mut() = Some(why);
                    self.refresh();
                }
            },
        }
    }

    /// Keep the typed line as a preset, and report what happened.
    ///
    /// One function behind Ctrl+S and the Save control, so the two cannot
    /// drift into saving different things or explaining one refusal two ways.
    /// The profile is the copy this surface loaded when it opened and is never
    /// re-read here: a file read on a keypress is what the open-path rule
    /// exists to stop.
    ///
    /// The label is the command line, because asking for a name is a second
    /// question at the exact moment the operator wanted to start working.
    /// Settings renames it and binds a chord to it.
    fn save_typed(self: &Rc<Self>) {
        let line = self.query.text().trim().to_string();
        let cwd = launch::tidy_dir(&self.here.borrow());
        let existing = self.store.borrow().presets.clone();
        let said = match launch::preset_from_typed(&line, &cwd, &existing) {
            Ok(preset) => {
                let label = preset.label.clone();
                let mut next = existing;
                next.push(preset);
                match launch::save_presets(&next) {
                    Ok(()) => {
                        self.store.borrow_mut().presets = next;
                        format!("Saved \u{201c}{label}\u{201d}. Bind a key to it in Settings.")
                    }
                    Err(why) => why,
                }
            }
            Err(why) => why,
        };
        *self.said.borrow_mut() = Some(said);
        self.rebuild_intents();
        self.refresh();
    }

    /// What the two fields do with a key press.
    fn key(self: &Rc<Self>, field: Field, event: &gdk::EventKey) -> glib::Propagation {
        let keyval = event.keyval();
        let state = event.state();
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = state.contains(gdk::ModifierType::MOD1_MASK);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        if ctrl && !alt && field == Field::What {
            match keyval.to_unicode() {
                Some('s' | 'S') => {
                    self.save_typed();
                    return glib::Propagation::Stop;
                }
                Some(digit @ '1'..='9') => {
                    let n = digit as usize - '0' as usize;
                    self.take(n - 1);
                    return glib::Propagation::Stop;
                }
                _ => {}
            }
        }
        if ctrl || alt {
            return glib::Propagation::Proceed;
        }

        let count = match field {
            Field::Where => self.drawn_dirs.borrow().len(),
            Field::What => self.picks.borrow().len(),
        };
        let slot = match field {
            Field::Where => &self.dir_hi,
            Field::What => &self.hi,
        };
        let cur = slot.get().min(count.saturating_sub(1));

        match keyval {
            gdk::keys::constants::Down if count > 0 => {
                slot.set((cur + 1) % count);
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::keys::constants::Up if count > 0 => {
                slot.set((cur + count - 1) % count);
                self.refresh();
                glib::Propagation::Stop
            }
            // Always swallowed on the query. There is nothing else focusable
            // on this surface, so a Tab that got through would move focus out
            // of the launcher entirely.
            gdk::keys::constants::Tab if !shift => {
                match field {
                    Field::Where if count > 0 => {
                        let pick = self.drawn_dirs.borrow().get(cur).cloned();
                        if let Some(pick) = pick {
                            self.take_dir(&pick);
                        }
                    }
                    // Tab with nothing to complete moves to `run`, because a
                    // dead key in a two-field form reads as the field being
                    // broken.
                    Field::Where => self.query.grab_focus(),
                    Field::What => {
                        // Complete the highlighted row into the field rather
                        // than committing it: a directory gains a separator so
                        // the next Tab offers what is inside, and a command is
                        // filled in whole with the caret at the end so a flag
                        // can be added without retyping the line.
                        let chosen = self.picks.borrow().get(cur).cloned();
                        let typed = self.query.text().to_string();
                        if let Some(next) = chosen.as_ref().and_then(|p| completion(p, &typed)) {
                            self.query.set_text(&next);
                            self.query.set_position(-1);
                        }
                    }
                }
                glib::Propagation::Stop
            }
            gdk::keys::constants::Return | gdk::keys::constants::KP_Enter => {
                match field {
                    // The directory is set as you type, so Enter here means
                    // "done with this field", not "launch": launching from the
                    // place field would start whatever the other field happens
                    // to hold.
                    Field::Where => {
                        if count > 0 {
                            let pick = self.drawn_dirs.borrow().get(cur).cloned();
                            if let Some(pick) = pick {
                                self.take_dir(&pick);
                            }
                        }
                        self.query.grab_focus();
                    }
                    Field::What => {
                        if count > 0 {
                            self.take(cur);
                        } else {
                            *self.said.borrow_mut() =
                                Some(no_row_reason(&self.query.text()));
                            self.refresh();
                        }
                    }
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }
}

impl Dialog for Launcher {

    fn root(&self) -> gtk::Widget {
        self.frame.root()
    }
}

/// How much room the launcher wants, in rem.
///
/// Counted rather than guessed, because the two bands are unbounded by
/// anything this surface controls: a profile with nine presets and twelve
/// recents is taller than any window, and the sheet has to know that so the
/// overflow becomes a scroll position rather than rows nobody can reach.
#[cfg(test)]
pub(crate) fn content(
    presets: &[launch::SavedPreset],
    recents: &[launch::RecentEntry],
    dirs: usize,
    rows: usize,
    banded: bool,
) -> (f64, f64) {
    let mut height = HEAD_REM + FIELD_REM * 2.0 + dirs as f64 * OPTION_REM;
    if banded {
        height += presets::native::content(presets).1;
        height += recents::native::content(recents).1;
    }
    height += rows as f64 * ROW_REM + NOTE_REM;
    (0.0, height)
}

/// The sheet's own head, in rem.
#[cfg(test)]
const HEAD_REM: f64 = 2.5;

/// One labelled field, in rem.
#[cfg(test)]
const FIELD_REM: f64 = 3.5;

/// One directory completion, in rem.
#[cfg(test)]
const OPTION_REM: f64 = 1.75;

/// One ranked row, in rem.
#[cfg(test)]
const ROW_REM: f64 = 2.0;

/// The one line the launcher is allowed to say, in rem.
#[cfg(test)]
const NOTE_REM: f64 = 1.5;

/// Rename one session.
///
/// The new title goes to the daemon, not into a client-side map. A title only
/// this window knows vanishes on restart and is invisible to a second window;
/// the server owns session identity, so it owns the name.
pub(crate) fn rename(shell: &Shell, seed: &RenameSeed) -> Rc<Sheet> {
    let panel = sheet::column("rg-sheet__panel");
    panel.pack_start(&sheet::head(shell, "Rename session"), false, false, 0);

    let field = sheet::column("rg-field");
    let entry = gtk::Entry::new();
    entry.style_context().add_class("rg-field__input");
    entry.set_text(&seed.title);
    field.pack_start(&entry, false, false, 0);
    field.pack_start(
        &sheet::label(
            "rg-field__hint",
            "Saved on the daemon, so every window sees it.",
        ),
        false,
        false,
        0,
    );
    panel.pack_start(&field, false, false, 0);

    let error = sheet::label("rg-sheet__error", "");
    error.set_no_show_all(true);
    panel.pack_start(&error, false, false, 0);

    let foot = sheet::row("rg-sheet__foot");
    let cancel = gtk::Button::with_label("Cancel");
    cancel.style_context().add_class("rg-btn");
    let commit = gtk::Button::with_label("Rename");
    commit.style_context().add_class("rg-btn");
    commit.style_context().add_class("rg-btn--primary");
    foot.pack_end(&commit, false, false, 0);
    foot.pack_end(&cancel, false, false, 0);
    panel.pack_start(&foot, false, false, 0);

    let apply = {
        let shell = shell.clone();
        let entry = entry.clone();
        let error = error.clone();
        let session = seed.session;
        move || send_rename(&shell, session, &entry, &error)
    };
    entry.connect_activate({
        let apply = apply.clone();
        move |_| apply()
    });
    entry.connect_changed({
        let error = error.clone();
        move |_| error.set_visible(false)
    });
    entry.connect_map(|entry| entry.grab_focus());
    commit.connect_clicked(move |_| apply());
    cancel.connect_clicked({
        let shell = shell.clone();
        move |_| shell.dismiss()
    });

    Sheet::new(sheet::RENAME, sheet::NARROW, &panel)
}

/// Send the new title, or say why there is none to send.
fn send_rename(shell: &Shell, session: SessionId, entry: &gtk::Entry, error: &gtk::Label) {
    let title = entry.text().trim().to_string();
    if title.is_empty() {
        error.set_text("A session needs a name. Type one, or cancel to keep the current title.");
        error.set_visible(true);
        return;
    }
    shell.send(ClientEvent::Msg {
        msg: vitrum_proto::ClientMsg::Rename { session, title },
    });
    shell.update(|st| st.window.layer = Layer::None);
    shell.dismiss();
}

/// How much room the rename field wants, in rem.
///
/// The refusal line is counted whether or not it is showing. A sheet that
/// grows when it refuses would move the control the operator is about to press
/// at the moment they are reading why it did not work.
#[cfg(test)]
pub(crate) fn rename_content() -> (f64, f64) {
    (0.0, HEAD_REM + FIELD_REM + NOTE_REM + FOOT_REM)
}

/// The row of controls at the foot of a sheet, in rem.
#[cfg(test)]
const FOOT_REM: f64 = 2.5;

#[cfg(test)]
mod tests;
