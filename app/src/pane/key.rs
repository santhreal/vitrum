//! Keystroke to terminal bytes.
//!
//! A terminal's input side is a byte protocol, not a character stream. The
//! child on the other end of the pty expects `\x1b[1;5C` for Ctrl+Right and
//! `\x7f` for Backspace, and it expects them whether the keystroke arrived
//! through a webview's `keydown` or through a GTK key event on a native
//! surface. This module owns that translation and nothing else: it takes a
//! [`Key`] and a [`Mods`] and returns the bytes, with no GTK, no gdk keyvals
//! and no session in scope.
//!
//! Keeping it free of the toolkit is what makes it testable. The encoding is
//! the part that is easy to get subtly wrong — an off-by-one in the modifier
//! parameter, SS3 where CSI was wanted, `\r\n` where the child wanted `\r` —
//! and none of those need a display to catch. The gdk half is a keyval-to-
//! [`Key`] lookup in [`super::surface`], which has no encoding decisions left
//! in it.
//!
//! # The encoding
//!
//! Cursor and editing keys follow xterm's default (DECCKM reset, normal
//! keypad) forms, because that is what every program that reads `terminfo`
//! for `xterm-256color` is compiled against:
//!
//! - cursor and Home/End: `ESC [ A`, and `ESC [ 1 ; m A` when modified
//! - Insert/Delete/Page: `ESC [ 2 ~`, and `ESC [ 2 ; m ~` when modified
//! - F1–F4: `ESC O P` unmodified, `ESC [ 1 ; m P` when modified
//! - F5–F12: `ESC [ 15 ~` and friends, with `; m` before the tilde
//!
//! `m` is the xterm modifier parameter: 1, plus 1 for Shift, 2 for Alt, 4 for
//! Ctrl. Alt on a key with no CSI form is an ESC prefix, which is the
//! eight-bit-meta-off convention every shell's readline assumes.
//!
//! Application cursor mode (DECCKM), which swaps `ESC [` for `ESC O` on the
//! arrows, is not handled here: it is a property of the emulator's current
//! mode state, so it belongs to whatever owns the parser, and the pane will
//! pass that mode in when it has one to pass.

/// A key that is not a character: the ones with a named escape sequence.
///
/// The variants are the list this module promises to encode, and
/// [`Named::ALL`] is that list in iterable form. The tests walk `ALL` and
/// match exhaustively on the variant, so adding a key here fails the build
/// until its exact bytes are recorded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Named {
    Enter,
    /// The keypad's Enter. Distinct from [`Named::Enter`] because application
    /// keypad mode (DECKPAM) encodes it as `ESC O M`, which is a mode this
    /// module does not track yet but will have to.
    KeypadEnter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Right,
    Left,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

impl Named {
    /// Every named key, in one place, for anything that must cover them all.
    pub(crate) const ALL: &'static [Named] = &[
        Named::Enter,
        Named::KeypadEnter,
        Named::Tab,
        Named::Backspace,
        Named::Escape,
        Named::Up,
        Named::Down,
        Named::Right,
        Named::Left,
        Named::Home,
        Named::End,
        Named::PageUp,
        Named::PageDown,
        Named::Insert,
        Named::Delete,
        Named::F1,
        Named::F2,
        Named::F3,
        Named::F4,
        Named::F5,
        Named::F6,
        Named::F7,
        Named::F8,
        Named::F9,
        Named::F10,
        Named::F11,
        Named::F12,
    ];

    /// The final byte of the `CSI ... <letter>` form, for the keys that have
    /// one.
    const fn csi_letter(self) -> Option<u8> {
        Some(match self {
            Named::Up => b'A',
            Named::Down => b'B',
            Named::Right => b'C',
            Named::Left => b'D',
            Named::Home => b'H',
            Named::End => b'F',
            // F1-F4 are SS3 letters unmodified and CSI letters modified, so
            // they are handled on their own path rather than here.
            _ => return None,
        })
    }

    /// The numeric parameter of the `CSI <n> ~` form, for the keys that have
    /// one.
    const fn tilde_number(self) -> Option<u8> {
        Some(match self {
            Named::Insert => 2,
            Named::Delete => 3,
            Named::PageUp => 5,
            Named::PageDown => 6,
            // 16 and 22 are skipped by xterm. This is not a typo.
            Named::F5 => 15,
            Named::F6 => 17,
            Named::F7 => 18,
            Named::F8 => 19,
            Named::F9 => 20,
            Named::F10 => 21,
            Named::F11 => 23,
            Named::F12 => 24,
            _ => return None,
        })
    }

    /// The SS3 final byte for F1 through F4.
    const fn ss3_letter(self) -> Option<u8> {
        Some(match self {
            Named::F1 => b'P',
            Named::F2 => b'Q',
            Named::F3 => b'R',
            Named::F4 => b'S',
            _ => return None,
        })
    }
}

