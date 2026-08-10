//! Home-relative rewriting and component elision, including the Windows-shaped
//! paths a daemon on another platform can hand us.

use crate::path::{Place, base_name, home_relative, shorten, shorten_home_relative, under};
use crate::text::display_width;

/// A path under the home directory is rewritten with `~`.
///
/// The most common single saving in the whole sidebar: `/home/mk` is eight
/// columns that every row would otherwise repeat.
#[test]
fn a_path_under_home_is_rewritten() {
    assert_eq!(home_relative("/home/mk/src/foo", "/home/mk"), "~/src/foo");
    assert_eq!(home_relative("/home/mk/a", "/home/mk"), "~/a");
    assert_eq!(
        home_relative("/Users/mk/src/vitrum", "/Users/mk"),
        "~/src/vitrum"
    );
}

/// The home directory itself is `~`, not `~/` and not empty.
///
/// An empty label for a project rooted at home would look like a failed lookup.
#[test]
fn the_home_directory_itself_is_a_tilde() {
    assert_eq!(home_relative("/home/mk", "/home/mk"), "~");
}

/// A trailing separator on the home directory is ignored.
///
/// `HOME` is allowed to end in a slash and some shells export it that way. If
/// it were compared literally, the separator would be counted twice and no path
/// would ever match.
#[test]
fn a_trailing_separator_on_home_is_ignored() {
    assert_eq!(home_relative("/home/mk/src", "/home/mk/"), "~/src");
    assert_eq!(home_relative("/home/mk/src", "/home/mk///"), "~/src");
    assert_eq!(home_relative("/home/mk", "/home/mk/"), "~");
}

/// A prefix match that is not a component boundary must not be rewritten.
///
/// `/home/mkother` starts with `/home/mk` as a string but is a different
/// directory. A naive `starts_with` renders it `~other`, which is not a path
/// and points at nothing.
#[test]
fn a_partial_component_match_is_not_home() {
    assert_eq!(home_relative("/home/mkother/x", "/home/mk"), "/home/mkother/x");
    assert_eq!(home_relative("/home/mk2", "/home/mk"), "/home/mk2");
    assert_eq!(home_relative("/home/m", "/home/mk"), "/home/m");
}

/// A path outside home, an empty home, and an already-relative path are all
/// left alone.
///
/// An empty `HOME` is what you get in a bare container or a systemd unit, and
/// rewriting every path to `~<path>` there would be catastrophic.
#[test]
fn paths_that_are_not_under_home_are_untouched() {
    assert_eq!(home_relative("/opt/tools/x", "/home/mk"), "/opt/tools/x");
    assert_eq!(home_relative("/home/mk/src", ""), "/home/mk/src");
    assert_eq!(home_relative("/home/mk/src", "/"), "/home/mk/src");
    assert_eq!(home_relative("~/src/foo", "/home/mk"), "~/src/foo");
    assert_eq!(home_relative("src/foo", "/home/mk"), "src/foo");
}

/// Windows home matching is case-insensitive and separator-insensitive.
///
/// NTFS is case-insensitive, and `HOMEPATH`, the registry, and whatever the
/// user typed disagree about both case and slash direction constantly. A
/// case-sensitive compare would leave every Windows path unabbreviated.
#[test]
fn windows_home_matching_ignores_case_and_separator_direction() {
    assert_eq!(home_relative("C:\\Users\\MK\\src", "c:\\users\\mk"), "~\\src");
    assert_eq!(home_relative("C:/Users/mk/src", "C:\\Users\\mk"), "~/src");
    assert_eq!(home_relative("C:\\Users\\mk", "C:\\Users\\mk"), "~");
    assert_eq!(
        home_relative("C:\\Users\\mkother\\x", "C:\\Users\\mk"),
        "C:\\Users\\mkother\\x",
        "case folding must not weaken the component-boundary rule"
    );
}

/// Unix home matching stays case-sensitive.
///
/// `/home/MK` and `/home/mk` are genuinely different directories on ext4, and
/// collapsing them would label one project with another's root.
#[test]
fn unix_home_matching_is_case_sensitive() {
    assert_eq!(home_relative("/home/MK/src", "/home/mk"), "/home/MK/src");
    assert_eq!(home_relative("/Home/mk/src", "/home/mk"), "/Home/mk/src");
}

