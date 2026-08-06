//// What the launcher SHOWS, as opposed to what it ranks.
////
//// This surface was rebuilt after being judged poorly designed,
//// and the two defects behind that were both about repetition and about paths
//// written in a way nobody writes them.

use super::*;

/// Home is written `~`, never `home/someone`.
///
/// `place_of` fell back to the last two path components, so the home
/// directory rendered as `home/user`: an absolute path with its
/// leading slash cut off, which reads as a relative path and is not how
/// anybody writes their home directory. On a machine with no projects yet
/// every suggestion runs in home, so this was on EVERY row of the
/// launcher on first launch.
#[test]
fn the_home_directory_is_written_the_way_people_write_it() {
    let home = "/home/user";
    assert_eq!(place_of(&[], home, home), "~");
    assert_eq!(place_of(&[], "/home/user/src", home), "~/src");
    assert_eq!(place_of(&[], "/home/user/src/vitrum", home), "~/src/vitrum");
}

/// A path outside home keeps the two-component form, and a project still
/// wins over both.
///
/// The `~` rule must not swallow the cases that already worked: `/tmp` is
/// not under home, and a known project is named by the sidebar's own word
/// for it rather than by either path rule.
#[test]
fn only_paths_under_home_get_a_tilde() {
    let home = "/home/user";
    assert_eq!(place_of(&[], "/tmp/scratch", home), "tmp/scratch");
    assert_eq!(place_of(&[], "/var/log", home), "var/log");
    let projects = vec![ProjectInfo {
        id: ProjectId(1),
        name: "vitrum".to_string(),
        root: "/home/user/src/vitrum".to_string(),
    }];
    assert_eq!(
        place_of(&projects, "/home/user/src/vitrum/app", home),
        "vitrum/app",
        "a known project must still win over the tilde form"
    );
}

/// A launcher row resolves its agent from the PROGRAM, not the line.
///
/// `Intent::command` carries the whole command line, and `AgentKind::of`
/// matches a program name exactly because it must never guess that
/// `claudex` is Claude. Handing it `bash -l` produced the unknown mark on
/// a shell: a confident wrong answer, on the one row every operator has.
#[test]
fn a_row_with_arguments_still_names_its_agent() {
    use crate::agent::AgentKind;
    for (line, want) in [
        ("bash -l", AgentKind::Shell),
        ("/bin/bash", AgentKind::Shell),
        ("claude --resume \"my project\"", AgentKind::Claude),
        ("codex --model o3", AgentKind::Codex),
        ("gemini", AgentKind::Gemini),
        ("some-unknown-tool --flag", AgentKind::Unknown),
    ] {
        let program = launch::split_command(line)
            .map(|(p, _)| p)
            .unwrap_or_else(|| line.to_string());
        assert_eq!(
            AgentKind::of(&program),
            want,
            "`{line}` resolved to the wrong agent"
        );
    }
}

/// A directory row claims no agent at all.
///
/// It starts nothing, so naming one would be an invention. The box is
/// still reserved in CSS so path rows and agent rows share a text column.
#[test]
fn a_directory_row_carries_no_agent_mark() {
    let v = view(&Pick::Cd("/src/vitrum/app".to_string()), "/home/u");
    assert!(v.mark.is_none(), "a path row claimed to be an agent");
}

/// An empty home must not turn every path into a tilde.
///
/// `user_home()` can legitimately return nothing, and `is_within("", p)`
/// is the kind of predicate that answers true for everything.
#[test]
fn an_unknown_home_leaves_paths_alone() {
    assert_eq!(place_of(&[], "/tmp/scratch", ""), "tmp/scratch");
    assert_eq!(place_of(&[], "/home/someone", ""), "home/someone");
}
