//! The settings surface, as a GTK dialog over the frame.
//!
//! # Why it is a dialog and not a window
//!
//! A second toplevel is placed by the window manager, and a settings sheet
//! that lands half off the screen or behind the window it belongs to is a
//! defect nothing in this process can fix. [`crate::shell::Shell::present`]
//! puts it in the frame's overlay instead, where the toolkit allocates it over
//! the frame and it cannot touch the pane's rectangle.
//!
//! # Why the pages are built from a table
//!
//! The rows are [`super::spec`], which is data. Everything here walks that
//! table: a page is a loop, a switch is one function, a menu is one function.
//! Nothing about a setting is written twice, so the catalogue check in
//! `spec::tests` is checking the surface an operator actually sees rather than
//! a list beside it.
//!
//! Six settings have a control of their own, because they do something a
//! switch cannot: a path field with Apply, an import that reads a file and can
//! fail, a chord recorder, a URL whose Save also redials. Those are built by
//! hand below and named in [`super::spec::BESPOKE`], so the completeness check
//! stays total.
//!
//! # Every control writes through one function
//!
//! [`edit`] mutates the document, publishes it to
//! [`crate::state::live`] and queues the write, in that order, inside one
//! [`crate::shell::Shell::update`]. A control that wrote the state and skipped
//! the publish would be a setting that does not reach the pane until a
//! restart, which is the defect this whole surface is being rebuilt to remove.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};

use gtk::prelude::*;

use crate::shell::{Dialog, Shell};
use crate::state::{Layer, Settings, SettingsTab};

use super::spec::{self, Control, Row};
use super::{commit, flush, host_palette_note, notify_support, system_theme_reads, when_note};

/// The settings sheet.
///
/// Held as an `Rc` by the shell for as long as it is presented. Every handler
/// captures a [`Weak`] rather than a strong handle: a strong one would make
/// the widget tree own the sheet that owns the widget tree, and the whole
/// graph would outlive every dismissal.
pub(crate) struct SettingsSheet {
    shell: Shell,
    root: gtk::Box,
    /// The tab strip, rebuilt only when the sheet is built.
    tabs: gtk::Box,
    /// The page body, emptied and refilled on every page change and commit.
    body: gtk::Box,
    tab: Cell<SettingsTab>,
    /// The last refusal, printed above the page until something succeeds.
    error: RefCell<String>,
    /// Text an operator typed that is not in the document yet.
    ///
    /// A page is rebuilt after every commit, which destroys its entries. A
    /// path being typed into is not a commit, so without this the backdrop
    /// field would empty itself the moment an unrelated switch was flipped.
    drafts: RefCell<BTreeMap<&'static str, String>>,
    /// What the update control is saying right now.
    update: RefCell<UpdateUi>,
}

/// What the update control is doing.
///
/// One value rather than a set of booleans, because the states are mutually
/// exclusive and a pair of flags is how a control ends up saying "checking"
/// and "up to date" at once.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum UpdateUi {
    #[default]
    Idle,
    Busy(String),
    Answer(crate::update::Status),
    Installed(String),
    Failed(String),
}

/// Present the settings sheet when `layer` asks for it.
///
/// The entry point every window uses. Named for the layer rather than for the
/// sheet so the caller does not have to know which surfaces this module owns.
pub(crate) fn present_layer(shell: &Shell, layer: &Layer) {
    let Layer::Settings(tab) = layer else {
        return;
    };
    shell.present(SettingsSheet::new(shell, *tab));
}

impl SettingsSheet {
    /// Build the sheet, open on `tab`.
    pub(crate) fn new(shell: &Shell, tab: SettingsTab) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.style_context().add_class("rg-sheet");
        root.style_context().add_class("rg-settings");

        let head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        head.style_context().add_class("rg-sheet__head");
        let title = gtk::Label::new(Some("Settings"));
        title.style_context().add_class("rg-sheet__title");
        title.set_halign(gtk::Align::Start);
        head.pack_start(&title, true, true, 0);
        let close = gtk::Button::with_label("Close");
        close.style_context().add_class("rg-btn-inline");
        head.pack_end(&close, false, false, 0);
        root.pack_start(&head, false, false, 0);

        let split = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        split.style_context().add_class("rg-sheet__body");
        let tabs = gtk::Box::new(gtk::Orientation::Vertical, 0);
        tabs.style_context().add_class("rg-sheet__tabs");
        split.pack_start(&tabs, false, false, 0);

        let scroller = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_hexpand(true);
        scroller.set_vexpand(true);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.style_context().add_class("rg-sheet__panel");
        scroller.add(&body);
        split.pack_start(&scroller, true, true, 0);
        root.pack_start(&split, true, true, 0);

        let this = Rc::new(Self {
            shell: shell.clone(),
            root,
            tabs,
            body,
            tab: Cell::new(tab),
            error: RefCell::new(String::new()),
            drafts: RefCell::new(BTreeMap::new()),
            update: RefCell::new(UpdateUi::default()),
        });