/// A path that fits comes back untouched.
#[test]
fn a_path_that_fits_is_not_shortened() {
    let path = "/home/mk/src/vitrum/crates/vitrum-fmt";
    assert_eq!(display_width(path), 37);
    assert_eq!(shorten(path, 37), path);
    assert_eq!(shorten(path, 100), path);
    assert!(!shorten(path, 37).contains('\u{2026}'));
}

/// Elision removes whole middle components and keeps the first and the last.
///
/// The first component says which root the project lives under and the last
/// says which project it is. Cutting characters off either end throws away the
/// only two pieces of the path a reader actually uses.
#[test]
fn elision_keeps_the_first_and_last_components() {
    let path = "/home/mk/src/vitrum/crates/vitrum-fmt";
    let short = shorten(path, 30);
    assert_eq!(short, "/home/\u{2026}/crates/vitrum-fmt");
    assert_eq!(display_width(&short), 25);
    assert!(short.starts_with("/home/"));
    assert!(short.ends_with("vitrum-fmt"));
}

/// A larger budget keeps more trailing components.
///
/// The elision point has to move with the budget, not sit at a fixed depth, or
/// a wide sidebar would show no more than a narrow one.
#[test]
fn a_larger_budget_keeps_more_trailing_components() {
    let path = "/home/mk/src/vitrum/crates/vitrum-fmt";
    assert_eq!(shorten(path, 36), "/home/\u{2026}/src/vitrum/crates/vitrum-fmt");
    assert_eq!(shorten(path, 33), "/home/\u{2026}/vitrum/crates/vitrum-fmt");
    assert_eq!(shorten(path, 30), "/home/\u{2026}/crates/vitrum-fmt");
    assert_eq!(shorten(path, 24), "/home/\u{2026}/vitrum-fmt");
}

/// Windows separators are preserved exactly, never normalised to forward
/// slashes.
///
/// Elision splices the original string rather than re-joining components, so a
/// path a user could paste back into `cd` stays one.
#[test]
fn windows_separators_survive_elision() {
    let path = "C:\\Users\\mk\\src\\vitrum\\crates\\vitrum-fmt";
    assert_eq!(display_width(path), 40);
    let short = shorten(path, 30);
    assert_eq!(short, "C:\\\u{2026}\\vitrum\\crates\\vitrum-fmt");
    assert_eq!(display_width(&short), 29);
    assert!(!short.contains('/'), "no forward slash was introduced");
}

/// A CJK path is elided by columns, never by characters, and never mid-glyph.
///
/// Three components of five-ish characters measure 33 columns, so a 20-column
/// budget cannot keep the first and last components whole and has to fall back
/// to a character-level middle cut. That cut still has to land on a cluster
/// boundary.
#[test]
fn a_cjk_path_is_elided_by_columns() {
    let path = "/プロジェクト/セッション/ファイル";
    assert_eq!(path.chars().count(), 18);
    assert_eq!(display_width(path), 33, "eighteen characters, thirty-three columns");

    let short = shorten(path, 20);
    assert_eq!(short, "/プロジェ\u{2026}/ファイル");
    assert_eq!(display_width(&short), 19, "one column left blank rather than split");
    assert!(short.ends_with("/ファイル"), "the file name is intact");
}

/// A path made of wide characters never exceeds its budget at any width.
///
/// The sweep catches the parity bug where a budget of the wrong evenness lets
/// one two-column character across the line. One overflowing row is enough to
/// push a whole sidebar's right border out of alignment.
#[test]
fn no_path_budget_is_ever_exceeded() {
    let paths = [
        "/プロジェクト/セッション/ファイル",
        "/home/mk/src/vitrum/crates/vitrum-fmt",
        "C:\\Users\\mk\\src\\vitrum\\crates\\vitrum-fmt",
        "~/src/漢字/vitrum-fmt",
        "/a/b/c",
        "/",
        "",
        "no-separators-at-all-just-one-very-long-component",
    ];
    for path in paths {
        for budget in 0..=45 {
            let short = shorten(path, budget);
            assert!(
                display_width(&short) <= budget,
                "shorten({path:?}, {budget}) = {short:?} is too wide"
            );
        }
    }
}

