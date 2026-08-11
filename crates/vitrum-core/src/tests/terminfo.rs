//! Terminal capability detection, for every host, from whichever one runs the
//! suite.
//!
//! # What these defend
//!
//! The check was Linux's, compiled and executed everywhere. On Windows it read
//! `$HOME`, which is never set there, searched `/etc/terminfo`, `/lib/terminfo`
//! and `/usr/share/terminfo`, which never exist there, and then told the
//! operator to install `ncurses-term`, which is a Debian package name. On macOS
//! it named the same package, which Homebrew does not have either. The Linux arm
//! was right and the other two were a Linux arm wearing a hat.
//!
//! The variant space is walked from [`guided_hosts`] rather than written out, so
//! adding a host without writing its search order and its fix turns this red.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::terminfo::{
    TermEnv, TerminfoCheck, advice_for, check, entry_present, guided_hosts, roots, separator,
};

/// A filesystem described as the exact set of paths that exist.
struct Fs(BTreeSet<PathBuf>);

impl Fs {
    fn with(paths: &[&str]) -> Self {
        Self(paths.iter().map(PathBuf::from).collect())
    }

    fn exists(&self) -> impl Fn(&Path) -> bool + '_ {
        move |p: &Path| self.0.contains(p)
    }
}

fn has(host: &str, roots: &[PathBuf], name: &str, fs: &Fs) -> bool {
    entry_present(roots, name, separator(host), &fs.exists())
}

/// Every host the product claims to run on has both a search order and a fix.
///
/// Derived from the guidance table at run time. A host in that table with no
/// search order, or a search order with no fix, fails here rather than
/// silently inheriting Linux's answer, which is exactly how the Windows arm
/// came to recommend a Debian package.
#[test]
fn every_guided_host_has_a_search_order_and_a_fix() {
    let env = TermEnv::from_pairs([
        ("HOME", "/home/mk"),
        ("USERPROFILE", r"C:\Users\mk"),
    ]);
    for host in guided_hosts() {
        let advice = advice_for(host).unwrap_or_else(|| panic!("{host} has no fix recorded"));
        assert!(
            advice.contains("install") || advice.contains("compile"),
            "{host}: the fix is not an instruction: {advice}"
        );
        assert!(
            advice.contains("TERM"),
            "{host}: the fix does not say what breaks without it: {advice}"
        );
        assert!(
            !roots(host, &env).is_empty(),
            "{host}: nowhere is searched, so the check can only ever say absent"
        );
    }
}

/// A host nobody wrote a line for gets no advice, and the caller learns that.
///
/// Handing a FreeBSD operator `apt install ncurses-term` is worse than an
/// honest blank, and the `Unguided` answer is what stops the blank being
/// silent.
#[test]
fn an_unguided_host_is_named_rather_than_given_linuxs_answer() {
    assert_eq!(advice_for("freebsd"), None);
    let fs = Fs::with(&[]);
    assert_eq!(
        check("freebsd", "vte-256color", &TermEnv::default(), &fs.exists()),
        TerminfoCheck::Unguided { host: "freebsd".to_string() }
    );
}

/// Linux searches the ncurses trees in ncurses' order, after the overrides.
#[test]
fn linux_searches_the_ncurses_trees() {
    let env = TermEnv::from_pairs([
        ("TERMINFO", "/opt/tinfo"),
        ("HOME", "/home/mk"),
        ("TERMINFO_DIRS", "/a:/b"),
    ]);
    assert_eq!(
        roots("linux", &env),
        vec![
            PathBuf::from("/opt/tinfo"),
            PathBuf::from("/home/mk/.terminfo"),
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            PathBuf::from("/etc/terminfo"),
            PathBuf::from("/lib/terminfo"),
            PathBuf::from("/usr/share/terminfo"),
        ]
    );
}

/// macOS shares the search order and not the package name.
///
/// `ncurses-term` does not exist on macOS. Naming it sends the reader to a
/// package manager that has never heard of it, which reads as the product being
/// wrong about their machine.
#[test]
fn macos_shares_the_search_order_but_not_the_package() {
    let env = TermEnv::from_pairs([("HOME", "/home/mk")]);
    assert_eq!(roots("macos", &env), roots("linux", &env));
    let macos = advice_for("macos").expect("macOS has a fix");
    assert!(!macos.contains("ncurses-term"), "macOS was told to install a Debian package");
    assert!(macos.contains("brew install ncurses"), "macOS fix is not actionable: {macos}");
    assert_ne!(advice_for("linux"), advice_for("macos"));
}

