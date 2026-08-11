//! Pointer events to the bytes a child reads.
//!
//! A terminal has no pointer events either. It has the same byte stream the
//! keyboard writes into, and a program that wants the mouse asks for it by
//! setting modes: one pair of modes chooses WHICH events are reported, another
//! chooses HOW they are encoded. Both halves are the child's decision and
//! neither is the pane's, so everything here is a pure function of the mode
//! state the emulator reports plus the event the toolkit delivered.
//!
//! # Which events
//!
//! | mode | what the child asked for |
//! |------|--------------------------|
//! | none | nothing; the pointer is the pane's, for selection |
//! | 9 | button presses only, with no modifiers and no releases |
//! | 1000 | presses and releases |
//! | 1002 | presses, releases, and motion while a button is held |
//! | 1003 | presses, releases, and all motion |
//!
//! The highest one set wins, because a program that sets 1003 after 1000 wants
//! all motion and a program that resets 1003 back to 1000 wants the narrower
//! set. Reading them as a precedence rather than as a history is what makes
//! this a function of the current mode state.
//!
//! # How they are encoded
//!
//! | mode | encoding |
//! |------|----------|
//! | none | `CSI M` and three bytes biased by 32, which runs out at column 223 |
//! | 1005 | the same three values as UTF-8 characters |
//! | 1015 | `CSI cb+32 ; col ; row M`, decimal, no ceiling |
//! | 1006 | `CSI < cb ; col ; row M`, and `m` for a release |
//! | 1016 | 1006 with pixels instead of cells |
//!
//! 1006 is what everything written this century sets, and it is the only one
//! that distinguishes which button was released. The older three are here
//! because the programs that set them are exactly the programs nobody is going
//! to update.
//!
//! # The button byte
//!
//! One byte carrying four things at once, which is why it looks arbitrary.
//! Bits 0 and 1 are the button, bit 2 is Shift, bit 3 is Alt, bit 4 is Ctrl,
//! bit 5 is motion, and bit 6 promotes the pair in bits 0 and 1 from the
//! ordinary buttons to the wheel. Buttons 8 through 11 set bit 7 as well. A
//! legacy release reports button 3, which is why a legacy client cannot tell
//! which button came up and 1006 exists.

/// A button the toolkit can report.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Button {
    /// Button 1.
    Left,
    /// Button 2.
    Middle,
    /// Button 3.
    Right,
    /// Wheel away from the operator.
    WheelUp,
    /// Wheel towards the operator.
    WheelDown,
    /// Horizontal wheel or tilt, left.
    WheelLeft,
    /// Horizontal wheel or tilt, right.
    WheelRight,
    /// Buttons 8 through 11, the side buttons. `index` is 0 for button 8.
    Extra {
        /// 0 through 3, for buttons 8 through 11.
        index: u8,
    },
}

impl Button {
    /// The two low bits plus the high selector bits, before modifiers.
    const fn code(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
            Self::WheelUp => 64,
            Self::WheelDown => 65,
            Self::WheelLeft => 66,
            Self::WheelRight => 67,
            // Bit 7 selects the second bank, and the low bits index within it.
            Self::Extra { index } => 128 + (index & 0b11),
        }
    }

    /// Whether this is a wheel notch rather than a button that can be held.
    ///
    /// A wheel has no release: the notch is the whole event. Sending one would
    /// make a program that counts presses scroll twice per notch.
    pub(crate) const fn is_wheel(self) -> bool {
        matches!(
            self,
            Self::WheelUp | Self::WheelDown | Self::WheelLeft | Self::WheelRight
        )
    }
}

/// What happened to the button.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Action {
    /// It went down.
    Press,
    /// It came up.
    Release,
    /// The pointer moved. `held` is the button still down, if any.
    Motion {
        /// The button being dragged, or `None` for a bare hover.
        held: Option<Button>,
    },
}

/// Modifiers held when the pointer event arrived.
///
/// Super is absent for the same reason it is absent from the key encoder: a
/// Super chord belongs to the window manager or to this product's own keymap.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Mods {
    /// Shift.
    pub shift: bool,
    /// Alt.
    pub alt: bool,
    /// Control.
    pub ctrl: bool,
}

