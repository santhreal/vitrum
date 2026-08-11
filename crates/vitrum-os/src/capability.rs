//! Honest capability reporting.
//!
//! Every desktop integration in this crate can fail in two structurally
//! different ways, and collapsing them is how a product ends up lying to its
//! user. "This OS has no such concept" is permanent and the UI should stop
//! offering the feature. "The service that provides it is not running right
//! now" is transient and the UI should say so and retry. A single boolean, or
//! worse a silent `Ok(())` from a no-op, gives the caller neither.
//!
//! So no backend in this crate returns success it did not achieve. A call that
//! cannot do the thing returns [`Unavailable`] carrying a [`UnavailableKind`]
//! and a human-readable detail naming the exact missing piece.

use core::fmt;

use crate::paths::Platform;

/// Why an integration is not usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnavailableKind {
    /// The platform has no equivalent of this feature, or this build was not
    /// compiled with a backend for it. Permanent for this binary on this OS.
    NotImplementedOnPlatform,
    /// The platform supports it but the providing service is absent: no D-Bus
    /// session bus, no notification daemon, no StatusNotifierWatcher, no
    /// desktop portal.
    ServiceMissing,
    /// The service exists and refused. macOS notification authorisation
    /// denied, a registry hive that is read-only, a lock file owned by another
    /// user.
    PermissionDenied,
    /// The service exists, accepted the call, and failed.
    RuntimeError,
}

impl UnavailableKind {
    /// Stable machine token, used in reports and tests.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotImplementedOnPlatform => "not-implemented-on-platform",
            Self::ServiceMissing => "service-missing",
            Self::PermissionDenied => "permission-denied",
            Self::RuntimeError => "runtime-error",
        }
    }

    /// True when retrying later could plausibly succeed.
    ///
    /// A UI uses this to decide between hiding a control forever and showing it
    /// disabled with a reason.
    pub const fn is_transient(self) -> bool {
        matches!(self, Self::ServiceMissing | Self::RuntimeError)
    }
}

impl fmt::Display for UnavailableKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad`, not `write_str`: a report column uses `{:<16}` and
        // `write_str` silently discards the width.
        f.pad(self.as_str())
    }
}

/// A feature that is not usable, and precisely why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    /// Which failure class this is, so a caller can choose between retrying
    /// later and hiding the control for good.
    pub kind: UnavailableKind,
    /// What is missing, in words a maintainer can act on. Never empty.
    pub detail: String,
}

impl Unavailable {
    /// Pair a failure class with the detail naming the exact missing piece.
    pub fn new(kind: UnavailableKind, detail: impl Into<String>) -> Self {
        Self { kind, detail: detail.into() }
    }

    /// No backend exists for this OS in this build.
    pub fn not_implemented(detail: impl Into<String>) -> Self {
        Self::new(UnavailableKind::NotImplementedOnPlatform, detail)
    }

    /// The backend exists but the desktop service it needs is not there.
    pub fn service_missing(detail: impl Into<String>) -> Self {
        Self::new(UnavailableKind::ServiceMissing, detail)
    }

    /// The service refused.
    pub fn permission_denied(detail: impl Into<String>) -> Self {
        Self::new(UnavailableKind::PermissionDenied, detail)
    }

    /// The service accepted and then failed.
    pub fn runtime_error(detail: impl Into<String>) -> Self {
        Self::new(UnavailableKind::RuntimeError, detail)
    }
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.detail)
    }
}

impl core::error::Error for Unavailable {}

/// Result of asking a backend whether it can work right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    /// Verified usable: the service answered.
    Available,
    /// Not usable, with the reason.
    Missing(Unavailable),
}

impl Support {
    /// True only when a probe actually reached the service, never merely
    /// because no error was raised.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    /// The reason, when unavailable.
    pub fn reason(&self) -> Option<&Unavailable> {
        match self {
            Self::Available => None,
            Self::Missing(u) => Some(u),
        }
    }

    /// Convenience for backends whose probe is a fallible call.
    pub fn from_result<T>(r: Result<T, Unavailable>) -> Self {
        match r {
            Ok(_) => Self::Available,
            Err(u) => Self::Missing(u),
        }
    }
}

impl fmt::Display for Support {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Available => f.write_str("available"),
            Self::Missing(u) => write!(f, "unavailable ({u})"),
        }
    }
}

/// The eight integrations this crate provides, as a stable enumeration so a
/// report can be iterated rather than hand-listed at every call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feature {
    /// Desktop notifications for session state changes.
    Notifications,
    /// The attention count drawn on the dock, taskbar or launcher icon.
    Badge,
    /// The status area icon and its menu.
    Tray,
    /// The cross-process claim that stops a second launch opening a second
    /// window, and the handoff that gives the first one the new arguments.
    SingleInstance,
    /// Reading the OS light/dark preference and subscribing to changes.
    Theme,
    /// Persisting window geometry between runs and clamping it back onto
    /// whatever monitors exist at the next launch.
    WindowState,
    /// Handling `vitrum://` URLs opened from outside the application.
    DeepLinks,
    /// Resolving the per-user config, data, cache and runtime directories.
    Paths,
}