        {
            let shell = shell.clone();
            close.connect_clicked(move |_| shell.dismiss());
        }
        this.build_tabs();
        this.build_page();
        this
    }

    /// The tab strip.
    ///
    /// Built once. Which tab is active is a style class the rebuild toggles,
    /// so switching pages never destroys the strip the pointer is over.
    fn build_tabs(self: &Rc<Self>) {
        for entry in SettingsTab::ALL {
            let button = gtk::Button::with_label(entry.label());
            button.style_context().add_class("rg-settings__tab");
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                if this.tab.get() == entry {
                    return;
                }
                this.tab.set(entry);
                this.shell.update(move |st| st.set_settings_tab(entry));
                this.build_page();
            });
            self.tabs.pack_start(&button, false, false, 0);
        }
    }

    /// Mark the active tab, and nothing else.
    fn mark_tab(&self) {
        let active = self.tab.get();
        for (index, child) in self.tabs.children().iter().enumerate() {
            let context = child.style_context();
            if SettingsTab::ALL.get(index) == Some(&active) {
                context.add_class("rg-settings__tab--active");
            } else {
                context.remove_class("rg-settings__tab--active");
            }
        }
    }

    /// Rebuild the page later, once the handler that asked has returned.
    ///
    /// Deferred because the caller is usually a signal on a widget this is
    /// about to destroy. Rebuilding underneath a running handler is how a
    /// toolkit ends up delivering a second signal to a widget that is already
    /// unparented.
    fn refresh(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(this) = weak.upgrade() {
                this.build_page();
            }
        });
    }

    /// Empty the body and fill it with the current page.
    fn build_page(self: &Rc<Self>) {
        for child in self.body.children() {
            self.body.remove(&child);
        }
        self.mark_tab();

        let error = self.error.borrow().clone();
        if !error.is_empty() {
            let label = wrapped(&error);
            label.style_context().add_class("rg-sheet__error");
            self.body.pack_start(&label, false, false, 0);
        }

        let settings = self.settings();
        let tab = self.tab.get();
        let host = self.host();
        match tab {
            SettingsTab::Workspaces => {
                let page = super::workspaces::page(&host);
                self.body.pack_start(&page, false, false, 0);
            }
            SettingsTab::Presets => {
                let page = super::presets::page(&host);
                self.body.pack_start(&page, false, false, 0);
            }
            SettingsTab::Keyboard => {
                let page = crate::ui::keybinds::page(&host);
                self.body.pack_start(&page, false, false, 0);
            }
            _ => {
                self.build_rows(tab, &settings);
            }
        }
        self.body.show_all();
    }

    /// Draw a page's declared rows, with its bespoke controls interleaved.
    fn build_rows(self: &Rc<Self>, tab: SettingsTab, settings: &Settings) {
        if tab == SettingsTab::Notifications {
            self.notifications_head();
        }
        if tab == SettingsTab::About {
            self.about_head(settings);
        }
        if tab == SettingsTab::Advanced {
            self.advanced(settings);
        }
        if tab == SettingsTab::Keyboard {
            return;
        }

        for row in spec::rows(tab) {
            if !row.is_visible(settings) {
                continue;
            }
            let widget = match &row.control {
                Control::Switch { get, set } => self.switch_row(row, settings, *get, *set),
                Control::Choice { options, get, set } => {
                    self.choice_row(row, settings, *options, *get, *set)
                }
            };
            self.body.pack_start(&widget, false, false, 0);

            // Bespoke controls sit where the operator expects to find them,
            // which is beside the declared row they belong with rather than
            // in a block of their own at the bottom of the page.
            match row.path {
                "theme" if settings.theme == crate::state::ThemePref::System => {
                    self.reread_theme();
                }
                "appearance.terminalOpacityPct" => self.backdrop_field(settings),
                "terminal.presentMode" => self.host_palette(settings),
                _ => {}
            }
        }

        if tab == SettingsTab::Notifications {
            let note = wrapped(
                "Clicking a notification opens the session it is about in a new window, through \
                 the same vitrum://session/<id> handoff a link from a browser takes. The window \
                 you are in is left where it is.",
            );
            note.style_context().add_class("rg-sheet__note");
            self.body.pack_start(&note, false, false, 0);
        }
    }

    /// The document in force.
    fn settings(&self) -> Settings {
        self.shell.peek(|st| st.daemon.settings.clone())
    }

    /// Mutate the document, publish it, and queue the write.
    ///
    /// The one path every control on this surface takes. Publishing happens
    /// inside the same update as the mutation, so a listener can never observe
    /// a document the state signal does not already hold.
    fn edit(&self, change: impl FnOnce(&mut Settings) + 'static) {
        self.shell.update(move |st| {
            change(&mut st.daemon.settings);
            commit(st);
        });
    }

    /// Record a refusal and redraw, or clear one.
    fn set_error(self: &Rc<Self>, why: String) {
        *self.error.borrow_mut() = why;
        self.refresh();
    }

    /// The text an operator has typed into `key`, or `fallback`.
    fn draft(&self, key: &'static str, fallback: &str) -> String {
        self.drafts
            .borrow()
            .get(key)
            .cloned()
            .unwrap_or_else(|| fallback.to_string())
    }

    // -- Rows ---------------------------------------------------------------

    /// A labelled on/off row.
    fn switch_row(
        self: &Rc<Self>,
        row: &'static Row,
        settings: &Settings,
        get: fn(&Settings) -> bool,
        set: fn(&mut Settings, bool),
    ) -> gtk::Widget {
        let field = self.field(row.label, &row.caption(settings), when_note(row.path));
        let switch = gtk::Switch::new();
        switch.style_context().add_class("rg-switch");
        switch.set_active(get(settings));
        let weak = Rc::downgrade(self);
        // Connected after the value is set, so seeding the control cannot look
        // like the operator having flipped it.
        switch.connect_state_set(move |_, want| {
            if let Some(this) = weak.upgrade() {
                this.edit(move |s| set(s, want));
                this.refresh();
            }
            glib::Propagation::Proceed
        });
        field.control.pack_start(&switch, false, false, 0);
        field.root.upcast()
    }

    /// A labelled menu row.
    fn choice_row(
        self: &Rc<Self>,
        row: &'static Row,
        settings: &Settings,
        options: fn() -> Vec<(String, String)>,
        get: fn(&Settings) -> String,
        set: fn(&mut Settings, &str),
    ) -> gtk::Widget {
        let field = self.field(row.label, &row.caption(settings), when_note(row.path));
        let combo = gtk::ComboBoxText::new();
        combo.style_context().add_class("rg-select");
        let current = get(settings);
        let offered = options();
        // A stored value the menu cannot express gets an entry of its own
        // rather than being swallowed. A combo whose active id matches nothing
        // shows blank, which reads as a setting with no value at all; the
        // honest answer is to name the state the operator is in.
        if let Some(label) = stray_option(&current, &offered) {
            combo.append(Some(&current), &label);
        }
        for (value, label) in &offered {
            combo.append(Some(value), label);
        }
        combo.set_active_id(Some(&current));
        let weak = Rc::downgrade(self);
        combo.connect_changed(move |combo| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let Some(picked) = combo.active_id() else {
                return;
            };
            this.edit(move |s| set(s, picked.as_str()));
            this.refresh();
        });
        field.control.pack_start(&combo, false, false, 0);
        field.root.upcast()
    }

    /// A handle on this sheet for a page that builds its own controls.
    fn host(self: &Rc<Self>) -> Host {
        Host {
            shell: self.shell.clone(),
            sheet: Rc::downgrade(self),
        }
    }

    /// The shared shape of every row: label, control, caption, timing.
    ///
    /// Delegated so a declared row and a hand-built one are the same widget
    /// tree. Two shapes is how one page ends up with captions the others do
    /// not have.
    fn field(self: &Rc<Self>, label: &str, desc: &str, when: &str) -> Field {
        self.host().field(label, desc, when)
    }

    /// A row whose only content is a button and a caption.
    fn action_row(self: &Rc<Self>, label: &str, desc: &str) -> Field {
        self.field(label, desc, "")
    }

    // -- Appearance ---------------------------------------------------------

    /// Ask the desktop what it looks like, again.
    fn reread_theme(self: &Rc<Self>) {
        let field = self.action_row(
            "",
            "Read on demand rather than watched. A background watcher would park a thread on a \
             D-Bus signal for the life of the process, and idle cost is the point of this \
             product.",
        );
        let button = gtk::Button::with_label("Re-read the system theme");
        button.style_context().add_class("rg-btn");
        let weak = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            // The read is reported, not discarded. A desktop that names no
            // preference leaves Auto on the light theme, and without a
            // sentence that case is indistinguishable from a button that did
            // nothing.
            let read = super::refresh_system_theme();
            if let Some(this) = weak.upgrade() {
                this.set_error(if read.is_some() {
                    String::new()
                } else {
                    "This desktop does not report a theme preference, so Auto stays on the light \
                     theme until it does."
                        .to_string()
                });
            }
        });
        field.control.pack_start(&button, false, false, 0);
        self.body.pack_start(&field.root, false, false, 0);
    }

    /// The backdrop path, with Apply and Clear.
    ///
    /// A field and a button rather than a live-bound entry: an absolute path
    /// typed one character at a time would ask the loader to read a file per
    /// keystroke, and every one of those reads but the last is of a path that
    /// does not exist.
    fn backdrop_field(self: &Rc<Self>, settings: &Settings) {
        let field = self.field(
            "Backdrop",
            "An absolute path to a PNG, JPEG, GIF or WEBP. Read by signature and not by \
             extension, so a file that is not an image is refused rather than drawn. SVG is \
             refused too: it is a scripted document.",
            when_note("appearance.backdrop"),
        );
        let entry = gtk::Entry::new();
        entry.style_context().add_class("rg-field__input");
        entry.set_placeholder_text(Some("/src/vitrum/wallpaper.png"));
        entry.set_hexpand(true);
        entry.set_text(&self.draft("appearance.backdrop", &settings.appearance.backdrop));
        {
            let weak = Rc::downgrade(self);
            entry.connect_changed(move |entry| {
                if let Some(this) = weak.upgrade() {
                    this.drafts
                        .borrow_mut()
                        .insert("appearance.backdrop", entry.text().to_string());
                }
            });
        }
        field.control.pack_start(&entry, true, true, 0);

        let apply = gtk::Button::with_label("Apply");
        apply.style_context().add_class("rg-btn");
        apply.style_context().add_class("rg-btn--primary");
        {
            let weak = Rc::downgrade(self);
            let entry = entry.clone();
            apply.connect_clicked(move |_| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let next = entry.text().trim().to_string();
                this.drafts.borrow_mut().remove("appearance.backdrop");
                this.edit(move |s| s.appearance.backdrop.clone_from(&next));
                this.refresh();
            });
        }
        field.control.pack_start(&apply, false, false, 0);

        if !settings.appearance.backdrop.is_empty() {
            let clear = gtk::Button::with_label("Clear");
            clear.style_context().add_class("rg-btn");
            let weak = Rc::downgrade(self);
            clear.connect_clicked(move |_| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                this.drafts.borrow_mut().remove("appearance.backdrop");
                this.edit(|s| s.appearance.backdrop.clear());
                this.refresh();
            });
            field.control.pack_start(&clear, false, false, 0);
        }
        self.body.pack_start(&field.root, false, false, 0);
    }

    // -- Terminal -----------------------------------------------------------

    /// Follow this machine's terminal, import a named file, and rescan.
    ///
    /// Three controls and one setting, because reading a palette off this
    /// machine can fail and a switch that springs back with no sentence is the
    /// shape of "the palette ignores my terminal".
    fn host_palette(self: &Rc<Self>, settings: &Settings) {
        let follow = self.field(
            "Follow this machine's terminal colours",
            &host_palette_note(&settings.terminal),
            when_note("terminal.followHostTerminal"),
        );
        let switch = gtk::Switch::new();
        switch.style_context().add_class("rg-switch");
        switch.set_active(settings.terminal.follow_host_terminal);
        {
            let weak = Rc::downgrade(self);
            switch.connect_state_set(move |_, want| {
                let Some(this) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if !want {
                    this.edit(|s| s.terminal.follow_host_terminal = false);
                    this.set_error(String::new());
                    return glib::Propagation::Proceed;
                }
                match super::import_host_palette() {
                    Ok(found) => {
                        this.edit(move |s| {
                            s.terminal.host_palette = found;
                            s.terminal.follow_host_terminal = true;
                        });
                        this.set_error(String::new());
                    }
                    Err(why) => this.refuse(why.to_string()),
                }
                glib::Propagation::Proceed
            });
        }
        follow.control.pack_start(&switch, false, false, 0);
        self.body.pack_start(&follow.root, false, false, 0);

        let named = self.field(
            "Import a named file",
            "The way in for a terminal the scan above does not know. Which of the four shapes \
             the file is in is decided by what is in it rather than by its name, so a palette \
             exported to any filename is read. A successful import turns the switch above on \
             and the row above names the shape it was read as.",
            when_note("terminal.hostPalette"),
        );
        let entry = gtk::Entry::new();
        entry.style_context().add_class("rg-field__input");
        entry.set_placeholder_text(Some("~/src/dotfiles/colours.conf"));
        entry.set_hexpand(true);
        entry.set_text(&self.draft("terminal.hostPalette", ""));
        {
            let weak = Rc::downgrade(self);
            entry.connect_changed(move |entry| {
                if let Some(this) = weak.upgrade() {
                    this.drafts
                        .borrow_mut()
                        .insert("terminal.hostPalette", entry.text().to_string());
                }
            });
        }
        named.control.pack_start(&entry, true, true, 0);

        let import = gtk::Button::with_label("Import");
        import.style_context().add_class("rg-btn");
        import.style_context().add_class("rg-btn--primary");
        {
            let weak = Rc::downgrade(self);
            let entry = entry.clone();
            import.connect_clicked(move |_| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let path = entry.text().trim().to_string();
                if path.is_empty() {
                    this.refuse("Name a file to import colours from.".to_string());
                    return;
                }
                match crate::state::hostterm::import_file(
                    std::path::Path::new(&path),
                    |p: &std::path::Path| std::fs::read_to_string(p),
                ) {
                    Ok(found) => {
                        this.drafts.borrow_mut().remove("terminal.hostPalette");
                        this.edit(move |s| {
                            s.terminal.host_palette = found;
                            s.terminal.follow_host_terminal = true;
                        });
                        this.set_error(String::new());
                    }
                    Err(why) => this.refuse(why.to_string()),
                }
            });
        }
        named.control.pack_start(&import, false, false, 0);
        self.body.pack_start(&named.root, false, false, 0);

        if settings.terminal.follow_host_terminal && settings.terminal.host_palette.is_complete() {
            let again = self.action_row(
                "Imported colours",
                "The import is stored, not repeated each launch, so the grid does not change \
                 colour because a configuration file moved.",
            );
            let button = gtk::Button::with_label("Read the colours again");
            button.style_context().add_class("rg-btn");
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                match super::import_host_palette() {
                    Ok(found) => {
                        this.edit(move |s| s.terminal.host_palette = found);
                        this.set_error(String::new());
                    }
                    Err(why) => this.refuse(why.to_string()),
                }
            });
            again.control.pack_start(&button, false, false, 0);
            self.body.pack_start(&again.root, false, false, 0);
        }
    }

    /// Show a refusal on the sheet and on the window's flash strip.
    ///
    /// Both, because the sheet is where the operator is looking and the flash
    /// is what survives the sheet being closed on the way to fixing it.
    fn refuse(self: &Rc<Self>, why: String) {
        let flash = why.clone();
        self.shell
            .update(move |st| st.window.flash = Some(crate::state::Flash::error(flash)));
        self.set_error(why);
    }

    // -- Notifications ------------------------------------------------------

    /// Say when this desktop cannot deliver a notification at all.
    fn notifications_head(self: &Rc<Self>) {
        let support = notify_support();
        let Some(why) = support.reason() else {
            return;
        };
        let label = wrapped(&format!(
            "This desktop cannot deliver notifications: {why}. The switches below still record \
             your preference, but nothing will be shown until the service is available."
        ));
        label.style_context().add_class("rg-sheet__warn");
        self.body.pack_start(&label, false, false, 0);
    }

    // -- About --------------------------------------------------------------

    /// Versions, the update controls, and the command-line equivalent.
    fn about_head(self: &Rc<Self>, _settings: &Settings) {
        let current = crate::update::current_version();
        let daemon = self.shell.peek(|st| match &st.daemon.conn {
            crate::state::ConnState::Live { server_version } => Some(server_version.clone()),
            _ => None,
        });
        let stale = daemon.as_deref().is_some_and(|v| v != current.to_string());

        let version = self.field(
            "Version",
            &format!("vitrum {current} ({})", crate::update::TARGET),
            "",
        );
        let about_daemon = wrapped(&match &daemon {
            Some(v) if stale => format!(
                "The daemon holding your sessions is still running {v}. Restarting it picks up \
                 {current} and ends every session it is holding."
            ),
            Some(v) => format!("Daemon {v}, running your sessions."),
            None => "Not connected to a daemon.".to_string(),
        });
        about_daemon.style_context().add_class("rg-field__hint");
        version.root.pack_start(&about_daemon, false, false, 0);
        self.body.pack_start(&version.root, false, false, 0);

        let state = self.update.borrow().clone();
        let updates = self.field("Updates", "", &self.update_sentence(&state, &current));
        let check = gtk::Button::with_label("Check for updates");
        check.style_context().add_class("rg-btn");
        check.style_context().add_class("rg-btn--primary");
        check.set_sensitive(!matches!(state, UpdateUi::Busy(_)));
        {
            let weak = Rc::downgrade(self);
            check.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.check_for_updates();
                }
            });
        }
        updates.control.pack_start(&check, false, false, 0);

        let available = self.shell.update_offer();
        if let Some(available) = available {
            let install = gtk::Button::with_label(&format!("Install {}", available.version));
            install.style_context().add_class("rg-btn");
            install.style_context().add_class("rg-btn--primary");
            install.set_sensitive(!matches!(state, UpdateUi::Busy(_)));
            let weak = Rc::downgrade(self);
            install.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.install_update(available.clone());
                }
            });
            updates.control.pack_start(&install, false, false, 0);
        }
        self.body.pack_start(&updates.root, false, false, 0);

        let terminal = self.field(
            "From a terminal",
            "",
            "vitrum update --check   reports what is available and installs nothing. \
             vitrum update           installs it. Same code as the button above.",
        );
        self.body.pack_start(&terminal.root, false, false, 0);
    }

    /// What the update control says under its buttons.
    fn update_sentence(&self, state: &UpdateUi, current: &semver::Version) -> String {
        match state {
            UpdateUi::Idle => format!(
                "Checks the latest release of {}, never the branch. The download's checksum must \
                 match the one published beside it.",
                crate::update::REPO
            ),
            UpdateUi::Busy(step) => step.clone(),
            UpdateUi::Answer(crate::update::Status::UpToDate { version }) => {
                format!("vitrum {version} is the newest release.")
            }
            UpdateUi::Answer(crate::update::Status::NoReleases) => {
                format!("No releases published for {} yet.", crate::update::REPO)
            }
            UpdateUi::Answer(crate::update::Status::NoAssetForPlatform { version, target }) => {
                format!(
                    "vitrum {version} is available but published no build for {target}. Build it \
                     from source."
                )
            }
            UpdateUi::Answer(crate::update::Status::Ready(a)) => {
                format!("vitrum {} is available. You have {current}.", a.version)
            }
            UpdateUi::Installed(v) => format!("Staged {v}. {}", crate::update::AFTER_INSTALL),
            UpdateUi::Failed(why) => why.clone(),
        }
    }

    /// Ask the release feed what is out, off the thread that paints.
    ///
    /// The check is a blocking HTTP round trip. On the main loop it would
    /// freeze every window in this process, since they share one.
    fn check_for_updates(self: &Rc<Self>) {
        *self.update.borrow_mut() = UpdateUi::Busy("checking".to_string());
        self.shell.set_update_offer(None);
        self.refresh();
        let weak = Rc::downgrade(self);
        off_thread(crate::update::check, move |got| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            match got {
                Ok(status) => {
                    // Written to the shell rather than kept here, so the
                    // titlebar's chip and this page cannot disagree about
                    // which release is on offer.
                    this.shell.set_update_offer(match &status {
                        crate::update::Status::Ready(a) => Some(a.clone()),
                        _ => None,
                    });
                    *this.update.borrow_mut() = UpdateUi::Answer(status);
                }
                Err(e) => *this.update.borrow_mut() = UpdateUi::Failed(format!("{e:#}")),
            }
            this.build_page();
        });
    }

    /// Download, verify and stage a release.
    fn install_update(self: &Rc<Self>, available: crate::update::Available) {
        *self.update.borrow_mut() = UpdateUi::Busy("starting".to_string());
        self.refresh();
        let weak = Rc::downgrade(self);
        let version = available.version.to_string();
        off_thread(
            move || {
                let dir = crate::update::install_dir()?;
                if !crate::update::writable(&dir) {
                    anyhow::bail!(
                        "cannot write to {}. This copy was installed by something else; update \
                         it the same way.",
                        dir.display()
                    );
                }
                crate::update::install(&available, &dir, &mut |_| {})?;
                Ok::<_, anyhow::Error>(())
            },
            move |done| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                match done {
                    Ok(()) => {
                        // Staged, so the offer is spent. Clearing it on the
                        // shell is what takes the titlebar's chip down too.
                        this.shell.set_update_offer(None);
                        *this.update.borrow_mut() = UpdateUi::Installed(version);
                    }
                    Err(e) => *this.update.borrow_mut() = UpdateUi::Failed(format!("{e:#}")),
                }
                this.build_page();
            },
        );
    }

    // -- Advanced -----------------------------------------------------------

    /// The daemon URL, the platform probe, and what the live bus has delivered.
    fn advanced(self: &Rc<Self>, settings: &Settings) {
        let field = self.field(
            "Daemon",
            "",
            "Empty means whatever --server said on the command line, which keeps the flag \
             authoritative for the case it exists for. Saving reconnects immediately.",
        );
        let entry = gtk::Entry::new();
        entry.style_context().add_class("rg-field__input");
        entry.set_placeholder_text(Some("ws://127.0.0.1:7737"));
        entry.set_hexpand(true);
        entry.set_text(&self.draft("daemonUrl", &settings.daemon_url));
        {
            let weak = Rc::downgrade(self);
            entry.connect_changed(move |entry| {
                if let Some(this) = weak.upgrade() {
                    this.drafts
                        .borrow_mut()
                        .insert("daemonUrl", entry.text().to_string());
                }
            });
        }
        field.control.pack_start(&entry, true, true, 0);
        let save = gtk::Button::with_label("Save and reconnect");
        save.style_context().add_class("rg-btn");
        save.style_context().add_class("rg-btn--primary");
        {
            let weak = Rc::downgrade(self);
            let entry = entry.clone();
            save.connect_clicked(move |_| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let next = entry.text().trim().to_string();
                this.drafts.borrow_mut().remove("daemonUrl");
                this.edit(move |s| s.daemon_url.clone_from(&next));
                // Read back through the resolver rather than dialling what was
                // typed: an empty field means "whatever the command line said",
                // and that answer lives on the document and not in the entry.
                let dial = this
                    .shell
                    .peek(|st| st.daemon.settings.resolved_daemon_url("").to_string());
                if !dial.is_empty() {
                    this.shell.send(crate::wire::ClientEvent::Redial { url: dial });
                }
                this.refresh();
            });
        }
        field.control.pack_start(&save, false, false, 0);
        self.body.pack_start(&field.root, false, false, 0);

        // Eight probes, several of them a service handshake. Once per build of
        // this page, which only happens when the operator asks for it.
        let report = vitrum_os::probe(None);
        let rows: Vec<(String, String)> = report
            .iter()
            .map(|(feature, support)| (feature.to_string(), support.to_string()))
            .collect();
        self.readout(
            "Platform integration",
            "Probed live, just now. Anything unavailable says why rather than failing silently \
             later.",
            &rows,
            "",
        );

        // What a control the operator just changed actually reached. A pane
        // that did not repaint is either a publish that never happened or a
        // fan-out that was skipped because the derived snapshot did not move,
        // and those two have different fixes.
        let live = [
            (
                "documents published",
                crate::state::live::publishes().to_string(),
            ),
            (
                "reached the pane",
                crate::state::live::pane_fanouts().to_string(),
            ),
            (
                "reached the shell",
                crate::state::live::shell_fanouts().to_string(),
            ),
            (
                "reached key dispatch",
                crate::state::live::keyboard_fanouts().to_string(),
            ),
            ("desktop appearance reads", system_theme_reads().to_string()),
        ]
        .map(|(a, b)| (a.to_string(), b));
        self.readout(
            "Live apply",
            "Counted since this process started. A publish reaches an audience only when the \
             values that audience reads actually changed, so typing into a field nothing outside \
             this sheet reads costs no deliveries at all.",
            &live,
            "",
        );

        // A picture that tears or stutters is either frames that cost too much
        // or frames that were never drawn, and those have different fixes: p99
        // answers the first, the skipped count answers the second.
        let frames = super::frame_rows(self.shell.ident().ordinal);
        self.readout(
            "Frame pacing",
            "This window's terminal pane. Percentiles cover the last few thousand frames, the \
             worst covers the whole run, and the time measured is how long the thread that reads \
             keystrokes was busy drawing rather than how long the GPU took.",
            &frames,
            "This window has no pane, so there is no frame clock to report.",
        );

        // The daemon already knows why its watcher is partial and says so in
        // finished sentences. Storing them and rendering nothing is how the
        // contested-files marker went missing on two platforms with no screen
        // saying why.
        let collisions = self.shell.peek(|st| st.daemon.collisions.clone());
        let reasons: Vec<(String, String)> = collisions
            .reasons()
            .iter()
            .map(|reason| (String::new(), reason.clone()))
            .collect();
        self.readout("Contested files", &collisions.summary(), &reasons, "");

        let path = match crate::state::ui_state_path() {
            Ok(p) => p.display().to_string(),
            Err(why) => format!("no config directory on this platform: {why}"),
        };
        let file = self.field("Settings file", "", &path);
        self.body.pack_start(&file.root, false, false, 0);
    }

    /// A block of name/value lines under one caption.
    fn readout(self: &Rc<Self>, label: &str, desc: &str, rows: &[(String, String)], empty: &str) {
        let field = self.field(label, desc, "");
        if rows.is_empty() {
            if !empty.is_empty() {
                let hint = wrapped(empty);
                hint.style_context().add_class("rg-field__hint");
                field.root.pack_start(&hint, false, false, 0);
            }
            self.body.pack_start(&field.root, false, false, 0);
            return;
        }
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        list.style_context().add_class("rg-keys");
        for (name, value) in rows {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            line.style_context().add_class("rg-keys__row");
            if !name.is_empty() {
                let left = gtk::Label::new(Some(name));
                left.style_context().add_class("rg-keys__chord");
                left.set_halign(gtk::Align::Start);
                line.pack_start(&left, false, false, 0);
            }
            let right = wrapped(value);
            right.style_context().add_class("rg-keys__what");
            line.pack_start(&right, true, true, 0);
            list.pack_start(&line, false, false, 0);
        }
        field.root.pack_start(&list, false, false, 0);
        self.body.pack_start(&field.root, false, false, 0);
    }
}

