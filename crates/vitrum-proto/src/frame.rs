//! The data plane: one binary frame carrying raw PTY bytes.
//!
//! The control plane is JSON and lives beside this in the crate root. This is
//! the other half of the split described there, and it is separate because it
//! answers a different question: not "what did either side say" but "which
//! session's stream is this, and where in it does this payload start".
//!
//! A frame is `[kind][session: u64 LE][seq: u64 LE][payload]`, fixed header
//! then verbatim bytes. Nothing here parses the payload, because a terminal
//! must forward bytes it cannot interpret: a partial UTF-8 sequence straddling
//! a read boundary, a mouse report, a DEC response.
//!
//! Every rejected shape has a named error and no path panics, because the
//! bytes arrive from a socket.

use crate::SessionId;

/// Byte length of a data-plane frame header: kind + session + seq.
pub const OUTPUT_HEADER_LEN: usize = 1 + 8 + 8;

/// Data-plane frame kind for PTY output.
pub const FRAME_KIND_OUTPUT: u8 = 1;

/// Errors from decoding a data-plane frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// Fewer bytes than [`OUTPUT_HEADER_LEN`].
    TooShort { len: usize },
    /// Leading kind byte is not a kind this version understands.
    UnknownKind(u8),
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FrameError::TooShort { len } => {
                write!(f, "frame is {len} bytes, need at least {OUTPUT_HEADER_LEN}")
            }
            FrameError::UnknownKind(k) => write!(f, "unknown frame kind {k}"),
        }
    }
}

impl core::error::Error for FrameError {}

/// Encode one PTY output frame: `[kind][session: u64 LE][seq: u64 LE][payload]`.
///
/// `seq` is the byte offset of `payload[0]` within the session's output stream,
/// which is what lets a reconnecting client ask for exactly the range it missed
/// and lets the server detect a gap rather than silently splicing.
pub fn encode_output(session: SessionId, seq: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(OUTPUT_HEADER_LEN + payload.len());
    encode_output_into(&mut out, session, seq, payload);
    out
}

/// Append a frame to `out` instead of allocating a fresh one.
///
/// The output pump encodes one frame per PTY read, so a per-frame `Vec` is an
/// allocation on the hottest path in the daemon. A pump that keeps one buffer
/// and clears it per frame pays none.
pub fn encode_output_into(out: &mut Vec<u8>, session: SessionId, seq: u64, payload: &[u8]) {
    out.reserve(OUTPUT_HEADER_LEN + payload.len());
    out.push(FRAME_KIND_OUTPUT);
    out.extend_from_slice(&session.0.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(payload);
}

/// Decode a frame produced by [`encode_output`], borrowing the payload.
///
/// Total: every rejected shape has a named error and there is no panicking
/// path, because the bytes arrive from a socket.
pub fn decode_output(frame: &[u8]) -> Result<(SessionId, u64, &[u8]), FrameError> {
    if frame.len() < OUTPUT_HEADER_LEN {
        return Err(FrameError::TooShort { len: frame.len() });
    }
    let Some((&kind, rest)) = frame.split_first() else {
        return Err(FrameError::TooShort { len: frame.len() });
    };
    if kind != FRAME_KIND_OUTPUT {
        return Err(FrameError::UnknownKind(kind));
    }
    // Fixed-size chunks rather than `try_into().expect(..)`: the length check
    // above already proves these succeed, and a proof the compiler checks is
    // worth more here than one in a panic message.
    let Some((session, rest)) = rest.split_first_chunk::<8>() else {
        return Err(FrameError::TooShort { len: frame.len() });
    };
    let Some((seq, payload)) = rest.split_first_chunk::<8>() else {
        return Err(FrameError::TooShort { len: frame.len() });
    };
    Ok((
        SessionId(u64::from_le_bytes(*session)),
        u64::from_le_bytes(*seq),
        payload,
    ))
}
