//! Command resolution, for both rule sets, from whichever host runs the suite.
//!
//! # What these defend
//!
//! Resolution used to be a POSIX implementation under `#[cfg(unix)]` and
//! `Ok(())` everywhere else. A Windows build therefore accepted every command
//! name, opened a pseudoterminal for it, and handed the operator the PTY
//! layer's kilobyte of `PATH` dump when the spawn failed. The Windows arm was
//! not wrong, it did not exist, and nothing could tell because nothing ran it.
//!
//! Every case here drives [`resolve`] directly with a described filesystem, so
//! the Windows rules are proven on Linux and the POSIX rules on Windows.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::SessionError;
use crate::command_path::{Search, SpawnRules, resolve};

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

/// A POSIX search over `/usr/bin` and `/bin`, running in `/src/repo`.
fn posix<'a>(cwd: &'a Path, exists: &'a dyn Fn(&Path) -> bool) -> Search<'a> {
    Search::new(
        vec![PathBuf::from("/usr/bin"), PathBuf::from("/bin")],
        Vec::new(),
        cwd,
        exists,
    )
}

/// A Windows search in the loader's order, with the stock `PATHEXT`.
fn windows<'a>(cwd: &'a Path, exists: &'a dyn Fn(&Path) -> bool) -> Search<'a> {
    Search::new(
        vec![
            PathBuf::from(r"C:\Program Files\vitrum"),
            cwd.to_path_buf(),
            PathBuf::from(r"C:\Windows\System32"),
        ],
        [".COM", ".EXE", ".BAT", ".CMD"].iter().map(|e| (*e).to_string()).collect(),
        cwd,
        exists,
    )
}

fn refusal(command: &str) -> SessionError {
    SessionError::NotOnPath { command: command.to_string() }
}

/// A bare name is found in the search directories, in order.
#[test]
fn a_bare_posix_name_is_found_on_the_path() {
    let fs = Fs::with(&["/usr/bin/claude"]);
    let exists = fs.exists();
    let cwd = Path::new("/src/repo");
    assert_eq!(resolve(SpawnRules::Posix, "claude", &posix(cwd, &exists)), Ok(()));
    assert_eq!(
        resolve(SpawnRules::Posix, "codex", &posix(cwd, &exists)),
        Err(refusal("codex"))
    );
}

/// A relative command resolves against the directory the SESSION starts in.
///
/// It used to resolve against the daemon's own working directory, so a session
/// created in a project with `./run-agent` in it was refused unless the daemon
/// happened to have been started from that same directory. The child is spawned
/// with the session's cwd, so that is the only directory the check may use.
#[test]
fn a_relative_command_resolves_against_the_session_directory() {
    let fs = Fs::with(&["/src/repo/run-agent"]);
    let exists = fs.exists();

    let inside = Path::new("/src/repo");
    assert_eq!(resolve(SpawnRules::Posix, "./run-agent", &posix(inside, &exists)), Ok(()));

    let elsewhere = Path::new("/src/other");
    assert_eq!(
        resolve(SpawnRules::Posix, "./run-agent", &posix(elsewhere, &exists)),
        Err(refusal("./run-agent"))
    );
}

/// An absolute command is taken as written, never joined onto the session
/// directory.
#[test]
fn an_absolute_posix_command_is_taken_as_written() {
    let fs = Fs::with(&["/opt/agents/bin/gemini"]);
    let exists = fs.exists();
    let cwd = Path::new("/src/repo");
    assert_eq!(
        resolve(SpawnRules::Posix, "/opt/agents/bin/gemini", &posix(cwd, &exists)),
        Ok(())
    );
    assert_eq!(
        resolve(SpawnRules::Posix, "/opt/agents/bin/absent", &posix(cwd, &exists)),
        Err(refusal("/opt/agents/bin/absent"))
    );
}

/// A backslash is an ordinary character in a POSIX file name, not a separator.
///
/// Splitting on it would refuse a real file and, worse, would silently look
/// somewhere else for a name the operator spelled exactly.
#[test]
fn a_backslash_is_part_of_a_posix_name() {
    let fs = Fs::with(&[r"/usr/bin/odd\name"]);
    let exists = fs.exists();
    let cwd = Path::new("/src/repo");
    assert_eq!(resolve(SpawnRules::Posix, r"odd\name", &posix(cwd, &exists)), Ok(()));
}

/// POSIX never appends a suffix. `git` means `git`, and `git.exe` is a
/// different file.
#[test]
fn posix_never_appends_an_extension() {
    let fs = Fs::with(&["/usr/bin/git.exe"]);
    let exists = fs.exists();
    let cwd = Path::new("/src/repo");
    assert_eq!(
        resolve(SpawnRules::Posix, "git", &posix(cwd, &exists)),
        Err(refusal("git"))
    );
}

/// A bare Windows name is found by appending a `PATHEXT` suffix.
///
/// This is the case the old `Ok(())` arm could not express at all: `git` on
/// Windows is `git.exe`, and a resolver that only looks for `git` would refuse
/// every command on the machine.
#[test]
fn a_bare_windows_name_is_found_through_pathext() {
    let fs = Fs::with(&[r"C:\Windows\System32\where.exe"]);
    let exists = fs.exists();
    let cwd = Path::new(r"C:\src\repo");
    assert_eq!(resolve(SpawnRules::Windows, "where", &windows(cwd, &exists)), Ok(()));
    assert_eq!(
        resolve(SpawnRules::Windows, "nosuch", &windows(cwd, &exists)),
        Err(refusal("nosuch"))
    );
}

