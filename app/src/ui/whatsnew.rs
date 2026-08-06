//! What changed, once, after an update.
//!
//! `vitrum update` replaces the binary in place, so the next launch is a
//! different product with no announcement. The changelog is already written
//! and already correct, and it ships inside the binary, so the honest version
//! of a release note is the file itself narrowed to the releases this
//! operator has not seen.
//!
//! # Why it is a parser and not a string
//!
//! Everything on the surface comes from [`releases_since`], which takes the
//! changelog text and two versions and returns data. No version is written
//! into the copy, and the file is the single source: a release documented in
//! `CHANGELOG.md` cannot fail to appear here, and one that is not documented
//! cannot be invented here.
//!
//! # The four cases that must show nothing
//!
//! All of them are silence, and none of them is an error:
//!
//! - **Nothing new.** Last seen equals the running version.
//! - **First ever run.** No last-seen version at all. A changelog is not a
//!   welcome; [`crate::ui::onboarding`] owns that launch, and stacking a
//!   release note on top of it is noise on the one surface that can least
//!   afford it.
//! - **A downgrade.** Last seen is newer than what is running, which is what
//!   an operator sees after rolling back. Reading them the notes for the
//!   version they just left would be backwards.
//! - **An unreadable changelog.** A malformed heading, a missing file, an
//!   empty string: every one of them parses to no releases rather than
//!   panicking. This is decoration, and decoration does not take the window
//!   down with it.

use dioxus::prelude::*;
use semver::Version;

/// The changelog, compiled in.
///
/// Read at build time rather than from disk, because an installed binary has
/// no repository beside it and a release note that depends on a checkout is a
/// release note that only developers ever see.
pub const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

/// One `### Heading` inside a release, with its bullets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// The heading text, or empty for bullets that precede any heading.
    pub heading: String,
    pub entries: Vec<String>,
}

/// One `## v0.1.0 — date` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// Whatever followed the version on the heading line, verbatim. Often a
    /// date, sometimes `unreleased`; empty when the heading carried nothing.
    pub date: String,
    pub groups: Vec<Group>,
}

/// Every release section the changelog contains, in file order.
///
/// A heading whose version is not semver is skipped along with its body, so a
/// hand-edited or half-written entry costs its own section and nothing else.
/// Never fails.
pub fn parse_changelog(text: &str) -> Vec<Release> {
    let mut releases: Vec<Release> = Vec::new();
    let mut skipping = false;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            match parse_heading(rest) {
                Some((version, date)) => {
                    skipping = false;
                    releases.push(Release {
                        version,
                        date,
                        groups: Vec::new(),
                    });
                }
                None => skipping = true,
            }
            continue;
        }

        if skipping {
            continue;
        }
        let Some(release) = releases.last_mut() else {
            continue;
        };

        if let Some(heading) = line.strip_prefix("### ") {
            release.groups.push(Group {
                heading: clean(heading),
                entries: Vec::new(),
            });
            continue;
        }

        if let Some(bullet) = line.strip_prefix("- ") {
            if release.groups.is_empty() {
                release.groups.push(Group {
                    heading: String::new(),
                    entries: Vec::new(),
                });
            }
            let group = release.groups.last_mut().expect("just ensured non-empty");
            group.entries.push(clean(bullet));
            continue;
        }

        // A wrapped bullet: indented, non-empty, and following one. Joined
        // with a space, because the line breaks in the file are hard wrapping
        // for an editor and mean nothing to a rendered sentence.
        if line.starts_with(' ') && !line.trim().is_empty()
            && let Some(entry) = release.groups.last_mut().and_then(|g| g.entries.last_mut()) {
                entry.push(' ');
                entry.push_str(&clean(line.trim()));
            }
    }

    releases
}

/// Split `v0.1.0 — 2026-08-05` into its version and the rest.
///
/// The separator is whatever the file uses. An em dash, an en dash and a
/// hyphen all read the same to a person, so all three are accepted rather
/// than making the surface depend on which one somebody typed.
fn parse_heading(rest: &str) -> Option<(Version, String)> {
    let rest = rest.trim();
    let (head, tail) = match rest.find([' ', '\t']) {
        Some(i) => (&rest[..i], rest[i..].trim()),
        None => (rest, ""),
    };
    let version = Version::parse(head.trim_start_matches('v')).ok()?;
    let date = tail.trim_start_matches(['—', '–', '-']).trim().to_string();
    Some((version, date))
}

/// Drop the markdown emphasis markers a rendered line should not carry.
fn clean(text: &str) -> String {
    text.replace("**", "").trim().to_string()
}