/// Degenerate budgets produce degenerate output, not a panic and not overflow.
///
/// A collapsed sidebar really does pass zero, and a subtraction underflow on a
/// `usize` budget would panic in debug and wrap to a huge value in release.
#[test]
fn degenerate_budgets_are_safe() {
    let path = "/home/mk/src/vitrum/crates/vitrum-fmt";
    assert_eq!(shorten(path, 0), "");
    assert_eq!(shorten(path, 1), "\u{2026}");
    assert_eq!(shorten(path, 2), "/\u{2026}");
    assert_eq!(shorten("", 0), "");
    assert_eq!(shorten("/", 0), "");
}

/// A path with too few components falls back to a character-level middle cut.
///
/// There is no middle component to drop, but both ends still carry meaning, so
/// the fallback has to be the middle truncator and not a tail cut.
#[test]
fn too_few_components_falls_back_to_a_character_cut() {
    assert_eq!(shorten("/verylongfilename", 10), "/very\u{2026}name");
    assert_eq!(shorten("src/main.rs", 8), "src/\u{2026}.rs");
    assert_eq!(
        shorten("no-separators-at-all-just-one-very-long-component", 12),
        "no-sep\u{2026}onent"
    );
}

/// A single component that is too long is cut in the middle, keeping both ends.
///
/// A worktree directory named after a branch is often one very long component
/// that cannot fit beside its root, so the whole string is cut in the middle
/// rather than the first component being kept at the last one's expense. Both
/// ends of the string survive and there is never more than one ellipsis.
#[test]
fn one_long_component_keeps_both_of_its_ends() {
    let path = "/home/mk/worktrees/feature-rename-vitrum-to-vitrum-2026";
    assert_eq!(display_width(path), 55);
    let short = shorten(path, 20);
    assert_eq!(short, "/home/mk/w\u{2026}trum-2026");
    assert_eq!(display_width(&short), 20);
    assert_eq!(short.matches('\u{2026}').count(), 1);
}

/// Home rewriting and elision compose, in that order.
///
/// Eliding first and rewriting after would try to match `~` against a home
/// directory that the elision had already removed.
#[test]
fn home_rewriting_and_elision_compose() {
    let path = "/home/mk/src/vitrum/crates/vitrum-fmt";
    assert_eq!(
        shorten_home_relative(path, "/home/mk", 24),
        "~/\u{2026}/crates/vitrum-fmt"
    );
    assert_eq!(
        shorten_home_relative(path, "/home/mk", 40),
        "~/src/vitrum/crates/vitrum-fmt",
        "the rewrite alone brings it under budget"
    );
    assert_eq!(
        shorten_home_relative(path, "/opt", 30),
        "/home/\u{2026}/crates/vitrum-fmt",
        "not under home, so only elision applies"
    );
    assert_eq!(shorten_home_relative("/home/mk", "/home/mk", 10), "~");
    assert_eq!(shorten_home_relative("/home/mk", "/home/mk", 0), "");
}

/// The base name ignores trailing separators.
///
/// A project root recorded as `/home/mk/src/foo/` must still be labelled `foo`,
/// not with an empty string.
#[test]
fn base_name_ignores_trailing_separators() {
    assert_eq!(base_name("/home/mk/src/foo"), "foo");
    assert_eq!(base_name("/home/mk/src/foo/"), "foo");
    assert_eq!(base_name("/home/mk/src/foo///"), "foo");
    assert_eq!(base_name("C:\\Users\\mk\\foo\\"), "foo");
    assert_eq!(base_name("foo"), "foo");
}

/// A path with no components at all yields itself rather than an empty label.
///
/// `/` is a legitimate project root on a container image, and an empty sidebar
/// label for it would be indistinguishable from a failed lookup.
#[test]
fn base_name_of_a_rootless_path_is_the_path() {
    assert_eq!(base_name("/"), "/");
    assert_eq!(base_name("///"), "///");
    assert_eq!(base_name(""), "");
}

/// The base name of a multi-byte component is the whole component.
///
/// Byte scanning for separators must not mistake a UTF-8 continuation byte for
/// a slash, which would slice the name mid-character.
#[test]
fn base_name_handles_multibyte_components() {
    assert_eq!(base_name("/home/mk/プロジェクト"), "プロジェクト");
    assert_eq!(base_name("/home/mk/漢字/セッション"), "セッション");
}

