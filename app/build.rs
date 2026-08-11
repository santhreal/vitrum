//! Records the target triple this binary was built for, and on Windows puts
//! the mark inside the executable.
//!
//! The updater downloads one archive per platform and has to name the right
//! one. That name has to describe the machine the binary was produced for, not
//! the machine it happens to be running on, so it is captured at compile time
//! from cargo's own `TARGET` rather than assembled at runtime from
//! `std::env::consts`, which cannot distinguish a gnu build from a musl one.
//!
//! # The icon
//!
//! Three platforms ask for the application's picture in three different
//! places. Linux reads it from the icon theme, which the installer writes.
//! Windows reads it from the executable itself: an `.exe` with no `RT_GROUP_ICON`
//! resource is drawn by Explorer, the taskbar, Alt-Tab and the shortcut the
//! installer creates with the generic binary glyph, and no amount of
//! `with_window_icon` at runtime changes any of those four, because none of
//! them ever runs the program.
//!
//! The `.ico` is generated here rather than checked in. `vitrum_os::iconfile`
//! already writes that container from the same procedural geometry the window
//! icon and the installer use, so the resource is a build artefact of the mark
//! rather than a fourth copy of it that can drift. That is also why there is no
//! ImageMagick, `rc.exe` invocation by hand, or committed binary in this
//! repository.

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=VITRUM_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_icon();
    }
}

/// Write the `.ico` and hand it to the resource compiler.
///
/// A failure here is a warning rather than a panic, deliberately. The icon is
/// cosmetic and the resource compiler is the one part of this that depends on
/// a toolchain component this build script does not own; refusing to produce a
/// working binary because Explorer would have drawn the wrong glyph is the
/// wrong trade. The warning says which half failed.
#[cfg(windows)]
fn embed_windows_icon() {
    let out = std::path::PathBuf::from(
        std::env::var("OUT_DIR").expect("cargo sets OUT_DIR for build scripts"),
    );
    let path = out.join("vitrum.ico");
    let images = vitrum_os::mark::mark_set(vitrum_os::mark::MARK_COLOUR);
    if let Err(why) = std::fs::write(&path, vitrum_os::iconfile::ico(&images)) {
        println!("cargo:warning=could not write {}: {why}", path.display());
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon(&path.to_string_lossy());
    if let Err(why) = res.compile() {
        println!("cargo:warning=the executable has no icon resource: {why}");
    }
}

/// Nothing to embed when the host is not Windows.
///
/// Cross-compiling to Windows from another host is not a path this product
/// takes. The binaries link the platform's own toolkit and pty, so every
/// target is built on its own machine, and a resource compiler that is not
/// there is not worth a second code path.
#[cfg(not(windows))]
fn embed_windows_icon() {
    println!(
        "cargo:warning=building for Windows from a non-Windows host: the \
         executable will carry no icon resource"
    );
}