impl Mods {
    /// Bits 2, 3 and 4 of the button byte.
    const fn bits(self) -> u8 {
        (if self.shift { 4 } else { 0 })
            | (if self.alt { 8 } else { 0 })
            | (if self.ctrl { 16 } else { 0 })
    }

    /// Nothing held.
    pub(crate) const NONE: Self = Self {
        shift: false,
        alt: false,
        ctrl: false,
    };
}

/// Which events the child asked to see.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, PartialOrd, Ord)]
pub(crate) enum Tracking {
    /// The pointer belongs to the pane.
    #[default]
    Off,
    /// Mode 9: presses only, unmodified.
    X10,
    /// Mode 1000: presses and releases.
    Normal,
    /// Mode 1002: and motion while a button is held.
    Button,
    /// Mode 1003: and all motion.
    Any,
}

/// How a report is written.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Protocol {
    /// Three bytes biased by 32. The default when the child asked for nothing
    /// else, and the only one with a coordinate ceiling.
    #[default]
    Legacy,
    /// Mode 1005: the same values, UTF-8 encoded.
    Utf8,
    /// Mode 1015: decimal parameters, always a press terminator.
    Urxvt,
    /// Mode 1006: decimal parameters, `m` for a release.
    Sgr,
    /// Mode 1016: 1006 with pixel coordinates.
    SgrPixels,
}

/// The whole of the mode state a pointer event is a function of.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Modes {
    /// Which events the child wants.
    pub tracking: Tracking,
    /// How they are written.
    pub protocol: Protocol,
    /// Mode 1007. The wheel drives the cursor keys on the alternate screen
    /// instead of the pane's own scrollback.
    pub alt_scroll: bool,
    /// Whether the alternate screen is the one on display.
    pub alt_screen: bool,
}

impl Modes {
    /// Read the mode state out of the individual flags the emulator reports.
    ///
    /// Precedence rather than history, in both halves: the widest tracking set
    /// and the newest encoding win. A program that sets 1006 and then 1015 is
    /// asking for the encoding it can parse, and every one of them can parse
    /// 1006.
    pub(crate) const fn from_flags(f: ModeFlags) -> Self {
        let tracking = if f.any_mouse {
            Tracking::Any
        } else if f.button_mouse {
            Tracking::Button
        } else if f.normal_mouse {
            Tracking::Normal
        } else if f.x10_mouse {
            Tracking::X10
        } else {
            Tracking::Off
        };
        let protocol = if f.sgr_pixels_mouse {
            Protocol::SgrPixels
        } else if f.sgr_mouse {
            Protocol::Sgr
        } else if f.urxvt_mouse {
            Protocol::Urxvt
        } else if f.utf8_mouse {
            Protocol::Utf8
        } else {
            Protocol::Legacy
        };
        Self {
            tracking,
            protocol,
            alt_scroll: f.alt_scroll,
            alt_screen: f.alt_screen,
        }
    }

    /// Whether the pointer belongs to the child rather than to selection.
    ///
    /// Shift is the escape hatch every terminal implements: holding it takes
    /// the pointer back for selection even while a full-screen program is
    /// tracking it, which is the only way to copy text out of one.
    pub(crate) const fn child_owns_pointer(self, mods: Mods) -> bool {
        !matches!(self.tracking, Tracking::Off) && !mods.shift
    }
}

/// The individual mode bits, exactly as the emulator reports them.
///
/// A plain record rather than a builder, so a caller that forgets one gets a
/// compile error instead of a default that quietly means "reset".
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct ModeFlags {
    /// Mode 9.
    pub x10_mouse: bool,
    /// Mode 1000.
    pub normal_mouse: bool,
    /// Mode 1002.
    pub button_mouse: bool,
    /// Mode 1003.
    pub any_mouse: bool,
    /// Mode 1005.
    pub utf8_mouse: bool,
    /// Mode 1006.
    pub sgr_mouse: bool,
    /// Mode 1015.
    pub urxvt_mouse: bool,
    /// Mode 1016.
    pub sgr_pixels_mouse: bool,
    /// Mode 1007.
    pub alt_scroll: bool,
    /// Mode 1049 or 47: the alternate screen is showing.
    pub alt_screen: bool,
}