impl Feature {
    /// Every feature, in report order.
    pub const ALL: [Feature; 8] = [
        Feature::Notifications,
        Feature::Badge,
        Feature::Tray,
        Feature::SingleInstance,
        Feature::Theme,
        Feature::WindowState,
        Feature::DeepLinks,
        Feature::Paths,
    ];

    /// Stable machine token, used in reports and tests.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Notifications => "notifications",
            Self::Badge => "badge",
            Self::Tray => "tray",
            Self::SingleInstance => "single-instance",
            Self::Theme => "theme",
            Self::WindowState => "window-state",
            Self::DeepLinks => "deep-links",
            Self::Paths => "paths",
        }
    }
}

impl fmt::Display for Feature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad`, not `write_str`: a report column uses `{:<16}` and
        // `write_str` silently discards the width.
        f.pad(self.as_str())
    }
}

/// What a build for one platform can do with one feature, before the running
/// machine is consulted at all.
///
/// [`Support`] answers "can this machine do it right now"; this answers "does a
/// backend for it exist on that platform at all". The two are different
/// questions and only the second can be answered for a platform you are not
/// running on, which is what makes the Windows and macOS arms reviewable from a
/// Linux box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformSupport {
    /// A backend is compiled in for this platform.
    Implemented,
    /// No backend exists. The detail names the missing capability and the
    /// corrective action, and is what [`crate::probe`] reports verbatim.
    Unimplemented(&'static str),
}

impl PlatformSupport {
    /// True when a backend exists.
    pub const fn is_implemented(self) -> bool {
        matches!(self, Self::Implemented)
    }

    /// The recorded reason, when there is no backend.
    pub const fn reason(self) -> Option<&'static str> {
        match self {
            Self::Implemented => None,
            Self::Unimplemented(detail) => Some(detail),
        }
    }

    /// The same answer as a [`Support`], for a caller building a report.
    pub fn to_support(self) -> Support {
        match self {
            Self::Implemented => Support::Available,
            Self::Unimplemented(detail) => Support::Missing(Unavailable::not_implemented(detail)),
        }
    }
}

/// The recorded decision for every feature on every platform.
///
/// One exhaustive match, no wildcard arm. Adding a [`Feature`] or a
/// [`Platform`] fails to compile until a decision is written here, which is the
/// only mechanism that stops a platform arm shipping with nobody having looked
/// at it.
#[must_use]
pub const fn platform_support(feature: Feature, platform: Platform) -> PlatformSupport {
    use Feature as F;
    use Platform as P;
    use PlatformSupport::{Implemented, Unimplemented};

    match (feature, platform) {
        (F::Notifications, P::Linux | P::MacOs | P::Windows) => Implemented,
        (F::Badge, P::Linux | P::MacOs | P::Windows) => Implemented,
        (F::Tray, P::Linux | P::MacOs | P::Windows) => Implemented,
        (F::SingleInstance, P::Linux | P::MacOs | P::Windows) => Implemented,
        (F::Theme, P::Linux | P::MacOs | P::Windows) => Implemented,
        (F::WindowState, P::Linux | P::MacOs | P::Windows) => Implemented,
        (F::DeepLinks, P::Linux | P::Windows) => Implemented,
        (F::DeepLinks, P::MacOs) => Unimplemented(
            "macOS resolves URL schemes from CFBundleURLTypes in the app bundle's Info.plist at \
             install time; there is no runtime registration. Use \
             deeplink::plan_registration(Platform::MacOs, ..) to get the fragment and the \
             lsregister step.",
        ),
        (F::Paths, P::Linux | P::MacOs | P::Windows) => Implemented,
    }
}

/// What this machine can actually do, feature by feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityReport {
    entries: Vec<(Feature, Support)>,
}

impl CapabilityReport {
    /// Build from a full set of probes. Panics in debug if a feature is
    /// missing, because a partial report is exactly the silent gap this type
    /// exists to prevent.
    pub fn new(entries: Vec<(Feature, Support)>) -> Self {
        debug_assert_eq!(
            entries.len(),
            Feature::ALL.len(),
            "capability report must cover every feature"
        );
        Self { entries }
    }

    /// Support for one feature, or `None` when this report does not cover it.
    pub fn get(&self, feature: Feature) -> Option<&Support> {
        self.entries.iter().find(|(f, _)| *f == feature).map(|(_, s)| s)
    }

    /// Every entry, in [`Feature::ALL`] order.
    pub fn iter(&self) -> impl Iterator<Item = (Feature, &Support)> {
        self.entries.iter().map(|(f, s)| (*f, s))
    }

    /// Features that are not usable right now.
    pub fn unavailable(&self) -> impl Iterator<Item = (Feature, &Unavailable)> {
        self.entries.iter().filter_map(|(f, s)| s.reason().map(|u| (*f, u)))
    }
}

impl fmt::Display for CapabilityReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (feature, support) in &self.entries {
            writeln!(f, "{feature:<16} {support}")?;
        }
        Ok(())
    }
}
