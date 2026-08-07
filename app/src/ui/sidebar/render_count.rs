//! Counts [`super::SessionRow`] body executions, for the memoization guard.
//!
//! Whether a row was rebuilt or skipped is invisible in the rendered HTML —
//! both produce the same bytes — so it is the one thing about the sidebar's
//! cost that no other guard in this crate can observe. This counts it.
//!
//! Lives in its own file for a blunt reason: `sidebar::tests::shipped_markup`
//! reads `sidebar.rs` as text and cuts it at the first `#[cfg(test)]`, so a
//! conditionally-compiled counter written inline above `SessionRow` would
//! truncate the markup that several ordering guards scan and break them.
//! `tick` is therefore unconditional at the call site and empty outside tests.
//!
//! The count is thread-local, not a process-global atomic. `cargo test` runs
//! test functions on their own threads, and three other modules in this
//! directory render session rows of their own; a shared counter saw all of
//! them at once and made the guard fail at random depending on scheduling.

#[cfg(test)]
thread_local! {
    static RENDERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Record that one row body ran.
///
/// Compiles to nothing in a release build: an empty `#[inline]` function with
/// no body left after the `cfg`.
#[inline]
pub(crate) fn tick() {
    #[cfg(test)]
    RENDERS.with(|c| c.set(c.get() + 1));
}

/// Rows counted on this thread since the last call, resetting the count.
#[cfg(test)]
pub(crate) fn take() -> usize {
    RENDERS.with(|c| c.replace(0))
}
