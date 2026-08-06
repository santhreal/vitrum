//! Capability reporting: the promise that nothing here lies about working.

use crate::capability::{
    CapabilityReport, Feature, Support, Unavailable, UnavailableKind,
};

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
