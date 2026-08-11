//! Control-plane encoding, the history budgets, and the client's event
//! vocabulary.
//!
//! Two transports, deliberately split, matching `vitrum-proto`:
//!
//! - Control plane: JSON text frames. Encoded here so there is one tested
//!   definition of every message the client sends, and put on the socket by
//!   [`crate::socket`]. Low rate: a handshake, a list, an attach, a resize, a
//!   keystroke.
//! - Data plane: binary frames of raw PTY bytes. Parsed in [`crate::socket`],
//!   which strips the 17-byte header, checks the offset is contiguous, and
//!   hands the payload to the pane as a borrowed slice of the frame it
//!   arrived in. Nothing on that path is JSON, base64, or a `String`.
//!
//! [`ClientEvent`] is the one vocabulary the reducer in [`crate::sync`]
//! reacts to. Its members come from two places, the socket task and the
//! native pane, and the reducer cannot tell which: a `Welcome` observed by a
//! socket and a resize observed by a drawing area move the same state through
//! the same function, because two reducers is two answers to what a message
//! means.

use serde::Deserialize;
use vitrum_proto::{ClientMsg, ServerMsg};

/// Where the session daemon listens unless `--server` says otherwise.
///
/// Loopback, because the daemon owns PTYs and scrollback for this machine's
/// agents; exposing that on a network interface would be a remote shell with
/// no authentication. `--server` exists for running a second daemon beside the
/// first, which is how the disconnect and reconnect paths are tested without
/// taking down the one everybody else is using.
pub const DEFAULT_WS_URL: &str = "ws://127.0.0.1:7737";

/// Assumed bytes of PTY output behind one line of terminal buffer.
///
/// The operator picks a line count and the protocol takes a byte count, so one
/// of the two has to be assumed. 64 bytes is a line of roughly fifty printable
/// columns, its `\r\n`, and one SGR colour pair, which is what tool and agent
/// output actually looks like. Bracketed by two measurements rather than
/// guessed: a full coloured build stream is 69.5 bytes a line and this
/// repository's own source files average 39.2, so 64 sits between the
/// chattiest realistic producer and the plainest.
///
/// Deliberately far below the ~850 bytes a dense, fully wrapped, heavily
/// coloured row costs. Sizing for that at the largest offered setting would be
/// 85 MB per attach, and the honest guarantee is "more history than the step
/// below", not "exactly N lines whatever the content".
pub const BACKFILL_BYTES_PER_LINE: u32 = 64;

/// Smallest backfill worth asking for, whatever the setting says.
///
/// 16 KiB is 256 lines at [`BACKFILL_BYTES_PER_LINE`], more rows than any
/// window shows. `scrollbackLines` is deserialized from the state file with no
/// clamp on it, so a hand-edited `0` would otherwise compute a zero-byte
/// budget and attach to a blank grid instead of repainting the screen the
/// daemon already has.
pub const BACKFILL_MIN_BYTES: u32 = 16 * 1024;

// What the floor is FOR, in the currency it is denominated in: it has to
// repaint the visible grid whatever the setting says, and no window shows 256
// rows. A bounds check on the byte count alone would pass at 64 bytes, which is
// one line and no floor at all. Compile-time, because both operands are.
const _: () = assert!(
    BACKFILL_MIN_BYTES / BACKFILL_BYTES_PER_LINE >= 256,
    "the backfill floor must cover at least 256 lines, or a session whose \
     buffer setting is zero attaches to a grid the backfill cannot fill"
);

/// Hard ceiling on one backfill, in bytes.
///
/// A backfill is the one control-plane message that carries bulk bytes, and it
/// carries them base64 inside a `ScrollbackChunk`. It is decoded once, in
/// [`crate::socket`], and reaches the terminal engine as those bytes. The
/// ceiling is what keeps a deliberate gesture from turning into a multi-second
/// decode: 2 MiB decodes in under a millisecond, 85 MB would not.
///
/// The latency matters on its own too: live frames queue in
/// [`crate::socket::PaneStream`] against a 1 MiB
/// [`crate::socket::PENDING_CAP`] while the backfill is in flight, and a
/// backfill slow enough to overflow that queue is discarded outright, costing
/// the operator all history rather than some.
///
/// 2 MiB is above 20,000 lines x [`BACKFILL_BYTES_PER_LINE`] and below
/// 100,000 x it, so every step the settings sheet offers still asks for
/// strictly more than the step beneath it. It is also well under the daemon's
/// 10 MiB per-session ring (`DEFAULT_SCROLLBACK_BYTES`), so the ceiling never
/// asks for bytes the server does not hold.
pub const BACKFILL_CEILING_BYTES: u32 = 2 * 1024 * 1024;

