//! A GTK key event, as the encoder sees it.
//!
//! Split from [`super::surface`] because none of it needs a surface. The
//! swapchain reaches X11 through two `libgdk-3` symbols and is built on Linux
//! alone; translating a keyval is toolkit work every platform compiles, and
//! the shell's chord matching needs it whether or not a pane can paint.

use gtk::gdk;

use super::key::{Key, Mods, Named, encode};

/// Translate a GTK key event into the key and modifiers it names.
///
/// Returns `None` for a keystroke that means nothing: a bare modifier press,
/// or a keyval with no character and no named sequence. That decision lives
/// here rather than in [`super::key`] so the encoder never has to represent
/// "no keystroke".
pub(crate) fn decode_event(ev: &gdk::EventKey) -> Option<(Key, Mods)> {
    let state = ev.state();
    let mods = Mods {
        shift: state.contains(gdk::ModifierType::SHIFT_MASK),
        // MOD1 is Alt on every desktop this ships to. Super is not read: a
        // Super chord belongs to the window manager or the shell keymap, and
        // the pane must not swallow it.
        alt: state.contains(gdk::ModifierType::MOD1_MASK),
        ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
    };
    Some((classify(ev.keyval())?, mods))
}

/// The bytes a keystroke sends to the child.
pub(crate) fn encode_event(ev: &gdk::EventKey) -> Option<Vec<u8>> {
    let (key, mods) = decode_event(ev)?;
    Some(encode(key, mods))
}

/// The top-row digit the physical key carries, if it carries one.
///
/// The layout's name for Ctrl+Shift+1 is `!`, so a chord bound to a digit
/// would never match the keystroke it is named after. Asking the keymap what
/// the same hardware key produces with no modifiers is the only way to get
/// the digit back, and it is asked once per press rather than kept in a
/// table that would go stale when the layout changes.
pub(crate) fn digit_of(ev: &gdk::EventKey) -> Option<char> {
    let keymap = gdk::Keymap::for_display(&ev.window()?.display())?;
    let (keyval, ..) = keymap.translate_keyboard_state(
        ev.hardware_keycode().into(),
        gdk::ModifierType::empty(),
        ev.group().into(),
    )?;
    let ch = gdk::keys::Key::from(keyval).to_unicode()?;
    ch.is_ascii_digit().then_some(ch)
}

/// A gdk keyval, as the encoder sees it.
///
/// The named table is the only place gdk constants appear. Everything past it
/// is toolkit-free, which is why the encoding is testable without a display.
fn classify(kv: gdk::keys::Key) -> Option<Key> {
    use gdk::keys::constants as k;

    let named = match kv {
        k::Return => Named::Enter,
        k::KP_Enter => Named::KeypadEnter,
        k::Tab | k::ISO_Left_Tab => Named::Tab,
        k::BackSpace => Named::Backspace,
        k::Escape => Named::Escape,
        k::Up | k::KP_Up => Named::Up,
        k::Down | k::KP_Down => Named::Down,
        k::Right | k::KP_Right => Named::Right,
        k::Left | k::KP_Left => Named::Left,
        k::Home | k::KP_Home => Named::Home,
        k::End | k::KP_End => Named::End,
        k::Page_Up | k::KP_Page_Up => Named::PageUp,
        k::Page_Down | k::KP_Page_Down => Named::PageDown,
        k::Insert | k::KP_Insert => Named::Insert,
        k::Delete | k::KP_Delete => Named::Delete,
        k::F1 => Named::F1,
        k::F2 => Named::F2,
        k::F3 => Named::F3,
        k::F4 => Named::F4,
        k::F5 => Named::F5,
        k::F6 => Named::F6,
        k::F7 => Named::F7,
        k::F8 => Named::F8,
        k::F9 => Named::F9,
        k::F10 => Named::F10,
        k::F11 => Named::F11,
        k::F12 => Named::F12,
        // Not a named key. If the layout produced a character, that character
        // is the keystroke; otherwise this was a modifier or a dead key and
        // there is nothing to send.
        _ => return kv.to_unicode().map(Key::Char),
    };
    Some(Key::Named(named))
}
