//! Writing the platform icon set out of the binary that carries it.
//!
//! The mark is geometry, not a picture: [`vitrum_os::mark`] rasterises it and
//! [`vitrum_os::iconfile`] wraps the rasters in the three containers the
//! platforms read. This module is the operator-facing end of that, and it
//! exists because of where the icons have to arrive.
//!
//! A release archive holds two files, `vitrum` and `vitrum-server`, and that is
//! what the verified digest covers. An installed copy therefore cannot unpack
//! icons it was never shipped, and it cannot build them either: there is no
//! toolchain on the machine and `cargo run` is not a thing an installer may
//! assume. So the binary writes them itself, on demand, at install time. The
//! installer calls `vitrum icons "$HOME/.local/share"` and the launcher entry's
//! `Icon=vitrum` resolves immediately afterwards.
//!
//! It is also how the icons are regenerated for the repository and for a
//! Windows shortcut, so there is exactly one emitter and no second path that
//! could produce a different picture.

use vitrum_proto::exit::{self, Exit};

/// `vitrum icons` - write the platform icon set under a directory.
///
/// Returns a code from the one table in [`vitrum_proto::exit`], because the
/// caller is an installer script that branches on it. The filesystem refusing
/// is [`Exit::Unavailable`] rather than a flat failure: it means the set can
/// land once the directory exists or the permissions allow it, which is a
/// different instruction to the operator than "this did not work".
///
/// A partial set is never left behind;
/// [`vitrum_os::iconfile::write_icon_set`] unwinds what it wrote.
pub(crate) fn run_icons(args: &[String]) -> i32 {
    let mut dir: Option<std::path::PathBuf> = None;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{}", icons_usage());
                return Exit::Ok.code();
            }
            other if other.starts_with('-') => {
                eprintln!("vitrum icons: unknown option {other}\n\n{}", icons_usage());
                return Exit::Usage.code();
            }
            other if dir.is_none() => dir = Some(std::path::PathBuf::from(other)),
            other => {
                eprintln!(
                    "vitrum icons: unexpected argument {other}\n\n{}",
                    icons_usage()
                );
                return Exit::Usage.code();
            }
        }
    }

    let Some(dir) = dir else {
        eprintln!("vitrum icons needs a directory\n\n{}", icons_usage());
        return Exit::Usage.code();
    };

    match vitrum_os::iconfile::write_icon_set(&dir) {
        Ok(written) => {
            for path in &written {
                println!("{}", path.display());
            }
            Exit::Ok.code()
        }
        Err(e) => {
            eprintln!(
                "vitrum icons: could not write the icon set under {}: {e}\n\
                 Pass a directory you can write to, or run this again with the \
                 privileges that own it. Nothing was left behind.",
                dir.display()
            );
            refusal(&e).code()
        }
    }
}

/// Which code a filesystem refusal is.
///
/// Permission, a missing parent, a read-only mount or a full disk all mean the
/// same thing to the installer script that called this: the request was right
/// and the destination was not ready, so fixing the destination and running it
/// again is the move. Anything else is a genuine failure of the write.
fn refusal(e: &std::io::Error) -> Exit {
    use std::io::ErrorKind;
    match e.kind() {
        ErrorKind::PermissionDenied
        | ErrorKind::NotFound
        | ErrorKind::ReadOnlyFilesystem
        | ErrorKind::StorageFull => Exit::Unavailable,
        _ => Exit::Failed,
    }
}

/// Every code `vitrum icons` can exit with.
pub(crate) const EXIT_CODES: &[Exit] = &[Exit::Ok, Exit::Failed, Exit::Usage, Exit::Unavailable];

/// Help for `vitrum icons`.
pub(crate) fn icons_usage() -> String {
    format!(
        "vitrum icons - write the platform icon set\n\n\
         usage: vitrum icons <directory>\n\n\
         Rasterises the vitrum mark and writes, under <directory>:\n  \
         icons/hicolor/<n>x<n>/apps/vitrum.png   the freedesktop theme sizes\n  \
         icons/vitrum.ico                        the Windows icon\n  \
         icons/vitrum.icns                       the macOS icon\n\n\
         Pass a data directory, not an icon directory: `~/.local/share` is what\n\
         a desktop entry's `Icon=vitrum` is resolved against. The installer\n\
         calls this so a launcher entry has a picture; run it yourself after\n\
         moving the binary by hand.\n\n\
         Nothing is drawn from a file. The mark is geometry compiled into this\n\
         binary, so the set is identical on every machine and every release.\n\n\
         options:\n  \
         -h, --help           show this message\n\n\
         exit status:\n\
         {}",
        exit::status_lines(EXIT_CODES)
    )
}