/// Every stock `PATHEXT` suffix is tried, not just `.EXE`.
///
/// A batch file is how most Node-based agents are installed on Windows: `npm i
/// -g @anthropic-ai/claude-code` writes `claude.cmd`, and a resolver that knew
/// only `.exe` would refuse the product's own primary target.
#[test]
fn every_stock_extension_is_tried() {
    for suffix in [".com", ".exe", ".bat", ".cmd"] {
        let installed = format!(r"C:\Windows\System32\claude{suffix}");
        let fs = Fs::with(&[installed.as_str()]);
        let exists = fs.exists();
        let cwd = Path::new(r"C:\src\repo");
        assert_eq!(
            resolve(SpawnRules::Windows, "claude", &windows(cwd, &exists)),
            Ok(()),
            "{suffix} was not tried"
        );
    }
}

/// A name that already carries a known suffix is not given a second one.
#[test]
fn a_spelled_out_extension_is_not_doubled() {
    let fs = Fs::with(&[r"C:\Windows\System32\git.exe.exe"]);
    let exists = fs.exists();
    let cwd = Path::new(r"C:\src\repo");
    assert_eq!(
        resolve(SpawnRules::Windows, "git.exe", &windows(cwd, &exists)),
        Err(refusal("git.exe"))
    );
}

/// The Windows loader searches the image directory and the working directory
/// before `PATH`, and so must this.
#[test]
fn windows_searches_the_session_directory_before_the_path() {
    let fs = Fs::with(&[r"C:\src\repo\build.cmd"]);
    let exists = fs.exists();
    let cwd = Path::new(r"C:\src\repo");
    assert_eq!(resolve(SpawnRules::Windows, "build", &windows(cwd, &exists)), Ok(()));

    let elsewhere = Path::new(r"C:\src\other");
    assert_eq!(
        resolve(SpawnRules::Windows, "build", &windows(elsewhere, &exists)),
        Err(refusal("build"))
    );
}

/// A drive-qualified command is absolute even when the host disagrees.
///
/// `Path::is_absolute` answers for the box the daemon runs on. A Linux build
/// asked about `C:\Windows\System32\cmd.exe` calls it relative, and joining it
/// onto the session directory produces a path that exists nowhere.
#[test]
fn a_drive_qualified_command_is_not_joined_onto_the_session_directory() {
    let fs = Fs::with(&[r"C:\Windows\System32\cmd.exe"]);
    let exists = fs.exists();
    let cwd = Path::new(r"D:\src\repo");
    assert_eq!(
        resolve(SpawnRules::Windows, r"C:\Windows\System32\cmd.exe", &windows(cwd, &exists)),
        Ok(())
    );
    assert_eq!(
        resolve(SpawnRules::Windows, r"C:\Windows\System32\cmd", &windows(cwd, &exists)),
        Ok(())
    );
}

/// Both separators divide a Windows path; a forward slash is legal there.
#[test]
fn a_forward_slash_separates_a_windows_path() {
    let fs = Fs::with(&[r"C:\src\repo\tools\run.bat"]);
    let exists = fs.exists();
    let cwd = Path::new(r"C:\src\repo");
    assert_eq!(resolve(SpawnRules::Windows, "tools/run", &windows(cwd, &exists)), Ok(()));
}

/// The refusal names the command and tells the operator what to do.
///
/// The whole reason resolution happens here is that the PTY layer's own error
/// is a kilobyte of `PATH` that answers no question. A refusal that lost the
/// command name would be no better.
#[test]
fn a_refusal_names_the_command_and_the_corrective_action() {
    let fs = Fs::with(&[]);
    let exists = fs.exists();
    let cwd = Path::new("/src/repo");
    let message = resolve(SpawnRules::Posix, "claude", &posix(cwd, &exists))
        .unwrap_err()
        .to_string();
    assert!(message.contains("claude"), "the refusal lost the command: {message}");
    assert!(
        message.contains("absolute path"),
        "the refusal names no corrective action: {message}"
    );
}

/// The host's rule set matches the host it was compiled for.
///
/// If this ever disagreed, a Windows build would resolve with POSIX rules and
/// refuse every installed agent, which is precisely the failure the whole
/// module exists to prevent.
#[test]
fn the_host_rule_set_matches_the_build_target() {
    let expected = if cfg!(windows) { SpawnRules::Windows } else { SpawnRules::Posix };
    assert_eq!(SpawnRules::host(), expected);
}

/// The two rule sets disagree, and both are reachable from this host.
///
/// A regression that collapsed them (for instance by making `host()` constant,
/// or by giving the POSIX arm `PATHEXT`) would leave every case above passing
/// under one rule set. This is the assertion that notices.
#[test]
fn the_two_rule_sets_are_not_the_same_rule_set() {
    let fs = Fs::with(&[r"C:\Windows\System32\ping.exe"]);
    let exists = fs.exists();
    let cwd = Path::new(r"C:\src\repo");
    assert_eq!(resolve(SpawnRules::Windows, "ping", &windows(cwd, &exists)), Ok(()));

    let posix_dirs = Search::new(
        vec![PathBuf::from(r"C:\Windows\System32")],
        Vec::new(),
        cwd,
        &exists,
    );
    assert_eq!(
        resolve(SpawnRules::Posix, "ping", &posix_dirs),
        Err(refusal("ping")),
        "the POSIX rules grew PATHEXT"
    );
}
