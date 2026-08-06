//! OSC 7373 chapter markers over the same byte stream.
//!
//! A scrubber with no landmarks is a scrubber you drag blindly. vitrum's sessions
//! already carry landmarks: an agent that speaks OSC 7373 announces when it started
//! working, when it needs approval, when it is asking a question, and when it is
//! done. Those announcements are in the output bytes, at exact positions, so a
//! replay can put a marker at each one and offer "jump to the moment it asked to
//! force push" instead of "drag until you find it".
//!
//! # Reuse, not a second parser
//!
//! The sequence is parsed by [`vitrum_model::HintParser`], the same streaming
//! parser the daemon uses on live output. There is one OSC 7373 parser in this
//! workspace and this is not it. What this module adds is the byte position, which
//! the daemon does not need and therefore does not track.
//!
//! # Why the scan is nearly free
//!
//! [`vitrum_model::HintParser`] in its ground state does exactly one thing with a
//! byte that is not `ESC`: nothing. So this scan does not feed those bytes at all.
//! It `memchr`s to the next `ESC`, feeds from there one byte at a time until the
//! parser is back in its ground state, and jumps again. On ordinary agent output
//! that is one SIMD pass over the stream plus a few bytes of real work per escape
//! sequence.
//!
//! Feeding one byte at a time inside a sequence is what makes the position exact.
//! A bulk feed would only tell you which chunk a hint completed in, and "somewhere
//! in these 4096 bytes" is not a marker you can seek to.

use memchr::memchr;
use vitrum_model::{HintDeclaration, HintParser};

use crate::stream::Stream;
use crate::timeline::Marker;

/// Escape, the only byte that moves [`vitrum_model::HintParser`] out of its ground
/// state.
const ESC: u8 = 0x1b;

/// Every OSC 7373 hint in `stream`, in stream order.
///
/// A marker's seq is one past the sequence's terminating byte, so seeking to it
/// shows the screen as it stood the instant the hint was fully delivered.
///
/// ```
/// use vitrum_replay::{Stream, hints};
///
/// let bytes: &[u8] = b"building\x1b]7373;approval;force push?\x07waiting";
/// let stream = Stream::new(0, std::slice::from_ref(&bytes));
/// let markers = hints::scan(&stream);
///
/// assert_eq!(markers.len(), 1);
/// assert_eq!(markers[0].label, "force push?");
/// // Just past the BEL that terminated the sequence.
/// assert_eq!(markers[0].seq, 8 + 28);
/// ```
#[must_use]
pub fn scan(stream: &Stream<'_>) -> Vec<Marker> {
    let mut parser = HintParser::new();
    let mut declarations = Vec::new();
    let mut markers = Vec::new();
    let mut seq = stream.base_seq();

    for chunk in stream.slices(stream.base_seq()..stream.head_seq()) {
        let mut i = 0usize;
        while i < chunk.len() {
            if !parser.is_mid_sequence() {
                // In the ground state every byte but `ESC` is a no-op, so skipping
                // to the next one is not an optimisation that could drift: there is
                // nothing to feed.
                match memchr(ESC, &chunk[i..]) {
                    Some(offset) => i += offset,
                    None => break,
                }
            }
            let at = seq + i as u64;
            parser.feed(&chunk[i..=i], &mut declarations);
            for declaration in declarations.drain(..) {
                markers.push(marker(at + 1, declaration));
            }
            i += 1;
        }
        seq += chunk.len() as u64;
    }

    markers
}

/// A declaration plus its position, as a timeline marker.
///
/// A hint with no label of its own gets the state's own name, because a marker with
/// an empty label is a tick a user cannot identify.
fn marker(seq: u64, declaration: HintDeclaration) -> Marker {
    let label = declaration
        .label
        .unwrap_or_else(|| state_label(declaration.state).to_string());
    Marker {
        seq,
        label,
        hint: Some(declaration.state),
    }
}

/// The operator-facing name of a hint state.
const fn state_label(state: vitrum_proto::HintState) -> &'static str {
    match state {
        vitrum_proto::HintState::Approval => "approval needed",
        vitrum_proto::HintState::Input => "input needed",
        vitrum_proto::HintState::Working => "working",
        vitrum_proto::HintState::Ready => "ready",
    }
}
