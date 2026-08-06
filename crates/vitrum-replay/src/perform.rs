//! The VT command set, mapped onto a [`Screen`].
//!
//! This is the whole of vitrum's terminal emulation, and it is deliberately thin.
//! [`vte`] turns bytes into events, [`vitrum_grid::CellGrid`] holds cells, and
//! [`crate::screen`] holds the rules. What is left is the translation table below.
//!
//! # What is implemented
//!
//! | Group | Sequences |
//! |---|---|
//! | Printing | text, autowrap with deferred wrap, double-width characters, insert mode |
//! | C0 | BS, HT, LF, VT, FF, CR, SO, SI |
//! | C1 (8-bit) | IND, NEL, HTS, RI |
//! | Cursor | CUU, CUD, CUF, CUB, CNL, CPL, CHA, HPA, CUP, HVP, VPA, VPR, HPR |
//! | Tabs | HTS, TBC, CHT, CBT |
//! | Erase | ED (0,1,2,3), EL (0,1,2), ECH |
//! | Edit | ICH, DCH, IL, DL, SU, SD |
//! | Rendition | SGR 0-9 subset, 22-27, 30-37, 38, 39, 40-47, 48, 49, 90-97, 100-107, both `;` and `:` extended colour forms |
//! | Modes | DECAWM, DECOM, DECTCEM, IRM, alternate screen (47, 1047, 1049) |
//! | Save/restore | DECSC, DECRC, `CSI s`, `CSI u` |
//! | Scrolling | DECSTBM |
//! | Charsets | `ESC ( x`, `ESC ) x`, DEC Special Graphics |
//! | Other | RIS, DECALN, OSC 0 and OSC 2 titles |
//!
//! # What is ignored, and why that is correct
//!
//! **Rendition with no cell to store it in.** [`vitrum_grid::Attrs`] models bold,
//! italic, underline and reverse, because those are what the renderer draws. Dim
//! (SGR 2), blink (5), conceal (8) and strikethrough (9) have no bit, so they are
//! dropped rather than mapped onto a bit that means something else. A replay that
//! turned blink into reverse would be lying about the session.
//!
//! **Anything that expects a reply.** DSR (`CSI n`), DECRQSS, and the primary and
//! secondary device attributes all ask the terminal to write back down the PTY.
//! Replay has no PTY and no input channel: it is reading bytes the session
//! already produced. Answering would mean inventing traffic that never happened.
//!
//! **DCS payloads.** Sixel images, DECRQSS replies and tmux passthrough all
//! arrive as device control strings. None of them place a character in a cell, so
//! [`vte::Perform::put`] discards them. A session that draws a sixel image
//! replays with that region blank, which is what a terminal without sixel support
//! shows too.
//!
//! **OSC 7373.** vitrum's own agent hint channel is not screen state, it is
//! timeline metadata, and it is extracted by [`crate::hints`] against the same
//! byte stream so that each hint keeps the exact seq it arrived at. Handling it
//! here would throw that seq away.
//!
//! **Mouse, focus and paste reporting, and synchronised update.** All of them
//! change what the terminal *sends*, and none change what it shows.
//!
//! # Ground state
//!
//! Three of these callbacks leave [`vte::Parser`] in its ground state no matter
//! which byte triggered them: [`Perform::print`], [`Perform::csi_dispatch`] and
//! [`Perform::esc_dispatch`]. Each sets `Screen::ground`, which is how
//! [`crate::Emulator::feed_byte`] knows a keyframe may be taken after that byte.
//!
//! [`Perform::execute`] does not set it, and that is not an oversight: vte calls
//! `execute` for a C0 byte arriving *inside* a CSI sequence, where the parser is
//! still mid-sequence. [`Perform::osc_dispatch`] and [`Perform::unhook`] do not
//! set it either, because `ESC ] ... ESC \` fires them on the `ESC`, leaving the
//! parser one byte short of ground. Treating any of those three as a safe
//! boundary would produce keyframes that cannot be resumed from.

use vitrum_grid::{Attrs, Rgba};
use vte::{Params, Perform};