/// Windows has no system database and no `$HOME`.
///
/// Reading `HOME` there produced no per-user tree at all, and the three Unix
/// system paths were three stat calls that could only fail. The per-user tree
/// comes from `%USERPROFILE%`, and `TERMINFO_DIRS` is semicolon-separated
/// because that is what a Windows path list is.
#[test]
fn windows_has_a_per_user_database_and_no_system_one() {
    let env = TermEnv::from_pairs([
        ("HOME", "/home/mk"),
        ("USERPROFILE", r"C:\Users\mk"),
        ("TERMINFO_DIRS", r"C:\msys64\usr\share\terminfo;C:\other"),
    ]);
    assert_eq!(
        roots("windows", &env),
        vec![
            PathBuf::from(r"C:\Users\mk\.terminfo"),
            PathBuf::from(r"C:\msys64\usr\share\terminfo"),
            PathBuf::from(r"C:\other"),
        ]
    );
    let advice = advice_for("windows").expect("Windows has a fix");
    assert!(!advice.contains("ncurses-term"), "Windows was told to install a Debian package");
    assert!(advice.contains("tic"), "Windows fix names no tool: {advice}");
}

/// Both on-disk layouts count: a letter directory and the hashed one.
///
/// macOS and several distributions build the hashed form, where `vte-256color`
/// lives under `76` rather than under `v`. Checking only the letter form
/// reported every entry on those machines as missing.
#[test]
fn both_database_layouts_are_recognised() {
    let letters = Fs::with(&["/usr/share/terminfo/v/vte-256color"]);
    let hashed = Fs::with(&["/usr/share/terminfo/76/vte-256color"]);
    let tree = vec![PathBuf::from("/usr/share/terminfo")];
    assert!(has("linux", &tree, "vte-256color", &letters));
    assert!(has("linux", &tree, "vte-256color", &hashed));
    assert!(!has("linux", &tree, "vte-256color", &Fs::with(&[])));
}

/// A present entry produces no advice at all.
#[test]
fn a_present_entry_is_reported_as_present() {
    let fs = Fs::with(&[r"C:\Users\mk\.terminfo\v\vte-256color"]);
    let env = TermEnv::from_pairs([("USERPROFILE", r"C:\Users\mk")]);
    assert_eq!(
        check("windows", "vte-256color", &env, &fs.exists()),
        TerminfoCheck::Present
    );
}

/// The absent case carries the host's own fix, not the first one in the table.
#[test]
fn an_absent_entry_carries_this_hosts_fix() {
    let fs = Fs::with(&[]);
    for host in guided_hosts() {
        let expected = advice_for(host).expect("guided");
        assert_eq!(
            check(host, "vte-256color", &TermEnv::default(), &fs.exists()),
            TerminfoCheck::Absent { advice: expected },
            "{host} was given another host's fix"
        );
    }
}

/// An empty terminal name cannot be looked up and must not be reported found.
#[test]
fn an_empty_terminal_name_is_never_present() {
    let fs = Fs::with(&["/usr/share/terminfo/v/vte-256color"]);
    assert!(!has("linux", &[PathBuf::from("/usr/share/terminfo")], "", &fs));
}

/// The daemon's own `TERM` is what gets checked, read from the same table the
/// children are given.
///
/// A check against a hardcoded `"vte-256color"` here would keep passing after
/// someone changed what sessions are actually told.
#[test]
fn the_checked_terminal_is_the_one_sessions_are_given() {
    let (_, name) = crate::session::DEFAULT_TERM_ENV
        .iter()
        .find(|(k, _)| *k == "TERM")
        .expect("sessions are told what terminal they have");
    let initial = name.chars().next().expect("a non-empty terminal name");
    let installed = format!("/usr/share/terminfo/{initial}/{name}");
    let fs = Fs::with(&[installed.as_str()]);
    let env = TermEnv::from_pairs([("TERMINFO", "/usr/share/terminfo")]);
    assert_eq!(check("linux", name, &env, &fs.exists()), TerminfoCheck::Present);
}
