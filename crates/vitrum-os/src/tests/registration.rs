//! URL scheme registration plans for all three platforms.
//!
//! The plan is a value, so the exact desktop entry text and the exact registry
//! keys are asserted from any host. Applying is the only platform-gated part,
//! and on macOS applying is honestly impossible, which is itself asserted.

use std::path::{Path, PathBuf};

use crate::capability::UnavailableKind;
use crate::deeplink::{
    RegistrationPlan, RegistryValue, apply_registration, plan_registration,
};
use crate::paths::{AppPaths, PathEnv, Platform};
use crate::tests::support::TempDir;

fn linux_paths() -> AppPaths {
    AppPaths::resolve(Platform::Linux, &PathEnv::from_pairs([("HOME", "/home/ada")]))
        .expect("HOME is set")
}

/// The desktop entry must be exactly this file.
///
/// `MimeType=x-scheme-handler/vitrum` is the line that makes the whole feature
/// work; `%u` is what passes the URL to the process. Losing either produces a
/// handler that is registered and does nothing, which is far harder to debug
/// than one that is not registered at all.
#[test]
fn the_linux_desktop_entry_is_exactly_this() {
    let plan = plan_registration(Platform::Linux, Path::new("/usr/bin/vitrum"), &linux_paths());
    let RegistrationPlan::DesktopEntry { path, contents, post_install } = plan else {
        panic!("Linux must plan a desktop entry");
    };
    assert_eq!(path, PathBuf::from("/home/ada/.local/share/applications/vitrum.desktop"));
    assert_eq!(
        contents,
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name=Vitrum\n\
         Comment=Terminal shell for coding agents\n\
         Exec=/usr/bin/vitrum %u\n\
         Icon=vitrum\n\
         Terminal=false\n\
         Categories=Development;TerminalEmulator;\n\
         MimeType=x-scheme-handler/vitrum;\n\
         StartupNotify=true\n\
         StartupWMClass=vitrum\n"
    );
    // The directory argument is a `PathBuf` rendered to a string, so it carries
    // the host's separator. Normalising it pins the segment sequence, which is
    // the thing under test, exactly as `assert_path` does for the rest.
    let post_install: Vec<Vec<String>> = post_install
        .into_iter()
        .map(|argv| {
            argv.into_iter()
                .map(|arg| arg.replace(std::path::MAIN_SEPARATOR, "/"))
                .collect()
        })
        .collect();
    assert_eq!(
        post_install,
        vec![
            vec![
                "update-desktop-database".to_string(),
                "/home/ada/.local/share/applications".to_string(),
            ],
            vec![
                "xdg-mime".to_string(),
                "default".to_string(),
                "vitrum.desktop".to_string(),
                "x-scheme-handler/vitrum".to_string(),
            ],
        ]
    );
}

/// An executable path with a space must be quoted per the desktop spec.
///
/// `/home/Some User/bin/vitrum` is entirely ordinary and, unquoted, produces an
/// `Exec` line the desktop parses as two arguments. The handler then silently
/// never launches.
#[test]
fn an_exec_path_with_a_space_is_quoted() {
    let plan = plan_registration(
        Platform::Linux,
        Path::new("/home/Some User/bin/vitrum"),
        &linux_paths(),
    );
    let RegistrationPlan::DesktopEntry { contents, .. } = plan else { panic!("desktop entry") };
    assert!(
        contents.contains("Exec=\"/home/Some User/bin/vitrum\" %u\n"),
        "unquoted exec in:\n{contents}"
    );
}

/// The reserved characters must be backslash-escaped inside the quotes.
///
/// The spec reserves `"`, backtick, `$` and backslash. An unescaped `$` in a
/// path makes the desktop expand a variable.
#[test]
fn reserved_characters_in_the_exec_path_are_escaped() {
    let plan =
        plan_registration(Platform::Linux, Path::new("/opt/a $b `c\"d/vitrum"), &linux_paths());
    let RegistrationPlan::DesktopEntry { contents, .. } = plan else { panic!("desktop entry") };
    assert!(
        contents.contains("Exec=\"/opt/a \\$b \\`c\\\"d/vitrum\" %u\n"),
        "badly escaped exec in:\n{contents}"
    );
}