/// Bytes of history to request when a pane gains focus, for an operator who
/// asked for `scrollback_lines` of local buffer.
///
/// A function of the setting rather than a constant, because the setting's
/// caption promises that raising it is how you see further back and for two
/// separate corrections of that caption the promise was false: the backfill
/// was a fixed 64 KiB, so choosing 100,000 lines grew the local buffer and
/// retrieved not one extra byte of the history the daemon already held.
///
/// Deeper history than this still stays on the server, which is the point of
/// the split. What changed is that the operator moves where the line falls.
#[must_use]
pub const fn backfill_max_bytes(scrollback_lines: u32) -> u32 {
    // `Ord::clamp` is not const, and the multiply saturates rather than
    // wrapping: the setting is a u32 read from a file nobody validates.
    let want = scrollback_lines.saturating_mul(BACKFILL_BYTES_PER_LINE);
    if want < BACKFILL_MIN_BYTES {
        BACKFILL_MIN_BYTES
    } else if want > BACKFILL_CEILING_BYTES {
        BACKFILL_CEILING_BYTES
    } else {
        want
    }
}

/// Sentinel `before_seq` meaning "everything up to the current head".
///
/// The server clamps it to `head_seq`. Sending the head we last saw instead
/// would race: the child can emit bytes between the `Attach` and the
/// `Scrollback` being processed, and those bytes would never be painted.
pub const BEFORE_SEQ_HEAD: u64 = u64::MAX;

/// Most history one pane will hold after paging back.
///
/// Paging back is a repaint: the daemon is asked for a bigger window ending at
/// the same head, the screen is reset and the whole span is replayed. The
/// terminal engine keeps its own scrollback of what it has been fed and offers
/// no way to splice older bytes in above it, so a bigger window replayed from
/// its start is the only exact way to put history above what is painted. It is
/// affordable because it happens on a deliberate gesture rather than on
/// output. It is not affordable without a ceiling, so this is one.
///
/// 8 MiB is four full [`BACKFILL_CEILING_BYTES`] windows, roughly 128,000
/// lines at the 64-byte estimate. Past it the operator is told the client will
/// not hold more rather than being given a window that takes seconds to
/// replay.
pub const PAGE_CEILING_BYTES: u32 = 8 * 1024 * 1024;

/// Budget for the next page-back, given what is already painted.
///
/// Grows by one attach-sized window per gesture, so each page-back shows about
/// as much new history as the operator saw when they arrived. Returns `None`
/// once the ceiling is reached, which is the caller's cue to say so instead of
/// silently repainting the same bytes.
#[must_use]
pub const fn page_back_max_bytes(painted: u64, scrollback_lines: u32) -> Option<u32> {
    if painted >= PAGE_CEILING_BYTES as u64 {
        return None;
    }
    let step = backfill_max_bytes(scrollback_lines) as u64;
    let want = painted + step;
    if want >= PAGE_CEILING_BYTES as u64 {
        Some(PAGE_CEILING_BYTES)
    } else {
        // `want` is below a u32 constant, so this cannot truncate.
        Some(want as u32)
    }
}

/// Bytes of context to keep after a search hit when jumping to it.
///
/// The daemon answers "the last N bytes before this offset", so landing on a
/// hit means asking for a window that ENDS past it. Without this slack the hit
/// would be the last byte painted, at the very bottom of the grid, with no
/// sight of what the agent said next, which is usually the reason the operator
/// searched for it.
pub const JUMP_TAIL_BYTES: u64 = 8 * 1024;

/// Encode a control-plane message as the JSON text frame the server expects.
///
/// Control plane only. Nothing on the data path passes through here: a PTY
/// byte reaches the terminal engine without ever being a `String`.
pub fn encode(msg: &ClientMsg) -> String {
    // ClientMsg is a closed enum of plain data with no map keys that can fail
    // to serialize, so this cannot error in practice; a panic here would mean
    // vitrum-proto changed shape underneath us.
    serde_json::to_string(msg).expect("ClientMsg is always serializable")
}

/// Socket lifecycle, as [`crate::socket`] observes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnEvent {
    Open,
    Closed,
    Error,
}

