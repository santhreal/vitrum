//! Test fixtures.
//!
//! [`SessionInfo`] has sixteen fields and every module needs to vary two or
//! three of them. A builder keeps each test's intent legible: what a test does
//! *not* set is as informative as what it does.

use vitrum_proto::{
    AgentHint, Attention, HintState, ProjectId, SessionId, SessionInfo, SessionStatus,
};

use crate::view::SessionView;

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
        created_at_ms: 1_000,
        last_activity_ms: 1_000,
        cols: 80,
        rows: 24,
        git_branch: None,
        unread: false,
        attention: Attention::default(),
        hint: None,
    }
}

/// A resting view over [`info`].
pub fn view(id: u64) -> SessionView {
    SessionView::new(info(id))
}

/// Fluent construction of a single row.
pub struct ViewBuilder {
    view: SessionView,
}

impl ViewBuilder {
    pub fn new(id: u64) -> Self {
        ViewBuilder { view: view(id) }
    }

    pub fn project(mut self, project_id: u64) -> Self {
        self.view.info_mut().project_id = ProjectId(project_id);
        self
    }

    pub fn running(mut self) -> Self {
        self.view.info_mut().status = SessionStatus::Running;
        self
    }

    pub fn starting(mut self) -> Self {
        self.view.info_mut().status = SessionStatus::Starting;
        self
    }

    pub fn exited(mut self, code: i32) -> Self {
        let info = self.view.info_mut();
        info.status = SessionStatus::Exited { code: Some(code) };
        info.attention.failed = code != 0;
        self
    }

    pub fn signalled(mut self) -> Self {
        let info = self.view.info_mut();
        info.status = SessionStatus::Exited { code: None };
        info.attention.failed = true;
        self
    }

    pub fn bell(mut self, bell: bool) -> Self {
        self.view.info_mut().attention.bell = bell;
        self
    }

    pub fn idle_ms(mut self, idle_ms: u64) -> Self {
        self.view.info_mut().attention.idle_ms = idle_ms;
        self
    }

    /// The operating system's answer to "is the foreground process blocked on
    /// the terminal". `None` models a platform that cannot tell.
    pub fn waiting(mut self, waiting: Option<bool>) -> Self {
        self.view.info_mut().attention.waiting = waiting;
        self
    }

    pub fn unread(mut self, unread: bool) -> Self {
        self.view.info_mut().unread = unread;
        self
    }

    pub fn created_at_ms(mut self, created_at_ms: u64) -> Self {
        self.view.info_mut().created_at_ms = created_at_ms;
        self
    }

    pub fn last_activity_ms(mut self, last_activity_ms: u64) -> Self {
        self.view.info_mut().last_activity_ms = last_activity_ms;
        self
    }

    pub fn last_visited_ms(mut self, last_visited_ms: Option<u64>) -> Self {
        self.view.last_visited_ms = last_visited_ms;
        self
    }

    pub fn hint(mut self, state: HintState, label: Option<&str>, received_at_ms: u64) -> Self {
        self.view.info_mut().hint = Some(AgentHint {
            state,
            label: label.map(str::to_string),
            received_at_ms,
        });
        self
    }

    pub fn snooze(mut self, snoozed_at_ms: u64, wake_at_ms: u64) -> Self {
        self.view.snooze = Some(crate::snooze::Snooze {
            snoozed_at_ms,
            wake_at_ms,
        });
        self
    }

    pub fn build(self) -> SessionView {
        self.view
    }
}