use crate::palette::Palette;
use crate::screen::{Charset, Screen};

impl Perform for Screen {
    fn print(&mut self, ch: char) {
        self.print_char(ch);
        self.ground = true;
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            // BEL. Demanding the operator's attention is a session event the
            // daemon already scans for; it changes no cell.
            0x07 => {}
            0x08 => self.backspace(),
            0x09 => self.tab_forward(1),
            0x0a | 0x0b | 0x0c => self.line_feed(),
            0x0d => self.carriage_return(),
            // SO and SI: shift G1 or G0 in.
            0x0e => self.charsets_mut().shifted = true,
            0x0f => self.charsets_mut().shifted = false,
            // IND.
            0x84 => self.line_feed(),
            // NEL.
            0x85 => {
                self.carriage_return();
                self.line_feed();
            }
            // HTS.
            0x88 => {
                let col = self.cursor().col;
                self.tabs_mut().set(col);
            }
            // RI.
            0x8d => self.reverse_index(),
            _ => {}
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}

    fn put(&mut self, _byte: u8) {}

    fn unhook(&mut self) {}

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        let Some(number) = params.first() else {
            return;
        };
        // OSC 1 is the icon name, which no vitrum surface shows, so only 0 (both)
        // and 2 (title) set the title.
        if !matches!(*number, b"0" | b"2") {
            return;
        }
        let Some(text) = params.get(1) else {
            return;
        };
        // Lossy on purpose: a title is display text, and a terminal shows the
        // replacement character rather than refusing to set a title at all.
        let title = String::from_utf8_lossy(text);
        self.set_title(&title);
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        // vte raises `ignore` when the sequence had more intermediates or
        // parameters than it can hold. The parameters it did keep are a prefix of
        // a sequence whose meaning is unknown, so acting on them would be a
        // guess. The parser is at ground either way.
        if ignore {
            self.ground = true;
            return;
        }
        let private = intermediates.first().copied();
        match (action, private) {
            ('@', None) => self.insert_chars(count(params, 0)),
            ('A', None) => self.move_by(0, -i32::from(count(params, 0))),
            ('B' | 'e', None) => self.move_by(0, i32::from(count(params, 0))),
            ('C' | 'a', None) => self.move_by(i32::from(count(params, 0)), 0),
            ('D', None) => self.move_by(-i32::from(count(params, 0)), 0),
            ('E', None) => {
                self.move_by(0, i32::from(count(params, 0)));
                self.move_col(0);
            }
            ('F', None) => {
                self.move_by(0, -i32::from(count(params, 0)));
                self.move_col(0);
            }
            ('G' | '`', None) => self.move_col(count(params, 0) - 1),
            ('H' | 'f', None) => {
                let row = count(params, 0) - 1;
                let col = count(params, 1) - 1;
                self.move_to(col, row);
            }
            ('I', None) => self.tab_forward(count(params, 0)),
            ('J', None) => self.erase_display(mode(params, 0)),
            ('K', None) => self.erase_line(mode(params, 0)),
            ('L', None) => self.insert_lines(count(params, 0)),
            ('M', None) => self.delete_lines(count(params, 0)),
            ('P', None) => self.delete_chars(count(params, 0)),
            ('S', None) => self.scroll_region_up(count(params, 0)),
            ('T', None) => self.scroll_region_down(count(params, 0)),
            ('X', None) => self.erase_chars(count(params, 0)),
            ('Z', None) => self.tab_backward(count(params, 0)),
            ('d', None) => self.move_row(count(params, 0) - 1),
            ('g', None) => match mode(params, 0) {
                0 => {
                    let col = self.cursor().col;
                    self.tabs_mut().clear(col);
                }
                3 => self.tabs_mut().clear_all(),
                _ => {}
            },
            ('h', None) => self.set_ansi_modes(params, true),
            ('l', None) => self.set_ansi_modes(params, false),
            ('h', Some(b'?')) => self.set_private_modes(params, true),
            ('l', Some(b'?')) => self.set_private_modes(params, false),
            ('m', None) => self.select_graphic_rendition(params),
            ('r', None) => {
                let rows = self.rows();
                let top = count(params, 0) - 1;
                let bottom = params
                    .iter()
                    .nth(1)
                    .and_then(|p| p.first().copied())
                    .filter(|v| *v != 0)
                    .unwrap_or(rows);
                self.set_region(top, bottom.min(rows) - 1);
            }
            ('s', None) => self.save_cursor(),
            ('u', None) => self.restore_cursor(),
            _ => {}
        }
        self.ground = true;
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if !ignore {
            match (intermediates, byte) {
                ([], b'7') => self.save_cursor(),
                ([], b'8') => self.restore_cursor(),
                // IND.
                ([], b'D') => self.line_feed(),
                // NEL.
                ([], b'E') => {
                    self.carriage_return();
                    self.line_feed();
                }
                // HTS.
                ([], b'H') => {
                    let col = self.cursor().col;
                    self.tabs_mut().set(col);
                }
                // RI.
                ([], b'M') => self.reverse_index(),
                // RIS.
                ([], b'c') => self.reset(),
                ([b'#'], b'8') => self.decaln(),
                ([b'('], designator) => self.charsets_mut().g0 = charset(designator),
                ([b')'], designator) => self.charsets_mut().g1 = charset(designator),
                _ => {}
            }
        }
        self.ground = true;
    }
}