impl Dialog for SettingsSheet {

    fn root(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    /// The sheet is the one surface whose whole purpose is changing settings,
    /// so its close is the moment the coalescing write timer is most likely
    /// still holding a document. Everything else can wait for the timer.
    fn dismissed(&self) {
        flush();
        self.shell.update(|st| st.window.layer = Layer::None);
    }
}

/// The two boxes every row is made of: the row itself, and the strip its
/// controls are packed into.
pub(crate) struct Field {
    pub(crate) root: gtk::Box,
    pub(crate) control: gtk::Box,
}

/// A label that wraps rather than widening the sheet.
pub(crate) fn wrapped(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_halign(gtk::Align::Start);
    label.set_xalign(0.0);
    label.set_line_wrap(true);
    label.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
    label
}

/// The extra menu entry a stored value that matches no choice needs.
///
/// A menu whose active id matches nothing renders blank, so the control shows
/// no setting at all while one is in force. Every numeric preference here can
/// reach that state: the clamps enforce a RANGE while these menus offer a
/// handful of STEPS, and a hand-edited `"textScalePct": 137` is accepted whole.
///
/// So the value gets an entry of its own, labelled as unoffered, because it is
/// a real state the operator is in. Picking any other row leaves it, and it
/// cannot be returned to.
pub(super) fn stray_option(value: &str, options: &[(String, String)]) -> Option<String> {
    if options.iter().any(|(v, _)| v == value) {
        return None;
    }
    Some(format!("{value} (in effect, not one of the choices)"))
}

/// Run `work` on a thread and hand the answer back on the main loop.
///
/// The two update controls are the only blocking network calls the settings
/// surface makes, and both are seconds long. Running one on the main loop
/// freezes every window in the process, because they share it.
fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    glib::MainContext::default().spawn_local(async move {
        if let Ok(value) = rx.await {
            done(value);
        }
    });
}

