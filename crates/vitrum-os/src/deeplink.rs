//! The `vitrum://` URL scheme: parsing, and per-OS handler registration.
//!
//! A deep link arrives from somewhere hostile by construction. On Linux it is
//! an `Exec=` argument the desktop file handed to us; on Windows it is `%1`
//! substituted by the shell out of a registry value; on macOS it is an
//! `NSAppleEventDescriptor` payload. In every case a web page can cause one to
//! be produced. So the parser is strict, allocation-bounded, and rejects rather
//! than repairs: there is no input for which a wrong guess is better than an
//! error the caller can log.
//!
//! Registration is split into a pure [`plan_registration`] and an effecting
//! [`apply_registration`]. The plan is a value, so the exact desktop entry text
//! and the exact registry keys are asserted in tests on any machine, and the
//! macOS plan is a document rather than a lie: macOS resolves URL schemes from
//! a bundle's `Info.plist` at install time and there is no supported runtime
//! registration for an unbundled binary.

use core::fmt;
use std::path::{Path, PathBuf};

use vitrum_proto::{ProjectId, SessionId};

use crate::branding::{
    APP_COMMENT, APP_DISPLAY_NAME, APP_NAME, BUNDLE_ID, DESKTOP_FILE_NAME, ICON_NAME, URL_SCHEME,
};
use crate::capability::Unavailable;
use crate::paths::{AppPaths, Platform};

/// Longest URL accepted.
///
/// Well past any legitimate `vitrum://` link and short enough that a hostile
/// megabyte-long argument costs one length check instead of a scan.
pub const MAX_URL_LEN: usize = 2048;

/// Longest accepted identifier text. `u64::MAX` is 20 digits.
const MAX_ID_DIGITS: usize = 20;

/// What a `vitrum://` URL asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeepLink {
    /// `vitrum://session/<id>`: focus this session, opening its project if the
    /// window is not already showing it.
    Session(SessionId),
    /// `vitrum://project/<id>`: focus this project.
    Project(ProjectId),
    /// `vitrum://` or `vitrum://home`: just raise the window.
    Home,
}

impl DeepLink {
    /// The canonical URL for this target. Round-trips through [`parse`].
    pub fn to_url(self) -> String {
        match self {
            Self::Session(SessionId(id)) => format!("{URL_SCHEME}://session/{id}"),
            Self::Project(ProjectId(id)) => format!("{URL_SCHEME}://project/{id}"),
            Self::Home => format!("{URL_SCHEME}://home"),
        }
    }
}

impl fmt::Display for DeepLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_url())
    }
}

/// Why a URL was rejected.
///
/// Deliberately granular: an operator debugging "my link does nothing" needs to
/// know whether the scheme was wrong, the id was garbage, or the shell handed
/// us a truncated string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepLinkError {
    /// Nothing but whitespace.
    Empty,
    /// Longer than [`MAX_URL_LEN`].
    TooLong { len: usize },
    /// A control character anywhere in the URL. A NUL or an ESC in an argument
    /// is an injection attempt, never a real link.
    ControlCharacter { at: usize },
    /// Not `vitrum:`, or no scheme at all.
    WrongScheme { found: String },
    /// `vitrum:` without the `//` that introduces the authority. Rejected
    /// rather than guessed, because `vitrum:session/42` and
    /// `vitrum://session/42` would otherwise both parse and only one is what
    /// the registered handlers emit.
    MissingAuthority,
    /// The authority named something that is not a known target.
    UnknownTarget { target: String },
    /// A target that needs an identifier got none.
    MissingId { target: &'static str },
    /// The identifier was not a bare unsigned decimal that fits in `u64`.
    InvalidId { target: &'static str, value: String },
    /// More path segments than the target accepts.
    TrailingSegments { target: &'static str },
}

impl fmt::Display for DeepLinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "empty url"),
            Self::TooLong { len } => {
                write!(f, "url is {len} bytes, limit is {MAX_URL_LEN}")
            }
            Self::ControlCharacter { at } => {
                write!(f, "control character at byte {at}")
            }
            Self::WrongScheme { found } => {
                write!(f, "expected scheme `{URL_SCHEME}`, found `{found}`")
            }
            Self::MissingAuthority => write!(f, "expected `{URL_SCHEME}://`"),
            Self::UnknownTarget { target } => write!(f, "unknown target `{target}`"),
            Self::MissingId { target } => write!(f, "`{target}` needs an id"),
            Self::InvalidId { target, value } => {
                write!(f, "`{target}` id `{value}` is not an unsigned decimal that fits u64")
            }
            Self::TrailingSegments { target } => {
                write!(f, "`{target}` takes no further path segments")
            }
        }
    }
}

