//! One pass over PTY output for every in-band signal the daemon needs.
//!
//! Five signals ride the same byte stream and are found together, because
//! walking a firehose five times to answer five questions is five times the
//! cost of walking it once:
//!
//! - **BEL**, the universal "a human please" every terminal program already
//!   knows how to send.
//! - **OSC 777**, the desktop-notification convention, which is a signal even
//!   when it is terminated by `ESC \` rather than BEL.
//! - **OSC 7373**, vitrum's opt-in agent hint channel, parsed by
//!   [`vitrum_model::hint::HintParser`] rather than by a second parser here.
//! - **OSC 0 and OSC 2**, the window title, which is what an agent puts its
//!   status in and therefore what approval detection reads.
//! - **OSC 7**, the working directory a shell reports, decoded by
//!   [`vitrum_vt::pwd_path`] so there is still exactly one decoder for it.
//!
//! # Why the last two are here and not in a terminal engine
//!
//! They used to come from libghostty: the daemon ran a full VT engine per
//! session, fed it every byte, and read two strings back off it. Measured
//! end to end through a real pty, that parse was 57% of everything a session
//! spent moving a megabyte, for a grid nothing in the daemon ever looks at —
//! the client has its own emulator and renders from the raw bytes. A terminal
//! engine is the right tool for a screen and the wrong one for two strings.
//!
//! The engine remains the right answer for a screen, and if the daemon ever
//! needs one it should have one. It should not be on the path of every byte to
//! answer a question a state machine with six states answers.
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
//! across two or three chunks is ordinary, not exotic. Every matcher here is
//! streaming: the hint parser keeps its own phase, OSC 777 is found with a
//! running match position, and the title and directory capture holds its phase
//! and its partial payload across as many chunks as it takes.

use vitrum_model::hint::{HintDeclaration, HintParser};

const BEL: u8 = 0x07;
const ESC: u8 = 0x1b;
const OSC_777: &[u8] = b"\x1b]777;";

/// Streaming scanner over one session's output, in order.
pub(crate) struct OutputScan {
    /// How many leading bytes of [`OSC_777`] have matched so far.
    matched: usize,
    hints: HintParser,
    osc: OscCapture,
}

impl OutputScan {
    pub(crate) fn new() -> Self {
        Self {
            matched: 0,
            hints: HintParser::new(),
            osc: OscCapture::new(),
        }
    }

    /// Scan `data`, appending every complete hint declaration to `hints`.
    ///
    /// Returns whether this run of output asked for the operator.
    pub(crate) fn scan(&mut self, data: &[u8], hints: &mut Vec<HintDeclaration>) -> bool {
        let mut wants_operator = false;
        let mut at = 0;
        while at < data.len() {
            if !self.mid_sequence() {
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
            self.osc.step(byte);
            at += 1;
        }
        wants_operator
    }

    /// Whether a partly-read sequence keeps the scan on the byte-at-a-time
    /// path.
    ///
    /// The fast path is the whole cost model of output: with nothing in
    /// flight the scan skips to the next ESC or BEL by vector search, and one
    /// session that is stuck mid-sequence pays a branch per byte instead, for
    /// as long as it stays stuck. Every bound on how long a sequence may stay
    /// open is a bound on this, so this is the thing worth asserting about.
    pub(crate) fn mid_sequence(&self) -> bool {
        self.matched != 0 || self.hints.is_mid_sequence() || self.osc.in_flight()
    }

    /// The most recent window title this session announced, once.
    ///
    /// Last-wins and taken rather than read: a program that retitles itself
    /// three times in one coalescing window has one current title, and the two
    /// it has already replaced are not information.
    pub(crate) fn take_title(&mut self) -> Option<String> {
        self.osc.title.take()
    }

    /// The most recent working directory this session reported, once.
    pub(crate) fn take_pwd(&mut self) -> Option<String> {
        self.osc.pwd.take()
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

/// The longest OSC payload worth keeping, in bytes.
///
/// A window title is a line of text and a directory is a path. Anything past
/// this is either a program using the title as a data channel or a stream that
/// lost its terminator, and in both cases holding onto it would let output
/// choose how much memory the daemon allocates. The payload is dropped rather
/// than truncated: half a title is not a title, and half a path would name the
/// wrong directory.
const OSC_PAYLOAD_MAX: usize = 2048;

/// Bytes one OSC string may consume before the scan gives up on it.
///
/// The payload cap above bounds what is KEPT. It does not bound how long the
/// scanner stays inside a string, and the scanner runs byte at a time for as
/// long as one is open: `printf '\e]x'` with no terminator leaves the capture
/// in flight for every byte the session writes afterwards, so one three-byte
/// line moves that session's whole output path off the vectorised skip for
/// the rest of its life. Twenty agents at eight megabytes a second is the
/// normal case, and this is the one thing output can do to that budget.
///
/// Twice the payload cap: anything past the point where the payload has
/// already been refused is a stream that lost its terminator, and abandoning
/// it returns to the fast path immediately. `HintParser` abandons an
/// unterminated sequence at 256 bytes for the same reason, and, like it, the
/// terminator that eventually arrives is then ordinary output — a BEL after
/// an abandoned string reads as a bell.
const OSC_STRING_MAX: usize = 2 * OSC_PAYLOAD_MAX;

/// Which of the two strings a payload is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Osc {
    Title,
    Pwd,
}

/// Where the capture is in an OSC string.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Nothing in flight. The scan may skip ahead from here.
    Ground,
    /// An ESC arrived. Only `]` opens a string.
    Introduced,
    /// Reading the numeric identifier before the first `;`.
    Ident,
    /// Reading a payload that is going to be kept.
    Payload(Osc),
    /// Reading a payload only to find where it ends.
    Discard,
}

/// Extracts the window title and the reported directory from a byte stream.
///
/// Five states and one bounded buffer, driven one byte at a time only while a
/// string is actually open. It does not model a screen, a cursor or a cell,
/// because nothing in the daemon reads one: the client renders from the raw
/// bytes with its own emulator.
///
/// Terminator handling is the part worth being exact about. `OSC 2 ; text BEL`
/// and `OSC 2 ; text ESC \` are the same sequence written two ways, both are
/// in the wild, and a capture that understood only one would silently miss
/// every title from the programs that use the other.
struct OscCapture {
    phase: Phase,
    ident: u32,
    digits: u8,
    payload: Vec<u8>,
    /// Whether the payload outgrew [`OSC_PAYLOAD_MAX`] and must be refused.
    overflowed: bool,
    /// Bytes consumed since `ESC ]`, whether kept or not.
    consumed: usize,
    title: Option<String>,
    pwd: Option<String>,
}

impl OscCapture {
    fn new() -> Self {
        Self {
            phase: Phase::Ground,
            ident: 0,
            digits: 0,
            payload: Vec::new(),
            overflowed: false,
            consumed: 0,
            title: None,
            pwd: None,
        }
    }