/// A plain path must not be quoted.
///
/// Quoting unconditionally is also correct per the spec, but it makes the
/// generated file differ from every other desktop entry on the system, which
/// invites someone to "fix" it.
#[test]
fn a_plain_exec_path_is_not_quoted() {
    let plan = plan_registration(Platform::Linux, Path::new("/usr/bin/vitrum"), &linux_paths());
    let RegistrationPlan::DesktopEntry { contents, .. } = plan else { panic!("desktop entry") };
    assert!(contents.contains("Exec=/usr/bin/vitrum %u\n"));
}

/// Applying the Linux plan must write exactly that file, creating parents.
#[test]
fn applying_the_linux_plan_writes_the_file() {
    let dir = TempDir::new("reg-linux");
    let path = dir.join("applications/vitrum.desktop");
    let plan = RegistrationPlan::DesktopEntry {
        path: path.clone(),
        contents: "[Desktop Entry]\n".to_string(),
        post_install: Vec::new(),
    };
    let outcome = apply_registration(&plan).expect("writing into a temp dir must succeed");
    assert_eq!(outcome.targets, vec![path.to_string_lossy().into_owned()]);
    assert!(!outcome.already_current, "the file did not exist, so it was written");
    assert_eq!(std::fs::read_to_string(&path).expect("file exists"), "[Desktop Entry]\n");
}

/// Registering twice must write once and say the second call changed nothing.
///
/// First run calls this, and so does every installer upgrade. A second write
/// would re-run `update-desktop-database` on every launch, and a caller with
/// no way to tell the two apart cannot log the difference between a fresh
/// install and a no-op.
#[test]
fn registering_twice_writes_once() {
    let dir = TempDir::new("reg-idempotent");
    let path = dir.join("applications/vitrum.desktop");
    let plan = RegistrationPlan::DesktopEntry {
        path: path.clone(),
        contents: "[Desktop Entry]\nExec=/usr/bin/vitrum %u\n".to_string(),
        post_install: Vec::new(),
    };

    let first = apply_registration(&plan).expect("first registration");
    assert!(!first.already_current);
    let stamp = std::fs::metadata(&path).expect("written").modified().expect("mtime");

    let second = apply_registration(&plan).expect("second registration");
    assert!(second.already_current, "the entry was already exactly right");
    assert!(second.refreshed.is_empty(), "nothing was written, so nothing needs refreshing");
    assert_eq!(second.targets, first.targets);
    assert_eq!(
        std::fs::metadata(&path).expect("still there").modified().expect("mtime"),
        stamp,
        "the file must not have been rewritten"
    );
}

/// An entry left over from an older version must be replaced, not kept.
///
/// The idempotence check compares contents, not existence. Comparing existence
/// would pin a stale `Exec=` line pointing at a path the upgrade moved.
#[test]
fn a_stale_entry_is_rewritten() {
    let dir = TempDir::new("reg-stale");
    let path = dir.join("applications/vitrum.desktop");
    std::fs::create_dir_all(path.parent().expect("has a parent")).expect("temp dir");
    std::fs::write(&path, "[Desktop Entry]\nExec=/old/vitrum %u\n").expect("seed");
    let plan = RegistrationPlan::DesktopEntry {
        path: path.clone(),
        contents: "[Desktop Entry]\nExec=/new/vitrum %u\n".to_string(),
        post_install: Vec::new(),
    };

    let outcome = apply_registration(&plan).expect("rewrite");
    assert!(!outcome.already_current);
    assert_eq!(
        std::fs::read_to_string(&path).expect("file exists"),
        "[Desktop Entry]\nExec=/new/vitrum %u\n"
    );
}