impl core::error::Error for DeepLinkError {}

/// Parse a `vitrum://` URL.
///
/// Accepts surrounding ASCII whitespace (a shell or an AppleEvent regularly
/// appends a newline), an ASCII-case-insensitive scheme and target (RFC 3986
/// makes both case-insensitive), one optional trailing slash, and an ignored
/// `?query` or `#fragment` (a browser appends them and the app has no use for
/// either). Everything else is an error.
///
/// Identifiers are strictly `[0-9]+` in range. `+42`, `-1`, ` 42`, `0x2a` and
/// percent-encoded digits are all rejected: `str::parse::<u64>` accepts a
/// leading `+`, and percent-decoding an identifier would let an attacker slip
/// a value past any log or filter that inspected the raw URL.
pub fn parse(url: &str) -> Result<DeepLink, DeepLinkError> {
    let url = url.trim_matches(|c: char| c.is_ascii_whitespace());
    if url.is_empty() {
        return Err(DeepLinkError::Empty);
    }
    if url.len() > MAX_URL_LEN {
        return Err(DeepLinkError::TooLong { len: url.len() });
    }
    if let Some((at, _)) = url.char_indices().find(|(_, c)| c.is_control()) {
        return Err(DeepLinkError::ControlCharacter { at });
    }

    let Some(colon) = url.find(':') else {
        return Err(DeepLinkError::WrongScheme { found: url.to_string() });
    };
    let (scheme, rest) = url.split_at(colon);
    if !scheme.eq_ignore_ascii_case(URL_SCHEME) {
        return Err(DeepLinkError::WrongScheme { found: scheme.to_string() });
    }
    let Some(rest) = rest.strip_prefix("://") else {
        return Err(DeepLinkError::MissingAuthority);
    };

    // Fragment first: a fragment may itself contain `?`.
    let rest = rest.split('#').next().unwrap_or("");
    let rest = rest.split('?').next().unwrap_or("");

    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    let target = authority.to_ascii_lowercase();

    let mut segments: Vec<&str> = path.split('/').collect();
    // One trailing slash is idiomatic and harmless; two empty tails are not.
    if segments.last() == Some(&"") {
        segments.pop();
    }

    match target.as_str() {
        "session" => {
            let id = single_id("session", &segments)?;
            Ok(DeepLink::Session(SessionId(id)))
        }
        "project" => {
            let id = single_id("project", &segments)?;
            Ok(DeepLink::Project(ProjectId(id)))
        }
        "" | "home" => {
            if segments.is_empty() {
                Ok(DeepLink::Home)
            } else {
                Err(DeepLinkError::TrailingSegments { target: "home" })
            }
        }
        _ => Err(DeepLinkError::UnknownTarget { target }),
    }
}

fn single_id(target: &'static str, segments: &[&str]) -> Result<u64, DeepLinkError> {
    match segments {
        [] => Err(DeepLinkError::MissingId { target }),
        [one] => parse_id(target, one),
        _ => Err(DeepLinkError::TrailingSegments { target }),
    }
}

fn parse_id(target: &'static str, text: &str) -> Result<u64, DeepLinkError> {
    let invalid = || DeepLinkError::InvalidId { target, value: text.to_string() };
    if text.is_empty() {
        return Err(DeepLinkError::MissingId { target });
    }
    if text.len() > MAX_ID_DIGITS || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    text.parse::<u64>().map_err(|_| invalid())
}

/// A registry value to create, as a plan rather than an effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryValue {
    /// Key path relative to `HKEY_CURRENT_USER`.
    pub key: String,
    /// `None` means the key's unnamed default value.
    pub name: Option<String>,
    /// Always a `REG_SZ` string for protocol registration.
    pub value: String,
}

/// Everything needed to make this OS route `vitrum://` to this executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationPlan {
    /// Linux: a freedesktop desktop entry declaring the scheme handler.
    DesktopEntry {
        /// Where the entry must be written.
        path: PathBuf,
        /// Exact file contents.
        contents: String,
        /// Argument vectors that refresh the desktop's MIME cache, program
        /// first. Advisory: most desktops pick the entry up on next login
        /// without them. Argv rather than a shell line, because the directory
        /// is under `$HOME` and a home directory may contain a space.
        post_install: Vec<Vec<String>>,
    },
    /// macOS: a bundle property, not a runtime call.
    BundleInfoPlist {
        /// The `CFBundleURLTypes` fragment to merge into `Info.plist`.
        fragment: String,
        /// Why this cannot be done at runtime.
        note: String,
    },
    /// Windows: per-user protocol registration under `HKCU\Software\Classes`.
    RegistryValues { values: Vec<RegistryValue> },
}