/// Everything that can move the client's state.
///
/// One vocabulary with two producers. [`Self::Server`] and [`Self::Conn`] are
/// built in [`crate::socket`] from what the connection did. The rest are built
/// by the pane and the window from what the operator did. The reducer in
/// [`crate::sync::on_client_event`] handles both without distinguishing them,
/// because what a `Welcome` means to the client cannot depend on which part of
/// the process saw it arrive.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientEvent {
    /// A control-plane text frame, parsed off the session socket.
    Server { msg: ServerMsg },
    /// Socket opened, closed, or refused.
    Conn {
        state: ConnEvent,
        detail: Option<String>,
    },
    /// Terminal geometry, in cells, after the pane measured its surface
    /// against the cell box the font produced.
    Resize { cols: u16, rows: u16 },
    /// Bytes the pane captured: a keystroke, a paste, or a raw 8-bit reply
    /// such as a mouse report or a DEC response.
    ///
    /// No session on the event. The pane draws whatever grid it is handed and
    /// does not know which session that is; this side does. One owner for the
    /// attachment means a keystroke cannot be addressed to a session the pane
    /// stopped showing while the event was in flight.
    Input { data: Vec<u8> },
    /// A chord the shell claimed, already resolved against the live table.
    ///
    /// Resolved rather than named, because both ends are this program:
    /// spelling the action as a string and parsing it back would put a
    /// fallible step on the path between a key press and what it does, for a
    /// value that was never going to leave the process.
    Key { action: crate::keymap::KeyAction },
    /// A chord bound to the operator's own action list.
    ///
    /// The chord and not the binding. The list can be edited between the
    /// press and the dispatch, and running a binding as it was is running a
    /// binding that no longer exists.
    CustomKey { chord: crate::launch::Chord },
    /// Result of a clipboard write.
    ///
    /// Reported rather than assumed: a clipboard write can be refused, and a
    /// "Copied" notice for a copy that did not happen is a lie the operator
    /// only discovers when they paste.
    Copied { ok: bool, text: String },
    /// The operator reached the top of the painted history.
    ///
    /// Sent on every arrival at the top, and deliberately unguarded: whether
    /// there is more history to ask for, and whether a request is already in
    /// flight, are both this side's to know. The pane reports the gesture and
    /// nothing else.
    PageBack,
    /// A control-plane message a panel built and wants sent.
    ///
    /// The route out for anything a panel decides that is not a chord: the
    /// launcher's `Start`, the terminate confirmation's `Close`, the search
    /// sweep. It goes through the reducer rather than through the socket
    /// because the reducer is where a fixture window is kept off the wire, and
    /// a panel holding the socket would make a fixture dial a daemon.
    Msg { msg: vitrum_proto::ClientMsg },
    /// The session list or the strip changed, so re-attach.
    ///
    /// Which session this window is attached to is the reducer's to know: it
    /// holds the attachment and it is the only place that can tell an
    /// unnecessary re-attach from a necessary one. A panel that changed the
    /// strip says so and stops there.
    Reconcile,
    /// Put text on the system clipboard.
    ///
    /// Raised rather than done, because the answer comes back as
    /// [`ClientEvent::Copied`] and a panel that wrote the clipboard itself
    /// would have to report its own success, which is how a "Copied" notice
    /// for a refused write gets written.
    Clipboard { text: String },
    /// Start a session the launcher decided on.
    ///
    /// The reducer holds the record of a launch that has been sent and not yet
    /// confirmed, which is what focuses the new row the moment the daemon
    /// answers. A panel that sent the request itself would leave that record
    /// unwritten and the new session unfocused.
    Start {
        project: vitrum_proto::ProjectId,
        launch: crate::launch::Launch,
    },
    /// Start a second session with this one's command, directory and title.
    Duplicate { session: vitrum_proto::SessionId },
    /// Terminate these sessions, asking first when the profile says to.
    ///
    /// The confirmation is a notice with a control on it rather than a modal,
    /// because a modal for this would be a fourth surface competing for Escape
    /// with the three that already exist.
    Terminate {
        targets: Vec<vitrum_proto::SessionId>,
    },
    /// Point this window's socket at another daemon and re-attach.
    ///
    /// The reducer owns the bridge and the attachment, and both have to move
    /// together: dialling a second daemon while still holding a session id
    /// minted by the first would address input to a session that does not
    /// exist there. A panel that dialled the socket itself would leave the
    /// attachment pointing at the old daemon's numbering.
    Redial { url: String },
    /// Reopen the socket after a failure.
    ///
    /// Raised rather than done because a retry goes through the same daemon
    /// probe as startup: on a machine where nothing is listening it starts the
    /// daemon that should have been running, and that needs the bridge and the
    /// command line, neither of which a panel holds.
    Retry,
    /// Start the top-ranked launch outright, with no launcher at all.
    ///
    /// The decision is not a panel's to make. Which launch is top-ranked is
    /// read from the operator's history, and whether it is confident enough to
    /// run unasked is `crate::ui::dialog::primary_launch`. Sent as an intent
    /// so the launcher and the sidebar's one-click control cannot come to two
    /// different answers about the same directory.
    LaunchNow {
        project: Option<vitrum_proto::ProjectId>,
    },
    /// Something the client could not make sense of: an undecodable frame, a
    /// surface that stopped answering. Surfaced, never swallowed.
    Bad { detail: String },
}

#[cfg(test)]
mod tests;
