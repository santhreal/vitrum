//! The one keyboard decision the encoder cannot make for itself.
//!
//! [`super::key::encode`] writes the cursor keys in their reset form: an arrow
//! is `CSI A`, because DECCKM reset is what a child sees before it changes
//! anything. A child that sets application cursor mode is asking for `SS3 A`
//! instead, and the encoder takes no mode argument, so the selection is made
//! here, against the mode state the emulator actually reports.
//!
//! Applied as a rewrite of the encoded bytes rather than as a second encoder.
//! Two tables of cursor sequences is two tables to keep in step, and the
//! difference between them is one byte in one position.
//!
//! Only the unmodified forms change. `CSI 1 ; 5 A` stays as it is under
//! application cursor mode, because the parameterised form has no SS3
//! spelling: there is nowhere to put the parameter.

/// Escape.
const ESC: u8 = 0x1b;

/// Rewrite an encoded keystroke for the cursor mode the child asked for.
///
/// Returns the input untouched when the mode is reset, when the keystroke is
/// not a cursor key, or when it carries a modifier parameter.
pub(crate) fn for_cursor_mode(bytes: &mut Vec<u8>, application: bool) {
    if !application || bytes.len() != 3 || bytes[0] != ESC || bytes[1] != b'[' {
        return;
    }
    // A, B, C and D are the arrows; H and F are Home and End, which travel
    // with them because the same programs set the same mode for all six.
    if matches!(bytes[2], b'A' | b'B' | b'C' | b'D' | b'H' | b'F') {
        bytes[1] = b'O';
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(input: &[u8], application: bool) -> Vec<u8> {
        let mut bytes = input.to_vec();
        for_cursor_mode(&mut bytes, application);
        bytes
    }

    /// WHY: a program in application cursor mode that receives `CSI A` reads
    /// it as an unknown sequence, and the arrow key does nothing. A program in
    /// the reset mode that receives `SS3 A` puts a literal `OA` into its
    /// command line. Both are visible immediately and both are the same
    /// missing decision.
    ///
    /// Every cursor key, both modes, asserted as a pair so a change that fixes
    /// one direction by breaking the other cannot pass.
    #[test]
    fn every_cursor_key_follows_the_mode_the_child_set() {
        for letter in [b'A', b'B', b'C', b'D', b'H', b'F'] {
            let reset = [ESC, b'[', letter];
            let application = [ESC, b'O', letter];

            assert_eq!(
                rewrite(&reset, true),
                application,
                "{} was not promoted",
                letter as char
            );
            assert_eq!(
                rewrite(&reset, false),
                reset,
                "{} was promoted with the mode reset",
                letter as char
            );
        }
    }

    /// WHY: the parameterised form has no SS3 spelling. Rewriting it produces
    /// `ESC O 1 ; 5 A`, which no program parses, so Ctrl-Left would stop
    /// working in exactly the programs that set the mode.
    #[test]
    fn a_modified_cursor_key_is_left_alone_in_both_modes() {
        for bytes in [
            b"\x1b[1;5A".as_slice(),
            b"\x1b[1;2D",
            b"\x1b[1;6C",
            b"\x1b[1;3H",
        ] {
            assert_eq!(rewrite(bytes, true), bytes);
            assert_eq!(rewrite(bytes, false), bytes);
        }
    }

    /// WHY: the rewrite is a byte substitution on a three-byte sequence, and
    /// the failure mode of a substitution that is not narrow enough is that it
    /// corrupts something else. Function keys, editing keys and plain
    /// characters all start with the same two bytes or are the same length.
    #[test]
    fn nothing_that_is_not_a_cursor_key_is_touched() {
        for bytes in [
            b"a".as_slice(),
            b"\x1b",
            b"\x1b[",
            // Editing keys: three bytes and a CSI, but a digit rather than a
            // letter, and the wrong final byte.
            b"\x1b[2~",
            b"\x1b[3~",
            // Function keys.
            b"\x1bOP",
            b"\x1b[15~",
            // A three-byte CSI whose final byte is not a cursor key.
            b"\x1b[E",
            b"\x1b[Z",
            b"\x1b[G",
            // Alt-prefixed forms are four bytes and must not be shortened.
            b"\x1b\x1b[A",
            // A pasted payload that happens to look like one.
            b"\x1b[200~",
            b"",
        ] {
            assert_eq!(rewrite(bytes, true), bytes, "{bytes:?} was rewritten");
            assert_eq!(rewrite(bytes, false), bytes);
        }
    }
}