/// Build the registration plan for a platform without touching anything.
///
/// `exe` is the absolute path to the installed binary, and `paths` supplies the
/// data directory the Linux desktop entry lives under.
pub fn plan_registration(platform: Platform, exe: &Path, paths: &AppPaths) -> RegistrationPlan {
    match platform {
        Platform::Linux => {
            let exec = escape_desktop_exec(&exe.to_string_lossy());
            let applications = paths.data_dir.parent().map_or_else(
                || paths.data_dir.join("applications"),
                |share| share.join("applications"),
            );
            let path = applications.join(DESKTOP_FILE_NAME);
            let contents = format!(
                "[Desktop Entry]\n\
                 Type=Application\n\
                 Version=1.0\n\
                 Name={APP_DISPLAY_NAME}\n\
                 Comment={APP_COMMENT}\n\
                 Exec={exec} %u\n\
                 Icon={ICON_NAME}\n\
                 Terminal=false\n\
                 Categories=Development;TerminalEmulator;\n\
                 MimeType=x-scheme-handler/{URL_SCHEME};\n\
                 StartupNotify=true\n\
                 StartupWMClass={APP_NAME}\n"
            );
            let dir = applications.to_string_lossy().into_owned();
            RegistrationPlan::DesktopEntry {
                path,
                contents,
                post_install: vec![
                    vec!["update-desktop-database".to_string(), dir],
                    vec![
                        "xdg-mime".to_string(),
                        "default".to_string(),
                        DESKTOP_FILE_NAME.to_string(),
                        format!("x-scheme-handler/{URL_SCHEME}"),
                    ],
                ],
            }
        }
        Platform::MacOs => RegistrationPlan::BundleInfoPlist {
            fragment: format!(
                "<key>CFBundleURLTypes</key>\n\
                 <array>\n\
                 \t<dict>\n\
                 \t\t<key>CFBundleURLName</key>\n\
                 \t\t<string>{BUNDLE_ID}</string>\n\
                 \t\t<key>CFBundleURLSchemes</key>\n\
                 \t\t<array>\n\
                 \t\t\t<string>{URL_SCHEME}</string>\n\
                 \t\t</array>\n\
                 \t</dict>\n\
                 </array>\n"
            ),
            note: format!(
                "Launch Services reads CFBundleURLTypes from {APP_DISPLAY_NAME}.app/Contents/Info.plist \
                 when the bundle is first seen. Merge the fragment at build time, then run \
                 `/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
                 -f /Applications/{APP_DISPLAY_NAME}.app` to register without a logout. An unbundled \
                 binary cannot claim a scheme: there is no supported runtime API for it."
            ),
        },
        Platform::Windows => {
            let exe = exe.to_string_lossy();
            let root = format!("Software\\Classes\\{URL_SCHEME}");
            RegistrationPlan::RegistryValues {
                values: vec![
                    RegistryValue {
                        key: root.clone(),
                        name: None,
                        value: format!("URL:{APP_DISPLAY_NAME} Protocol"),
                    },
                    RegistryValue {
                        key: root.clone(),
                        name: Some("URL Protocol".to_string()),
                        value: String::new(),
                    },
                    RegistryValue {
                        key: format!("{root}\\DefaultIcon"),
                        name: None,
                        value: format!("\"{exe}\",0"),
                    },
                    RegistryValue {
                        key: format!("{root}\\shell\\open\\command"),
                        name: None,
                        value: format!("\"{exe}\" \"%1\""),
                    },
                ],
            }
        }
    }
}

/// Quote an `Exec=` value per the desktop entry specification.
///
/// The spec reserves ``" ` $ \`` inside a quoted argument and requires the whole
/// argument be quoted if it contains a space. An installation under
/// `/home/Some User/bin` is common enough that skipping this produces a handler
/// that silently never launches.
fn escape_desktop_exec(exe: &str) -> String {
    let needs_quotes = exe.contains(|c: char| {
        c.is_ascii_whitespace() || matches!(c, '"' | '\'' | '`' | '$' | '\\' | '>' | '<' | '~' | '|' | '&' | ';' | '*' | '?' | '#' | '(' | ')')
    });
    if !needs_quotes {
        return exe.to_string();
    }
    let mut out = String::with_capacity(exe.len() + 2);
    out.push('"');
    for c in exe.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// What applying a plan actually did.
///
/// Registration runs from an installer and again on first launch, so "I wrote
/// the entry" and "it was already correct" are different facts and both are
/// worth logging. A bare `Ok(())` cannot tell a fresh install from a no-op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationOutcome {
    /// Files or registry values this plan owns.
    pub targets: Vec<String>,
    /// Nothing was written, because every target already held the planned
    /// value.
    pub already_current: bool,
    /// Cache-refresh commands attempted, each with what came of it. Empty when
    /// nothing was written, because there is then nothing to refresh.
    pub refreshed: Vec<String>,
}

impl fmt::Display for RegistrationOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verb = if self.already_current { "already registered" } else { "registered" };
        write!(f, "{verb}: {}", self.targets.join(", "))?;
        for line in &self.refreshed {
            write!(f, "; {line}")?;
        }
        Ok(())
    }
}

/// Make this OS route `vitrum://` to the running executable, for this user.
///
/// The one call an installer or a first run makes. Idempotent: a second call
/// from the same path writes nothing and reports `already_current`. On macOS
/// it fails with the reason, because a scheme belongs to a bundle's
/// `Info.plist` and there is no runtime API to claim one.
pub fn register_this_executable() -> Result<RegistrationOutcome, Unavailable> {
    let exe = std::env::current_exe().map_err(|e| {
        Unavailable::runtime_error(format!("cannot resolve this executable's path: {e}"))
    })?;
    let paths = AppPaths::for_current_platform().map_err(|e| {
        Unavailable::runtime_error(format!("cannot resolve this platform's directories: {e}"))
    })?;
    apply_registration(&plan_registration(Platform::current(), &exe, &paths))
}

/// Carry out a plan on the running platform.
///
/// Reports the path or key set the plan owns and whether it had to touch it,
/// so a caller can log exactly what changed. On macOS this always reports
/// unavailable, with the reason.
pub fn apply_registration(plan: &RegistrationPlan) -> Result<RegistrationOutcome, Unavailable> {
    match plan {
        RegistrationPlan::DesktopEntry { path, contents, post_install } => {
            let targets = vec![path.to_string_lossy().into_owned()];
            if std::fs::read_to_string(path).is_ok_and(|current| current == *contents) {
                return Ok(RegistrationOutcome {
                    targets,
                    already_current: true,
                    refreshed: Vec::new(),
                });
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    Unavailable::runtime_error(format!("cannot create {}: {e}", parent.display()))
                })?;
            }
            std::fs::write(path, contents).map_err(|e| {
                Unavailable::runtime_error(format!("cannot write {}: {e}", path.display()))
            })?;
            let refreshed = post_install.iter().map(|argv| run_post_install(argv)).collect();
            Ok(RegistrationOutcome { targets, already_current: false, refreshed })
        }
        RegistrationPlan::BundleInfoPlist { note, .. } => {
            Err(Unavailable::not_implemented(note.clone()))
        }
        RegistrationPlan::RegistryValues { values } => {
            if registry_values_are_current(values) {
                return Ok(RegistrationOutcome {
                    targets: values.iter().map(registry_target).collect(),
                    already_current: true,
                    refreshed: Vec::new(),
                });
            }
            Ok(RegistrationOutcome {
                targets: write_registry_values(values)?,
                already_current: false,
                refreshed: Vec::new(),
            })
        }
    }
}