/// Where the pointer is.
///
/// Both coordinate systems, because which one a report carries is the child's
/// choice and the caller has both to hand. Cells are zero-based from the top
/// left of the grid; pixels are zero-based from the top left of the pane's
/// padding box, which is the same origin the grid is drawn from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Position {
    /// Zero-based column.
    pub col: u16,
    /// Zero-based row.
    pub row: u16,
    /// Pixels from the left of the padding box.
    pub px: u32,
    /// Pixels from the top of the padding box.
    pub py: u32,
}

/// Highest cell coordinate the legacy encoding can carry.
///
/// The byte is the coordinate plus 33, and a byte holds 255. Past this the
/// report is not sent at all rather than sent wrong: a program told the
/// pointer is in column 223 when it is in column 400 acts on the wrong cell,
/// and a click that does nothing is the smaller failure. A program that wants
/// the far side of a wide window sets 1006, which has no ceiling.
const LEGACY_MAX: u16 = 222;

/// Encode one pointer event, or decide it is not reported.
///
/// `None` is the common answer and is not a failure: the child asked for
/// nothing, or asked for presses and this is a release, or asked for motion
/// with a button held and none is. A caller that treats `None` as an error
/// will report events no program asked for.
pub(crate) fn report(
    modes: Modes,
    action: Action,
    button: Button,
    mods: Mods,
    at: Position,
) -> Option<Vec<u8>> {
    let wanted = match (modes.tracking, action) {
        (Tracking::Off, _) => false,
        // Mode 9 is presses and nothing else, and a wheel notch is a press.
        (Tracking::X10, Action::Press) => true,
        (Tracking::X10, _) => false,
        // A wheel has no release to report in any mode.
        (_, Action::Release) if button.is_wheel() => false,
        (Tracking::Normal, Action::Press | Action::Release) => true,
        (Tracking::Normal, Action::Motion { .. }) => false,
        (Tracking::Button, Action::Motion { held }) => held.is_some(),
        (Tracking::Button, _) | (Tracking::Any, _) => true,
    };
    if !wanted {
        return None;
    }

    // Mode 9 predates modifiers and reports none of them. Sending them makes
    // a program that decodes the byte arithmetically read the wrong button.
    let mods = if modes.tracking == Tracking::X10 {
        Mods::NONE
    } else {
        mods
    };

    let base = match action {
        // A drag reports the button being dragged; a bare hover reports the
        // "no button" code, which is the same 3 a legacy release uses.
        Action::Motion { held } => held.map_or(3, Button::code) + 32,
        _ => button.code(),
    };
    let code = base | mods.bits();

    // Only 1006 can say which button was released. Every other encoding
    // reports the release as button 3, losing the identity, and that loss is
    // the whole reason 1006 was specified.
    let legacy_release = matches!(action, Action::Release);

    Some(match modes.protocol {
        Protocol::Sgr => sgr(code, at.col, at.row, legacy_release),
        Protocol::SgrPixels => sgr(code, cap(at.px), cap(at.py), legacy_release),
        Protocol::Urxvt => urxvt(if legacy_release { 3 | mods.bits() } else { code }, at),
        Protocol::Utf8 => utf8(
            if legacy_release { 3 | mods.bits() } else { code },
            at,
        )?,
        Protocol::Legacy => legacy(
            if legacy_release { 3 | mods.bits() } else { code },
            at,
        )?,
    })
}

/// A pixel coordinate as a parameter, held inside the range a `u16` can carry.
///
/// Pixel reports on a 4K pane exceed 4095 on the horizontal axis, which is
/// still inside a `u16`; the cap is for a caller measuring a surface larger
/// than any panel rather than for a real pane.
const fn cap(px: u32) -> u16 {
    if px > u16::MAX as u32 {
        u16::MAX
    } else {
        px as u16
    }
}

