//! The opt-in agent hint channel: a strict streaming parser for OSC 7373.
//!
//! # The sequence
//!
//! ```text
//! ESC ] 7373 ; <state> [ ; <label> ] ST
//! ```
//!
//! - `ESC ]` is 0x1B 0x5D, the standard OSC introducer.
//! - `7373` is [`vitrum_proto::HINT_OSC`], from the private range.
//! - `<state>` is one of `approval`, `input`, `working`, `ready`, lowercase and
//!   exact.
//! - `<label>` is optional operator-facing text, for example the question being
//!   asked. Everything after the second `;` is label, semicolons included.
//! - `ST` is BEL (0x07) or `ESC \` (0x1B 0x5C). Both are accepted because both
//!   are in the wild and a harness author should not have to care.
//!
//! A harness emits it the same way it would emit a title:
//!
//! ```sh
//! printf '\033]7373;approval;write src/main.rs\033\\'
//! ```
//!
//! Any terminal that does not know OSC 7373 ignores it, so a harness can emit
//! it unconditionally without corrupting output elsewhere.
//!
//! # What this buys
//!
//! Observation already gives the sidebar its states for every process on the
//! machine. The hint channel refines them: it splits "blocked on the operator"
//! into approval versus a plain question, and supplies a label the shell could
//! never infer. An agent that emits nothing loses the refinement and keeps
//! everything else.
//!
//! # Strictness
//!
//! This parser sits directly on a PTY byte stream that carries arbitrary
//! output, including hostile output: any program can print any bytes, and a
//! coding agent routinely prints other programs' output verbatim. So the rule
//! is that a sequence is accepted only when it is exactly right, and anything
//! else is dropped and counted. In particular:
//!
//! - An unknown state token is rejected, never defaulted. A future `paused`
//!   state must read as "no hint", not as `ready`.
//! - A wrong OSC number is not ours; it is dropped without comment.
//! - Non-UTF-8 payloads are rejected outright.
//! - A control byte inside the sequence ends it as malformed, because a real
//!   OSC payload never contains one and a stray ESC almost always means the
//!   producer emitted something else entirely.
//! - An unterminated sequence is abandoned at [`MAX_SEQUENCE_BYTES`] rather
//!   than buffering without limit. A hostile stream must not be able to make
//!   the client allocate.
//!
//! Sequences split across reads are the normal case, not an edge case: a PTY
//! read boundary lands wherever the kernel put it. The parser is a byte-at-a-
//! time state machine for exactly that reason.

use vitrum_proto::{AgentHint, HINT_OSC, HintState};

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const DEL: u8 = 0x7f;

/// Longest OSC payload retained before the sequence is abandoned as malformed.
///
/// Bounds the parser's memory against a stream that emits `ESC ]` and never
/// terminates it. Generous next to a real hint, which is a token plus a short
/// label.
pub const MAX_SEQUENCE_BYTES: usize = 256;

/// Longest label kept, in characters.
///
/// A longer label is truncated rather than rejected: an over-long display
/// string is a formatting problem, not a malformed sequence, and discarding a
/// valid state because its label ran long would lose real information.
pub const MAX_LABEL_CHARS: usize = 120;

/// A well-formed declaration, before it is stamped with a receipt time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HintDeclaration {
    pub state: HintState,
    pub label: Option<String>,
}