/// How a registry value is named in a report.
fn registry_target(value: &RegistryValue) -> String {
    match &value.name {
        Some(name) => format!("HKCU\\{}\\{name}", value.key),
        None => format!("HKCU\\{}", value.key),
    }
}

/// Run one cache-refresh command and describe the result.
///
/// A missing `update-desktop-database` is not a failure: the entry is written
/// and every desktop reads it at next login. It is reported so an operator
/// debugging "the link only works after I log out" can see why.
fn run_post_install(argv: &[String]) -> String {
    let shown = argv.join(" ");
    let Some((program, args)) = argv.split_first() else {
        return shown;
    };
    let result = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match result {
        Ok(status) if status.success() => format!("{shown}: ok"),
        Ok(status) => format!("{shown}: failed ({status})"),
        Err(e) => format!("{shown}: not run ({e})"),
    }
}

#[cfg(target_os = "windows")]
fn write_registry_values(values: &[RegistryValue]) -> Result<Vec<String>, Unavailable> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegSetValueExW,
    };
    use windows::core::PCWSTR;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(core::iter::once(0)).collect()
    }

    let mut written = Vec::with_capacity(values.len());
    for value in values {
        let key_w = wide(&value.key);
        let mut hkey = HKEY::default();
        // SAFETY: `key_w` is NUL-terminated and outlives the call; `hkey` is a
        // valid out-parameter.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(key_w.as_ptr()),
                None,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut hkey,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(Unavailable::permission_denied(format!(
                "RegCreateKeyEx(HKCU\\{}) failed with {}",
                value.key, status.0
            )));
        }

        let name_w = value.name.as_deref().map(wide);
        let data: Vec<u16> = wide(&value.value);
        let bytes: &[u8] = unsafe {
            core::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * 2)
        };
        // SAFETY: `bytes` borrows `data`, which outlives the call, and its
        // length is exactly the UTF-16 payload including the terminator, as
        // REG_SZ requires.
        let status = unsafe {
            RegSetValueExW(
                hkey,
                name_w.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr())),
                None,
                REG_SZ,
                Some(bytes),
            )
        };
        // SAFETY: `hkey` came from a successful RegCreateKeyExW and is not used
        // again.
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        if status != ERROR_SUCCESS {
            return Err(Unavailable::permission_denied(format!(
                "RegSetValueEx(HKCU\\{}, {:?}) failed with {}",
                value.key, value.name, status.0
            )));
        }
        written.push(registry_target(value));
    }
    Ok(written)
}

