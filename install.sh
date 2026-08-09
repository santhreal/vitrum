#!/usr/bin/env sh
#
# vitrum installer (Linux + macOS).
#
# Downloads one published release archive, verifies it against the release
# `SHA256SUMS`, and installs `vitrum` and `vitrum-server` into a directory on
# your `PATH`. Nothing is installed unless the checksum matches.
#
#   sh install.sh                 install the latest release
#   sh install.sh 0.1.0           install a specific version
#   sh install.sh --help          full usage
#
# Env overrides:
#   VITRUM_VERSION       version to install, same as the argument
#   VITRUM_INSTALL_DIR   where the binaries go (default: $HOME/.local/bin)
#   GITHUB_TOKEN         sent to the GitHub API, for rate-limited networks

set -eu

REPO="santhreal/vitrum"
VERSION="${VITRUM_VERSION:-}"
INSTALL_DIR="${VITRUM_INSTALL_DIR:-$HOME/.local/bin}"
INTEGRATE=1
if [ -n "${VITRUM_NO_INTEGRATE:-}" ]; then INTEGRATE=0; fi
TMPDIR_SELF=""

# ============================================================
# output
# ============================================================

say() { printf '%s\n' "$*"; }

# Every failure leaves through here, so every failure names what to do next.
die() {
    printf 'error: %s\n' "$1" >&2
    shift
    for line in "$@"; do
        printf '  %s\n' "$line" >&2
    done
    exit 1
}

warn() { printf 'warning: %s\n' "$*" >&2; }

cleanup() {
    if [ -n "$TMPDIR_SELF" ] && [ -d "$TMPDIR_SELF" ]; then
        rm -rf "$TMPDIR_SELF"
    fi
}
trap cleanup EXIT HUP INT TERM

usage() {
    cat <<'EOF'
Install vitrum, one interface for every agent TUI you have running.

Usage:
  sh install.sh [VERSION] [options]

Arguments:
  VERSION                 version to install, with or without a leading `v`
                          (default: the latest published release)

Options:
  --install-dir=PATH      where to put the binaries
                          (default: $HOME/.local/bin)
  --version=VERSION       same as the positional VERSION argument
  --no-integrate          install the binaries only: no launcher entry, no
                          PATH edit, no `vu` shortcut
  -h, --help              show this help and exit

Environment:
  VITRUM_VERSION          same as --version
  VITRUM_INSTALL_DIR      same as --install-dir
  GITHUB_TOKEN            bearer token for the GitHub API
  VITRUM_NO_INTEGRATE     set to anything for --no-integrate

Beyond the binaries, the installer adds a launcher entry, puts the install
directory on your `PATH`, and defines `vu` as `vitrum update`. Each step is
idempotent and each is skipped by --no-integrate.

The installer downloads `vitrum-<version>-<target>.tar.gz` and the release
`SHA256SUMS`, refuses to install if the digests disagree, and then places
`vitrum` and `vitrum-server` in the install directory. Both are needed: the
client will not run without the daemon beside it or on your `PATH`.

Published targets are x86_64 Linux (glibc), Apple silicon macOS, Intel macOS,
and x86_64 Windows. On any other platform, build from source:

  git clone https://github.com/santhreal/vitrum && cd vitrum && cargo build --release
EOF
}

# ============================================================
# arguments
# ============================================================

while [ $# -gt 0 ]; do
    case "$1" in
        -h | --help)
            usage
            exit 0
            ;;
        --install-dir=*)
            INSTALL_DIR="${1#--install-dir=}"
            ;;
        --install-dir)
            [ $# -ge 2 ] || die "--install-dir needs a path" \
                "Pass it as --install-dir=$HOME/.local/bin."
            INSTALL_DIR="$2"
            shift
            ;;
        --version=*)
            VERSION="${1#--version=}"
            ;;
        --version)
            [ $# -ge 2 ] || die "--version needs a version" \
                "Pass it as --version=0.1.0."
            VERSION="$2"
            shift
            ;;
        --no-integrate)
            INTEGRATE=0
            ;;
        -*)
            die "unknown option: $1" "Run 'sh install.sh --help' for the options."
            ;;
        *)
            VERSION="$1"
            ;;
    esac
    shift