impl HintDeclaration {
    /// Stamp the declaration to produce the wire type.
    pub fn into_hint(self, received_at_ms: u64) -> AgentHint {
        AgentHint {
            state: self.state,
            label: self.label,
            received_at_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Outside any sequence.
    Ground,
    /// Saw ESC, waiting to see whether it introduces an OSC.
    Escape,
    /// Inside an OSC payload.
    Payload,
    /// Inside an OSC payload, saw ESC; `\` terminates.
    PayloadEscape,
}

/// Incremental OSC 7373 extractor.
///
/// Feed it every byte of the session's output in order. It keeps only the bytes
/// of a sequence currently in flight, so the steady-state cost on ordinary
/// output is one comparison per byte and no allocation.
#[derive(Debug)]
pub struct HintParser {
    phase: Phase,
    payload: Vec<u8>,
    accepted: u64,
    rejected: u64,
}

impl Default for HintParser {
    fn default() -> Self {
        Self::new()
    }
}

impl HintParser {
    pub fn new() -> Self {
        HintParser {
            phase: Phase::Ground,
            payload: Vec::new(),
            accepted: 0,
            rejected: 0,
        }
    }

    /// Consume `bytes`, appending every complete valid declaration to `out`.
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<HintDeclaration>) {
        for &byte in bytes {
            match self.phase {
                Phase::Ground => {
                    if byte == ESC {
                        self.phase = Phase::Escape;
                    }
                }
                Phase::Escape => match byte {
                    b']' => {
                        self.payload.clear();
                        self.phase = Phase::Payload;
                    }
                    // Two ESCs in a row: the second is the live one.
                    ESC => {}
                    _ => self.phase = Phase::Ground,
                },
                Phase::Payload => match byte {
                    BEL => self.terminate(out),
                    ESC => self.phase = Phase::PayloadEscape,
                    // No legitimate OSC payload contains a C0 control or DEL.
                    0x00..=0x1f | DEL => self.abandon(),
                    _ => {
                        if self.payload.len() >= MAX_SEQUENCE_BYTES {
                            self.abandon();
                        } else {
                            self.payload.push(byte);
                        }
                    }
                },
                Phase::PayloadEscape => match byte {
                    b'\\' => self.terminate(out),
                    // `ESC ]` inside a payload means the producer started a new
                    // OSC without finishing this one. Drop this one, take the
                    // new one, and resynchronise on the spot.
                    b']' => {
                        self.rejected += 1;
                        self.payload.clear();
                        self.phase = Phase::Payload;
                    }
                    // ESC ESC: abandon this sequence, the second ESC is live.
                    ESC => {
                        self.rejected += 1;
                        self.payload.clear();
                        self.phase = Phase::Escape;
                    }
                    _ => self.abandon(),
                },
            }
        }
    }

    /// Convenience form of [`HintParser::feed`] for callers that want a fresh
    /// vector, such as tests and one-shot parsing.
    pub fn feed_to_vec(&mut self, bytes: &[u8]) -> Vec<HintDeclaration> {
        let mut out = Vec::new();
        self.feed(bytes, &mut out);
        out
    }

    /// Complete valid declarations seen so far.
    pub fn accepted(&self) -> u64 {
        self.accepted
    }

    /// Sequences dropped as malformed so far.
    ///
    /// Exposed because "the hint never showed up" and "the hint was malformed"
    /// are very different problems for a harness author, and the second one
    /// should be visible somewhere rather than being silent.
    pub fn rejected(&self) -> u64 {
        self.rejected
    }

    /// Bytes currently buffered for a sequence in flight. Zero at rest.
    pub fn pending_bytes(&self) -> usize {
        self.payload.len()
    }

    /// True while a partial sequence is buffered.
    pub fn is_mid_sequence(&self) -> bool {
        self.phase != Phase::Ground
    }

    /// Drop any partial sequence and return to the ground state.
    ///
    /// Call on reconnect, where the byte stream restarts and a half-read
    /// sequence from before the gap must not fuse onto the new one.
    pub fn reset(&mut self) {
        self.phase = Phase::Ground;
        self.payload.clear();
    }

    fn terminate(&mut self, out: &mut Vec<HintDeclaration>) {
        match parse_payload(&self.payload) {
            Some(declaration) => {
                self.accepted += 1;
                out.push(declaration);
            }
            None => self.rejected += 1,
        }
        self.payload.clear();
        self.phase = Phase::Ground;
    }

    fn abandon(&mut self) {
        self.rejected += 1;
        self.payload.clear();
        self.phase = Phase::Ground;
    }
}

/// Parse a terminated OSC payload, without the introducer or terminator.
///
/// Returns `None` for anything that is not exactly an OSC 7373 hint, including
/// other applications' OSC sequences, which flow through here constantly.
pub fn parse_payload(payload: &[u8]) -> Option<HintDeclaration> {
    let text = core::str::from_utf8(payload).ok()?;
    let mut parts = text.splitn(3, ';');

    let number = parts.next()?;
    if !is_our_osc_number(number) {
        return None;
    }

    let state = HintState::parse(parts.next()?)?;

    let label = match parts.next() {
        None => None,
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else if trimmed.chars().any(char::is_control) {
                return None;
            } else {
                Some(truncate_chars(trimmed, MAX_LABEL_CHARS))
            }
        }
    };

    Some(HintDeclaration { state, label })
}