    /// Whether a string is open, and the scan therefore may not skip ahead.
    fn in_flight(&self) -> bool {
        self.phase != Phase::Ground
    }

    fn step(&mut self, byte: u8) {
        // Counted before the transition, so `consumed` is the number of bytes
        // seen since the introducer that opened the string currently in
        // flight, covering its identifier, its payload and a discarded string
        // alike.
        if self.phase == Phase::Ground {
            self.consumed = 0;
        } else {
            self.consumed += 1;
            if self.consumed > OSC_STRING_MAX {
                // A string this long has lost its terminator. Leaving it open
                // would hold the whole session on the byte-at-a-time path.
                self.payload.clear();
                self.overflowed = false;
                self.phase = Phase::Ground;
                return;
            }
        }
        self.phase = match self.phase {
            Phase::Ground if byte == ESC => Phase::Introduced,
            Phase::Ground => Phase::Ground,
            Phase::Introduced => match byte {
                b']' => {
                    self.ident = 0;
                    self.digits = 0;
                    // A string that ends by introducing the next one never
                    // passes through Ground, so the counter is rearmed here
                    // as well: the bound is per string, not per run of them.
                    self.consumed = 0;
                    Phase::Ident
                }
                // `ESC ESC` is a cancel followed by a fresh introducer, not a
                // sequence: a stream that lost a byte must not be able to leave
                // this parser permanently one state behind.
                ESC => Phase::Introduced,
                _ => Phase::Ground,
            },
            Phase::Ident => match byte {
                b'0'..=b'9' if self.digits < 5 => {
                    self.ident = self.ident * 10 + u32::from(byte - b'0');
                    self.digits += 1;
                    Phase::Ident
                }
                b';' => self.open(),
                ESC => Phase::Introduced,
                BEL => Phase::Ground,
                // An identifier that is not a number, or one long enough to be
                // an attack on this counter, is not one of ours.
                _ => Phase::Discard,
            },
            Phase::Payload(kind) => match byte {
                BEL => self.complete(kind),
                // An ESC ends the string, whether or not the `\` that would
                // make it a proper ST follows. That is what a real terminal
                // does, and it is the behaviour a program relies on when it
                // writes a title and then immediately writes a CSI: refusing
                // the payload here would drop titles that arrive today.
                ESC => {
                    self.complete(kind);
                    Phase::Introduced
                }
                // CAN and SUB end the string where they appear, and what was
                // read up to that point still counts. They are the two bytes a
                // terminal treats as "abandon whatever you were parsing", and
                // a program uses them to get out of a sequence it started.
                0x18 | 0x1a => self.complete(kind),
                // A C0 control inside a payload is dropped and the string
                // carries on, which is what the engine this replaced did. It
                // is also the only reading that keeps a title whole: a program
                // that pads its status line with a stray carriage return still
                // meant the text around it.
                0x00..=0x1f => Phase::Payload(kind),
                _ => {
                    if self.payload.len() < OSC_PAYLOAD_MAX {
                        self.payload.push(byte);
                    } else {
                        self.overflowed = true;
                    }
                    Phase::Payload(kind)
                }
            },
            Phase::Discard => match byte {
                BEL => Phase::Ground,
                ESC => Phase::Introduced,
                _ => Phase::Discard,
            },
        };
    }

    /// Start a payload once the identifier is known.
    fn open(&mut self) -> Phase {
        self.payload.clear();
        self.overflowed = false;
        // `OSC ; text` with no identifier at all is malformed, and defaulting
        // it to 0 would let a stray semicolon retitle a session.
        if self.digits == 0 {
            return Phase::Discard;
        }
        match self.ident {
            // 0 sets the icon name and the title together; 2 sets the title.
            // 1 is the icon name alone and is deliberately not a title.
            0 | 2 => Phase::Payload(Osc::Title),
            7 => Phase::Payload(Osc::Pwd),
            _ => Phase::Discard,
        }
    }

    /// Record a finished payload, if it is one that can be used.
    ///
    /// Invalid UTF-8 is dropped rather than repaired. The title is rendered in
    /// a sidebar and the directory is opened as a path, and a lossy conversion
    /// would put replacement characters in one and name the wrong directory in
    /// the other.
    fn complete(&mut self, kind: Osc) -> Phase {
        if !self.overflowed {
            if let Ok(text) = std::str::from_utf8(&self.payload) {
                match kind {
                    Osc::Title => self.title = Some(text.to_owned()),
                    Osc::Pwd => self.pwd = Some(text.to_owned()),
                }
            }
        }
        self.payload.clear();
        self.overflowed = false;
        Phase::Ground
    }
}