done

# The tag carries a leading `v`; the version inside the asset name does not.
# Accepting either spelling here is what keeps `install.sh v0.1.0` and
# `install.sh 0.1.0` from building two different URLs.
VERSION="${VERSION#v}"

[ -n "$INSTALL_DIR" ] || die "install directory is empty" \
    "Pass --install-dir=PATH or unset VITRUM_INSTALL_DIR."

# ============================================================
# tools
# ============================================================

need() {
    command -v "$1" >/dev/null 2>&1 || die "$1 is required and was not found" "$2"
}

if command -v curl >/dev/null 2>&1; then
    FETCH="curl"
elif command -v wget >/dev/null 2>&1; then
    FETCH="wget"
else
    die "neither curl nor wget is available" \
        "Install one of them, or download the archive by hand from" \
        "https://github.com/$REPO/releases and unpack it into $INSTALL_DIR."
fi

need tar "Install tar, or unpack the release archive by hand."

# macOS ships `shasum`, Linux ships `sha256sum`. The verification step is not
# optional, so a host with neither is a host this script refuses to install on.
if command -v sha256sum >/dev/null 2>&1; then
    sha256_of() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
    sha256_of() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
    die "no SHA-256 tool found (looked for sha256sum and shasum)" \
        "Install coreutils (sha256sum) or perl (shasum)." \
        "The download cannot be verified without one, and this installer" \
        "will not install an unverified binary."
fi

fetch() {
    # $1 url, $2 destination
    if [ "$FETCH" = "curl" ]; then
        curl -fsSL --retry 3 -o "$2" "$1"
    else
        wget -q -O "$2" "$1"
    fi
}

fetch_api() {
    # $1 url. Prints the body. GITHUB_TOKEN lifts the anonymous rate limit.
    if [ "$FETCH" = "curl" ]; then
        if [ -n "${GITHUB_TOKEN:-}" ]; then
            curl -fsSL --retry 3 -H "Authorization: Bearer $GITHUB_TOKEN" \
                -H "X-GitHub-Api-Version: 2022-11-28" "$1"
        else
            curl -fsSL --retry 3 "$1"
        fi
    else
        if [ -n "${GITHUB_TOKEN:-}" ]; then
            wget -q -O - --header="Authorization: Bearer $GITHUB_TOKEN" "$1"
        else
            wget -q -O - "$1"
        fi
    fi
}

# ============================================================
# platform
# ============================================================

os=$(uname -s)
arch=$(uname -m)

# Exactly the four triples `.github/workflows/release.yml` builds and uploads.
# Anything else has no asset to download, so it is told to build from source
# rather than sent to a URL that will 404.
case "$os" in
    Linux)
        case "$arch" in
            x86_64 | amd64) TARGET="x86_64-unknown-linux-gnu" ;;
            *)
                die "no published release for Linux $arch" \
                    "Releases carry x86_64 Linux only." \
                    "Build from source instead: https://github.com/$REPO/blob/main/CONTRIBUTING.md" \
                    "You will need a WebKitGTK 4.1 development package first; see" \
                    "https://github.com/$REPO/blob/main/CONTRIBUTING.md"
                ;;
        esac
        ;;
    Darwin)
        case "$arch" in
            arm64 | aarch64) TARGET="aarch64-apple-darwin" ;;
            x86_64) TARGET="x86_64-apple-darwin" ;;
            *)
                die "no published release for macOS $arch" \
                    "Releases carry Apple silicon and Intel macOS only." \
                    "Build from source instead: https://github.com/$REPO/blob/main/CONTRIBUTING.md"
                ;;
        esac
        ;;
    *)
        die "this installer supports Linux and macOS; found $os" \
            "On Windows, use install.ps1 from the same repository." \
            "Anywhere else, build from source: https://github.com/$REPO/blob/main/CONTRIBUTING.md"
        ;;