/// The charset `ESC ( x` and `ESC ) x` designate.
///
/// `0` is DEC Special Graphics. Everything else, including the national variants,
/// is read as ASCII: they differ from ASCII in a handful of punctuation glyphs
/// that no agent emits, and mapping them wrong would corrupt ordinary text.
const fn charset(designator: u8) -> Charset {
    match designator {
        b'0' => Charset::DecSpecialGraphics,
        _ => Charset::Ascii,
    }
}

/// A parameter that counts something, where both absent and zero mean one.
///
/// `CSI A` and `CSI 0 A` both move the cursor up one row. This is the rule for
/// every repeat count in the table, and getting it wrong turns `CSI 0 J` from
/// "erase forwards" into "erase one screen".
fn count(params: &Params, index: usize) -> u16 {
    params
        .iter()
        .nth(index)
        .and_then(|p| p.first().copied())
        .filter(|v| *v != 0)
        .unwrap_or(1)
}

/// A parameter that selects a variant, where zero is a real choice.
///
/// `CSI 0 J` erases forwards, `CSI 2 J` erases everything. Absent means zero.
fn mode(params: &Params, index: usize) -> u16 {
    params
        .iter()
        .nth(index)
        .and_then(|p| p.first().copied())
        .unwrap_or(0)
}

impl Screen {
    /// `CSI h` and `CSI l` without a private marker.
    fn set_ansi_modes(&mut self, params: &Params, on: bool) {
        for param in params.iter() {
            // IRM. LNM (20) is not modelled: it changes what a bare LF does on
            // *input*, and replay has no input.
            if param.first() == Some(&4) {
                self.modes_mut().insert = on;
            }
        }
    }

    /// `CSI ? h` and `CSI ? l`.
    fn set_private_modes(&mut self, params: &Params, on: bool) {
        for param in params.iter() {
            match param.first().copied() {
                // DECOM. Setting it homes the cursor, and with origin mode on
                // "home" is the top of the scrolling region.
                Some(6) => {
                    self.modes_mut().origin = on;
                    self.move_to(0, 0);
                }
                // DECAWM.
                Some(7) => self.modes_mut().autowrap = on,
                // DECTCEM.
                Some(25) => self.cursor_mut().visible = on,
                // The two older alternate-screen spellings: buffer swap only.
                Some(47 | 1047) => self.set_alt_screen(on, false),
                // 1049: swap the buffer *and* the cursor, which is why a program
                // using it leaves the shell prompt where it found it.
                Some(1049) => self.set_alt_screen(on, true),
                _ => {}
            }
        }
    }

