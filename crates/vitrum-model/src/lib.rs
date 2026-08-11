//! Headless sidebar model: ordering, grouping, status, disposition and labels.
//!
//! No UI, no I/O, no clock, no threads. Every answer in this crate is a pure
//! function of a session snapshot plus a [`Clock`] the caller supplies, which is
//! what makes calendar boundaries, wake timers and status precedence testable
//! without a daemon, a display, or waiting until midnight.
//!
//! # The three axes
//!
//! A sidebar row is described by three independent facts, and keeping them
//! independent is the whole design:
//!
//! | Axis | Type | Owner | Question |
//! |------|------|-------|----------|
//! | Lifecycle | [`SessionStatus`](vitrum_proto::SessionStatus) | the operating system | Is the process alive? |
//! | Status | [`SidebarStatus`] | the agent | What is it doing? |
//! | Disposition | [`Disposition`] | the operator | Am I done with it? |
//!
//! Conflating status and disposition is what makes a twenty-session list
//! unusable. "Done" means the agent finished. "Settled" means *you* are
//! finished. A session you have read and mentally closed keeps shouting if the
//! sidebar only knows the first one.
//!
//! # Where the states come from
//!
//! We spawn the child and hold the PTY master, so most of this is measured
//! rather than guessed:
//!
//! - [`SidebarStatus::Working`], [`SidebarStatus::Ready`] and
//!   [`SidebarStatus::Failed`] are OBSERVED. On Linux and macOS they are proven
//!   by asking the operating system what the foreground process is blocked in;
//!   on Windows, where ConPTY cannot answer, they are inferred from bells and
//!   output timing and [`StatusSource::is_inferred`] says so.
//! - [`SidebarStatus::Approval`] and [`SidebarStatus::Input`] are DECLARED and
//!   never guessed. A shell at a prompt and an agent asking "may I force-push?"
//!   block in the same syscall, so the operating system can prove the next move
//!   is yours but not what is being asked. Two channels carry that, both of
//!   them the agent speaking: the [`hint`] channel an agent opts into, and the
//!   terminal title, read through the per-agent rule in
//!   [`AgentKind::title_claim`]. A title-derived state reports
//!   [`StatusSource::Title`] and hedges, because the agent published that
//!   banner for a title bar rather than for us.
//!
//! An agent that emits nothing gets the full three observed states and a working
//! sidebar. One that opts in gets the specific request and a label. Nothing is
//! guessed either way.
//!
//! # Module map
//!
//! - [`agent`]: which agent a session runs, and what its title announces.
//! - [`civil`]: calendar arithmetic for snooze labels.
//! - [`status`]: the five states and their precedence.
//! - [`view`]: one row, and everything derived from it.
//! - [`disposition`]: the operator's axis, snooze wakes and auto-settle.
//! - [`snooze`]: wake instants, human labels, and the preset menu.
//! - [`order`]: sections, and the sort inside each.
//! - [`rollup`]: what a collapsed project group shows.
//! - [`hint`]: the OSC 7373 streaming parser.
//! - [`tree`]: which rows are actually on screen.
//! - [`traversal`]: keyboard movement over those rows.
//! - [`selection`]: multi-select and its context menu.
//!
//! # No timers
//!
//! Nothing here schedules anything. A snooze expires because
//! [`SessionView::effective_snoozed`] starts answering false, not because
//! something fired. That is deliberate: a timer per snoozed session is exactly
//! the idle CPU cost the product forbids, and derived expiry costs nothing at
//! all while parked.

pub mod agent;
pub mod civil;
pub mod disposition;
pub mod hint;
pub mod order;
pub mod rollup;
pub mod selection;
pub mod snooze;
pub mod status;
pub mod traversal;
pub mod tree;
pub mod view;

#[cfg(test)]
mod testkit;

pub use agent::{ALL_AGENT_KINDS, AgentKind};
pub use civil::{Civil, Weekday};
pub use disposition::{Disposition, DispositionPolicy, Section, SettleOverride};
pub use hint::{HintDeclaration, HintParser};
pub use order::{ActiveOrder, Arranged, SectionCounts, SectionSplit, arrange, arrange_sections};
pub use rollup::{ProjectRollup, StatusCounts, rollup_all, rollup_project};
pub use selection::{MenuAction, MenuItem, Selection, SelectionFacts, context_menu};
pub use snooze::{
    Snooze, SnoozeHours, SnoozePreset, SnoozePresetId, snooze_presets, wake_countdown_label,
    wake_description,
};
pub use status::{
    ALL_STATUS_SOURCES, ALL_STATUSES, SidebarStatus, StatusResolution, StatusSource, TitleClaim,
    resolve_status,
};
pub use traversal::{Direction, Wrap, adjacent, adjacent_matching};
pub use tree::{PreviewSplit, ProjectGroup, preview_sessions, visible_session_ids};
pub use view::{Clock, SessionView, format_duration_label};
