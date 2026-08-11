//! Capability reporting: the promise that nothing here lies about working.

use crate::capability::{
    CapabilityReport, Feature, PlatformSupport, Support, Unavailable, UnavailableKind,
    platform_support,
};
use crate::paths::Platform;

/// The two failure classes must be distinguishable.
///
/// "macOS has no taskbar overlay" is permanent and the control should be
/// removed; "your desktop has no notification daemon running" is transient and
/// the control should be shown disabled with the reason. Collapsing them into
/// one boolean makes both pieces of UI impossible.
#[test]
fn permanent_and_transient_failures_are_distinguishable() {
    assert!(!UnavailableKind::NotImplementedOnPlatform.is_transient());
    assert!(!UnavailableKind::PermissionDenied.is_transient());
    assert!(UnavailableKind::ServiceMissing.is_transient());
    assert!(UnavailableKind::RuntimeError.is_transient());
}

/// The kind tokens are stable, because they appear in logs and reports.
#[test]
fn the_kind_tokens_are_stable() {
    assert_eq!(UnavailableKind::NotImplementedOnPlatform.as_str(), "not-implemented-on-platform");
    assert_eq!(UnavailableKind::ServiceMissing.as_str(), "service-missing");
    assert_eq!(UnavailableKind::PermissionDenied.as_str(), "permission-denied");
    assert_eq!(UnavailableKind::RuntimeError.as_str(), "runtime-error");
}

/// An unavailable capability must render its kind and its reason.
///
/// A log line reading "unavailable" with no reason is why nobody can debug a
/// missing tray icon.
#[test]
fn an_unavailable_capability_renders_its_reason() {
    let u = Unavailable::service_missing("nothing owns org.kde.StatusNotifierWatcher");
    assert_eq!(u.to_string(), "service-missing: nothing owns org.kde.StatusNotifierWatcher");
    assert_eq!(
        Support::Missing(u).to_string(),
        "unavailable (service-missing: nothing owns org.kde.StatusNotifierWatcher)"
    );
    assert_eq!(Support::Available.to_string(), "available");
}

/// `Support` must expose the reason only when there is one.
#[test]
fn the_reason_is_available_only_when_missing() {
    assert!(Support::Available.is_available());
    assert_eq!(Support::Available.reason(), None);

    let missing = Support::Missing(Unavailable::runtime_error("boom"));
    assert!(!missing.is_available());
    assert_eq!(missing.reason().map(|u| u.kind), Some(UnavailableKind::RuntimeError));
}

/// Converting a result must not turn an error into a success.
///
/// `Support::from_result` is used by every backend's `capability`. If it
/// swallowed the error the whole reporting layer would be decorative.
#[test]
fn converting_a_failed_probe_keeps_the_failure() {
    let ok: Result<u32, Unavailable> = Ok(1);
    assert_eq!(Support::from_result(ok), Support::Available);

    let err: Result<u32, Unavailable> =
        Err(Unavailable::permission_denied("notification authorisation denied"));
    let support = Support::from_result(err);
    assert!(!support.is_available());
    assert_eq!(support.reason().map(|u| u.kind), Some(UnavailableKind::PermissionDenied));
    assert_eq!(
        support.reason().map(|u| u.detail.as_str()),
        Some("notification authorisation denied")
    );
}

/// A report must cover every feature and be queryable by feature.
#[test]
fn a_report_covers_every_feature() {
    let report = CapabilityReport::new(
        Feature::ALL.iter().map(|f| (*f, Support::Available)).collect(),
    );
    assert_eq!(Feature::ALL.len(), 8);
    for feature in Feature::ALL {
        assert_eq!(report.get(feature), Some(&Support::Available), "{feature} missing");
    }
    assert_eq!(report.iter().count(), 8);
    assert_eq!(report.unavailable().count(), 0);
}

/// The unavailable iterator must yield exactly the failing features.
#[test]
fn the_unavailable_iterator_yields_only_failures() {
    let entries = Feature::ALL
        .iter()
        .map(|f| {
            let support = if *f == Feature::Badge {
                Support::Missing(Unavailable::not_implemented("no dock here"))
            } else {
                Support::Available
            };
            (*f, support)
        })
        .collect();
    let report = CapabilityReport::new(entries);
    let failures: Vec<_> = report.unavailable().map(|(f, u)| (f, u.detail.clone())).collect();
    assert_eq!(failures, vec![(Feature::Badge, "no dock here".to_string())]);
}

/// Feature tokens are stable, because a report is something an operator pastes
/// into a bug.
#[test]
fn feature_tokens_are_stable() {
    let tokens: Vec<&str> = Feature::ALL.iter().map(|f| f.as_str()).collect();
    assert_eq!(
        tokens,
        vec![
            "notifications",
            "badge",
            "tray",
            "single-instance",
            "theme",
            "window-state",
            "deep-links",
            "paths",
        ]
    );
}

