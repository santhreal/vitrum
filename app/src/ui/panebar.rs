//! The strip under the pane, as a GTK panel.
//!
//! # Why this is one line at every state, forever
//!
//! The bar is the only thing below the pane that occupies space, so its height
//! is the pane's rectangle. A strip that appeared when a session exited, or a
//! chip that arrived when the daemon reported a worktree, would shorten the
//! grid by a line at the moment the operator is reading its last output, and
//! that is one of the ways the terminal was seen to move as other things
//! rendered.
//!
//! So every element is emitted at every state. An exit is a WORD in this bar.
//! A missing branch is an empty branch element rather than an absent one, the
//! same rule [`crate::ui::sidebar::tree::Node::reserved`] states for a session
//! row, and the sheet's `--empty` modifiers are its other half.
//!
//! # Why it reuses the sidebar's interpreter
//!
//! Everything here is a [`Node`] tree, patched by
//! [`crate::ui::sidebar::widgets::Surface`]. Two interpreters for one
//! vocabulary is two answers to what a node means, and the second one is the
//! one nobody tests.

use std::rc::Rc;

use crate::shell::{self, Shell};
use crate::state::UiState;
use crate::ui::sidebar::tree::{Act, Kind, Node};
use crate::ui::sidebar::widgets::Surface;
use crate::ui::terminal::{self, PaneBar};

/// Mount the pane bar into the shell's pane-bar slot.
pub(crate) fn panel(shell: &Shell) -> Rc<dyn shell::Panel> {
    let home = shell.ident().home.clone();
    let server = shell.ident().server.clone();
    let first = shell.peek(|st| strip(st, &home, &server));
    Rc::new(PaneBarPanel {
        shell: shell.clone(),
        surface: Surface::new(&first, shell),
        home,
        server,
    })
}

struct PaneBarPanel {
    shell: Shell,
    surface: Surface,
    home: String,
    server: String,
}

impl shell::Observer for PaneBarPanel {
    /// The bar carries no relative time, so the shared clock reading is not
    /// one of its inputs.
    fn state_changed(&self, state: &UiState, _at: crate::Tick) {
        let next = strip(state, &self.home, &self.server);
        self.surface.apply(&next, &self.shell);
    }
}

impl shell::Panel for PaneBarPanel {
    fn root(&self) -> gtk::Widget {
        self.surface.root()
    }
}

/// The bar, folded.
///
/// Pure, so what the strip says at each state is asserted without a display.
/// Every fact in it comes from [`crate::ui::terminal::pane_bar`], which the
/// window and the tests share; nothing is resolved twice.
pub(crate) fn strip(st: &UiState, home: &str, server: &str) -> Node {
    let bar = terminal::pane_bar(st, home, server);
    Node::row("rg-panebar")
        .with(agent(&bar))
        // Never empty: with nothing focused the place says whether the daemon
        // answered, which is the only fact a window with no session has and is
        // worth the line the bar is already paying for.
        .with(Node::label("rg-panebar__place", bar.place.clone()).eliding())
        .with(worktree(&bar))
        .with(
            Node::reserved("rg-panebar__branch", bar.branch.clone().unwrap_or_default()).eliding(),
        )
        .with(Node::new(Kind::Row, "rg-panebar__gap").growing())
        .with(exit(&bar))
        .with(Node::reserved(
            "rg-panebar__grid",
            bar.grid.clone().unwrap_or_default(),
        ))
        .with(state_chip(&bar))
}

/// The focused agent's mark, or the space it will take.
///
/// A reserved element and not an absent one: the mark arrives with the first
/// session, and a window that started empty would otherwise have its whole bar
/// shift right the moment one does.
fn agent(bar: &PaneBar) -> Node {
    match bar.mark {
        Some(mark) => Node::new(Kind::Mark(mark), "rg-panebar__agent")
            .named(bar.agent.unwrap_or_default()),
        None => Node::new(Kind::Row, "rg-panebar__agent rg-panebar__agent--empty"),
    }
}

/// The linked worktree, as a captioned pair.
///
/// The caption is drawn because a bare name beside a path reads as part of the
/// path. It is emitted with the pair, so the two cannot appear a frame apart.
fn worktree(bar: &PaneBar) -> Node {
    let name = bar.worktree.clone().unwrap_or_default();
    let class = if name.is_empty() {
        "rg-panebar__worktree rg-panebar__worktree--empty"
    } else {
        "rg-panebar__worktree"
    };
    Node::row(class)
        .with(Node::reserved(
            "rg-panebar__key",
            if name.is_empty() { "" } else { "worktree" },
        ))
        .with(Node::reserved("rg-panebar__value", name).eliding())
}

/// How the child ended, and the one control that answers it.
///
/// "Stop viewing" and not "Close": the session has already exited, and this
/// takes its tab out of the strip. Same wording as the row menu and the
/// shortcut list, so three surfaces cannot describe one action three ways.
fn exit(bar: &PaneBar) -> Node {
    let line = bar.exit.clone().unwrap_or_default();
    let done = !line.is_empty();
    let class = if done {
        "rg-panebar__exit"
    } else {
        "rg-panebar__exit rg-panebar__exit--empty"
    };
    // The control is emitted whether or not it can be pressed. Drawn only for
    // an exited session it would arrive in the bar at the moment the agent
    // ends, push the grid size and the state word left, and do it while the
    // operator is reading the last of the output.
    Node::row(class)
        .with(Node::reserved("rg-panebar__exit-line", line).eliding())
        .with(
            Node::press("rg-btn-inline", Act::StopViewing)
                .saying("Stop viewing")
                .named("Stop viewing this session")
                .refusing(!done),
        )
}

/// The state word, in the hue the sidebar row gives the same session.
fn state_chip(bar: &PaneBar) -> Node {
    match &bar.state {
        Some(pill) => Node::row(&format!("rg-panebar__state {}", pill.class))
            .with(Node::reserved("rg-pill__word", pill.word.to_string())),
        None => Node::row("rg-panebar__state rg-panebar__state--empty")
            .with(Node::reserved("rg-pill__word", "")),
    }
}

#[cfg(test)]
mod tests;