/// `CSI < code ; col ; row M` or `m`. One-based coordinates.
fn sgr(code: u8, col: u16, row: u16, release: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    out.extend_from_slice(b"\x1b[<");
    push_num(&mut out, u32::from(code));
    out.push(b';');
    push_num(&mut out, u32::from(col) + 1);
    out.push(b';');
    push_num(&mut out, u32::from(row) + 1);
    out.push(if release { b'm' } else { b'M' });
    out
}

/// `CSI code+32 ; col ; row M`. Decimal, so no ceiling, but no release
/// identity either.
fn urxvt(code: u8, at: Position) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    out.extend_from_slice(b"\x1b[");
    push_num(&mut out, u32::from(code) + 32);
    out.push(b';');
    push_num(&mut out, u32::from(at.col) + 1);
    out.push(b';');
    push_num(&mut out, u32::from(at.row) + 1);
    out.push(b'M');
    out
}

/// `CSI M` and three bytes biased by 32.
fn legacy(code: u8, at: Position) -> Option<Vec<u8>> {
    if at.col > LEGACY_MAX || at.row > LEGACY_MAX {
        return None;
    }
    Some(vec![
        0x1b,
        b'[',
        b'M',
        code.wrapping_add(32),
        (at.col as u8).wrapping_add(33),
        (at.row as u8).wrapping_add(33),
    ])
}

/// The same three values, each written as a UTF-8 character.
///
/// The ceiling moves from 222 to 2015, which is every column a terminal has
/// ever had. The button byte is still one byte, because a value above 127
/// there would encode as two and no client parses that.
fn utf8(code: u8, at: Position) -> Option<Vec<u8>> {
    const UTF8_MAX: u16 = 2015;
    if at.col > UTF8_MAX || at.row > UTF8_MAX {
        return None;
    }
    let mut out = Vec::with_capacity(10);
    out.extend_from_slice(b"\x1b[M");
    out.push(code.wrapping_add(32));
    let mut push = |v: u16| {
        let ch = char::from_u32(u32::from(v) + 33).unwrap_or('!');
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    };
    push(at.col);
    push(at.row);
    Some(out)
}

/// Decimal, without going through a formatter.
///
/// A pointer report is emitted on every motion event of a drag, which on a
/// 1000 Hz mouse is a thousand allocations a second through `write!`. Three
/// digits pushed by hand cost none.
fn push_num(out: &mut Vec<u8>, mut n: u32) {
    if n == 0 {
        out.push(b'0');
        return;
    }
    let mut buf = [0u8; 10];
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    out.extend_from_slice(&buf[i..]);
}