esac

# ============================================================
# version
# ============================================================

if [ -z "$VERSION" ]; then
    say "Resolving the latest release of $REPO."
    latest=$(fetch_api "https://api.github.com/repos/$REPO/releases/latest") || latest=""
    [ -n "$latest" ] || die "could not reach the GitHub releases API" \
        "Check your network, or pass an explicit version:" \
        "  sh install.sh 0.1.0" \
        "Published versions are listed at https://github.com/$REPO/releases"
    VERSION=$(printf '%s\n' "$latest" |
        sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' | head -1)
    [ -n "$VERSION" ] || die "the releases API returned no tag_name" \
        "Pass an explicit version: sh install.sh 0.1.0" \
        "Published versions are listed at https://github.com/$REPO/releases"
fi

ARCHIVE="vitrum-${VERSION}-${TARGET}.tar.gz"
BASE="https://github.com/$REPO/releases/download/v${VERSION}"

say ""
say "  version      v${VERSION}"
say "  target       ${TARGET}"
say "  archive      ${ARCHIVE}"
say "  install to   ${INSTALL_DIR}"
say "  binaries     vitrum, vitrum-server"
say ""

# ============================================================
# download and verify
# ============================================================

TMPDIR_SELF=$(mktemp -d 2>/dev/null || mktemp -d -t vitrum) ||
    die "could not create a temporary directory" \
        "Check that TMPDIR points somewhere writable."

say "Downloading $ARCHIVE."
fetch "$BASE/$ARCHIVE" "$TMPDIR_SELF/$ARCHIVE" ||
    die "could not download $BASE/$ARCHIVE" \
        "Check that v${VERSION} is published and carries an asset for ${TARGET}:" \
        "  https://github.com/$REPO/releases/tag/v${VERSION}"

say "Downloading SHA256SUMS."
fetch "$BASE/SHA256SUMS" "$TMPDIR_SELF/SHA256SUMS" ||
    die "could not download $BASE/SHA256SUMS" \
        "Every vitrum release publishes it, so a release without one is" \
        "incomplete and must not be installed. Report it at" \
        "https://github.com/$REPO/issues"

# Matched literally rather than with a regex: the archive name is full of dots,
# and a dot in a pattern matches anything, which would accept a digest filed
# under a neighbouring platform's archive.
expected=""
while read -r digest name; do
    name="${name#\*}"
    if [ "$name" = "$ARCHIVE" ]; then
        expected="$digest"
        break
    fi
done < "$TMPDIR_SELF/SHA256SUMS"
[ -n "$expected" ] || die "SHA256SUMS has no entry for $ARCHIVE" \
    "The release is inconsistent with its own checksum file and this" \
    "installer will not install an unverified archive. Report it at" \
    "https://github.com/$REPO/issues"

actual=$(sha256_of "$TMPDIR_SELF/$ARCHIVE")
if [ "$actual" != "$expected" ]; then
    die "checksum mismatch for $ARCHIVE; nothing was installed" \
        "expected $expected" \
        "actual   $actual" \
        "Do not use this download. Retry, and if it fails again report it at" \
        "https://github.com/$REPO/issues"
fi
say "Checksum verified."

# ============================================================
# install
# ============================================================

mkdir -p "$INSTALL_DIR" ||
    die "could not create $INSTALL_DIR" \
        "Pick a writable directory with --install-dir=PATH."
[ -w "$INSTALL_DIR" ] ||
    die "$INSTALL_DIR is not writable" \
        "Pick a writable directory with --install-dir=PATH, or run this as the" \
        "user that owns $INSTALL_DIR. Do not run the installer as root to" \
        "install into your own home directory."

tar xzf "$TMPDIR_SELF/$ARCHIVE" -C "$TMPDIR_SELF" ||
    die "could not unpack $ARCHIVE" \
        "The archive verified, so this is a tar problem, not a corrupt download."

for bin in vitrum vitrum-server; do
    [ -f "$TMPDIR_SELF/$bin" ] ||
        die "$ARCHIVE does not contain $bin" \
            "The release archive is incomplete; both binaries ship together." \
            "Report it at https://github.com/$REPO/issues"
done

# Both binaries move in one pass. The client and the daemon speak a versioned
# protocol, so a half-finished install is a pair that refuses to talk.
for bin in vitrum vitrum-server; do
    chmod 755 "$TMPDIR_SELF/$bin"
    mv -f "$TMPDIR_SELF/$bin" "$INSTALL_DIR/$bin" ||
        die "could not install $bin into $INSTALL_DIR" \
            "Check permissions on $INSTALL_DIR, or use --install-dir=PATH."
done

say "Installed vitrum and vitrum-server into $INSTALL_DIR."

# ============================================================
# desktop integration
# ============================================================
#
# The installer finishes the job. A launcher entry, a PATH entry and the `vu`
# shortcut are not follow-up steps for you to paste; they are what installing
# an application means. Every step here is idempotent and skipped by
# --no-integrate.

rc_files() {
    # ~/.profile is read by login shells and by most desktop sessions, so it is
    # what makes the launcher entry find the binary. The interactive rc is what
    # makes the current terminal find it.
    printf '%s\n' "$HOME/.profile"
    case "$(basename "${SHELL:-/bin/sh}")" in
        zsh) printf '%s\n' "$HOME/.zshrc" ;;
        bash) printf '%s\n' "$HOME/.bashrc" ;;
    esac
}

