//! The terminate prompt's arm-and-answer rule.
//!
//! Found by running the product: terminating a session left the prompt that
//! asked about it on screen. An error flash never expires, deliberately, so the
//! strip kept asking whether to kill a row that was already gone, and a spent
//! prompt is indistinguishable from an armed one.

use crate::actions::answers_prompt;
use crate::state::{Flash, FlashKind};
use vitrum_proto::SessionId;

const PROMPT: &str = "Terminate sh? Its child process is killed and there is no undo.";

fn id(n: u64) -> SessionId {
    SessionId(n)
}

/// Nothing armed, so the press asks rather than kills.
#[test]
fn a_first_press_arms_the_prompt_instead_of_answering_it() {
    assert_eq!(answers_prompt(&[], &[id(1)], None, PROMPT), None);
}

/// Armed for other rows is not armed for these.
///
/// Otherwise a prompt raised about one session would let the next press kill a
/// different one without asking, which is the whole thing the prompt prevents.
#[test]
fn a_prompt_armed_for_other_rows_does_not_answer_for_these() {
    let flash = Flash::error(PROMPT);
    assert_eq!(
        answers_prompt(&[id(2)], &[id(1)], Some(&flash), PROMPT),
        None
    );
    assert_eq!(
        answers_prompt(&[id(1), id(2)], &[id(1)], Some(&flash), PROMPT),
        None
    );
}

/// The second press for the same rows kills, and takes its own prompt down.
#[test]
fn answering_the_prompt_retires_it() {
    let flash = Flash::error(PROMPT);
    assert_eq!(
        answers_prompt(&[id(1)], &[id(1)], Some(&flash), PROMPT),
        Some(true)
    );
}

/// A newer message about something else stays on screen.
///
/// The prompt is identified by its text, so answering it must not erase a
/// failure raised after it was armed. Errors are the messages the operator has
/// to act on.
#[test]
fn answering_the_prompt_leaves_a_different_message_alone() {
    let other = Flash::error("Could not attach to the daemon.");
    assert_eq!(
        answers_prompt(&[id(1)], &[id(1)], Some(&other), PROMPT),
        Some(false)
    );
    assert_eq!(answers_prompt(&[id(1)], &[id(1)], None, PROMPT), Some(false));
    // A notice raised after the prompt is likewise not the prompt.
    let notice = Flash::notice(PROMPT.to_string() + " ");
    assert_eq!(notice.kind, FlashKind::Notice);
    assert_eq!(
        answers_prompt(&[id(1)], &[id(1)], Some(&notice), PROMPT),
        Some(false)
    );
}