    /// `CSI m`.
    fn select_graphic_rendition(&mut self, params: &Params) {
        let palette = *self.palette();
        if params.is_empty() {
            *self.pen_mut() = palette.default_style();
            return;
        }
        let mut pen = self.pen();
        let mut iter = params.iter();
        while let Some(param) = iter.next() {
            let Some(&code) = param.first() else {
                continue;
            };
            match code {
                0 => pen = palette.default_style(),
                1 => pen.attrs = pen.attrs.with(Attrs::BOLD),
                3 => pen.attrs = pen.attrs.with(Attrs::ITALIC),
                // `4:0` is underline off, every other subparameter is a style of
                // underline this grid draws as a plain rule.
                4 => {
                    if param.get(1) == Some(&0) {
                        pen.attrs = pen.attrs.without(Attrs::UNDERLINE);
                    } else {
                        pen.attrs = pen.attrs.with(Attrs::UNDERLINE);
                    }
                }
                7 => pen.attrs = pen.attrs.with(Attrs::REVERSE),
                22 => pen.attrs = pen.attrs.without(Attrs::BOLD),
                23 => pen.attrs = pen.attrs.without(Attrs::ITALIC),
                24 => pen.attrs = pen.attrs.without(Attrs::UNDERLINE),
                27 => pen.attrs = pen.attrs.without(Attrs::REVERSE),
                30..=37 => pen.fg = palette.indexed((code - 30) as u8),
                38 => {
                    if let Some(colour) = extended_colour(param, &mut iter, &palette) {
                        pen.fg = colour;
                    }
                }
                39 => pen.fg = palette.fg,
                40..=47 => pen.bg = palette.indexed((code - 40) as u8),
                48 => {
                    if let Some(colour) = extended_colour(param, &mut iter, &palette) {
                        pen.bg = colour;
                    }
                }
                49 => pen.bg = palette.bg,
                90..=97 => pen.fg = palette.indexed((code - 90 + 8) as u8),
                100..=107 => pen.bg = palette.indexed((code - 100 + 8) as u8),
                // Dim, blink, conceal and strikethrough have no bit in
                // vitrum-grid's Attrs. See the module header.
                _ => {}
            }
        }
        *self.pen_mut() = pen;
    }
}

/// The colour named by SGR 38 or 48, in either spelling.
///
/// Both of these are legal and mean the same thing:
///
/// ```text
/// CSI 38 ; 2 ; 255 ; 128 ; 0 m      one parameter each, semicolon separated
/// CSI 38 : 2 : : 255 : 128 : 0 m    one parameter with subparameters
/// ```
///
/// The colon form is what a terminal that supports it prefers, because the whole
/// colour is one parameter and cannot be split by a parser that does not
/// understand it. The colon form also has an optional colour-space id in the
/// third slot, which is why both a 5-long and a 6-long form are accepted.
fn extended_colour<'a, I>(param: &[u16], iter: &mut I, palette: &Palette) -> Option<Rgba>
where
    I: Iterator<Item = &'a [u16]>,
{
    if param.len() > 1 {
        return match param[1] {
            2 => match param.len() {
                5 => Some(Rgba::rgb(channel(param[2]), channel(param[3]), channel(param[4]))),
                6.. => Some(Rgba::rgb(channel(param[3]), channel(param[4]), channel(param[5]))),
                _ => None,
            },
            5 => param.get(2).map(|index| palette.indexed(channel(*index))),
            _ => None,
        };
    }
    match iter.next().and_then(|p| p.first().copied())? {
        2 => {
            let r = channel(iter.next().and_then(|p| p.first().copied())?);
            let g = channel(iter.next().and_then(|p| p.first().copied())?);
            let b = channel(iter.next().and_then(|p| p.first().copied())?);
            Some(Rgba::rgb(r, g, b))
        }
        5 => Some(palette.indexed(channel(iter.next().and_then(|p| p.first().copied())?))),
        _ => None,
    }
}

/// A CSI parameter is a `u16`; a colour channel is a `u8`.
///
/// Saturating rather than truncating: `38;2;300;0;0` should be as red as the
/// terminal can manage, not `300 & 0xff` which is a dark red.
const fn channel(value: u16) -> u8 {
    if value > 255 { 255 } else { value as u8 }
}