append_once() {
    file="$1"
    marker="$2"
    line="$3"
    [ -e "$file" ] || : > "$file" 2>/dev/null || return 0
    if grep -qsF "$marker" "$file"; then return 0; fi
    printf '\n# vitrum\n%s\n' "$line" >> "$file" 2>/dev/null || return 0
    say "  $file"
}

if [ "$INTEGRATE" = 1 ]; then
    say ""
    say "Setting up."

    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *)
            rc_files | while read -r rc; do
                append_once "$rc" "$INSTALL_DIR:\$PATH" \
                    "export PATH=\"$INSTALL_DIR:\$PATH\""
            done
            PATH="$INSTALL_DIR:$PATH"
            export PATH
            ;;
    esac

    rc_files | while read -r rc; do
        case "$rc" in
            "$HOME/.profile") continue ;;
        esac
        append_once "$rc" 'alias vu=' 'alias vu="vitrum update"'
    done

    # The icon set is drawn by the binary, not shipped beside it. The release
    # archive carries `vitrum` and `vitrum-server` and nothing else, so there
    # is no PNG to unpack and no converter on the machine to make one; the mark
    # is geometry compiled into the client and `vitrum icons` rasterises it.
    #
    # Idempotent: it overwrites the same paths every time, so a second install
    # replaces the set rather than adding to it.
    icon_data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
    icons_written=0
    if [ "$os" = "Linux" ]; then
        if "$INSTALL_DIR/vitrum" icons "$icon_data_dir" >/dev/null 2>&1; then
            icons_written=1
            say "  $icon_data_dir/icons/hicolor/*/apps/vitrum.png"
            if command -v gtk-update-icon-cache >/dev/null 2>&1; then
                gtk-update-icon-cache -q -t -f "$icon_data_dir/icons/hicolor" 2>/dev/null || true
            fi
        else
            warn "could not write the icon set, so the launcher entry has no picture"
        fi
    fi

    if [ "$os" = "Linux" ]; then
        apps="$HOME/.local/share/applications"
        if mkdir -p "$apps" 2>/dev/null; then
            cat > "$apps/vitrum.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=vitrum
