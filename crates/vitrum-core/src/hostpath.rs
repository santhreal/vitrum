//! Building and judging a path the way a target platform spells it, from any
//! host.
//!
//! `Path::join` and `Path::is_absolute` answer for the box the daemon is running
//! on. A Linux build resolving a Windows layout joined `C:\Windows\System32`
//! with `where` and produced `C:\Windows\System32/where`, and called
//! `C:\Windows\System32\cmd.exe` a relative path. Windows itself does not mind a
//! forward slash, so this never broke on Windows; it broke every attempt to
//! check the Windows rules from anywhere else, and a rule that can only be
//! checked on the platform it describes is a rule nobody checks.

use std::path::{Path, PathBuf};

/// The separator a POSIX host writes.
pub(crate) const POSIX: char = '/';
/// The separator Windows writes. It also accepts `/` everywhere.
pub(crate) const WINDOWS: char = '\\';

/// Rewrite every separator in `value` as `separator`.
///
/// Windows accepts `/` everywhere and normalises it inside the kernel, so
/// `C:\src\tools/run` and `C:\src\tools\run` are one file there and two strings
/// here. Under POSIX rules nothing is rewritten: a backslash is an ordinary
/// character in a Unix file name and replacing it would name a different file.
pub(crate) fn spell(separator: char, value: &str) -> String {
    if separator == POSIX {
        return value.to_string();
    }
    value.replace(POSIX, "\\")
}

/// Append `child` to `base`, spelled with `separator`.
pub(crate) fn join(separator: char, base: &Path, child: &str) -> PathBuf {
    let mut out = spell(separator, &base.as_os_str().to_string_lossy());
    let child = spell(separator, child);
    if !out.is_empty() && !out.ends_with(separator) {
        out.push(separator);
    }
    out.push_str(&child);
    PathBuf::from(out)
}

/// Whether `value` is rooted under Windows' rules.
///
/// A drive qualifier, a UNC share, or a device path. A bare `\path` is rooted
/// and still resolves against whichever drive is current, so it is not somewhere
/// a resolver may stop looking.
pub(crate) fn windows_rooted(value: &str) -> bool {
    if value.starts_with(r"\\") || value.starts_with("//") {
        return true;
    }
    let mut chars = value.chars();
    let Some(drive) = chars.next() else {
        return false;
    };
    if !drive.is_ascii_alphabetic() || chars.next() != Some(':') {
        return false;
    }
    // `C:` alone names the current directory on that drive, which is not a
    // location. `C:\` and `C:/` are.
    matches!(chars.next(), Some(WINDOWS | POSIX))
}

/// Whether `value` carries a drive qualifier, rooted or not.
///
/// `C:tools\run` is drive-relative: it resolves against the current directory of
/// drive C, never against the session's directory, so it must not be joined onto
/// one.
pub(crate) fn windows_drive_qualified(value: &str) -> bool {
    let mut chars = value.chars();
    matches!((chars.next(), chars.next()), (Some(c), Some(':')) if c.is_ascii_alphabetic())
}