/// What the operator was holding down.
///
/// Super is deliberately absent. A chord with Super in it belongs to the
/// window manager or to the shell's own keymap, and a terminal that swallowed
/// it would be taking a key the desktop had already claimed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Mods {
    pub(crate) shift: bool,
    pub(crate) alt: bool,
    pub(crate) ctrl: bool,
}

impl Mods {
    /// Nothing held.
    pub(crate) const NONE: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: false,
    };

    /// The xterm modifier parameter: 1 + Shift + 2·Alt + 4·Ctrl.
    const fn param(self) -> u8 {
        1 + (self.shift as u8) + 2 * (self.alt as u8) + 4 * (self.ctrl as u8)
    }

    /// Whether any modifier that changes a named key's escape form is held.
    const fn any(self) -> bool {
        self.shift || self.alt || self.ctrl
    }
}

/// One keystroke, as far as encoding is concerned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Key {
    /// A key that produced a character. The character is the one the layout
    /// produced, so Shift is already applied to it and must not be applied
    /// again by the encoder.
    Char(char),
    /// A key with a named escape sequence.
    Named(Named),
}

/// The byte a Ctrl'd character collapses to, if it has one.
///
/// The C0 controls are `Ctrl` plus the ASCII character 0x40 above them, which
/// is why the table looks arbitrary and is not. The digit aliases are xterm's:
/// on a US layout `Ctrl+6` and `Ctrl+^` are the same physical keystroke, and a
/// terminal that only accepted the shifted spelling would make `Ctrl+^`
/// unreachable on a layout that puts `^` behind a dead key.
const fn ctrl_byte(ch: char) -> Option<u8> {
    Some(match ch {
        ' ' | '@' | '2' => 0x00,
        'a'..='z' => ch as u8 - 0x60,
        'A'..='Z' => ch as u8 - 0x40,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '7' | '/' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

/// Encode one keystroke into the bytes the child expects.
///
/// Always returns at least one byte: a key that should send nothing is not
/// represented as a [`Key`] at all, because deciding that is the toolkit's job
/// (a bare modifier press, a dead key mid-composition) and doing it here would
/// give two places an opinion on whether a keystroke exists.
pub(crate) fn encode(key: Key, mods: Mods) -> Vec<u8> {
    match key {
        Key::Char(ch) => encode_char(ch, mods),
        Key::Named(named) => encode_named(named, mods),
    }
}

fn encode_char(ch: char, mods: Mods) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    if mods.alt {
        out.push(0x1b);
    }
    match mods.ctrl.then(|| ctrl_byte(ch)).flatten() {
        Some(byte) => out.push(byte),
        None => {
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
    }
    out
}

fn encode_named(named: Named, mods: Mods) -> Vec<u8> {
    // The keys whose modified form is a CSI parameter, which is most of them.
    if let Some(letter) = named.csi_letter() {
        return csi_letter(letter, mods);
    }
    if let Some(n) = named.tilde_number() {
        return csi_tilde(n, mods);
    }
    if let Some(letter) = named.ss3_letter() {
        // Unmodified F1-F4 are SS3; modified, they take the same `CSI 1 ; m`
        // prefix as the cursor keys and keep their letter.
        return if mods.any() {
            csi_letter(letter, mods)
        } else {
            vec![0x1b, b'O', letter]
        };
    }

    match named {
        // CR, not LF and not CRLF. The line discipline turns CR into whatever
        // the child's termios asks for; sending LF here bypasses that and
        // breaks every program that reads a line.
        Named::Enter | Named::KeypadEnter => alt_prefixed(mods, &[b'\r']),
        Named::Escape => alt_prefixed(mods, &[0x1b]),
        // Shift+Tab is CSI Z, the only back-tab spelling terminfo knows.
        Named::Tab if mods.shift => vec![0x1b, b'[', b'Z'],
        Named::Tab => alt_prefixed(mods, &[b'\t']),
        // DEL, not BS. `xterm-256color` sets `kbs=\177`, and a terminal that
        // sends BS instead leaves readline deleting forward.
        Named::Backspace if mods.ctrl => alt_prefixed(mods, &[0x08]),
        Named::Backspace => alt_prefixed(mods, &[0x7f]),
        // Every remaining variant went out through one of the tables above.
        // This arm is unreachable rather than a default, and stays a `match`
        // so a new variant with no table entry lands here loudly.
        Named::Up
        | Named::Down
        | Named::Right
        | Named::Left
        | Named::Home
        | Named::End
        | Named::Insert
        | Named::Delete
        | Named::PageUp
        | Named::PageDown
        | Named::F1
        | Named::F2
        | Named::F3
        | Named::F4
        | Named::F5
        | Named::F6
        | Named::F7
        | Named::F8
        | Named::F9
        | Named::F10
        | Named::F11
        | Named::F12 => unreachable!("{named:?} has an escape-sequence table entry"),
    }
}

/// `ESC [ <letter>`, or `ESC [ 1 ; m <letter>` when modified.
fn csi_letter(letter: u8, mods: Mods) -> Vec<u8> {
    let mut out = vec![0x1b, b'['];
    if mods.any() {
        out.push(b'1');
        out.push(b';');
        push_param(&mut out, mods.param());
    }
    out.push(letter);
    out
}

/// `ESC [ <n> ~`, or `ESC [ <n> ; m ~` when modified.
fn csi_tilde(n: u8, mods: Mods) -> Vec<u8> {
    let mut out = vec![0x1b, b'['];
    push_param(&mut out, n);
    if mods.any() {
        out.push(b';');
        push_param(&mut out, mods.param());
    }
    out.push(b'~');
    out
}

/// Decimal, without going through a formatter for a number under 100.
fn push_param(out: &mut Vec<u8>, n: u8) {
    if n >= 10 {
        out.push(b'0' + n / 10);
    }
    out.push(b'0' + n % 10);
}

/// The bytes, with ESC in front when Alt is held.
fn alt_prefixed(mods: Mods, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 1);
    if mods.alt {
        out.push(0x1b);
    }
    out.extend_from_slice(bytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIFT: Mods = Mods {
        shift: true,
        alt: false,
        ctrl: false,
    };
    const ALT: Mods = Mods {
        shift: false,
        alt: true,
        ctrl: false,
    };
    const CTRL: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: true,
    };

    /// The unmodified bytes for every named key.
    ///
    /// This is an exhaustive match on purpose. A new [`Named`] variant makes
    /// this function stop compiling, which is the point: a key cannot enter
    /// the encoder's vocabulary without someone writing down the exact bytes
    /// it sends.
    fn expected_plain(named: Named) -> &'static [u8] {
        match named {
            Named::Enter | Named::KeypadEnter => b"\r",
            Named::Tab => b"\t",
            Named::Backspace => b"\x7f",
            Named::Escape => b"\x1b",
            Named::Up => b"\x1b[A",
            Named::Down => b"\x1b[B",
            Named::Right => b"\x1b[C",
            Named::Left => b"\x1b[D",
            Named::Home => b"\x1b[H",
            Named::End => b"\x1b[F",
            Named::PageUp => b"\x1b[5~",
            Named::PageDown => b"\x1b[6~",
            Named::Insert => b"\x1b[2~",
            Named::Delete => b"\x1b[3~",
            Named::F1 => b"\x1bOP",
            Named::F2 => b"\x1bOQ",
            Named::F3 => b"\x1bOR",
            Named::F4 => b"\x1bOS",
            Named::F5 => b"\x1b[15~",
            Named::F6 => b"\x1b[17~",
            Named::F7 => b"\x1b[18~",
            Named::F8 => b"\x1b[19~",
            Named::F9 => b"\x1b[20~",
            Named::F10 => b"\x1b[21~",
            Named::F11 => b"\x1b[23~",
            Named::F12 => b"\x1b[24~",
        }
    }

    /// Walk the list in source, not a sample of it.
    #[test]
    fn every_named_key_sends_its_recorded_bytes() {
        for &named in Named::ALL {
            let got = encode(Key::Named(named), Mods::NONE);
            assert_eq!(
                got,
                expected_plain(named),
                "{named:?} encoded as {got:x?}, expected {:x?}",
                expected_plain(named)
            );
        }
    }

    /// `ALL` is the list every other test walks, so a variant missing from it
    /// would silently narrow the whole file's coverage.
    #[test]
    fn the_named_key_list_is_complete_and_has_no_duplicates() {
        let mut seen: Vec<Named> = Vec::new();
        for &named in Named::ALL {
            assert!(!seen.contains(&named), "{named:?} listed twice in ALL");
            seen.push(named);
        }
        // Enter, KeypadEnter, Tab, Backspace, Escape, four cursor keys,
        // Home, End, PageUp, PageDown, Insert, Delete, F1-F12.
        assert_eq!(seen.len(), 27, "ALL no longer covers every Named variant");
    }

    /// No named key may encode to nothing: a key that reaches the encoder has
    /// already been decided to be a keystroke, and dropping it here would be a
    /// dead key with no diagnostic.
    #[test]
    fn no_modifier_combination_produces_an_empty_sequence() {
        for &named in Named::ALL {
            for bits in 0..8u8 {
                let mods = Mods {
                    shift: bits & 1 != 0,
                    alt: bits & 2 != 0,
                    ctrl: bits & 4 != 0,
                };
                let got = encode(Key::Named(named), mods);
                assert!(!got.is_empty(), "{named:?} with {mods:?} sent nothing");
            }
        }
    }

    /// The modifier parameter is the one thing every modified sequence shares,
    /// so an off-by-one here would be wrong on every key at once.
    #[test]
    fn the_modifier_parameter_follows_xterm() {
        let cases: [(Mods, &[u8]); 7] = [
            (SHIFT, b"\x1b[1;2C"),
            (ALT, b"\x1b[1;3C"),
            (
                Mods {
                    shift: true,
                    alt: true,
                    ctrl: false,
                },
                b"\x1b[1;4C",
            ),
            (CTRL, b"\x1b[1;5C"),
            (
                Mods {
                    shift: true,
                    alt: false,
                    ctrl: true,
                },
                b"\x1b[1;6C",
            ),
            (
                Mods {
                    shift: false,
                    alt: true,
                    ctrl: true,
                },
                b"\x1b[1;7C",
            ),
            (
                Mods {
                    shift: true,
                    alt: true,
                    ctrl: true,
                },
                b"\x1b[1;8C",
            ),
        ];
        for (mods, want) in cases {
            assert_eq!(encode(Key::Named(Named::Right), mods), want, "{mods:?}");
        }
    }

    /// Every key with a modified form, spelled out. The plain forms are
    /// covered by walking `ALL`; these are the ones where CSI, SS3 and tilde
    /// diverge, and where copying xterm's table wrongly is easiest.
    #[test]
    fn modified_named_keys_take_their_csi_form() {
        let cases: &[(Named, Mods, &[u8])] = &[
            (Named::Up, CTRL, b"\x1b[1;5A"),
            (Named::Down, CTRL, b"\x1b[1;5B"),
            (Named::Left, CTRL, b"\x1b[1;5D"),
            (Named::Home, SHIFT, b"\x1b[1;2H"),
            (Named::End, SHIFT, b"\x1b[1;2F"),
            (Named::PageUp, CTRL, b"\x1b[5;5~"),
            (Named::PageDown, ALT, b"\x1b[6;3~"),
            (Named::Insert, SHIFT, b"\x1b[2;2~"),
            (Named::Delete, CTRL, b"\x1b[3;5~"),
            // SS3 unmodified, CSI once a modifier is held, same final byte.
            (Named::F1, SHIFT, b"\x1b[1;2P"),
            (Named::F4, CTRL, b"\x1b[1;5S"),
            (Named::F5, SHIFT, b"\x1b[15;2~"),
            (Named::F12, CTRL, b"\x1b[24;5~"),
        ];
        for &(named, mods, want) in cases {
            let got = encode(Key::Named(named), mods);
            assert_eq!(got, want, "{named:?} with {mods:?} encoded as {got:x?}");
        }
    }

    /// The editing keys where a plausible implementation sends the wrong
    /// control byte and nobody notices until readline misbehaves.
    #[test]
    fn editing_keys_send_the_bytes_terminfo_advertises() {
        assert_eq!(encode(Key::Named(Named::Backspace), Mods::NONE), b"\x7f");
        assert_eq!(encode(Key::Named(Named::Backspace), CTRL), b"\x08");
        assert_eq!(encode(Key::Named(Named::Backspace), ALT), b"\x1b\x7f");
        assert_eq!(encode(Key::Named(Named::Tab), SHIFT), b"\x1b[Z");
        assert_eq!(encode(Key::Named(Named::Tab), ALT), b"\x1b\t");
        assert_eq!(encode(Key::Named(Named::Enter), Mods::NONE), b"\r");
        assert_eq!(encode(Key::Named(Named::KeypadEnter), Mods::NONE), b"\r");
        assert_eq!(encode(Key::Named(Named::Enter), ALT), b"\x1b\r");
        assert_eq!(encode(Key::Named(Named::Escape), ALT), b"\x1b\x1b");
    }

    /// Ctrl on a character is the C0 control 0x40 below it, and the interrupt
    /// and end-of-file keystrokes are the two a broken table strands a user
    /// with no way out.
    #[test]
    fn control_characters_collapse_to_c0() {
        let cases: &[(char, u8)] = &[
            ('c', 0x03),
            ('C', 0x03),
            ('d', 0x04),
            ('z', 0x1a),
            ('a', 0x01),
            ('@', 0x00),
            (' ', 0x00),
            ('[', 0x1b),
            ('\\', 0x1c),
            (']', 0x1d),
            ('^', 0x1e),
            ('_', 0x1f),
            ('?', 0x7f),
            ('/', 0x1f),
            // xterm's digit aliases for the same physical keys.
            ('2', 0x00),
            ('3', 0x1b),
            ('4', 0x1c),
            ('5', 0x1d),
            ('6', 0x1e),
            ('7', 0x1f),
            ('8', 0x7f),
        ];
        for &(ch, want) in cases {
            assert_eq!(
                encode(Key::Char(ch), CTRL),
                vec![want],
                "Ctrl+{ch:?} should be {want:#04x}"
            );
        }
    }

    /// A Ctrl chord with no control byte still has to type the character,
    /// because dropping it makes the key dead for as long as Ctrl is down.
    #[test]
    fn ctrl_on_a_character_with_no_control_byte_types_it() {
        assert_eq!(encode(Key::Char('1'), CTRL), b"1");
        assert_eq!(encode(Key::Char('\u{f6}'), CTRL), "\u{f6}".as_bytes());
    }

    /// Alt is an ESC prefix, and it composes with Ctrl rather than replacing
    /// it: Alt+Ctrl+C is the two bytes a shell reads as meta-interrupt.
    #[test]
    fn alt_prefixes_escape_and_composes_with_ctrl() {
        assert_eq!(encode(Key::Char('x'), ALT), b"\x1bx");
        assert_eq!(
            encode(
                Key::Char('c'),
                Mods {
                    shift: false,
                    alt: true,
                    ctrl: true
                }
            ),
            b"\x1b\x03"
        );
    }

    /// Shift is already in the character the layout produced. Applying it
    /// again would uppercase twice, or worse, uppercase a symbol.
    #[test]
    fn shift_does_not_touch_a_character_the_layout_already_shifted() {
        assert_eq!(encode(Key::Char('A'), SHIFT), b"A");
        assert_eq!(encode(Key::Char('!'), SHIFT), b"!");
    }

    /// Text is UTF-8 on the wire, not a byte per keypress.
    #[test]
    fn characters_outside_ascii_go_out_as_utf8() {
        assert_eq!(encode(Key::Char('\u{e9}'), Mods::NONE), b"\xc3\xa9");
        assert_eq!(encode(Key::Char('\u{1f44d}'), Mods::NONE), b"\xf0\x9f\x91\x8d");
        assert_eq!(encode(Key::Char('\u{4e2d}'), ALT), b"\x1b\xe4\xb8\xad");
    }
}