Comment=One interface for every agent TUI you have running
Exec=$INSTALL_DIR/vitrum
Terminal=false
Categories=Development;TerminalEmulator;
StartupWMClass=vitrum
EOF
            # Named, not a path: the launcher resolves `vitrum` against the
            # hicolor tree written above and picks the size it needs. Only
            # written when that tree exists, because an `Icon=` naming nothing
            # is how an entry ends up with a broken-image placeholder instead
            # of the desktop's own generic one.
            if [ "$icons_written" = 1 ]; then
                printf 'Icon=vitrum\n' >> "$apps/vitrum.desktop"
            fi
            if command -v update-desktop-database >/dev/null 2>&1; then
                update-desktop-database "$apps" 2>/dev/null || true
            fi
            say "  $apps/vitrum.desktop"
        else
            warn "could not write $apps, so there is no launcher entry"
        fi
    fi

    if [ "$os" = "Darwin" ]; then
        app="$HOME/Applications/vitrum.app"
        if mkdir -p "$app/Contents/MacOS" 2>/dev/null; then
            # Staged and copied rather than written into the bundle directly:
            # `CFBundleIconFile` names a file in `Resources`, and the emitter
            # also writes a freedesktop theme tree that has no business in an
            # app bundle.
            bundle_icon=""
            if mkdir -p "$app/Contents/Resources" 2>/dev/null &&
                "$INSTALL_DIR/vitrum" icons "$TMPDIR_SELF/iconset" >/dev/null 2>&1 &&
                cp "$TMPDIR_SELF/iconset/icons/vitrum.icns" \
                    "$app/Contents/Resources/vitrum.icns" 2>/dev/null; then
                bundle_icon='  <key>CFBundleIconFile</key><string>vitrum.icns</string>'
            else
                warn "could not write the app icon, so the bundle shows a blank one"
            fi
            cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>vitrum</string>
  <key>CFBundleIdentifier</key><string>dev.santhreal.vitrum</string>
  <key>CFBundleExecutable</key><string>vitrum</string>
$bundle_icon
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF
            # The daemon is linked in beside the client on purpose: an app
            # launched from Finder inherits no shell PATH, so `vitrum` finds
            # `vitrum-server` next to itself or not at all.
            ln -sf "$INSTALL_DIR/vitrum" "$app/Contents/MacOS/vitrum"
            ln -sf "$INSTALL_DIR/vitrum-server" "$app/Contents/MacOS/vitrum-server"
            say "  $app"
            say "  the bundle is unsigned; the first launch needs right-click, then Open"
        else
            warn "could not write $app, so there is no launcher entry"
        fi
    fi
fi

if [ "$os" = "Linux" ]; then
    # A WebKit runtime is vitrum's only system dependency. `ldconfig -p` is the
    # cheap, reliable check on glibc hosts; where it is absent the requirement
    # is stated and left to you rather than guessed at.
    if command -v ldconfig >/dev/null 2>&1; then
        if ldconfig -p 2>/dev/null | grep -q 'libwebkit2gtk-4\.1'; then
            :
        else
            warn "libwebkit2gtk-4.1 was not found; vitrum will not open a window without it."
            say "  Debian and Ubuntu: sudo apt install libwebkit2gtk-4.1"
            say "  Fedora:            sudo dnf install webkit2gtk4.1"
            say "  Arch:              sudo pacman -S webkit2gtk-4.1"
        fi
    else
        say "vitrum needs a WebKitGTK 4.1 runtime, its only system dependency."
        say "  Debian and Ubuntu: sudo apt install libwebkit2gtk-4.1"
        say "  Fedora:            sudo dnf install webkit2gtk4.1"
        say "  Arch:              sudo pacman -S webkit2gtk-4.1"
    fi
fi

say ""
say "Run 'vitrum', or open it from your app launcher."
say "Update with 'vitrum update', or 'vu' in a new shell."