/// Exact decimal match against [`HINT_OSC`].
///
/// Deliberately not `str::parse`, which would accept `"07373"` and `"+7373"`.
/// A terminal parser that is loose about the sequence number is one that will
/// eventually claim someone else's OSC.
fn is_our_osc_number(number: &str) -> bool {
    !number.is_empty()
        && number.as_bytes()[0] != b'0'
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && number.parse::<u32>() == Ok(HINT_OSC)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((byte_index, _)) => text[..byte_index].to_string(),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(chunks: &[&[u8]]) -> (Vec<HintDeclaration>, HintParser) {
        let mut parser = HintParser::new();
        let mut out = Vec::new();
        for chunk in chunks {
            parser.feed(chunk, &mut out);
        }
        (out, parser)
    }

    /// The documented happy path with both terminators. A harness author copies
    /// one of these two forms out of the docs, so both must work exactly.
    #[test]
    fn a_minimal_hint_parses_with_either_terminator() {
        for sequence in [
            b"\x1b]7373;working\x07".as_slice(),
            b"\x1b]7373;working\x1b\\".as_slice(),
        ] {
            let (hints, parser) = feed_all(&[sequence]);
            assert_eq!(
                hints,
                vec![HintDeclaration {
                    state: HintState::Working,
                    label: None
                }]
            );
            assert_eq!(parser.accepted(), 1);
            assert_eq!(parser.rejected(), 0);
            assert_eq!(parser.pending_bytes(), 0);
            assert!(!parser.is_mid_sequence());
        }
    }

    /// All four state tokens must be accepted, since dropping one silently
    /// removes a state from the sidebar for every harness that emits it.
    #[test]
    fn every_state_token_is_accepted() {
        let cases = [
            (b"\x1b]7373;approval\x07".as_slice(), HintState::Approval),
            (b"\x1b]7373;input\x07".as_slice(), HintState::Input),
            (b"\x1b]7373;working\x07".as_slice(), HintState::Working),
            (b"\x1b]7373;ready\x07".as_slice(), HintState::Ready),
        ];
        for (sequence, expected) in cases {
            let (hints, _) = feed_all(&[sequence]);
            assert_eq!(hints.len(), 1);
            assert_eq!(hints[0].state, expected);
        }
    }

    /// The label is the thing observation can never supply, so it must survive
    /// intact, including internal semicolons and non-ASCII text.
    #[test]
    fn a_label_survives_semicolons_and_non_ascii() {
        let (hints, _) = feed_all(&[b"\x1b]7373;approval;write src/main.rs; then run tests\x07"]);
        assert_eq!(
            hints,
            vec![HintDeclaration {
                state: HintState::Approval,
                label: Some("write src/main.rs; then run tests".to_string()),
            }]
        );

        let (unicode, _) = feed_all(&["\x1b]7373;input;renommer «café» ?\x07".as_bytes()]);
        assert_eq!(unicode[0].label.as_deref(), Some("renommer «café» ?"));
    }

    /// Surrounding whitespace is trimmed and an all-whitespace label becomes
    /// `None`, so a harness that pads its format string does not produce a badge
    /// that renders as an empty box.
    #[test]
    fn a_blank_label_becomes_none() {
        let (padded, _) = feed_all(&[b"\x1b]7373;ready;   done   \x07"]);
        assert_eq!(padded[0].label.as_deref(), Some("done"));

        for blank in [
            b"\x1b]7373;ready;\x07".as_slice(),
            b"\x1b]7373;ready;   \x07".as_slice(),
        ] {
            let (hints, parser) = feed_all(&[blank]);
            assert_eq!(hints[0].label, None);
            assert_eq!(hints[0].state, HintState::Ready);
            assert_eq!(parser.rejected(), 0);
        }
    }

    /// A PTY read boundary lands wherever the kernel put it. Splitting the same
    /// sequence at every single byte offset must yield the identical result, or
    /// hints work on a quiet stream and vanish on a busy one.
    #[test]
    fn a_sequence_split_at_every_byte_offset_parses_identically() {
        let sequence = b"\x1b]7373;approval;deploy to prod\x1b\\";
        let expected = HintDeclaration {
            state: HintState::Approval,
            label: Some("deploy to prod".to_string()),
        };
        for split in 0..=sequence.len() {
            let (head, tail) = sequence.split_at(split);
            let (hints, parser) = feed_all(&[head, tail]);
            assert_eq!(hints, vec![expected.clone()], "split at {split}");
            assert_eq!(parser.rejected(), 0, "split at {split}");
        }
    }

    /// The pathological split: one byte per chunk. This is what a slow pipe or
    /// a keystroke-echoing shell actually produces.
    #[test]
    fn a_sequence_delivered_one_byte_at_a_time_parses() {
        let sequence = b"\x1b]7373;input;continue?\x07";
        let mut parser = HintParser::new();
        let mut out = Vec::new();
        for byte in sequence {
            parser.feed(&[*byte], &mut out);
        }
        assert_eq!(
            out,
            vec![HintDeclaration {
                state: HintState::Input,
                label: Some("continue?".to_string()),
            }]
        );
        assert_eq!(parser.accepted(), 1);
    }

    /// A truncated sequence must produce nothing and must not be counted as a
    /// rejection while it could still be completed. The parser cannot know the
    /// stream ended.
    #[test]
    fn a_truncated_sequence_yields_nothing_and_stays_pending() {
        let (hints, parser) = feed_all(&[b"\x1b]7373;appro"]);
        assert_eq!(hints, vec![]);
        assert_eq!(parser.accepted(), 0);
        assert_eq!(parser.rejected(), 0);
        assert!(parser.is_mid_sequence());
        assert_eq!(parser.pending_bytes(), 10);
    }

    /// A truncated sequence completed by a later chunk must parse, and a
    /// truncated one abandoned by `reset` must not fuse onto the next stream.
    #[test]
    fn reset_discards_a_pending_sequence_instead_of_fusing_it() {
        let mut parser = HintParser::new();
        let mut out = Vec::new();
        parser.feed(b"\x1b]7373;work", &mut out);
        assert!(parser.is_mid_sequence());

        parser.reset();
        assert!(!parser.is_mid_sequence());
        assert_eq!(parser.pending_bytes(), 0);

        // Without the reset these bytes would have completed "working".
        parser.feed(b"ing\x07", &mut out);
        assert_eq!(out, vec![]);
        assert_eq!(parser.accepted(), 0);
        assert_eq!(parser.rejected(), 0);
    }

    /// An unterminated sequence must be bounded. A stream that opens an OSC and
    /// never closes it would otherwise buffer without limit, which is a remote
    /// memory-exhaustion path from any program the agent runs.
    #[test]
    fn an_unterminated_sequence_is_abandoned_at_the_byte_cap() {
        let mut parser = HintParser::new();
        let mut out = Vec::new();
        parser.feed(b"\x1b]", &mut out);
        parser.feed(&vec![b'x'; MAX_SEQUENCE_BYTES - 1], &mut out);
        assert_eq!(parser.pending_bytes(), MAX_SEQUENCE_BYTES - 1);
        assert_eq!(parser.rejected(), 0);

        parser.feed(&vec![b'x'; 10_000], &mut out);
        assert_eq!(out, vec![]);
        assert_eq!(parser.rejected(), 1);
        assert_eq!(parser.pending_bytes(), 0);
        assert!(!parser.is_mid_sequence());

        // And the parser still works afterwards.
        parser.feed(b"\x1b]7373;ready\x07", &mut out);
        assert_eq!(out.len(), 1);
    }

    /// A valid hint arriving right after an abandoned one must parse. Resync is
    /// the whole point of bounding the buffer.
    #[test]
    fn the_parser_resynchronises_after_garbage() {
        let (hints, parser) = feed_all(&[
            b"\x1b]999;other app title\x07",
            b"random terminal output \x1b[0m\x1b[1;32m",
            b"\x1b]7373;ready;all tests pass\x07",
        ]);
        assert_eq!(
            hints,
            vec![HintDeclaration {
                state: HintState::Ready,
                label: Some("all tests pass".to_string()),
            }]
        );
        assert_eq!(parser.accepted(), 1);
        assert_eq!(parser.rejected(), 1, "the foreign OSC 999 counts as one drop");
    }

    /// An unknown state token must be dropped, never defaulted. A future
    /// `paused` state emitted by a newer harness must read as "no hint", not as
    /// whichever variant happens to be first.
    #[test]
    fn an_unknown_state_token_is_rejected_not_defaulted() {
        for sequence in [
            b"\x1b]7373;paused\x07".as_slice(),
            b"\x1b]7373;Approval\x07".as_slice(),
            b"\x1b]7373;APPROVAL\x07".as_slice(),
            b"\x1b]7373; approval\x07".as_slice(),
            b"\x1b]7373;approval \x07".as_slice(),
            b"\x1b]7373;\x07".as_slice(),
            b"\x1b]7373\x07".as_slice(),
        ] {
            let (hints, parser) = feed_all(&[sequence]);
            assert_eq!(hints, vec![], "accepted {sequence:?}");
            assert_eq!(parser.accepted(), 0);
            assert_eq!(parser.rejected(), 1);
        }
    }

    /// Another application's OSC must not be claimed. Terminals are full of
    /// OSC 0/2 title sets and OSC 8 hyperlinks, and a loose number match would
    /// turn a window title into a fake approval prompt.
    #[test]
    fn foreign_and_near_miss_osc_numbers_are_rejected() {
        for sequence in [
            b"\x1b]0;bash\x07".as_slice(),
            b"\x1b]2;window title\x07".as_slice(),
            b"\x1b]8;;https://example.com\x07".as_slice(),
            b"\x1b]737;approval\x07".as_slice(),
            b"\x1b]73730;approval\x07".as_slice(),
            b"\x1b]07373;approval\x07".as_slice(),
            b"\x1b]+7373;approval\x07".as_slice(),
            b"\x1b] 7373;approval\x07".as_slice(),
            b"\x1b]7373 ;approval\x07".as_slice(),
            b"\x1b];approval\x07".as_slice(),
        ] {
            let (hints, parser) = feed_all(&[sequence]);
            assert_eq!(hints, vec![], "accepted {sequence:?}");
            assert_eq!(parser.rejected(), 1, "for {sequence:?}");
        }
    }

    /// Invalid UTF-8 in the payload must be rejected outright rather than
    /// lossily decoded. A lossy decode would let arbitrary binary output become
    /// a label full of replacement characters.
    #[test]
    fn a_non_utf8_payload_is_rejected() {
        let mut sequence = b"\x1b]7373;ready;".to_vec();
        sequence.extend_from_slice(&[0xff, 0xfe, 0x80]);
        sequence.push(BEL);
        let (hints, parser) = feed_all(&[&sequence]);
        assert_eq!(hints, vec![]);
        assert_eq!(parser.rejected(), 1);
    }

    /// A control byte inside the payload ends the sequence as malformed. A real
    /// hint never contains one, and letting a newline through would allow output
    /// on the next line to be absorbed into a label.
    #[test]
    fn control_bytes_inside_a_payload_abandon_the_sequence() {
        for injected in [b"\n".as_slice(), b"\r".as_slice(), b"\x00".as_slice(), b"\x7f".as_slice()]
        {
            let mut sequence = b"\x1b]7373;ready;label".to_vec();
            sequence.extend_from_slice(injected);
            sequence.extend_from_slice(b"more\x07");
            let (hints, parser) = feed_all(&[&sequence]);
            assert_eq!(hints, vec![], "accepted control byte {injected:?}");
            assert_eq!(parser.rejected(), 1, "for {injected:?}");
        }
    }

    /// An OSC opened inside an unterminated OSC takes over. Nested introducers
    /// are what a program printing another program's captured output produces,
    /// and the inner complete sequence is the one that means something.
    #[test]
    fn a_nested_introducer_abandons_the_outer_sequence_and_keeps_the_inner() {
        let (hints, parser) = feed_all(&[b"\x1b]7373;working\x1b]7373;approval;rm -rf build\x07"]);
        assert_eq!(
            hints,
            vec![HintDeclaration {
                state: HintState::Approval,
                label: Some("rm -rf build".to_string()),
            }]
        );
        assert_eq!(parser.accepted(), 1);
        assert_eq!(parser.rejected(), 1);
    }

    /// `ESC` followed by anything other than `\` or `]` inside a payload is not
    /// a terminator; the sequence is malformed and must not be silently
    /// completed by the next terminator that comes along.
    #[test]
    fn an_escape_that_is_not_a_terminator_abandons_the_sequence() {
        let (hints, parser) = feed_all(&[b"\x1b]7373;ready\x1b[0m\x07"]);
        assert_eq!(hints, vec![]);
        assert_eq!(parser.rejected(), 1);

        // A double ESC keeps the second one live, so a following OSC still works.
        let (recovered, recovered_parser) =
            feed_all(&[b"\x1b]7373;ready\x1b\x1b]7373;working\x07"]);
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].state, HintState::Working);
        assert_eq!(recovered_parser.rejected(), 1);
    }

    /// Several hints in a single read must all come out, in order. A busy agent
    /// emitting working then ready inside one flush must not lose the last one.
    #[test]
    fn multiple_hints_in_one_chunk_all_emerge_in_order() {
        let (hints, parser) = feed_all(&[
            b"start\x1b]7373;working\x07middle\x1b]7373;ready;done\x1b\\end",
        ]);
        assert_eq!(
            hints,
            vec![
                HintDeclaration {
                    state: HintState::Working,
                    label: None
                },
                HintDeclaration {
                    state: HintState::Ready,
                    label: Some("done".to_string()),
                },
            ]
        );
        assert_eq!(parser.accepted(), 2);
        assert_eq!(parser.rejected(), 0);
    }

    /// An over-long label is truncated on a character boundary, not rejected and
    /// not sliced mid-codepoint. Slicing a multi-byte character would panic.
    #[test]
    fn an_over_long_label_is_truncated_on_a_character_boundary() {
        let label = "é".repeat(200);
        let sequence = format!("\x1b]7373;input;{label}\x07");
        let mut parser = HintParser::new();
        let hints = parser.feed_to_vec(sequence.as_bytes());
        // 200 two-byte chars plus the prefix exceeds the payload cap, so this
        // one is abandoned rather than truncated.
        assert_eq!(hints, vec![]);
        assert_eq!(parser.rejected(), 1);

        // Inside the byte cap but past the character cap: truncated, kept.
        let long_ascii = "x".repeat(MAX_LABEL_CHARS + 30);
        let mut parser = HintParser::new();
        let hints = parser.feed_to_vec(format!("\x1b]7373;input;{long_ascii}\x07").as_bytes());
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].label.as_deref().map(str::len), Some(MAX_LABEL_CHARS));
        assert_eq!(parser.rejected(), 0);

        // Two bytes per char, so this must stay inside MAX_SEQUENCE_BYTES while
        // still exceeding MAX_LABEL_CHARS.
        let multibyte = "é".repeat(MAX_LABEL_CHARS + 2);
        let mut parser = HintParser::new();
        let hints = parser.feed_to_vec(format!("\x1b]7373;input;{multibyte}\x07").as_bytes());
        assert_eq!(hints.len(), 1);
        assert_eq!(
            hints[0].label.as_deref().map(|label| label.chars().count()),
            Some(MAX_LABEL_CHARS)
        );
    }

    /// Ordinary output must cost nothing and leave no state behind, or a busy
    /// session slowly accumulates buffered bytes for a sequence that never was.
    #[test]
    fn ordinary_output_leaves_the_parser_at_rest() {
        let (hints, parser) = feed_all(&[
            b"\x1b[1;32mPASS\x1b[0m 42 tests\r\n",
            b"\x1b[2J\x1b[H",
            b"\x07",
            b"plain text with ] and ; and 7373 in it",
        ]);
        assert_eq!(hints, vec![]);
        assert_eq!(parser.accepted(), 0);
        assert_eq!(parser.rejected(), 0);
        assert_eq!(parser.pending_bytes(), 0);
        assert!(!parser.is_mid_sequence());
    }

    /// The pure payload parser is the unit the state machine delegates to;
    /// pinning it directly keeps the acceptance rules readable and stops the
    /// two from drifting.
    #[test]
    fn the_payload_parser_accepts_exactly_the_documented_grammar() {
        assert_eq!(
            parse_payload(b"7373;ready"),
            Some(HintDeclaration {
                state: HintState::Ready,
                label: None
            })
        );
        assert_eq!(
            parse_payload(b"7373;ready;label"),
            Some(HintDeclaration {
                state: HintState::Ready,
                label: Some("label".to_string()),
            })
        );
        assert_eq!(parse_payload(b""), None);
        assert_eq!(parse_payload(b"7373"), None);
        assert_eq!(parse_payload(b"7373;"), None);
        assert_eq!(parse_payload(b";;"), None);
        assert_eq!(parse_payload(b"7373;ready;with\x07bell"), None);
    }

    /// Stamping a declaration produces the wire type the daemon publishes, and
    /// the receipt time is the client's, not something parsed out of the stream.
    #[test]
    fn a_declaration_stamps_into_the_wire_hint() {
        let declaration = HintDeclaration {
            state: HintState::Approval,
            label: Some("git push --force".to_string()),
        };
        let hint = declaration.into_hint(1_772_580_600_000);
        assert_eq!(hint.state, HintState::Approval);
        assert_eq!(hint.label.as_deref(), Some("git push --force"));
        assert_eq!(hint.received_at_ms, 1_772_580_600_000);
    }
}
