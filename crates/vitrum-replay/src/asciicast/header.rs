//! The asciicast v2 header line.
//!
//! The first line of the file is a JSON object. `version`, `width` and `height` are
//! required; everything else is optional metadata a player may show.
//!
//! # Unknown keys survive
//!
//! [`Header::extra`] collects any key this crate does not model, and writing puts
//! them back. asciinema has added header keys over time and will add more, and a
//! round trip that silently dropped a key it did not recognise would quietly damage
//! recordings made by a newer version than this reader.

use serde::{Deserialize, Serialize};

/// The v2 header.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Header {
    /// Format version. Always 2 for a file this crate reads or writes.
    #[serde(default)]
    pub version: u64,
    /// Screen width in columns. Required: the same bytes are a different screen at
    /// a different width.
    #[serde(default)]
    pub width: u16,
    /// Screen height in rows.
    #[serde(default)]
    pub height: u16,
    /// Unix epoch seconds the recording started at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
    /// Total length in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// Seconds a player should compress an idle gap down to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_time_limit: Option<f64>,
    /// The command that was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Human title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Captured environment, conventionally `SHELL` and `TERM`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<serde_json::Value>,
    /// Terminal colours the recording was made with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<serde_json::Value>,
    /// Every header key this crate does not model, preserved verbatim.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Header {
    /// The version this crate reads and writes.
    pub const VERSION: u64 = 2;

    /// A minimal v2 header for a `cols` x `rows` screen.
    #[must_use]
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            version: Self::VERSION,
            width: cols,
            height: rows,
            timestamp: None,
            duration: None,
            idle_time_limit: None,
            command: None,
            title: None,
            env: None,
            theme: None,
            extra: serde_json::Map::new(),
        }
    }

    /// The same header with a title.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// The same header with a start time in Unix epoch seconds.
    #[must_use]
    pub const fn with_timestamp(mut self, seconds: u64) -> Self {
        self.timestamp = Some(seconds);
        self
    }
}