/// What a page that builds its own controls needs from the sheet.
///
/// Three pages cannot be a list of rows: workspaces, saved commands and the
/// keyboard each edit something that is not a field of [`Settings`], and each
/// has operations that can be refused. They still must not grow their own
/// idea of how a change is persisted or where a refusal is printed, because
/// that is exactly how one surface ends up with two commit paths.
///
/// So they get this and nothing else. Every mutation here goes through
/// [`commit`], and every refusal lands in the sheet's one error banner.
#[derive(Clone)]
pub(crate) struct Host {
    shell: Shell,
    sheet: Weak<SettingsSheet>,
}

impl Host {
    /// The window this page is in.
    pub(crate) fn shell(&self) -> &Shell {
        &self.shell
    }

    /// The document in force.
    pub(crate) fn settings(&self) -> Settings {
        self.shell.peek(|st| st.daemon.settings.clone())
    }

    /// Mutate the document, publish it, and queue the write.
    pub(crate) fn edit(&self, change: impl FnOnce(&mut Settings) + 'static) {
        let sheet = self.sheet.clone();
        self.shell.update(move |st| {
            change(&mut st.daemon.settings);
            commit(st);
            wake(&sheet, String::new());
        });
    }

    /// Mutate anything else on the state, then publish and persist.
    ///
    /// The workspace list is not a settings field and is still written to the
    /// same profile, so it takes the same path.
    pub(crate) fn edit_state(&self, change: impl FnOnce(&mut crate::state::UiState) + 'static) {
        let sheet = self.sheet.clone();
        self.shell.update(move |st| {
            change(st);
            commit(st);
            wake(&sheet, String::new());
        });
    }