/// The releases worth showing, newest first.
///
/// One rule: strictly newer than `last_seen`, no newer than `current`. That
/// single half-open window is also what makes the silent cases silent. Equal
/// versions select nothing because nothing is both above and at `current`, and
/// a downgrade selects nothing because the window is empty when `last_seen`
/// sits above `current`. An explicit guard for either would be a second rule
/// no input could tell apart from this one.
///
/// A first ever run is the one case that is genuinely separate: absent is not
/// a version, and there is no window to take.
pub fn releases_since(text: &str, last_seen: Option<&Version>, current: &Version) -> Vec<Release> {
    let Some(last_seen) = last_seen else {
        return Vec::new();
    };
    let mut out: Vec<Release> = parse_changelog(text)
        .into_iter()
        .filter(|r| r.version > *last_seen && r.version <= *current)
        .collect();
    out.sort_by(|a, b| b.version.cmp(&a.version));
    out
}

/// [`releases_since`] over the compiled-in changelog and the running version.
pub fn whats_new(last_seen: Option<&Version>) -> Vec<Release> {
    releases_since(CHANGELOG, last_seen, &crate::update::current_version())
}

/// The sheet's title: one version named, or a count.
pub fn title(releases: &[Release]) -> String {
    match releases {
        [] => String::new(),
        [one] => format!("What changed in {}", one.version),
        many => format!("What changed across {} releases", many.len()),
    }
}

// ---------------------------------------------------------------------------
// The component
// ---------------------------------------------------------------------------

#[derive(Props, Clone, PartialEq)]
pub struct WhatsNewProps {
    /// Sections to show, from [`whats_new`]. The caller renders nothing when
    /// this is empty; that decision is data, so it does not need a component.
    pub releases: Vec<Release>,
    /// Dismissed. Recording the version so it does not return is the caller's
    /// job.
    pub on_dismiss: EventHandler<()>,
}