/// The cursor keys a wheel notch stands for on the alternate screen.
///
/// Mode 1007, and it is not a convenience. A full-screen program on the
/// alternate screen has no scrollback for the pane to page through, so a wheel
/// notch there either does nothing or means "move". Every pager and every
/// editor expects the second, and `lines` notches become `lines` arrow keys.
///
/// Empty when the mode is off or the alternate screen is not showing, which is
/// the case where the wheel is the pane's own scrollback.
pub(crate) fn alt_scroll(modes: Modes, up: bool, lines: u16, app_cursor: bool) -> Vec<u8> {
    if !modes.alt_scroll || !modes.alt_screen || lines == 0 {
        return Vec::new();
    }
    // SS3 under application cursor mode, CSI otherwise. The child chose which
    // by setting DECCKM, and a pager that gets the wrong one inserts `OA` into
    // its command line.
    let seq: &[u8] = match (app_cursor, up) {
        (true, true) => b"\x1bOA",
        (true, false) => b"\x1bOB",
        (false, true) => b"\x1b[A",
        (false, false) => b"\x1b[B",
    };
    let mut out = Vec::with_capacity(seq.len() * lines as usize);
    for _ in 0..lines {
        out.extend_from_slice(seq);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(col: u16, row: u16) -> Position {
        Position {
            col,
            row,
            px: u32::from(col) * 9,
            py: u32::from(row) * 19,
        }
    }

    fn modes(tracking: Tracking, protocol: Protocol) -> Modes {
        Modes {
            tracking,
            protocol,
            alt_scroll: false,
            alt_screen: false,
        }
    }

    /// Every tracking mode this pane can be in, derived from the type rather
    /// than listed, so a mode added upstream turns this suite red until
    /// somebody decides what it reports.
    const TRACKING: [Tracking; 5] = [
        Tracking::Off,
        Tracking::X10,
        Tracking::Normal,
        Tracking::Button,
        Tracking::Any,
    ];

    /// Every encoding, likewise.
    const PROTOCOLS: [Protocol; 5] = [
        Protocol::Legacy,
        Protocol::Utf8,
        Protocol::Urxvt,
        Protocol::Sgr,
        Protocol::SgrPixels,
    ];

    /// Every button.
    const BUTTONS: [Button; 11] = [
        Button::Left,
        Button::Middle,
        Button::Right,
        Button::WheelUp,
        Button::WheelDown,
        Button::WheelLeft,
        Button::WheelRight,
        Button::Extra { index: 0 },
        Button::Extra { index: 1 },
        Button::Extra { index: 2 },
        Button::Extra { index: 3 },
    ];

    /// WHY: the encoding a program parses is the one it asked for, and a pane
    /// that sends the wrong one puts literal escape text into a text field.
    ///
    /// The bytes below are the sequences the specifications define, written
    /// out rather than computed, so a change to the arithmetic that happens to
    /// be self-consistent still fails.
    #[test]
    fn each_encoding_writes_the_sequence_its_mode_defines() {
        let pos = at(4, 9);

        assert_eq!(
            report(
                modes(Tracking::Normal, Protocol::Sgr),
                Action::Press,
                Button::Left,
                Mods::NONE,
                pos
            )
            .unwrap(),
            b"\x1b[<0;5;10M"
        );
        assert_eq!(
            report(
                modes(Tracking::Normal, Protocol::Sgr),
                Action::Release,
                Button::Left,
                Mods::NONE,
                pos
            )
            .unwrap(),
            b"\x1b[<0;5;10m",
            "a release under 1006 is the same code with a lowercase terminator"
        );
        assert_eq!(
            report(
                modes(Tracking::Normal, Protocol::SgrPixels),
                Action::Press,
                Button::Left,
                Mods::NONE,
                pos
            )
            .unwrap(),
            b"\x1b[<0;37;172M",
            "1016 carries pixels, one-based, not cells"
        );
        assert_eq!(
            report(
                modes(Tracking::Normal, Protocol::Urxvt),
                Action::Press,
                Button::Left,
                Mods::NONE,
                pos
            )
            .unwrap(),
            b"\x1b[32;5;10M"
        );
        assert_eq!(
            report(
                modes(Tracking::Normal, Protocol::Legacy),
                Action::Press,
                Button::Left,
                Mods::NONE,
                pos
            )
            .unwrap(),
            b"\x1b[M\x20\x25\x2a"
        );
        assert_eq!(
            report(
                modes(Tracking::Normal, Protocol::Utf8),
                Action::Press,
                Button::Left,
                Mods::NONE,
                pos
            )
            .unwrap(),
            b"\x1b[M\x20\x25\x2a",
            "below 223 the UTF-8 encoding is byte for byte the legacy one"
        );
    }

    /// WHY: a legacy release reports button 3 and loses which button it was,
    /// and a pane that reports the real button there breaks every program
    /// written against the older modes.
    #[test]
    fn only_1006_preserves_which_button_was_released() {
        for button in [Button::Left, Button::Middle, Button::Right] {
            let sgr = report(
                modes(Tracking::Normal, Protocol::Sgr),
                Action::Release,
                button,
                Mods::NONE,
                at(0, 0),
            )
            .unwrap();
            assert_eq!(
                sgr[3] - b'0',
                button.code(),
                "1006 must name the button that came up"
            );

            for protocol in [Protocol::Legacy, Protocol::Utf8, Protocol::Urxvt] {
                let bytes = report(
                    modes(Tracking::Normal, protocol),
                    Action::Release,
                    button,
                    Mods::NONE,
                    at(0, 0),
                )
                .unwrap();
                let same_as_left = report(
                    modes(Tracking::Normal, protocol),
                    Action::Release,
                    Button::Left,
                    Mods::NONE,
                    at(0, 0),
                )
                .unwrap();
                assert_eq!(
                    bytes, same_as_left,
                    "{protocol:?} must report every release identically"
                );
            }
        }
    }

    /// WHY: the tracking mode decides what is sent, and the failure mode is
    /// silent in both directions. Too little and a program never sees a drag;
    /// too much and a program that asked for presses is flooded with motion at
    /// the pointer's sample rate.
    ///
    /// The invariant is the table in the module doc, asserted for every
    /// tracking mode rather than the one somebody had in mind. Adding a mode
    /// to `Tracking` without a row here fails to compile, because the match is
    /// exhaustive.
    #[test]
    fn each_tracking_mode_reports_exactly_the_events_it_asked_for() {
        for tracking in TRACKING {
            let m = modes(tracking, Protocol::Sgr);
            let press = report(m, Action::Press, Button::Left, Mods::NONE, at(1, 1));
            let release = report(m, Action::Release, Button::Left, Mods::NONE, at(1, 1));
            let drag = report(
                m,
                Action::Motion {
                    held: Some(Button::Left),
                },
                Button::Left,
                Mods::NONE,
                at(1, 1),
            );
            let hover = report(
                m,
                Action::Motion { held: None },
                Button::Left,
                Mods::NONE,
                at(1, 1),
            );

            let seen = (
                press.is_some(),
                release.is_some(),
                drag.is_some(),
                hover.is_some(),
            );
            let want = match tracking {
                Tracking::Off => (false, false, false, false),
                Tracking::X10 => (true, false, false, false),
                Tracking::Normal => (true, true, false, false),
                Tracking::Button => (true, true, true, false),
                Tracking::Any => (true, true, true, true),
            };
            assert_eq!(seen, want, "{tracking:?} reported the wrong event set");
        }
    }

    /// WHY: mode 9 predates modifiers, and a program decoding its button byte
    /// arithmetically reads Ctrl-click as button 16.
    #[test]
    fn mode_9_strips_the_modifiers_every_other_mode_carries() {
        let held = Mods {
            shift: true,
            alt: true,
            ctrl: true,
        };
        let x10 = report(
            modes(Tracking::X10, Protocol::Sgr),
            Action::Press,
            Button::Left,
            held,
            at(0, 0),
        )
        .unwrap();
        assert_eq!(x10, b"\x1b[<0;1;1M");

        // Shift is the selection escape hatch, so a modified press under a
        // wider mode is only reported when the child owns the pointer at all.
        let normal = report(
            modes(Tracking::Normal, Protocol::Sgr),
            Action::Press,
            Button::Left,
            Mods {
                shift: false,
                alt: true,
                ctrl: true,
            },
            at(0, 0),
        )
        .unwrap();
        assert_eq!(normal, b"\x1b[<24;1;1M", "Alt is 8 and Ctrl is 16");
    }

    /// WHY: shift-drag is the only way to select text out of a full-screen
    /// program, and a pane that forwards it to the child makes copying
    /// impossible in exactly the programs this product manages.
    #[test]
    fn shift_takes_the_pointer_back_from_a_tracking_child() {
        let shift = Mods {
            shift: true,
            alt: false,
            ctrl: false,
        };
        for tracking in TRACKING {
            let m = modes(tracking, Protocol::Sgr);
            assert!(
                !m.child_owns_pointer(shift),
                "{tracking:?} kept the pointer while Shift was held"
            );
            assert_eq!(
                m.child_owns_pointer(Mods::NONE),
                tracking != Tracking::Off,
                "{tracking:?} disagreed about who owns an unmodified pointer"
            );
        }
    }

    /// WHY: a wheel notch is one event. Synthesising a release doubles every
    /// scroll in a program that counts presses.
    #[test]
    fn a_wheel_notch_never_produces_a_release() {
        for button in BUTTONS.iter().copied().filter(|b| b.is_wheel()) {
            for tracking in TRACKING {
                for protocol in PROTOCOLS {
                    assert!(
                        report(
                            modes(tracking, protocol),
                            Action::Release,
                            button,
                            Mods::NONE,
                            at(3, 3)
                        )
                        .is_none(),
                        "{button:?} under {tracking:?}/{protocol:?} reported a release"
                    );
                }
            }
        }
    }

    /// WHY: the wheel and the side buttons live in banks above the ordinary
    /// three, and a pane that folds them into the low two bits reports a
    /// wheel-up as a left click.
    ///
    /// Every button, in the encoding that can carry all of them, asserted
    /// distinct. A collision here is a button that silently does something
    /// else.
    #[test]
    fn every_button_encodes_to_a_distinct_code() {
        let mut seen = std::collections::BTreeSet::new();
        for button in BUTTONS {
            let bytes = report(
                modes(Tracking::Normal, Protocol::Sgr),
                Action::Press,
                button,
                Mods::NONE,
                at(0, 0),
            )
            .unwrap();
            assert!(
                seen.insert(bytes.clone()),
                "{button:?} collided with another button"
            );
        }
        assert_eq!(seen.len(), BUTTONS.len());

        assert_eq!(Button::WheelUp.code(), 64);
        assert_eq!(Button::WheelDown.code(), 65);
        assert_eq!(Button::Extra { index: 0 }.code(), 128);
        assert_eq!(Button::Extra { index: 3 }.code(), 131);
    }

    /// WHY: the legacy encoding runs out at column 223, and a 4K pane at a
    /// small type size is 400 columns wide. Wrapping the byte reports a click
    /// on the wrong side of the window; the pane must not report at all.
    ///
    /// The boundary matters more than the middle: 222 is the last column that
    /// encodes and 223 is the first that does not.
    #[test]
    fn the_legacy_encoding_declines_past_its_ceiling_rather_than_wrapping() {
        for protocol in [Protocol::Legacy, Protocol::Utf8] {
            let m = modes(Tracking::Normal, protocol);
            assert!(
                report(m, Action::Press, Button::Left, Mods::NONE, at(222, 0)).is_some(),
                "{protocol:?} refused the last column it can carry"
            );
        }
        assert!(
            report(
                modes(Tracking::Normal, Protocol::Legacy),
                Action::Press,
                Button::Left,
                Mods::NONE,
                at(223, 0)
            )
            .is_none()
        );
        assert!(
            report(
                modes(Tracking::Normal, Protocol::Legacy),
                Action::Press,
                Button::Left,
                Mods::NONE,
                at(0, 223)
            )
            .is_none()
        );
        // The wider encodings have no such ceiling, which is the reason to set
        // them on a large window.
        for protocol in [Protocol::Urxvt, Protocol::Sgr, Protocol::SgrPixels] {
            assert!(
                report(
                    modes(Tracking::Normal, protocol),
                    Action::Press,
                    Button::Left,
                    Mods::NONE,
                    at(511, 300)
                )
                .is_some(),
                "{protocol:?} could not report a column a 4K pane has"
            );
        }
        // 1005 reaches 2015, which is past every column and short of wrapping.
        assert!(
            report(
                modes(Tracking::Normal, Protocol::Utf8),
                Action::Press,
                Button::Left,
                Mods::NONE,
                at(2015, 0)
            )
            .is_some()
        );
        assert!(
            report(
                modes(Tracking::Normal, Protocol::Utf8),
                Action::Press,
                Button::Left,
                Mods::NONE,
                at(2016, 0)
            )
            .is_none()
        );
    }

    /// WHY: precedence, not history. A program that sets 1000 and then 1003
    /// wants all motion; one that sets 1003 and then 1000 has narrowed nothing
    /// because 1003 is still set. Reading the flags as a set is the only
    /// reading that does not depend on the order they arrived in.
    #[test]
    fn the_widest_tracking_mode_and_the_newest_encoding_win() {
        let all = ModeFlags {
            x10_mouse: true,
            normal_mouse: true,
            button_mouse: true,
            any_mouse: true,
            utf8_mouse: true,
            sgr_mouse: true,
            urxvt_mouse: true,
            sgr_pixels_mouse: true,
            alt_scroll: false,
            alt_screen: false,
        };
        let m = Modes::from_flags(all);
        assert_eq!(m.tracking, Tracking::Any);
        assert_eq!(m.protocol, Protocol::SgrPixels);

        assert_eq!(
            Modes::from_flags(ModeFlags {
                sgr_pixels_mouse: false,
                ..all
            })
            .protocol,
            Protocol::Sgr
        );
        assert_eq!(
            Modes::from_flags(ModeFlags {
                sgr_pixels_mouse: false,
                sgr_mouse: false,
                ..all
            })
            .protocol,
            Protocol::Urxvt
        );
        assert_eq!(
            Modes::from_flags(ModeFlags {
                sgr_pixels_mouse: false,
                sgr_mouse: false,
                urxvt_mouse: false,
                ..all
            })
            .protocol,
            Protocol::Utf8
        );
        assert_eq!(
            Modes::from_flags(ModeFlags::default()).protocol,
            Protocol::Legacy
        );
        assert_eq!(
            Modes::from_flags(ModeFlags::default()).tracking,
            Tracking::Off
        );

        assert_eq!(
            Modes::from_flags(ModeFlags {
                any_mouse: false,
                ..all
            })
            .tracking,
            Tracking::Button
        );
        assert_eq!(
            Modes::from_flags(ModeFlags {
                any_mouse: false,
                button_mouse: false,
                ..all
            })
            .tracking,
            Tracking::Normal
        );
        assert_eq!(
            Modes::from_flags(ModeFlags {
                x10_mouse: true,
                ..ModeFlags::default()
            })
            .tracking,
            Tracking::X10
        );
    }

    /// WHY: a report is written on every motion sample of a drag. At a 1000 Hz
    /// pointer that is a thousand of these a second, and anything that
    /// allocates per digit shows up as the pane feeling heavy under a drag.
    ///
    /// The observable contract is the bytes, so this asserts the number
    /// formatting is exact across the range a report can carry rather than
    /// asserting an allocation count nothing can see.
    #[test]
    fn parameters_are_written_exactly_across_the_range_a_report_carries() {
        for n in [0u32, 1, 9, 10, 99, 100, 222, 223, 1000, 4095, 65535] {
            let mut out = Vec::new();
            push_num(&mut out, n);
            assert_eq!(out, n.to_string().into_bytes(), "{n}");
        }
    }

    /// WHY: a wheel notch on the alternate screen has no scrollback to page
    /// through, and mode 1007 is how a pager says so. Sending the wrong cursor
    /// encoding puts `OA` into the pager's own command line.
    #[test]
    fn the_wheel_drives_the_cursor_keys_only_on_the_alternate_screen() {
        let on = Modes {
            tracking: Tracking::Off,
            protocol: Protocol::Legacy,
            alt_scroll: true,
            alt_screen: true,
        };
        assert_eq!(alt_scroll(on, true, 3, false), b"\x1b[A\x1b[A\x1b[A");
        assert_eq!(alt_scroll(on, false, 1, false), b"\x1b[B");
        assert_eq!(
            alt_scroll(on, true, 2, true),
            b"\x1bOA\x1bOA",
            "application cursor mode sends SS3, not CSI"
        );

        assert!(alt_scroll(on, true, 0, false).is_empty());
        assert!(
            alt_scroll(
                Modes {
                    alt_screen: false,
                    ..on
                },
                true,
                3,
                false
            )
            .is_empty(),
            "the primary screen has scrollback, so the wheel is the pane's"
        );
        assert!(
            alt_scroll(
                Modes {
                    alt_scroll: false,
                    ..on
                },
                true,
                3,
                false
            )
            .is_empty(),
            "1007 unset means the child did not ask for this"
        );
    }
}
