//! How this binary got its terminal engine.
//!
//! The decision is made once, in `build.rs`, from a per-platform table. This
//! module is the read side: it turns that decision into a value the app can put
//! in `--version` output, a diagnostics pane, or a bug report.
//!
//! That matters because the two routes produce different binaries. A vendored
//! build carries a known engine commit; a system build tracks whatever the
//! machine has installed, and a bug report that does not say which one is being
//! run is missing the first thing anyone would ask.

/// How libghostty was obtained for this build.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Route {
    /// Built from Ghostty source with Zig, pinned by the engine crate.
    Vendored,
    /// Linked against a libghostty the platform provides.
    System,
}

impl Route {
    /// The lowercase name used by the `VITRUM_VT_LINKAGE` build override.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vendored => "vendored",
            Self::System => "system",
        }
    }
}

impl core::fmt::Display for Route {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the build decided, frozen into the binary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Linkage {
    /// Which route was taken.
    pub route: Route,
    /// Where the engine came from: `pkg-config`, `zig, pinned upstream`, or
    /// `zig, GHOSTTY_SOURCE_DIR` for a local Ghostty checkout.
    pub source: &'static str,
}

/// The route this binary was built with.
///
/// # Panics
///
/// Never at runtime. The value is a compile-time constant written by
/// `build.rs`, and an unrecognised one fails the build rather than reaching
/// here.
#[must_use]
pub const fn linkage() -> Linkage {
    let source = env!("VITRUM_VT_LINKAGE_SOURCE");
    // `match` on a string is not const, and `const_str_eq` is not stable, so
    // the route is recovered from its first byte, which differs between the
    // only two values `build.rs` ever emits.
    let route = match env!("VITRUM_VT_LINKAGE_ROUTE").as_bytes()[0] {
        b's' => Route::System,
        _ => Route::Vendored,
    };
    Linkage { route, source }
}

/// One line naming the engine and how it got here, for `--version` output.
///
/// ```
/// let line = vitrum_vt::linkage::describe();
/// assert!(line.starts_with("libghostty-vt "));
/// ```
#[must_use]
pub fn describe() -> String {
    let Linkage { route, source } = linkage();
    format!("libghostty-vt {ENGINE_VERSION} ({route}, {source})")
}

/// Version of the engine binding this build links.
///
/// Read from the dependency rather than written by hand, so it cannot drift
/// from what Cargo actually resolved.
pub const ENGINE_VERSION: &str = env!("VITRUM_VT_ENGINE_VERSION");
