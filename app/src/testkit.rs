//! Row fixtures for the client's own tests.
//!
//! `vitrum-model` keeps its builder private to the crate, and rightly: it is a
//! test detail, not API. The client needs the same shape, so this is the
//! client's copy, with the defaults the client cares about.
//!
//! [`SessionInfo`] has fifteen fields and a test normally varies two. A builder
//! keeps each test's intent legible: what a test does *not* set says as much as
//! what it does.

use vitrum_model::{SessionView, Snooze};
use vitrum_proto::{
    AgentHint, Attention, HintState, ProjectId, ProjectInfo, SessionId, SessionInfo, SessionStatus,
};

/// A wall-clock instant far enough from the epoch that calendar arithmetic in
/// snooze labels lands on a real date. 2026-03-04T00:50:00Z.
pub const NOW: u64 = 1_772_580_600_000;

pub const HOUR: u64 = 3_600_000;

/// A session with everything at rest: live, quiet, seen, unhinted.
pub fn info(id: u64) -> SessionInfo {
    SessionInfo {
        id: SessionId(id),
        project_id: ProjectId(1),
        title: format!("session {id}"),
        cwd: "/tmp".to_string(),
        command: "bash".to_string(),
        args: Vec::new(),
        status: SessionStatus::Running,
        created_at_ms: NOW - HOUR,
        last_activity_ms: NOW - HOUR,
        cols: 80,
        rows: 24,
        git_branch: None,
        worktree: None,
        unread: false,
        attention: Attention::default(),
        hint: None,
        term_title: None,
    }
}

/// A named project, for group and rollup tests.
pub fn project(id: u64, name: &str) -> ProjectInfo {
    ProjectInfo {
        id: ProjectId(id),
        name: name.to_string(),
        root: format!("/src/{name}"),
    }
}

/// Fluent construction of one sidebar row.
pub struct Row {
    view: SessionView,
}

/// Start building the row for session `id`.
pub fn row(id: u64) -> Row {
    Row {
        view: SessionView::new(info(id)),
    }
}

impl Row {
    pub fn project(mut self, project_id: u64) -> Self {
        self.view.info.project_id = ProjectId(project_id);
        self
    }

    pub fn title(mut self, title: &str) -> Self {
        self.view.info.title = title.to_string();
        self
    }

    /// What the program last announced in its terminal title, which is the
    /// channel the status resolver reads. Not the session's name.
    pub fn term_title(mut self, title: &str) -> Self {
        self.view.info.term_title = Some(title.to_string());
        self
    }

    pub fn cwd(mut self, cwd: &str) -> Self {
        self.view.info.cwd = cwd.to_string();
        self
    }

    /// The program behind the session, which is what resolves its agent
    /// identity. Defaults to `bash`, so a test that does not care gets the
    /// shell mark rather than the unknown one.
    pub fn command(mut self, command: &str) -> Self {
        self.view.info.command = command.to_string();
        self
    }

    pub fn running(mut self) -> Self {
        self.view.info.status = SessionStatus::Running;
        self
    }

    pub fn exited(mut self, code: Option<i32>) -> Self {
        self.view.info.status = SessionStatus::Exited { code };
        self.view.info.attention.failed = code != Some(0);
        self
    }

    pub fn idle_ms(mut self, idle_ms: u64) -> Self {
        self.view.info.attention.idle_ms = idle_ms;
        self
    }

    /// The operating system's answer to "is the foreground process blocked on
    /// the terminal". `None` models a platform that cannot tell, which is
    /// Windows and is the case the inferred pill exists for.
    pub fn waiting(mut self, waiting: Option<bool>) -> Self {
        self.view.info.attention.waiting = waiting;
        self
    }

    pub fn unread(mut self, unread: bool) -> Self {
        self.view.info.unread = unread;
        self
    }

    pub fn created_at_ms(mut self, created_at_ms: u64) -> Self {
        self.view.info.created_at_ms = created_at_ms;
        self
    }

    pub fn last_activity_ms(mut self, last_activity_ms: u64) -> Self {
        self.view.info.last_activity_ms = last_activity_ms;
        self
    }

    pub fn visited(mut self, visited_ms: u64) -> Self {
        self.view.last_visited_ms = Some(visited_ms);
        self
    }

    pub fn hint(mut self, state: HintState, label: Option<&str>, received_at_ms: u64) -> Self {
        self.view.info.hint = Some(AgentHint {
            state,
            label: label.map(str::to_string),
            received_at_ms,
        });
        self
    }

    pub fn snooze(mut self, snoozed_at_ms: u64, wake_at_ms: u64) -> Self {
        self.view.snooze = Some(Snooze {
            snoozed_at_ms,
            wake_at_ms,
        });
        self
    }

    pub fn build(self) -> SessionView {
        self.view
    }
}

/// The shipped client shell, as one string.
///
/// The crate root used to be a single six thousand line `main.rs`, and a dozen
/// guards check their claim by scanning it: that a menu item reaches a
/// handler, that a caption matches the request it describes. Carving the root
/// into modules moved that code without changing it, and every one of those
/// guards went green-to-red for no behavioural reason. They now scan this
/// instead, so the next carve is a one line change here rather than a hunt
/// through the suites.
///
/// None of these files carries inline tests any more, so all of it is shipped
/// code and no `#[cfg(test)]` half needs stripping.
pub fn shell() -> String {
    SHELL_FILES
        .iter()
        .map(|(_, src)| *src)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The files `shell` is made of, named, for guards that report where a link
/// broke.
pub const SHELL_FILES: [(&str, &str); 8] = [
    ("main.rs", include_str!("main.rs")),
    ("actions.rs", include_str!("actions.rs")),
    ("chrome.rs", include_str!("chrome.rs")),
    ("cli.rs", include_str!("cli.rs")),
    ("geometry.rs", include_str!("geometry.rs")),
    ("instance.rs", include_str!("instance.rs")),
    ("keys.rs", include_str!("keys.rs")),
    ("sync.rs", include_str!("sync.rs")),
];
