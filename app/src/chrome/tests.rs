//! The backdrop protocol, which is the one place in this process that turns a
//! string from `ui.json` into a filesystem read.
//!
//! Everything else in `chrome.rs` builds a window and needs an event loop to
//! observe, so it is proven by the screenshot regression rig instead. These
//! two helpers are pure, and they are the ones where a mistake is a security
//! bug rather than a cosmetic one.

use super::{backdrop_path, image_mime, percent_decode};

/// A relative path is refused.
///
/// WHY: the resolver would otherwise join it against the process's working
/// directory, which is wherever the operator happened to launch from. That
/// turns one stored string into a different file per launch, and it is the
/// half of a traversal that does not announce itself with `..`.
#[test]
fn a_relative_backdrop_path_is_refused() {
    assert_eq!(backdrop_path("etc/passwd"), None);
    assert_eq!(backdrop_path(""), None);
    assert_eq!(backdrop_path("./wallpaper.png"), None);
}

/// A path containing `..` is refused, however it is spelled.
///
/// WHY: the encoded spelling is the interesting one. `%2e%2e` survives a
/// check written against the raw URL and arrives at the filesystem as `..`,
/// so the check has to run after decoding. Decode-then-inspect is the order
/// this asserts.
#[test]
fn a_traversing_backdrop_path_is_refused() {
    assert_eq!(backdrop_path("/home/you/../../etc/shadow"), None);
    assert_eq!(backdrop_path("/home/you/%2e%2e/%2e%2e/etc/shadow"), None);
    assert_eq!(backdrop_path("/.."), None);
}

/// An ordinary absolute path survives, including one with a space in it.
///
/// WHY: the guard above is worthless if it is so strict that the feature does
/// not work. `~/My Pictures/wall paper.png` is the common case, and it is
/// exactly the case percent-encoding exists for.
#[test]
fn an_absolute_backdrop_path_survives() {
    assert_eq!(
        backdrop_path("/home/you/wall.png").as_deref(),
        Some(std::path::Path::new("/home/you/wall.png"))
    );
    assert_eq!(
        backdrop_path("/home/you/My%20Pictures/wall%20paper.png").as_deref(),
        Some(std::path::Path::new("/home/you/My Pictures/wall paper.png"))
    );
    // A directory named `..something` is not a traversal and is not refused.
    assert!(backdrop_path("/home/you/..hidden/wall.png").is_some());
}

/// A round trip through the encoder and the decoder is the identity.
///
/// WHY: these two are written in different files by different halves of this
/// feature, and a mismatch shows up as a backdrop that silently fails to load
/// for anyone whose path is not pure ASCII.
#[test]
fn the_backdrop_url_round_trips() {
    for path in [
        "/home/you/wall.png",
        "/home/you/My Pictures/wall paper.png",
        "/home/you/\u{e5}rstid/v\u{e5}r.jpg",
        "/home/you/100% done/x.gif",
        "/home/you/a+b&c.png",
    ] {
        let url = crate::ui::settings::backdrop_url(path);
        let encoded = url
            .strip_prefix("vitrum-backdrop://local")
            .expect("the encoder emits its own scheme and authority");
        assert_eq!(
            percent_decode(encoded).as_deref(),
            Some(path),
            "round trip failed for {path}"
        );
    }
}

/// A Windows path survives the encoder on every platform.
///
/// WHY: `C:\\Users\\you\\wall.png` does not begin with a slash, so without one
/// grown for it the URL would read `vitrum-backdrop://localC%3A...` and the
/// authority would swallow the drive letter. This half is pure string work, so
/// Linux CI can and must catch it; the matching `is_absolute` handling is
/// platform-dependent and is asserted separately below.
#[test]
fn a_windows_path_keeps_its_separator_from_the_authority() {
    let url = crate::ui::settings::backdrop_url("C:\\Users\\you\\wall.png");
    assert!(url.starts_with("vitrum-backdrop://local/"), "{url}");
    let encoded = url
        .strip_prefix("vitrum-backdrop://local")
        .expect("the encoder emits its own scheme and authority");
    assert_eq!(
        percent_decode(encoded).as_deref(),
        Some("/C:\\Users\\you\\wall.png"),
        "the handler strips the grown slash back off on Windows"
    );
}

/// A backslash traversal is refused on Windows.
///
/// WHY: `..\\` is the traversal spelling that a split on `/` never sees. The
/// guard walks path components instead, which is separator-aware, and this
/// pins that it stays that way on the platform where it matters.
#[cfg(windows)]
#[test]
fn a_backslash_traversal_is_refused() {
    assert_eq!(backdrop_path("/C:/Users/you/../../Windows/win.ini"), None);
    assert_eq!(
        backdrop_path("/C:\\Users\\you\\..\\..\\Windows\\win.ini"),
        None
    );
    assert_eq!(backdrop_path("/C:%5CUsers%5Cyou%5C%2e%2e%5Cwin.ini"), None);
    // The ordinary case still works, or the guard is just an outage.
    assert!(backdrop_path("/C:%5CUsers%5Cyou%5Cwall.png").is_some());
}

/// A malformed escape decodes to nothing rather than to garbage.
#[test]
fn a_malformed_escape_is_refused() {
    assert_eq!(percent_decode("/a%"), None);
    assert_eq!(percent_decode("/a%2"), None);
    assert_eq!(percent_decode("/a%zz"), None);
    // Valid escapes that do not form UTF-8 are refused rather than lossily
    // replaced: a path is bytes the operator typed, not a best guess.
    assert_eq!(percent_decode("/a%ff%fe"), None);
}

/// Images are recognised by signature.
#[test]
fn the_image_signatures_are_recognised() {
    assert_eq!(image_mime(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
    assert_eq!(image_mime(b"\xff\xd8\xff\xe0rest"), Some("image/jpeg"));
    assert_eq!(image_mime(b"GIF89arest"), Some("image/gif"));
    assert_eq!(image_mime(b"GIF87arest"), Some("image/gif"));
    assert_eq!(image_mime(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
}

/// Anything that is not an image is refused, and SVG especially.
///
/// WHY: this response is rendered inside the application page, which holds
/// the bridge to the process. SVG is a scripted document, so serving one on
/// the strength of a `.svg` name would be script injection with extra steps.
/// It has no binary signature, so refusing it is the same code path as
/// refusing a text file, and this test pins that it stays that way.
#[test]
fn a_non_image_is_refused() {
    assert_eq!(
        image_mime(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
        None
    );
    assert_eq!(image_mime(b"<?xml version=\"1.0\"?><svg/>"), None);
    assert_eq!(image_mime(b"root:x:0:0:root:/root:/bin/bash\n"), None);
    assert_eq!(image_mime(b"\x7fELF"), None);
    assert_eq!(image_mime(b""), None);
    // A prefix of a real signature is not a signature. PNG's is eight bytes
    // and these are four, so this is a truncated file, not a PNG.
    assert_eq!(image_mime(b"\x89PNG"), None);
    assert_eq!(image_mime(b"RIFF\0\0\0\0AVI "), None);
    assert_eq!(image_mime(b"RIFF"), None);
}
