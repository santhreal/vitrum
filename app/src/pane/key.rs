//! Keystroke to bytes.
//!
//! A terminal has no key events. It has a byte stream, and a keyboard is a
//! function from a key press to the bytes that stand for it. That function is
//! not a matter of taste: an agent TUI reads these bytes with a library that
//! was written against the sequences DEC's terminals emitted and every
//! emulator since has copied, so a table that is nearly right produces a
//! product where Ctrl-Left jumps a word in one program and inserts `[1;5D` in
//! the next.
//!
//! Cursor and editing keys follow the default modes, which is what a child
//! sees before it changes anything: DECCKM reset, so an arrow is `CSI A` and
//! not `SS3 A`, and the normal keypad, so the keypad's Enter is the same
//! carriage return the main one sends. A child that sets application cursor
//! mode is asking for the other encoding, and the emulator's mode state is
//! what has to select it; that selection is not made here, and [`encode`]
//! takes no mode argument today because nothing yet reads one back out of the
//! parser. That gap is named in [`super`] rather than hidden behind a default
//! that silently means "reset".
//!
//! # The modifier parameter
//!
//! A modified special key is the unmodified sequence with a parameter spliced
//! in: `CSI 1 ; m A` for an arrow, `CSI n ; m ~` for an editing key. `m` is 1,
//! plus 1 for Shift, 2 for Alt, 4 for Ctrl, which is why an unmodified key has
//! `m == 1` and why the parameter is never 0. The three bits combine, so
//! Ctrl-Shift is 1 + 1 + 4 = 6.
//!
//! # Control characters
//!
//! Ctrl with a letter is not a sequence at all: it is the letter's low five
//! bits, so Ctrl-A is 0x01 and Ctrl-Z is 0x1a. The punctuation cases are the
//! rest of the C0 range, and the digit aliases exist because the keys that
//! carry that punctuation on a US layout are digits on many others. The table
//! below looks arbitrary and is not: it is the C0 assignment read backwards.
//!
//! Alt is an ESC prefix. That is a convention rather than a standard, and it
//! is the one every shell's readline and every TUI's key reader expects.

/// A key press the encoder can turn into bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    /// A character the layout produced. Already the composed result: the
    /// encoder never sees a dead key or an in-flight composition.
    Char(char),
    /// A key with no character of its own.
    Named(Named),
}

/// Keys that stand for a sequence rather than a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Named {
    Enter,
    /// The keypad's Enter. Distinct from [`Named::Enter`] because application
    /// keypad mode encodes it differently, even though normal mode does not.
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

/// The modifiers held when the key went down.
///
/// Super is absent on purpose: a Super chord belongs to the window manager or
/// to this product's own keymap, and a pane that encoded it would swallow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl Mods {
    /// The modifier parameter: 1 + Shift + 2·Alt + 4·Ctrl.
    fn param(self) -> u8 {
        1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl)
    }

    /// Whether any modifier that changes a special key's encoding is held.
    fn any(self) -> bool {
        self.shift || self.alt || self.ctrl
    }
}

/// Escape, the byte every sequence here starts with.
const ESC: u8 = 0x1b;

/// The bytes a child receives for one key press.
///
/// Never empty: a caller that has nothing to send does not call this. The
/// decision that a key press sends nothing at all belongs to the toolkit half,
/// which is the only side that can tell a bare modifier press from a key.
pub(crate) fn encode(key: Key, mods: Mods) -> Vec<u8> {
    match key {
        Key::Char(c) => encode_char(c, mods),
        Key::Named(n) => encode_named(n, mods),
    }
}