/// A path shorter than the home directory cannot match it.
///
/// The comparison slices the path to the home's byte length. Without the length
/// guard that slice panics on a shorter path, which is exactly what a project
/// recorded as `/` would produce on a machine with a long `HOME`.
#[test]
fn a_path_shorter_than_home_is_not_rewritten() {
    assert_eq!(home_relative("/", "/home/user"), "/");
    assert_eq!(home_relative("", "/home/user"), "");
    assert_eq!(home_relative("/ho", "/home/mk"), "/ho");
    assert_eq!(shorten_home_relative("/", "/home/user", 10), "/");
}

/// A multi-byte home directory matches on cluster boundaries, not bytes.
///
/// Slicing the path at the home's byte length would panic if that offset landed
/// inside a multi-byte character. Home directories with non-ASCII user names
/// are ordinary outside English-speaking teams.
#[test]
fn a_multibyte_home_directory_matches_safely() {
    assert_eq!(home_relative("/home/ユーザー/src", "/home/ユーザー"), "~/src");
    assert_eq!(home_relative("/home/ユーザー", "/home/ユーザー"), "~");
    assert_eq!(
        home_relative("/home/ユーザーズ/src", "/home/ユーザー"),
        "/home/ユーザーズ/src",
        "a longer name that shares a prefix is a different directory"
    );
}

/// A directory inside the root reports the part the root does not already say.
///
/// This is the whole point of the function: the sidebar groups by project, so
/// a row repeating the project's own path spends columns on what the header
/// above it already said. What it must show is the remainder.
#[test]
fn a_directory_inside_the_root_reports_the_remainder() {
    assert_eq!(
        under("/src/vitrum/crates/vitrum-fmt", "/src/vitrum"),
        Place::Under("crates/vitrum-fmt")
    );
    assert_eq!(under("/src/vitrum/app", "/src/vitrum"), Place::Under("app"));
}

/// The root itself is `At`, not an empty `Under`.
///
/// The caller draws nothing for `At` and something for `Under`, so collapsing
/// the two into an empty string would make "at the project root" and "one
/// directory in, named nothing" indistinguishable.
#[test]
fn the_root_itself_is_at_not_an_empty_remainder() {
    assert_eq!(under("/src/vitrum", "/src/vitrum"), Place::At);
    assert_eq!(under("/src/vitrum", "/src/vitrum/"), Place::At);
    assert_eq!(under("/src/vitrum/", "/src/vitrum"), Place::Under(""));
}

/// A prefix match that is not a component boundary is outside.
///
/// The same rule home matching has, and for the same reason: `/src/vitrum2`
/// is a different project, and reporting it as `2` inside `/src/vitrum` would
/// be worse than saying nothing.
#[test]
fn a_partial_component_match_is_outside_the_root() {
    assert_eq!(under("/src/vitrum2", "/src/vitrum"), Place::Outside);
    assert_eq!(under("/src/vitrum2/app", "/src/vitrum"), Place::Outside);
    assert_eq!(under("/src/vit", "/src/vitrum"), Place::Outside);
}

/// A worktree beside the project is outside it.
///
/// The case this was written for. A session in a git worktree runs on another
/// branch in another directory, and it is the one row where the project's own
/// path says nothing true about where the work is.
#[test]
fn a_worktree_beside_the_project_is_outside() {
    assert_eq!(under("/src/worktrees/topic", "/src/vitrum"), Place::Outside);
    assert_eq!(under("/opt/elsewhere", "/src/vitrum"), Place::Outside);
}

/// An empty root is outside, not a match on everything.
#[test]
fn an_empty_root_matches_nothing() {
    assert_eq!(under("/src/vitrum", ""), Place::Outside);
    assert_eq!(under("/src/vitrum", "/"), Place::Outside);
}

/// Windows roots fold case and separator direction, exactly as home does.
#[test]
fn windows_roots_ignore_case_and_separator_direction() {
    assert_eq!(
        under("C:\\src\\Vitrum\\app", "c:/src/vitrum"),
        Place::Under("app")
    );
    assert_eq!(under("C:\\src\\vitrum", "C:\\src\\vitrum"), Place::At);
    assert_eq!(
        under("C:\\src\\vitrum2\\app", "C:\\src\\vitrum"),
        Place::Outside,
        "case folding must not weaken the component-boundary rule"
    );
}

/// Unix roots are case-sensitive, because the filesystem is.
#[test]
fn unix_roots_are_case_sensitive() {
    assert_eq!(under("/src/Vitrum/app", "/src/vitrum"), Place::Outside);
}