    /// Run a mutation that can be refused, surfacing the refusal.
    ///
    /// A dropped `Err` is a button that visibly does nothing, which is the
    /// worst of the three possible behaviours: worse than refusing and worse
    /// than allowing.
    pub(crate) fn try_edit<T, E: core::fmt::Display>(
        &self,
        change: impl FnOnce(&mut crate::state::UiState) -> Result<T, E> + 'static,
    ) {
        let sheet = self.sheet.clone();
        self.shell.update(move |st| match change(st) {
            Ok(_) => {
                commit(st);
                wake(&sheet, String::new());
            }
            Err(why) => {
                let why = why.to_string();
                st.window.flash = Some(crate::state::Flash::error(why.clone()));
                wake(&sheet, why);
            }
        });
    }

    /// Print `why` above the page, or clear the banner when it is empty.
    pub(crate) fn report(&self, why: String) {
        wake(&self.sheet, why);
    }

    /// Redraw the page without changing anything.
    pub(crate) fn refresh(&self) {
        wake(&self.sheet, String::new());
    }

    /// The text an operator typed into `key` and has not committed.
    pub(crate) fn draft(&self, key: &'static str, fallback: &str) -> String {
        self.sheet
            .upgrade()
            .map_or_else(|| fallback.to_string(), |s| s.draft(key, fallback))
    }