/// A rendered report must be one line per feature, naming both.
#[test]
fn a_rendered_report_names_every_feature_and_its_state() {
    let report = CapabilityReport::new(
        Feature::ALL
            .iter()
            .map(|f| {
                let support = if *f == Feature::Tray {
                    Support::Missing(Unavailable::service_missing("no watcher"))
                } else {
                    Support::Available
                };
                (*f, support)
            })
            .collect(),
    );
    let text = report.to_string();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 8);
    assert_eq!(lines[0], "notifications    available");
    assert_eq!(lines[2], "tray             unavailable (service-missing: no watcher)");
}

/// The live probe must answer for every feature, and every failure must carry a
/// usable reason.
///
/// This is the acceptance criterion in one test: on a machine where a service
/// is missing, the probe must report it as missing with a sentence, not claim
/// success. It runs against whatever this machine actually has.
#[test]
fn the_live_probe_answers_for_every_feature_with_a_real_reason() {
    let report = crate::probe(None);
    for feature in Feature::ALL {
        let support = report.get(feature).unwrap_or_else(|| panic!("{feature} was not probed"));
        if let Some(reason) = support.reason() {
            assert!(
                !reason.detail.is_empty(),
                "{feature} is unavailable with an empty reason, which is useless to a caller"
            );
            assert!(
                reason.detail.len() > 15,
                "{feature} reason is too terse to act on: {}",
                reason.detail
            );
        }
    }
}

/// Without a window handle the badge must be reported as unavailable on
/// Windows and available on the platforms whose badge is process-wide.
///
/// This pins the honest answer to "can I show a badge right now" rather than a
/// blanket yes.
#[test]
fn the_badge_probe_respects_the_missing_window_handle() {
    let report = crate::probe(None);
    let badge = report.get(Feature::Badge).expect("badge is probed");
    if cfg!(target_os = "windows") {
        assert!(
            !badge.is_available(),
            "the Windows taskbar overlay needs an HWND and none was supplied"
        );
    }
    // On Linux and macOS the answer depends on the live desktop, so the only
    // universal claim is that a negative answer explains itself, which the
    // previous test asserts.
}

/// Every feature on every platform has a recorded decision, and every refusal
/// says what is missing and what to do about it.
///
/// The matrix is walked at run time from `Feature::ALL` and `Platform::ALL`
/// rather than written out here. `Platform::ALL` is generated with the enum and
/// the table behind `platform_support` is an exhaustive match with no wildcard,
/// so adding a platform or a feature fails to build until a decision exists,
/// and this test then fails until that decision is a usable sentence.
///
/// What it does not catch: a decision that is recorded and wrong. Only the
/// live probe on that platform can catch that.
#[test]
fn every_platform_and_feature_pair_has_an_actionable_decision() {
    let mut refusals = 0;
    for platform in Platform::ALL {
        for feature in Feature::ALL {
            let decision = platform_support(feature, *platform);
            let Some(detail) = decision.reason() else {
                assert!(decision.is_implemented(), "{feature} on {platform}: neither answer");
                continue;
            };
            refusals += 1;
            assert!(
                detail.len() > 40,
                "{feature} on {platform}: refusal is too short to act on: {detail}"
            );
            assert!(
                detail.contains("Use ") || detail.contains("Install ") || detail.contains("install"),
                "{feature} on {platform}: refusal names no corrective action: {detail}"
            );
            assert_eq!(
                decision.to_support().reason().map(|u| u.kind),
                Some(UnavailableKind::NotImplementedOnPlatform),
                "{feature} on {platform}: a missing backend is permanent, not transient"
            );
        }
    }
    assert!(refusals > 0, "the matrix records no refusal at all, so nothing was proven");
}

/// A recorded refusal must reach the operator through `probe`, not be swallowed
/// by a backend that reports success it did not achieve.
///
/// The live probe is the only place the two halves meet: the table says a
/// platform has no backend, and the report for a build on that platform has to
/// say the same thing with the same words.
#[test]
fn a_recorded_refusal_is_what_the_live_probe_reports() {
    let here = Platform::current();
    let report = crate::probe(None);
    for feature in Feature::ALL {
        let PlatformSupport::Unimplemented(detail) = platform_support(feature, here) else {
            continue;
        };
        let support = report.get(feature).unwrap_or_else(|| panic!("{feature} was not probed"));
        let reason = support
            .reason()
            .unwrap_or_else(|| panic!("{feature} has no backend here but the probe says it works"));
        assert_eq!(reason.detail, detail, "{feature}: the probe invented its own reason");
    }
}

/// Platform tokens are stable and unique, because they key the decision matrix
/// and appear in every report an operator pastes.
#[test]
fn platform_tokens_are_stable_and_unique() {
    let tokens: Vec<&str> = Platform::ALL.iter().map(|p| p.as_str()).collect();
    assert_eq!(tokens, vec!["linux", "macos", "windows"]);
    let mut sorted = tokens.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), tokens.len(), "two platforms share a token");
}
