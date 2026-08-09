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

/// `vitrum icons` - write the platform icon set under a directory.
///
/// Returns the process exit code, because the caller is an installer script
/// that branches on it: `0` when the whole set landed, `2` for a bad
/// invocation, and `1` when the filesystem refused. A partial set is never
/// left behind; [`vitrum_os::iconfile::write_icon_set`] unwinds what it wrote.
pub(crate) fn run_icons(args: &[String]) -> i32 {
    let mut dir: Option<std::path::PathBuf> = None;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{}", icons_usage());
                return 0;
            }
            other if other.starts_with('-') => {
                eprintln!("vitrum icons: unknown option {other}\n\n{}", icons_usage());
                return 2;
            }
            other if dir.is_none() => dir = Some(std::path::PathBuf::from(other)),
            other => {
                eprintln!("vitrum icons: unexpected argument {other}\n\n{}", icons_usage());
                return 2;
            }
        }
    }

    let Some(dir) = dir else {
        eprintln!("vitrum icons needs a directory\n\n{}", icons_usage());
        return 2;
    };

    match vitrum_os::iconfile::write_icon_set(&dir) {
        Ok(written) => {
            for path in &written {
                println!("{}", path.display());
            }
            0
        }
        Err(e) => {
            eprintln!("vitrum icons: {e}");
            1
        }
    }
}

/// Help for `vitrum icons`.
pub(crate) fn icons_usage() -> String {
    "vitrum icons - write the platform icon set\n\n\
     usage: vitrum icons <directory>\n\n\
     Rasterises the vitrum mark and writes, under <directory>:\n  \
     icons/hicolor/<n>x<n>/apps/vitrum.png   the freedesktop theme sizes\n  \
     icons/vitrum.ico                        the Windows icon\n  \
     icons/vitrum.icns                       the macOS icon\n\n\
     Pass a data directory, not an icon directory: `~/.local/share` is what a\n\
     desktop entry's `Icon=vitrum` is resolved against. The installer calls\n\
     this so a launcher entry has a picture; run it yourself after moving the\n\
     binary by hand.\n\n\
     Nothing is drawn from a file. The mark is geometry compiled into this\n\
     binary, so the set is identical on every machine and every release.\n\n\
     options:\n  \
     -h, --help           show this message\n\n\
     exit status:\n  \
     0                    every file was written\n  \
     1                    the directory could not be written to\n  \
     2                    no directory, or an argument that does not exist\n"
        .to_string()
}
