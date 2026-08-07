//! The titlebar only speaks when a ready release is not already dismissed.

use semver::Version;

use super::{Available, Status, chrome_offer};

fn ready(version: &str) -> Status {
    let version = Version::parse(version).unwrap();
    Status::Ready(Available {
        version: version.clone(),
        tag: format!("v{version}"),
        asset_url: Some("https://example.invalid/a".into()),
        sums_url: Some("https://example.invalid/s".into()),
    })
}

#[test]
fn a_ready_release_becomes_chrome() {
    let status = ready("9.9.9");
    let offer = chrome_offer(&status, "").expect("ready must surface");
    assert_eq!(offer.version.to_string(), "9.9.9");
}

#[test]
fn a_dismissed_version_stays_quiet() {
    let status = ready("9.9.9");
    assert!(chrome_offer(&status, "9.9.9").is_none());
}

#[test]
fn a_newer_release_after_a_dismissal_surfaces_again() {
    let status = ready("9.9.10");
    let offer = chrome_offer(&status, "9.9.9").expect("a later version must surface");
    assert_eq!(offer.version.to_string(), "9.9.10");
}

#[test]
fn up_to_date_is_not_chrome() {
    let status = Status::UpToDate {
        version: Version::parse("0.1.0").unwrap(),
    };
    assert!(chrome_offer(&status, "").is_none());
}

#[test]
fn missing_platform_asset_is_not_chrome() {
    let status = Status::NoAssetForPlatform {
        version: Version::parse("9.9.9").unwrap(),
        target: "x86_64-unknown-linux-gnu".into(),
    };
    assert!(chrome_offer(&status, "").is_none());
}

#[test]
fn no_releases_is_not_chrome() {
    assert!(chrome_offer(&Status::NoReleases, "").is_none());
}