/// The post-update changelog sheet.
#[component]
pub fn WhatsNew(props: WhatsNewProps) -> Element {
    let heading = title(&props.releases);

    rsx! {
        div {
            class: "rg-layer rg-layer--dim",
            onclick: move |_| props.on_dismiss.call(()),
            div {
                class: "rg-sheet rg-sheet--whatsnew",
                role: "dialog",
                aria_label: "What changed",
                onclick: move |e| e.stop_propagation(),

                div { class: "rg-sheet__head",
                    span { class: "rg-sheet__title", "{heading}" }
                }

                div { class: "rg-sheet__body",
                    for release in props.releases.iter() {
                        div { class: "rg-whatsnew__release", key: "{release.version}",
                            div { class: "rg-whatsnew__version",
                                span { class: "rg-whatsnew__number", "{release.version}" }
                                if !release.date.is_empty() {
                                    span { class: "rg-whatsnew__date", "{release.date}" }
                                }
                            }
                            for group in release.groups.iter() {
                                div { class: "rg-whatsnew__group", key: "{group.heading}",
                                    if !group.heading.is_empty() {
                                        span { class: "rg-whatsnew__heading", "{group.heading}" }
                                    }
                                    ul { class: "rg-whatsnew__entries",
                                        for entry in group.entries.iter() {
                                            li { class: "rg-whatsnew__entry", key: "{entry}", "{entry}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                div { class: "rg-sheet__foot",
                    button {
                        class: "rg-btn rg-btn--primary",
                        r#type: "button",
                        onclick: move |_| props.on_dismiss.call(()),
                        "Got it"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREE: &str = "\
# Changelog

Prose that belongs to no release.

## v0.2.0 — 2026-03-01

### Added

- **Workspaces** that are genuinely separate: a new one opens
  onto nothing.
- Cross-session scrollback search.

### Fixed

- A repository cannot forge a sidebar row.

## v0.1.1 — 2026-02-01

- A bullet with no heading above it.

## v0.1.0 — 2026-01-01

### The product

- First public release.
";

    fn v(text: &str) -> Version {
        Version::parse(text).expect("test version is semver")
    }

    /// A release heading yields its version, its date and its groups.
    ///
    /// The defect: treating the changelog as flat text, which loses the
    /// grouping the file spends `###` headings on and turns two unrelated
    /// sections into one run of bullets.
    #[test]
    fn a_release_parses_into_versioned_grouped_entries() {
        let releases = parse_changelog(THREE);
        assert_eq!(
            releases
                .iter()
                .map(|r| r.version.to_string())
                .collect::<Vec<_>>(),
            ["0.2.0", "0.1.1", "0.1.0"]
        );

        let newest = &releases[0];
        assert_eq!(newest.date, "2026-03-01");
        assert_eq!(
            newest.groups,
            vec![
                Group {
                    heading: "Added".to_string(),
                    entries: vec![
                        "Workspaces that are genuinely separate: a new one opens onto nothing."
                            .to_string(),
                        "Cross-session scrollback search.".to_string(),
                    ],
                },
                Group {
                    heading: "Fixed".to_string(),
                    entries: vec!["A repository cannot forge a sidebar row.".to_string()],
                },
            ]
        );
    }

    /// A bullet with no heading above it still reaches the surface.
    ///
    /// The defect: requiring a `###` before recording entries, which silently
    /// drops every bullet in a release that did not bother to group them.
    #[test]
    fn bullets_before_any_heading_are_kept_under_an_unnamed_group() {
        let release = parse_changelog(THREE)
            .into_iter()
            .find(|r| r.version == v("0.1.1"))
            .expect("0.1.1 is in the fixture");
        assert_eq!(
            release.groups,
            vec![Group {
                heading: String::new(),
                entries: vec!["A bullet with no heading above it.".to_string()],
            }]
        );
    }

    /// Prose ahead of the first release never lands inside one.
    ///
    /// The defect: the file's own preamble, which is not a change, appearing
    /// as the first bullet of the newest release.
    #[test]
    fn text_outside_a_release_section_is_not_attributed_to_one() {
        for release in parse_changelog(THREE) {
            for group in release.groups {
                for entry in group.entries {
                    assert!(!entry.contains("belongs to no release"), "{entry}");
                }
            }
        }
    }

    /// The window is exactly `(last_seen, current]`, by semver order.
    ///
    /// The defect: string comparison, which puts `0.10.0` below `0.9.0`, and
    /// an unbounded upper edge, which announces an unreleased section as
    /// something that just landed.
    #[test]
    fn only_releases_strictly_newer_than_last_seen_and_no_newer_than_current() {
        let cases: &[(&str, &str, &[&str])] = &[
            ("0.1.0", "0.1.1", &["0.1.1"]),
            ("0.1.1", "0.2.0", &["0.2.0"]),
            ("0.1.0", "0.2.0", &["0.2.0", "0.1.1"]),
            // Running 0.1.1 with 0.2.0 already documented: the future section
            // is not this binary's news.
            ("0.1.0", "0.1.1", &["0.1.1"]),
        ];
        for (last, current, expected) in cases {
            let got: Vec<String> = releases_since(THREE, Some(&v(last)), &v(current))
                .iter()
                .map(|r| r.version.to_string())
                .collect();
            assert_eq!(got, *expected, "last={last} current={current}");
        }
    }

    /// The four silent cases show nothing and never panic.
    ///
    /// The defect: a dialog that opens on every launch because last seen
    /// equals current, or on the first ever launch on top of onboarding, or
    /// after a rollback with the notes for the version just left, or a panic
    /// on a changelog somebody edited badly.
    #[test]
    fn nothing_new_first_run_downgrade_and_garbage_all_show_nothing() {
        let current = v("0.1.1");
        assert!(releases_since(THREE, Some(&current), &current).is_empty());
        assert!(releases_since(THREE, None, &current).is_empty());
        assert!(releases_since(THREE, Some(&v("0.2.0")), &current).is_empty());

        for text in [
            "",
            "not a changelog at all",
            "## not-a-version\n\n- an entry\n",
            "## \n- an entry\n",
            "### Orphan heading\n- an entry\n",
        ] {
            assert!(
                releases_since(text, Some(&v("0.1.0")), &v("9.9.9")).is_empty(),
                "{text:?}"
            );
        }
    }

    /// A section with an unparseable version is skipped, not merged.
    ///
    /// The defect: falling through to the previous release, so a half-written
    /// heading donates its bullets to the last good one.
    #[test]
    fn an_unparseable_heading_does_not_donate_its_bullets() {
        let text = "## v0.1.0 — 2026-01-01\n\n- real\n\n## nightly\n\n- junk\n";
        let releases = parse_changelog(text);
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].groups[0].entries, ["real"]);
    }

    /// The shipped changelog parses, and its versions are real semver.
    ///
    /// The defect: the compiled-in file drifting into a format this parser
    /// does not read, which turns the whole surface silent with nothing to
    /// notice it.
    #[test]
    fn the_compiled_in_changelog_yields_at_least_one_release() {
        let releases = parse_changelog(CHANGELOG);
        assert!(!releases.is_empty(), "CHANGELOG.md parsed to no releases");
        let newest = &releases[0];
        assert!(
            newest.groups.iter().any(|g| !g.entries.is_empty()),
            "the newest release parsed to no entries"
        );
    }

    /// The title names the one release or counts them.
    ///
    /// The defect: "What changed in 0.2.0" over a sheet that also lists
    /// 0.1.1, which mislabels half its own contents.
    #[test]
    fn the_title_matches_how_many_releases_are_shown() {
        let one = releases_since(THREE, Some(&v("0.1.1")), &v("0.2.0"));
        assert_eq!(title(&one), "What changed in 0.2.0");

        let two = releases_since(THREE, Some(&v("0.1.0")), &v("0.2.0"));
        assert_eq!(title(&two), "What changed across 2 releases");

        assert_eq!(title(&[]), "");
    }
}
