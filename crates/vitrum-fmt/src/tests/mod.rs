//! Test suites for `vitrum-fmt`, one concern per module.
//!
//! - [`text_width`]: column measurement over grapheme clusters.
//! - [`text_truncation`]: end and middle truncation, wide characters, budgets.
//! - [`text_sanitizing`]: control characters and whitespace in untrusted titles.
//! - [`relative_time`]: every threshold in the relative-timestamp table.
//! - [`absolute_date`]: the calendar conversion and UTC offsets.
//! - [`durations`]: compact, terse, and clock elapsed-time labels.
//! - [`paths`]: home-relative rewriting and component elision.
//! - [`byte_sizes`]: binary units, rounding, and unit promotion.
//! - [`counts`]: pluralisation and thousands grouping.
//! - [`exit_status`]: exit codes, signals, and NTSTATUS decoding.
//! - [`git_head`]: ref prefixes, branch elision, detached HEAD.

mod absolute_date;
mod byte_sizes;
mod counts;
mod durations;
mod exit_status;
mod git_head;
mod paths;
mod relative_time;
mod text_sanitizing;
mod text_truncation;
mod text_width;
