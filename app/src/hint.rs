//! Emitting OSC 7373, the agent hint channel.
//!
//! The parser for this sequence lives in [`vitrum_model::hint`] and the daemon
//! has read it from every PTY since the channel existed. Nothing shipped ever
//! wrote one, so `Approval` and `Input` were states the sidebar could render
//! and no install could reach: with claude, codex or gemini out of the box,
//! every row resolved through the observed path. This module is the writing
//! half, and `vitrum hint` puts it one command away from a prompt hook, a
//! wrapper script or an agent that can run a shell command.
//!
//! Every sequence is built here and nowhere else, so the one place that knows
//! the wire format is the one place a change to it would land.
//!
//! # What comes out
//!
//! ```text
//! ESC ] 7373 ; <state> [ ; <label> ] ESC \
//! ```
//!
//! `ESC \` is the terminator rather than BEL. Both are legal, but BEL is also
//! how a program asks for the operator, and a terminal that does not know OSC
//! 7373 would beep on every declaration.
//!
//! # Labels are shaped to what the parser accepts
//!
//! The parser is strict on purpose: a control byte anywhere in the payload
//! drops the whole sequence, and a payload longer than
//! [`MAX_SEQUENCE_BYTES`] is abandoned. A label comes from an agent's own
//! output and can carry either. Rather than emit bytes that will be silently
//! rejected, [`sequence`] flattens control characters to spaces and truncates
//! to whatever budget is left, so what is written always parses back.

use super::*;

use std::io::{self, Write};

use vitrum_model::hint::{MAX_LABEL_CHARS, MAX_SEQUENCE_BYTES};
use vitrum_proto::Exit;
use vitrum_proto::{HINT_OSC, HintState};

const ESC: char = '\u{1b}';

/// The token the parser expects for each state.
///
/// Not [`HintState`]'s serde name, which is a JSON concern and could be
/// renamed without anyone noticing the wire format moved. Pinned instead by a
/// test that feeds every token back through `HintState::parse`.
pub(crate) fn token(state: HintState) -> &'static str {
    match state {
        HintState::Approval => "approval",
        HintState::Input => "input",
        HintState::Working => "working",
        HintState::Ready => "ready",
    }
}

/// The bytes declaring `state`, with `label` if it survives shaping.
pub(crate) fn sequence(state: HintState, label: Option<&str>) -> String {
    let token = token(state);
    let mut out = format!("{ESC}]{HINT_OSC};{token}");
    // The payload the parser buffers is everything between the introducer and
    // the terminator, plus the separator the label would need.
    let prefix = format!("{HINT_OSC};{token};").len();
    if let Some(text) = label.and_then(|text| shape(text, MAX_SEQUENCE_BYTES - prefix)) {
        out.push(';');
        out.push_str(&text);
    }
    out.push(ESC);
    out.push('\\');
    out
}

/// Fit a label into what the parser will accept, or drop it if nothing is left.
///
/// `byte_budget` is what remains of [`MAX_SEQUENCE_BYTES`] after the state
/// token. Both limits are real: the character cap is the parser's own
/// truncation point, and the byte cap is where it abandons the sequence.
fn shape(label: &str, byte_budget: usize) -> Option<String> {
    let flattened: String = label
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = flattened.trim();
    let mut end = trimmed.len().min(byte_budget);
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    let mut kept = &trimmed[..end];
    if let Some((at, _)) = kept.char_indices().nth(MAX_LABEL_CHARS) {
        kept = &kept[..at];
    }
    // Truncation can expose trailing space the parser would trim anyway.
    let kept = kept.trim_end();
    if kept.is_empty() {
        None
    } else {
        Some(kept.to_string())
    }
}

/// `vitrum hint` - write a hint sequence to stdout.
///
/// Returns the process exit code from the one table in
/// [`vitrum_proto::exit`]. This command exists to be called from a prompt
/// command or a wrapper script, where the caller checks the status and
/// nothing reads prose, so the codes carry the whole answer:
/// [`Exit::Ok`] wrote the bytes, [`Exit::Usage`] means the arguments were
/// wrong and nothing was written, and [`Exit::Failed`] means stdout would not
/// take them.
pub(crate) fn run_hint(args: &[String]) -> i32 {
    let stdout = io::stdout();
    hint_command(args, &mut stdout.lock())
}

/// [`run_hint`] against any sink.
///
/// Split out so a test can assert the exact bytes and, for a usage error, that
/// nothing at all reached stdout: a prompt command that printed half a
/// sequence before failing would corrupt the line it was called from.
pub(crate) fn hint_command(args: &[String], out: &mut dyn Write) -> i32 {
    let request = match parse_hint(args) {
        Ok(HintRequest::Help) => {
            // Help is what was asked for, so it is the output, not a diagnostic.
            if writeln!(out, "{}", hint_usage()).is_err() {
                eprintln!(
                    "vitrum hint: could not write the help to stdout; \
                     the reader on the other end of the pipe has gone"
                );
                return Exit::Failed.code();
            }
            return Exit::Ok.code();
        }
        Ok(HintRequest::Declare { state, label }) => sequence(state, label.as_deref()),
        Err(message) => {
            eprintln!("{message}");
            return Exit::Usage.code();
        }
    };
    // No terminal check. The whole point is the pipeline and the prompt
    // command, where stdout is a pipe and the bytes still have to arrive.
    if out.write_all(request.as_bytes()).is_err() || out.flush().is_err() {
        eprintln!(
            "vitrum hint: could not write to stdout; the row keeps whatever \
             status vitrum observes, so nothing was declared"
        );
        return Exit::Failed.code();
    }
    Exit::Ok.code()
}

#[cfg(test)]
mod the_sequence_a_harness_emits;