    /// Remember what is in a field so a redraw does not empty it.
    pub(crate) fn set_draft(&self, key: &'static str, value: String) {
        if let Some(sheet) = self.sheet.upgrade() {
            sheet.drafts.borrow_mut().insert(key, value);
        }
    }

    /// Forget a draft, because it has been committed.
    pub(crate) fn clear_draft(&self, key: &'static str) {
        if let Some(sheet) = self.sheet.upgrade() {
            sheet.drafts.borrow_mut().remove(key);
        }
    }

    /// One labelled row, in the shape every other row on the surface has.
    pub(crate) fn field(&self, label: &str, desc: &str, when: &str) -> Field {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.style_context().add_class("rg-field");
        if !label.is_empty() {
            let text = gtk::Label::new(Some(label));
            text.style_context().add_class("rg-field__label");
            text.set_halign(gtk::Align::Start);
            root.pack_start(&text, false, false, 0);
        }
        let control = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        control.style_context().add_class("rg-field__control");
        root.pack_start(&control, false, false, 0);
        if !desc.is_empty() {
            let text = wrapped(desc);
            text.style_context().add_class("rg-field__desc");
            root.pack_start(&text, false, false, 0);
        }
        if !when.is_empty() {
            let text = wrapped(when);
            text.style_context().add_class("rg-field__hint");
            root.pack_start(&text, false, false, 0);
        }
        Field { root, control }
    }
}

/// Set the sheet's banner and redraw its page.
///
/// Called from inside a state mutation, so the redraw is deferred: rebuilding
/// the page underneath the handler that asked for it would unparent the widget
/// whose signal is still running.
fn wake(sheet: &Weak<SettingsSheet>, why: String) {
    if let Some(sheet) = sheet.upgrade() {
        sheet.set_error(why);
    }
}

#[cfg(test)]
mod tests;
