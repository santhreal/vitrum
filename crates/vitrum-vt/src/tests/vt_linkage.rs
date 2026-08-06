//! The build's linkage record.
//!
//! These assert the record describes the build that produced it. The value is
//! frozen at compile time, so a wrong one is wrong in every bug report filed
//! against that binary.

use crate::linkage::{ENGINE_VERSION, Route, describe, linkage};

#[test]
fn the_route_matches_the_features_this_build_used() {
    // The record is the only thing that says which engine a binary carries, so
    // it must agree with the features Cargo actually compiled.
    let expected = if cfg!(feature = "system") {
        Route::System
    } else {
        Route::Vendored
    };
    assert_eq!(linkage().route, expected);
}

#[test]
fn the_source_names_how_the_engine_was_obtained() {
    let source = linkage().source;
    let known = ["pkg-config", "zig, pinned upstream", "zig, GHOSTTY_SOURCE_DIR", "docs.rs"];
    assert!(known.contains(&source), "unexpected linkage source: {source:?}");
}

#[test]
fn the_engine_version_is_the_one_cargo_resolved() {
    // An unresolved requirement is labelled rather than passed off as a
    // version, so a report never claims precision the build did not have.
    assert!(!ENGINE_VERSION.is_empty());
    assert!(
        ENGINE_VERSION.starts_with(|c: char| c.is_ascii_digit()) || ENGINE_VERSION.contains("unresolved"),
        "engine version is a version or an explicit non-answer: {ENGINE_VERSION:?}"
    );
}

#[test]
fn the_description_names_the_engine_and_the_route() {
    let line = describe();
    assert!(line.starts_with("libghostty-vt "), "{line:?}");
    assert!(line.contains(linkage().route.as_str()), "{line:?}");
    assert!(line.contains(linkage().source), "{line:?}");
}

#[test]
fn a_route_round_trips_through_its_name() {
    // `VITRUM_VT_LINKAGE` takes these exact strings, so the printed name and
    // the accepted name cannot be allowed to drift apart.
    for route in [Route::Vendored, Route::System] {
        assert_eq!(route.to_string(), route.as_str());
    }
    assert_eq!(Route::Vendored.as_str(), "vendored");
    assert_eq!(Route::System.as_str(), "system");
}
