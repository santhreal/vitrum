//! Spelling a path the way a target platform spells it, from any host.
//!
//! # What these defend
//!
//! `Path::join` and `Path::is_absolute` answer for the box the daemon runs on.
//! Every rule in this crate that describes a platform the daemon is not running
//! on went through one of them and came out wrong: a Linux build joined
//! `C:\Windows\System32` with `where` and got `C:\Windows\System32/where`, and
//! called `C:\Windows\System32\cmd.exe` relative, which meant it was joined onto
//! the session's directory. Neither was visible on Windows, and neither was
//! checkable anywhere else.

use std::path::{Path, PathBuf};

use crate::hostpath::{POSIX, WINDOWS, join, spell, windows_drive_qualified, windows_rooted};

/// A Windows path is built with backslashes wherever it is built.
#[test]
fn a_windows_path_is_spelled_with_backslashes() {
    assert_eq!(
        join(WINDOWS, Path::new(r"C:\Windows\System32"), "where"),
        PathBuf::from(r"C:\Windows\System32\where")
    );
}

/// A separator already there is not doubled, in either spelling.
#[test]
fn a_trailing_separator_is_not_doubled() {
    assert_eq!(join(POSIX, Path::new("/usr/bin/"), "git"), PathBuf::from("/usr/bin/git"));
    assert_eq!(join(WINDOWS, Path::new(r"C:\tools\"), "run"), PathBuf::from(r"C:\tools\run"));
    assert_eq!(join(WINDOWS, Path::new("C:/tools/"), "run"), PathBuf::from(r"C:\tools\run"));
}

/// Windows accepts a forward slash, so two spellings of one file compare equal.
///
/// Without this a command written `tools/run` was looked for at
/// `C:\src\tools/run` and never found, while the same command typed with a
/// backslash worked.
#[test]
fn the_two_windows_spellings_are_one_path() {
    assert_eq!(spell(WINDOWS, "tools/run"), r"tools\run");
    assert_eq!(
        join(WINDOWS, Path::new(r"C:\src"), "tools/run"),
        join(WINDOWS, Path::new("C:/src"), r"tools\run")
    );
}

/// A backslash is an ordinary character in a POSIX file name and survives.
///
/// Rewriting it would name a different file, and the daemon would refuse a
/// command that is installed.
#[test]
fn a_posix_name_keeps_its_backslash() {
    assert_eq!(spell(POSIX, r"odd\name"), r"odd\name");
    assert_eq!(
        join(POSIX, Path::new("/usr/bin"), r"odd\name"),
        PathBuf::from(r"/usr/bin/odd\name")
    );
}

/// What counts as rooted on Windows is a drive, a share, or nothing.
///
/// A bare `\path` is rooted and still resolves against whichever drive is
/// current, so a resolver that stopped there would look on the wrong volume.
#[test]
fn windows_rootedness_needs_a_drive_or_a_share() {
    assert!(windows_rooted(r"C:\Windows"));
    assert!(windows_rooted("C:/Windows"));
    assert!(windows_rooted(r"\\server\share"));
    assert!(windows_rooted("//server/share"));

    assert!(!windows_rooted(r"\Windows"));
    assert!(!windows_rooted("/Windows"));
    assert!(!windows_rooted("C:"));
    assert!(!windows_rooted(r"C:tools\run"));
    assert!(!windows_rooted("tools/run"));
    assert!(!windows_rooted(""));
}

/// A drive-relative command is drive-qualified without being rooted.
///
/// `C:tools\run` resolves against the current directory of drive C, so it must
/// not be joined onto the session's directory and must not be treated as a
/// location either.
#[test]
fn a_drive_relative_command_is_qualified_but_not_rooted() {
    assert!(windows_drive_qualified(r"C:tools\run"));
    assert!(!windows_rooted(r"C:tools\run"));
    assert!(windows_drive_qualified(r"C:\Windows"));
    assert!(!windows_drive_qualified(r"\\server\share"));
    assert!(!windows_drive_qualified("tools/run"));
}

/// An empty base contributes no separator of its own.
#[test]
fn an_empty_base_yields_the_child() {
    assert_eq!(join(POSIX, Path::new(""), "git"), PathBuf::from("git"));
    assert_eq!(join(WINDOWS, Path::new(""), "git"), PathBuf::from("git"));
}

/// The filesystem root joins without doubling its slash.
#[test]
fn the_root_joins_without_doubling() {
    assert_eq!(join(POSIX, Path::new("/"), "bin"), PathBuf::from("/bin"));
}