#[cfg(not(target_os = "windows"))]
fn write_registry_values(_values: &[RegistryValue]) -> Result<Vec<String>, Unavailable> {
    Err(Unavailable::not_implemented(
        "the Windows registry is only reachable from a Windows build",
    ))
}

/// Whether every planned value is already in the registry with that exact
/// string, so a re-run can report that it changed nothing.
#[cfg(target_os = "windows")]
fn registry_values_are_current(values: &[RegistryValue]) -> bool {
    values.iter().all(|value| read_registry_string(value).as_deref() == Some(&*value.value))
}

/// Read one `REG_SZ` under `HKEY_CURRENT_USER`, or `None` if it is absent or
/// of another type.
#[cfg(target_os = "windows")]
fn read_registry_string(value: &RegistryValue) -> Option<String> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegOpenKeyExW,
        RegQueryValueExW,
    };
    use windows::core::PCWSTR;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(core::iter::once(0)).collect()
    }

    let key_w = wide(&value.key);
    let mut hkey = HKEY::default();
    // SAFETY: `key_w` is NUL-terminated and outlives the call.
    let status = unsafe {
        RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(key_w.as_ptr()), None, KEY_READ, &mut hkey)
    };
    if status != ERROR_SUCCESS {
        return None;
    }

    let name_w = value.name.as_deref().map(wide);
    let name = name_w.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
    let mut kind = REG_VALUE_TYPE::default();
    let mut size: u32 = 0;
    // SAFETY: a null data pointer with a live size asks only for the length,
    // which is the documented way to size the buffer.
    let status =
        unsafe { RegQueryValueExW(hkey, name, None, Some(&mut kind), None, Some(&mut size)) };
    let mut text = None;
    if status == ERROR_SUCCESS && kind == REG_SZ {
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: `buf` is live for the call and `size` states its exact byte
        // length.
        let status = unsafe {
            RegQueryValueExW(
                hkey,
                name,
                None,
                Some(&mut kind),
                Some(buf.as_mut_ptr().cast::<u8>()),
                Some(&mut size),
            )
        };
        if status == ERROR_SUCCESS && kind == REG_SZ {
            let len = (size as usize / 2).min(buf.len());
            let chars = &buf[..len];
            let chars = chars.strip_suffix(&[0]).unwrap_or(chars);
            text = Some(String::from_utf16_lossy(chars));
        }
    }
    // SAFETY: `hkey` came from a successful open and is not used again.
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    text
}

/// Off Windows there is no registry to compare against, and applying the plan
/// fails rather than pretending, so nothing is ever already current.
#[cfg(not(target_os = "windows"))]
fn registry_values_are_current(_values: &[RegistryValue]) -> bool {
    false
}