/// The Windows registration must be exactly these four values.
///
/// `URL Protocol` with an empty value is what marks the key as a protocol
/// handler; without it Windows treats `vitrum` as a file association and the
/// link does nothing. The `"%1"` must be quoted or a URL containing a space
/// arrives split across two arguments.
#[test]
fn the_windows_registry_values_are_exactly_these() {
    let paths = AppPaths::resolve(
        Platform::Windows,
        &PathEnv::from_pairs([
            ("APPDATA", "C:/Users/ada/AppData/Roaming"),
            ("LOCALAPPDATA", "C:/Users/ada/AppData/Local"),
        ]),
    )
    .expect("both roots are set");
    let plan =
        plan_registration(Platform::Windows, Path::new(r"C:\Program Files\Vitrum\vitrum.exe"), &paths);
    let RegistrationPlan::RegistryValues { values } = plan else {
        panic!("Windows must plan registry values");
    };
    assert_eq!(
        values,
        vec![
            RegistryValue {
                key: r"Software\Classes\vitrum".to_string(),
                name: None,
                value: "URL:Vitrum Protocol".to_string(),
            },
            RegistryValue {
                key: r"Software\Classes\vitrum".to_string(),
                name: Some("URL Protocol".to_string()),
                value: String::new(),
            },
            RegistryValue {
                key: r"Software\Classes\vitrum\DefaultIcon".to_string(),
                name: None,
                value: "\"C:\\Program Files\\Vitrum\\vitrum.exe\",0".to_string(),
            },
            RegistryValue {
                key: r"Software\Classes\vitrum\shell\open\command".to_string(),
                name: None,
                value: "\"C:\\Program Files\\Vitrum\\vitrum.exe\" \"%1\"".to_string(),
            },
        ]
    );
}

/// The macOS plan must be the plist fragment, verbatim.
#[test]
fn the_macos_plan_is_the_plist_fragment() {
    let paths = AppPaths::resolve(Platform::MacOs, &PathEnv::from_pairs([("HOME", "/Users/ada")]))
        .expect("HOME is set");
    let plan = plan_registration(Platform::MacOs, Path::new("/Applications/Vitrum.app"), &paths);
    let RegistrationPlan::BundleInfoPlist { fragment, note } = plan else {
        panic!("macOS must plan a plist fragment");
    };
    assert_eq!(
        fragment,
        "<key>CFBundleURLTypes</key>\n\
         <array>\n\
         \t<dict>\n\
         \t\t<key>CFBundleURLName</key>\n\
         \t\t<string>dev.santhreal.vitrum</string>\n\
         \t\t<key>CFBundleURLSchemes</key>\n\
         \t\t<array>\n\
         \t\t\t<string>vitrum</string>\n\
         \t\t</array>\n\
         \t</dict>\n\
         </array>\n"
    );
    assert!(note.contains("lsregister"), "the note must give the install step: {note}");
    assert!(note.contains("Info.plist"), "the note must name the file: {note}");
}

/// Applying the macOS plan must fail, saying why, rather than pretending.
///
/// This is the capability contract in miniature. There is no runtime API for an
/// unbundled binary to claim a URL scheme, and a backend that returned `Ok(())`
/// here would produce a settings screen with a working-looking "register" button
/// that does nothing at all.
#[test]
fn applying_the_macos_plan_reports_unimplemented_rather_than_succeeding() {
    let paths = AppPaths::resolve(Platform::MacOs, &PathEnv::from_pairs([("HOME", "/Users/ada")]))
        .expect("HOME is set");
    let plan = plan_registration(Platform::MacOs, Path::new("/Applications/Vitrum.app"), &paths);
    let err = apply_registration(&plan).expect_err("macOS has no runtime registration");
    assert_eq!(err.kind, UnavailableKind::NotImplementedOnPlatform);
    assert!(!err.kind.is_transient(), "retrying will never help");
    assert!(err.detail.contains("lsregister"));
}

/// Applying a registry plan on a non-Windows host must fail, not no-op.
#[cfg(not(target_os = "windows"))]
#[test]
fn applying_registry_values_off_windows_reports_unimplemented() {
    let plan = RegistrationPlan::RegistryValues {
        values: vec![RegistryValue {
            key: "Software\\Classes\\vitrum".to_string(),
            name: None,
            value: "x".to_string(),
        }],
    };
    let err = apply_registration(&plan).expect_err("there is no registry here");
    assert_eq!(err.kind, UnavailableKind::NotImplementedOnPlatform);
    assert_eq!(
        err.detail,
        "the Windows registry is only reachable from a Windows build"
    );
}
