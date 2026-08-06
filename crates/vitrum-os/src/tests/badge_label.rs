//! Badge text and the Unity LauncherEntry payload.

use crate::badge::{
    UNITY_BUS_NAME, UNITY_INTERFACE, UNITY_OBJECT_PATH, UnityValue,
    WindowHandle, dock_badge_label, main_window, overlay_description, register_main_window,
    resolve_window, unity_app_uri, unity_properties,
};

/// Zero must clear the dock badge, not draw a zero.
///
/// `Some(String::new())` sets an empty red pill on the dock, which looks like a
/// rendering bug; `Some("0")` tells the user there is something to look at when
/// there is not.
#[test]
fn zero_clears_the_dock_badge() {
    assert_eq!(dock_badge_label(0), None);
}

/// One to ninety-nine must be the number itself.
#[test]
fn small_counts_are_the_number() {
    assert_eq!(dock_badge_label(1).as_deref(), Some("1"));
    assert_eq!(dock_badge_label(99).as_deref(), Some("99"));
}

/// Past ninety-nine the dock badge must be `99+`.
///
/// Three digits do not fit legibly in a dock badge; Mail and Messages both cap
/// at 99+ for the same reason.
#[test]
fn large_counts_are_capped_on_the_dock() {
    assert_eq!(dock_badge_label(100).as_deref(), Some("99+"));
    assert_eq!(dock_badge_label(u32::MAX).as_deref(), Some("99+"));
}

/// The taskbar overlay description must be a sentence, and grammatical.
///
/// A screen reader announces this string. "3" is not an announcement.
#[test]
fn the_overlay_description_is_a_grammatical_sentence() {
    assert_eq!(overlay_description(0), "");
    assert_eq!(overlay_description(1), "1 session needs attention");
    assert_eq!(overlay_description(4), "4 sessions need attention");
}

/// `count-visible` must be false at zero and true otherwise.
///
/// The Unity protocol has no "clear" message. A launcher sent only `count = 0`
/// keeps drawing a badge with a zero in it, forever.
#[test]
fn count_visible_is_the_only_way_to_clear_a_unity_badge() {
    assert_eq!(
        unity_properties(0),
        vec![("count", UnityValue::Int64(0)), ("count-visible", UnityValue::Bool(false))]
    );
    assert_eq!(
        unity_properties(3),
        vec![("count", UnityValue::Int64(3)), ("count-visible", UnityValue::Bool(true))]
    );
}

/// The count must be an `i64`, because that is the protocol's type.
///
/// The signal signature is `a{sv}` with the count as `x`. Sending a `u32` makes
/// every launcher that type-checks the variant ignore the update silently.
#[test]
fn the_unity_count_is_a_signed_64_bit_integer() {
    let props = unity_properties(u32::MAX);
    assert_eq!(props[0], ("count", UnityValue::Int64(4_294_967_295)));
}

/// The Unity app URI must name the desktop file, not a path.
///
/// A launcher matches this against the desktop file of the window it is
/// showing. `application:///usr/share/applications/vitrum.desktop` matches
/// nothing.
#[test]
fn the_unity_app_uri_is_the_desktop_file_name() {
    assert_eq!(unity_app_uri(), "application://vitrum.desktop");
}

/// The D-Bus names are a wire contract with every launcher that implements the
/// protocol, so pin them.
#[test]
fn the_unity_dbus_names_are_stable() {
    assert_eq!(UNITY_INTERFACE, "com.canonical.Unity.LauncherEntry");
    assert_eq!(UNITY_OBJECT_PATH, "/com/canonical/Unity/LauncherEntry");
    assert_eq!(UNITY_BUS_NAME, "com.canonical.Unity");
}

/// A registered main window must fill in for a caller that has none.
///
/// The count is published from a process-wide place with no window in hand, so
/// it passes `None`. Before the fallback existed that made the Windows overlay
/// permanently unconstructible: every badge call refused, whatever the window
/// had done. An explicit handle still wins, so a second window can badge its
/// own taskbar button.
#[test]
fn a_registered_main_window_fills_in_for_a_caller_with_none() {
    let main = WindowHandle(0x1234);
    let other = WindowHandle(0x5678);
    assert_eq!(resolve_window(None, Some(main)), Some(main));
    assert_eq!(resolve_window(Some(other), Some(main)), Some(other));
    assert_eq!(resolve_window(None, None), None);
}

/// Registration must be readable back, and zero must never read as a handle.
///
/// The slot is an atomic `u64` with zero meaning "unregistered", so a handle
/// that round-tripped as `Some(WindowHandle(0))` would be an HWND the badge
/// then hands to Win32.
#[test]
fn registering_a_window_makes_it_readable_and_zero_is_never_a_handle() {
    register_main_window(WindowHandle(0xabcd));
    assert_eq!(main_window(), Some(WindowHandle(0xabcd)));
    register_main_window(WindowHandle(0));
    assert_eq!(main_window(), None);
}
