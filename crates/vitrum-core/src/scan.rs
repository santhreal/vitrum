//! One pass over PTY output for every in-band signal the sidebar needs.
//!
//! Three signals ride the same byte stream and are found together, because
//! walking a firehose three times to answer three questions is three times the
//! cost of walking it once:
//!
//! - **BEL**, the universal "a human please" every terminal program already
//!   knows how to send.
//! - **OSC 777**, the desktop-notification convention, which is a signal even
//!   when it is terminated by `ESC \` rather than BEL.
//! - **OSC 7373**, vitrum's opt-in agent hint channel, parsed by
//!   [`vitrum_model::hint::HintParser`] rather than by a second parser here.
//!
//! # Cost
//!
//! The scan is byte-at-a-time only while a sequence is actually in flight. The
//! rest of the time it jumps straight to the next ESC or BEL, which is a
//! vectorisable search, so a megabyte of plain output costs a scan and no
//! per-byte state machine. That matters: twenty agents streaming at once is the
//! normal case, and a per-byte match on eight megabytes a second is real CPU.
//!
//! # Split sequences
//!
//! A read boundary lands wherever the kernel put it, so an introducer split
//! across two or three chunks is ordinary, not exotic. Both matchers here are
//! streaming: the hint parser keeps its own phase, and OSC 777 is found with a
//! running match position rather than by buffering a tail.

use vitrum_model::hint::{HintDeclaration, HintParser};

const BEL: u8 = 0x07;
const ESC: u8 = 0x1b;
const OSC_777: &[u8] = b"\x1b]777;";

/// Streaming scanner over one session's output, in order.
pub(crate) struct OutputScan {
    /// How many leading bytes of [`OSC_777`] have matched so far.
    matched: usize,
    hints: HintParser,
}

impl OutputScan {
    pub(crate) fn new() -> Self {
        Self {
            matched: 0,
            hints: HintParser::new(),
        }
    }

    /// Scan `data`, appending every complete hint declaration to `hints`.
    ///
    /// Returns whether this run of output asked for the operator.
    pub(crate) fn scan(&mut self, data: &[u8], hints: &mut Vec<HintDeclaration>) -> bool {
        let mut wants_operator = false;
        let mut at = 0;
        while at < data.len() {
            if self.matched == 0 && !self.hints.is_mid_sequence() {
                // Nothing is in flight, so only an ESC or a BEL can change
                // anything. Skipping to it is what keeps ordinary output cheap.
                let Some(offset) = data[at..].iter().position(|&b| b == ESC || b == BEL) else {
                    break;
                };
                at += offset;
                if data[at] == BEL {
                    wants_operator = true;
                    at += 1;
                    continue;
                }
            }
            let byte = data[at];
            // A BEL inside an OSC payload is that sequence's terminator, not a
            // request for a human: a window title and a hint both end in one.
            // Asking the parser BEFORE the feed is what keeps the two in step,
            // because the feed is what consumes the terminator.
            wants_operator |= byte == BEL && !self.hints.is_mid_sequence();
            wants_operator |= self.step_777(byte);
            self.hints.feed(&data[at..at + 1], hints);
            at += 1;
        }
        wants_operator
    }

    /// Sequences dropped as malformed so far.
    ///
    /// A harness author debugging "my hint never showed up" needs to know
    /// whether the daemon saw nothing or saw something it refused.
    pub(crate) fn rejected_hints(&self) -> u64 {
        self.hints.rejected()
    }

    /// Advance the streaming match for the OSC 777 introducer.
    ///
    /// The introducer is reported as soon as it is complete, without waiting
    /// for a terminator, because that is what makes an ST-terminated
    /// notification a signal too.
    fn step_777(&mut self, byte: u8) -> bool {
        if byte == OSC_777[self.matched] {
            self.matched += 1;
            if self.matched == OSC_777.len() {
                self.matched = 0;
                return true;
            }
        } else {
            // `\x1b]777;` overlaps itself only at its leading ESC, so a byte
            // that breaks the match can restart it only by being that ESC.
            self.matched = usize::from(byte == OSC_777[0]);
        }
        false
    }
}
