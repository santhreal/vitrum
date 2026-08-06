//! Decoding the three platforms' appearance encodings.
//!
//! Each is a small integer or a string with a non-obvious meaning, and each is
//! the kind of thing that gets inverted once and then stays inverted because
//! the person who wrote it uses dark mode and never noticed.

use crate::theme::{
    NO_PREFERENCE_THEME, Theme, theme_from_apps_use_light_theme,
    theme_from_ns_appearance_name, theme_from_portal_color_scheme,
};

/// The portal's three defined values must map as the specification says.
#[test]
fn the_portal_values_map_as_specified() {
    assert_eq!(theme_from_portal_color_scheme(0), None, "0 is no preference");
    assert_eq!(theme_from_portal_color_scheme(1), Some(Theme::Dark), "1 is prefer dark");
    assert_eq!(theme_from_portal_color_scheme(2), Some(Theme::Light), "2 is prefer light");
}

/// "No preference" must be distinct from light.
///
/// Collapsing 0 into light is the standard bug. It means a user who has never
/// touched the setting gets a white terminal, and it makes a
/// "follow the system" toggle impossible to render honestly because the app
/// cannot tell "the user chose light" from "the user chose nothing".
#[test]
fn no_preference_is_not_light() {
    assert_ne!(theme_from_portal_color_scheme(0), Some(Theme::Light));
    assert_eq!(theme_from_portal_color_scheme(0).unwrap_or(NO_PREFERENCE_THEME), Theme::Dark);
}

/// A value the portal has not defined yet must be treated as no preference.
///
/// The portal is versioned and a future value must not be guessed at; guessing
/// "not 1, therefore light" would turn a new high-contrast mode into a white
/// terminal.
#[test]
fn an_unknown_portal_value_is_no_preference() {
    assert_eq!(theme_from_portal_color_scheme(3), None);
    assert_eq!(theme_from_portal_color_scheme(u32::MAX), None);
}

/// The Windows registry polarity must be right: zero means dark.
///
/// `AppsUseLightTheme = 0` is dark mode. The name reads like a light-mode flag
/// and the value reads like a boolean, so getting this backwards produces an
/// app that is light exactly when the system is dark.
#[test]
fn zero_apps_use_light_theme_means_dark() {
    assert_eq!(theme_from_apps_use_light_theme(0), Theme::Dark);
    assert_eq!(theme_from_apps_use_light_theme(1), Theme::Light);
    // Any nonzero is light; Windows only ever writes 0 or 1 but the type is a
    // DWORD.
    assert_eq!(theme_from_apps_use_light_theme(2), Theme::Light);
    assert_eq!(theme_from_apps_use_light_theme(u32::MAX), Theme::Light);
}

/// The two `NSAppearance` names Apple documents must decode.
#[test]
fn the_documented_appearance_names_decode() {
    assert_eq!(theme_from_ns_appearance_name("NSAppearanceNameAqua"), Theme::Light);
    assert_eq!(theme_from_ns_appearance_name("NSAppearanceNameDarkAqua"), Theme::Dark);
}

/// The accessibility and vibrant variants must decode too.
///
/// An exact-match list would report light for a user in high-contrast dark
/// mode, which is the population least able to read a mis-themed window.
#[test]
fn the_accessibility_and_vibrant_variants_decode() {
    assert_eq!(
        theme_from_ns_appearance_name("NSAppearanceNameAccessibilityHighContrastDarkAqua"),
        Theme::Dark
    );
    assert_eq!(
        theme_from_ns_appearance_name("NSAppearanceNameAccessibilityHighContrastAqua"),
        Theme::Light
    );
    assert_eq!(theme_from_ns_appearance_name("NSAppearanceNameVibrantDark"), Theme::Dark);
    assert_eq!(theme_from_ns_appearance_name("NSAppearanceNameVibrantLight"), Theme::Light);
}

/// An unrecognised appearance name must fall to light, not panic.
///
/// Apple adds names; the app must keep drawing.
#[test]
fn an_unknown_appearance_name_falls_to_light() {
    assert_eq!(theme_from_ns_appearance_name(""), Theme::Light);
    assert_eq!(theme_from_ns_appearance_name("NSAppearanceNameSomethingNew"), Theme::Light);
}

/// The theme tokens are used in CSS class names and logs, so pin them.
#[test]
fn the_theme_tokens_are_stable() {
    assert_eq!(Theme::Light.as_str(), "light");
    assert_eq!(Theme::Dark.as_str(), "dark");
    assert_eq!(Theme::Dark.to_string(), "dark");
}

/// An absent portal must be reported as a missing service, including when the
/// bus discovers it is absent by failing to start it.
///
/// WHY: CI has a session bus and no portal. D-Bus treats the destination as
/// activatable, tries to start it, and answers `TimedOut` after
/// `service_start_timeout`, which defaults to 120 seconds. That was classified
/// as a runtime error, so a machine with no portal reported "the portal is
/// broken" instead of "there is no portal", after waiting four minutes to say
/// it. The exact message below is the one the failing run produced.
#[cfg(target_os = "linux")]
#[test]
fn a_portal_that_never_starts_is_missing_rather_than_broken() {
    use crate::theme::linux::names_an_absent_service as absent;

    let activation_timeout = "Failed to activate service 'org.freedesktop.portal.Desktop': \
                              timed out (service_start_timeout=120000ms)";
    assert!(
        absent("org.freedesktop.DBus.Error.TimedOut", Some(activation_timeout)),
        "an activation timeout is the service being absent"
    );
    assert!(absent("org.freedesktop.DBus.Error.NoReply", Some(activation_timeout)));
    assert!(absent("org.freedesktop.DBus.Error.ServiceUnknown", None));
    assert!(absent("org.freedesktop.DBus.Error.NameHasNoOwner", None));
    assert!(absent("org.freedesktop.DBus.Error.ServiceStartFailed", None));
    assert!(absent("org.freedesktop.DBus.Error.Spawn.ExecFailed", None));

    // A portal that exists and hung is a runtime error, and must stay one:
    // telling the operator to install a portal they already have is worse than
    // saying nothing. The distinction is whether the bus was activating.
    assert!(
        !absent("org.freedesktop.DBus.Error.TimedOut", Some("Message recipient disconnected")),
        "a timeout that does not name activation is a live portal hanging"
    );
    assert!(!absent("org.freedesktop.DBus.Error.TimedOut", None));
    assert!(!absent("org.freedesktop.DBus.Error.AccessDenied", None));
    assert!(!absent("org.freedesktop.DBus.Error.InvalidArgs", None));
}