/// A character key, with Ctrl folded into the C0 range and Alt as a prefix.
fn encode_char(c: char, mods: Mods) -> Vec<u8> {
    let mut out = Vec::with_capacity(4);
    if mods.alt {
        out.push(ESC);
    }
    match control_byte(c, mods.ctrl) {
        Some(b) => out.push(b),
        None => {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    out
}

/// The C0 byte a Ctrl chord stands for, if it stands for one.
///
/// `None` means Ctrl does not change this character, in which case the
/// character itself is sent. That is the honest answer for Ctrl-1 and for
/// every letter outside the ASCII range: there is no control code to send, and
/// inventing one would make the pane disagree with every other terminal.
fn control_byte(c: char, ctrl: bool) -> Option<u8> {
    if !ctrl {
        return None;
    }
    match c {
        // The letters, upper or lower: the low five bits.
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        // The punctuation that completes C0.
        '@' | ' ' => Some(0),
        '[' => Some(27),
        '\\' => Some(28),
        ']' => Some(29),
        '^' => Some(30),
        '_' => Some(31),
        '?' => Some(127),
        // The digit aliases for the same physical keys. A layout that does not
        // put `@` on 2 still has to be able to send NUL.
        '2' => Some(0),
        '3' => Some(27),
        '4' => Some(28),
        '5' => Some(29),
        '6' => Some(30),
        '7' => Some(31),
        '8' => Some(127),
        _ => None,
    }
}

/// A named key: a fixed byte, or a sequence with the modifier spliced in.
fn encode_named(n: Named, mods: Mods) -> Vec<u8> {
    use Named::*;

    // The keys that are one byte in every mode, where a modifier changes the
    // byte rather than adding a parameter.
    match n {
        Enter | KeypadEnter => return prefixed(b"\r", mods),
        Tab if mods.shift => return b"\x1b[Z".to_vec(),
        Tab => return prefixed(b"\t", mods),
        // 0x7f rather than 0x08: the terminfo entry this product sets says
        // DEL, and a child that reads 0x08 there deletes forward.
        Backspace => return prefixed(&[0x7f], mods),
        Escape => return prefixed(&[ESC], mods),
        _ => {}
    }

    // `CSI 1 ; m X` keys: the final letter carries the identity.
    let letter = match n {
        Up => Some(b'A'),
        Down => Some(b'B'),
        Right => Some(b'C'),
        Left => Some(b'D'),
        End => Some(b'F'),
        Home => Some(b'H'),
        // F1 to F4 are SS3 keys unmodified and CSI keys modified, which is the
        // one place the two families meet.
        F1 => Some(b'P'),
        F2 => Some(b'Q'),
        F3 => Some(b'R'),
        F4 => Some(b'S'),
        _ => None,
    };
    if let Some(letter) = letter {
        return if mods.any() {
            let mut out = vec![ESC, b'['];
            out.extend_from_slice(b"1;");
            out.extend_from_slice(mods.param().to_string().as_bytes());
            out.push(letter);
            out
        } else if matches!(n, F1 | F2 | F3 | F4) {
            vec![ESC, b'O', letter]
        } else {
            vec![ESC, b'[', letter]
        };
    }

    // `CSI n ~` keys, where the number is the identity. 16 and 22 are skipped
    // by the historical assignment. This is not a typo.
    let number: u8 = match n {
        Insert => 2,
        Delete => 3,
        PageUp => 5,
        PageDown => 6,
        F5 => 15,
        F6 => 17,
        F7 => 18,
        F8 => 19,
        F9 => 20,
        F10 => 21,
        F11 => 23,
        F12 => 24,
        // Every other variant returned above.
        Enter | KeypadEnter | Tab | Backspace | Escape | Up | Down | Right | Left | Home | End
        | F1 | F2 | F3 | F4 => unreachable!("handled above"),
    };
    let mut out = vec![ESC, b'['];
    out.extend_from_slice(number.to_string().as_bytes());
    if mods.any() {
        out.push(b';');
        out.extend_from_slice(mods.param().to_string().as_bytes());
    }
    out.push(b'~');
    out
}

/// A fixed byte sequence, with Alt's escape prefix if it is held.
fn prefixed(bytes: &[u8], mods: Mods) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 1);
    if mods.alt {
        out.push(ESC);
    }
    out.extend_from_slice(bytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(shift: bool, alt: bool, ctrl: bool) -> Mods {
        Mods { shift, alt, ctrl }
    }

    const NONE: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: false,
    };

    /// WHY: the parameter is the whole of how a modified special key is
    /// spelled, and every one of the eight combinations appears in the wild.
    /// An off-by-one here is invisible in the common case and wrong for
    /// Ctrl-Shift-Left, which is the chord an editor binds to selection.
    #[test]
    fn the_modifier_parameter_is_one_plus_the_bits() {
        assert_eq!(NONE.param(), 1);
        assert_eq!(mods(true, false, false).param(), 2);
        assert_eq!(mods(false, true, false).param(), 3);
        assert_eq!(mods(true, true, false).param(), 4);
        assert_eq!(mods(false, false, true).param(), 5);
        assert_eq!(mods(true, false, true).param(), 6);
        assert_eq!(mods(false, true, true).param(), 7);
        assert_eq!(mods(true, true, true).param(), 8);
    }

    /// WHY: an arrow key is the most pressed special key there is, and the
    /// difference between the unmodified and the modified spelling is a whole
    /// different sequence shape rather than an added byte.
    #[test]
    fn an_arrow_gains_a_parameter_only_when_modified() {
        assert_eq!(encode(Key::Named(Named::Left), NONE), b"\x1b[D");
        assert_eq!(
            encode(Key::Named(Named::Left), mods(false, false, true)),
            b"\x1b[1;5D"
        );
        assert_eq!(
            encode(Key::Named(Named::Right), mods(true, false, true)),
            b"\x1b[1;6C"
        );
        assert_eq!(encode(Key::Named(Named::Up), NONE), b"\x1b[A");
        assert_eq!(encode(Key::Named(Named::Down), NONE), b"\x1b[B");
    }

    /// WHY: Home and End share the arrow family's shape but not its letters,
    /// and the pair is where a table copied by eye goes wrong, because `F` and
    /// `H` are not in the order the keys are in.
    #[test]
    fn home_and_end_are_the_letters_they_are_and_not_the_ones_they_look_like() {
        assert_eq!(encode(Key::Named(Named::Home), NONE), b"\x1b[H");
        assert_eq!(encode(Key::Named(Named::End), NONE), b"\x1b[F");
        assert_eq!(
            encode(Key::Named(Named::Home), mods(false, true, false)),
            b"\x1b[1;3H"
        );
    }

    /// WHY: F1 to F4 change family when a modifier is held, from SS3 to CSI.
    /// A pane that always sent one or always sent the other breaks either the
    /// bare key or the modified one, and the bare key is the one a TUI's help
    /// screen is bound to.
    #[test]
    fn the_first_four_function_keys_change_family_under_a_modifier() {
        assert_eq!(encode(Key::Named(Named::F1), NONE), b"\x1bOP");
        assert_eq!(encode(Key::Named(Named::F4), NONE), b"\x1bOS");
        assert_eq!(
            encode(Key::Named(Named::F1), mods(true, false, false)),
            b"\x1b[1;2P"
        );
    }

    /// WHY: the tilde family's numbers are an assignment with two gaps in it,
    /// and a table generated by counting produces F11 as 22, which every
    /// terminal reads as nothing at all.
    #[test]
    fn the_tilde_keys_keep_the_gaps_in_their_numbering() {
        assert_eq!(encode(Key::Named(Named::F5), NONE), b"\x1b[15~");
        assert_eq!(encode(Key::Named(Named::F6), NONE), b"\x1b[17~");
        assert_eq!(encode(Key::Named(Named::F10), NONE), b"\x1b[21~");
        assert_eq!(encode(Key::Named(Named::F11), NONE), b"\x1b[23~");
        assert_eq!(encode(Key::Named(Named::F12), NONE), b"\x1b[24~");
        assert_eq!(encode(Key::Named(Named::Delete), NONE), b"\x1b[3~");
        assert_eq!(
            encode(Key::Named(Named::PageUp), mods(false, false, true)),
            b"\x1b[5;5~"
        );
    }

    /// WHY: Ctrl with a letter is the single most common chord a TUI reads,
    /// and it is not a sequence, so a pane that routed it through the special
    /// key path would send `CSI` bytes where a byte was expected.
    #[test]
    fn ctrl_with_a_letter_is_the_low_five_bits() {
        assert_eq!(encode(Key::Char('a'), mods(false, false, true)), [0x01]);
        assert_eq!(encode(Key::Char('c'), mods(false, false, true)), [0x03]);
        assert_eq!(encode(Key::Char('z'), mods(false, false, true)), [0x1a]);
        // Case does not change the control code.
        assert_eq!(encode(Key::Char('C'), mods(false, false, true)), [0x03]);
    }

    /// WHY: the punctuation half of C0 is the half nobody remembers, and the
    /// digit aliases exist for layouts that do not carry that punctuation.
    /// Ctrl-Space sending NUL is what a shell's set-mark is bound to.
    #[test]
    fn ctrl_completes_the_control_range_through_punctuation_and_digits() {
        assert_eq!(encode(Key::Char(' '), mods(false, false, true)), [0x00]);
        assert_eq!(encode(Key::Char('['), mods(false, false, true)), [0x1b]);
        assert_eq!(encode(Key::Char('?'), mods(false, false, true)), [0x7f]);
        assert_eq!(encode(Key::Char('2'), mods(false, false, true)), [0x00]);
        assert_eq!(encode(Key::Char('6'), mods(false, false, true)), [30]);
        assert_eq!(encode(Key::Char('8'), mods(false, false, true)), [0x7f]);
    }

    /// WHY: Ctrl does not have a control code for every key, and the failure
    /// that matters is inventing one. Ctrl-1 sends `1`, and a pane that sent
    /// 0x01 for it would make Ctrl-1 indistinguishable from Ctrl-A.
    #[test]
    fn ctrl_leaves_a_key_with_no_control_code_alone() {
        assert_eq!(encode(Key::Char('1'), mods(false, false, true)), b"1");
        assert_eq!(encode(Key::Char('9'), mods(false, false, true)), b"9");
        assert_eq!(encode(Key::Char('e'), mods(false, false, true)), [0x05]);
        assert_ne!(
            encode(Key::Char('1'), mods(false, false, true)),
            encode(Key::Char('a'), mods(false, false, true)),
        );
    }

    /// WHY: Alt is a prefix and not a parameter, and it composes with Ctrl.
    /// Alt-Backspace deleting a word is the chord this most often breaks.
    #[test]
    fn alt_prefixes_an_escape_and_composes_with_ctrl() {
        assert_eq!(encode(Key::Char('b'), mods(false, true, false)), b"\x1bb");
        assert_eq!(encode(Key::Char('b'), mods(false, true, true)), [ESC, 0x02]);
        assert_eq!(
            encode(Key::Named(Named::Backspace), mods(false, true, false)),
            [ESC, 0x7f]
        );
    }

    /// WHY: Backspace is DEL and not BS. A child told 0x08 by a terminal whose
    /// terminfo promises 0x7f deletes in the wrong direction, and the operator
    /// sees the character to the right vanish.
    #[test]
    fn backspace_is_delete() {
        assert_eq!(encode(Key::Named(Named::Backspace), NONE), [0x7f]);
    }

    /// WHY: Shift-Tab is the only special key whose shifted form is a
    /// different sequence rather than the same one with a parameter, so the
    /// general rule produces `CSI 1 ; 2 I` and every form loses back-tab.
    #[test]
    fn shift_tab_is_its_own_sequence() {
        assert_eq!(encode(Key::Named(Named::Tab), NONE), b"\t");
        assert_eq!(
            encode(Key::Named(Named::Tab), mods(true, false, false)),
            b"\x1b[Z"
        );
    }

    /// WHY: the keypad's Enter is the main Enter in the default mode, and a
    /// pane that sent `SS3 M` for it in that mode submits nothing in a shell.
    #[test]
    fn both_enters_are_a_carriage_return_in_the_default_mode() {
        assert_eq!(encode(Key::Named(Named::Enter), NONE), b"\r");
        assert_eq!(encode(Key::Named(Named::KeypadEnter), NONE), b"\r");
    }

    /// WHY: a keystroke that produces no bytes at all is a keystroke a child
    /// can never see. The toolkit half decides a press sends nothing; once a
    /// key reaches here it always has bytes, and an empty answer would be a
    /// silent dropped keystroke rather than a visible wrong one.
    #[test]
    fn every_key_and_modifier_combination_sends_at_least_one_byte() {
        let named = [
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
        for n in named {
            for bits in 0..8u8 {
                let m = mods(bits & 1 != 0, bits & 2 != 0, bits & 4 != 0);
                let out = encode(Key::Named(n), m);
                assert!(!out.is_empty(), "{n:?} with {m:?} encoded to nothing");
            }
        }
        for c in ['a', 'Z', '1', ' ', '?', 'é', '中'] {
            for bits in 0..8u8 {
                let m = mods(bits & 1 != 0, bits & 2 != 0, bits & 4 != 0);
                let out = encode(Key::Char(c), m);
                assert!(!out.is_empty(), "{c:?} with {m:?} encoded to nothing");
            }
        }
    }

    /// WHY: a character outside ASCII is sent as its own UTF-8 and not folded
    /// into a control code, and the pane must not truncate it to one byte.
    #[test]
    fn a_character_outside_ascii_is_sent_as_its_own_utf8() {
        assert_eq!(encode(Key::Char('é'), NONE), "é".as_bytes());
        assert_eq!(encode(Key::Char('中'), NONE), "中".as_bytes());
        assert_eq!(
            encode(Key::Char('中'), mods(false, true, false)),
            [&[ESC][..], "中".as_bytes()].concat()
        );
        // Ctrl has no control code for it, so the character survives whole.
        assert_eq!(
            encode(Key::Char('é'), mods(false, false, true)),
            "é".as_bytes()
        );
    }
}
