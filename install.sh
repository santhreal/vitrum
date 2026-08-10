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
#   sh install.sh --uninstall     remove everything the installer wrote
#   sh install.sh --help          full usage
#
# Env overrides:
#   VITRUM_VERSION       version to install, same as the argument
#   VITRUM_INSTALL_DIR   where the binaries go (default: $HOME/.local/bin)
#   VITRUM_BASE_URL      where the release assets live, for a mirror
#   GITHUB_TOKEN         sent to the GitHub API, for rate-limited networks
#
# Everything this script writes is recorded in an install manifest, and
# `--uninstall` removes exactly what the manifest lists. A machine that has a
# proxy, no write permission, a running vitrum, a truncated download, an
# unsupported libc, or a library this build needs and this machine has not
# got, is told which of those it is, and the installer exits non-zero without
# installing half of anything.

set -eu

REPO="santhreal/vitrum"
VERSION="${VITRUM_VERSION:-}"
INSTALL_DIR="${VITRUM_INSTALL_DIR:-$HOME/.local/bin}"
BASE_URL="${VITRUM_BASE_URL:-}"
INTEGRATE=1
RUNTIME_CHECK=1
UNINSTALL=0
if [ -n "${VITRUM_NO_INTEGRATE:-}" ]; then INTEGRATE=0; fi
if [ -n "${VITRUM_NO_RUNTIME_CHECK:-}" ]; then RUNTIME_CHECK=0; fi
TMPDIR_SELF=""

DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
MANIFEST="$DATA_DIR/vitrum/install-manifest"
MANIFEST_NEW=""

# One marked block per shell rc, so the uninstaller can take back exactly the
# lines the installer added and leave the rest of the file untouched.
BLOCK_BEGIN="# >>> vitrum >>>"
BLOCK_END="# <<< vitrum <<<"

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

# A failure of the network path. Identical to `die`, except that it names the
# proxy when there is one: a proxy is the single most common reason a download
# arrives empty, truncated, or as a login page, and an operator who is not told
# it was in force will go looking at the release instead.
die_net() {
    printf 'error: %s\n' "$1" >&2
    shift
    for line in "$@"; do
        printf '  %s\n' "$line" >&2
    done
    if [ -n "$PROXY" ]; then
        printf '  %s\n' "A proxy is in force: $PROXY" >&2
        printf '  %s\n' "It has to allow HTTPS to the download host. Add the host to no_proxy," >&2
        printf '  %s\n' "unset the proxy, or fetch the archive on a machine that can reach it and" >&2
        printf '  %s\n' "install from a local copy with --base-url=file:///path/to/assets." >&2
    fi
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
  --base-url=URL          directory holding the release archive and its
                          SHA256SUMS, for a mirror or an air-gapped copy;
                          needs an explicit VERSION
  --no-integrate          install the binaries only: no launcher entry, no
                          PATH edit, no `vu` shortcut
  --no-runtime-check      install even though this machine is missing a
                          library the build needs, for an image that adds it
                          separately
  --uninstall             remove everything this installer wrote, and nothing
                          else
  -h, --help              show this help and exit

Environment:
  VITRUM_VERSION          same as --version
  VITRUM_INSTALL_DIR      same as --install-dir
  VITRUM_BASE_URL         same as --base-url
  GITHUB_TOKEN            bearer token for the GitHub API
  VITRUM_NO_INTEGRATE     set to anything for --no-integrate
  VITRUM_NO_RUNTIME_CHECK set to anything for --no-runtime-check

Beyond the binaries, the installer adds a launcher entry, puts the install
directory on your `PATH` for bash, zsh and fish, and defines `vu` as
`vitrum update`. Each step is idempotent and each is skipped by
--no-integrate.

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
        --base-url=*)
            BASE_URL="${1#--base-url=}"
            ;;
        --base-url)
            [ $# -ge 2 ] || die "--base-url needs a URL" \
                "Pass it as --base-url=https://mirror.example/vitrum/v0.1.0."
            BASE_URL="$2"
            shift
            ;;
        --no-integrate)
            INTEGRATE=0
            ;;
        --no-runtime-check)
            RUNTIME_CHECK=0
            ;;
        --uninstall)
            UNINSTALL=1
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
BASE_URL="${BASE_URL%/}"

[ -n "$INSTALL_DIR" ] || die "install directory is empty" \
    "Pass --install-dir=PATH or unset VITRUM_INSTALL_DIR."

os=$(uname -s)
arch=$(uname -m)

# ============================================================
# what is already running
# ============================================================

# The pid of a process running exactly the binary at $1, or nothing.
#
# The path is compared, not the name: an unrelated `vitrum` from a source
# checkout is none of this installer's business, and refusing to install
# because of it would be a false alarm the operator cannot clear.
running_pid() {
    rp_path="$1"
    rp_name="${rp_path##*/}"
    rp_pids=""
    if command -v pgrep >/dev/null 2>&1; then
        rp_pids=$(pgrep -x "$rp_name" 2>/dev/null || true)
    fi
    for rp_pid in $rp_pids; do
        rp_exe=""
        if [ -r "/proc/$rp_pid/exe" ]; then
            rp_exe=$(readlink "/proc/$rp_pid/exe" 2>/dev/null || true)
            rp_exe="${rp_exe% (deleted)}"
        elif command -v ps >/dev/null 2>&1; then
            # macOS `ps -o comm=` prints the executable path, Linux the name.
            rp_exe=$(ps -p "$rp_pid" -o comm= 2>/dev/null || true)
        fi
        if [ "$rp_exe" = "$rp_path" ]; then
            printf '%s\n' "$rp_pid"
            return 0
        fi
    done
    return 1
}

# The client is refused, the daemon is not.
#
# Replacing the client under a running window leaves that window on the old
# build and, on some filesystems, fails outright. Quitting it costs nothing:
# the sessions are the daemon's children and survive the window closing.
#
# The daemon is a different call. Refusing while it runs would mean no install
# could ever complete without ending every session on the machine, which is the
# one thing this product promises not to make you do. Its file is replaced by
# rename, the running process keeps its own open image, and it is told plainly
# that it stays on the old code until someone restarts it.
refuse_if_client_running() {
    if rc_pid=$(running_pid "$INSTALL_DIR/vitrum"); then
        die "vitrum is running from $INSTALL_DIR/vitrum (pid $rc_pid)" \
            "Quit the vitrum window, then run this again." \
            "Your sessions are not affected: they belong to vitrum-server, which" \
            "this installer never stops." \
            "To leave the running copy alone, install elsewhere with --install-dir=PATH."
    fi
}

# ============================================================
# shell rc files
# ============================================================

fish_config() {
    printf '%s\n' "${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish"
}

# True when this machine has that shell: it is installed, it is the login
# shell, or its rc is already there. A machine with no fish gets no fish file
# invented for it, and a machine with fish gets its PATH edit whether or not
# fish is the shell that happens to be running the installer.
shell_present() {
    if [ -e "$2" ]; then return 0; fi
    if command -v "$1" >/dev/null 2>&1; then return 0; fi
    if [ "$(basename "${SHELL:-/bin/sh}")" = "$1" ]; then return 0; fi
    return 1
}

# The file a bash login shell actually reads: the first of `~/.bash_profile`,
# `~/.bash_login`, `~/.profile` that exists, and only that one. Empty when
# neither of the first two is there, because then `~/.profile` is the file bash
# reads and `each_rc` already writes it.
bash_login_rc() {
    if [ -e "$HOME/.bash_profile" ]; then
        printf '%s\n' "$HOME/.bash_profile"
    elif [ -e "$HOME/.bash_login" ]; then
        printf '%s\n' "$HOME/.bash_login"
    fi
}

# Calls $1 as `$1 <syntax> <file>` for every rc that should carry the block.
#
# `~/.profile` is read by login shells and by most desktop sessions, so it is
# what makes the launcher entry find the binary. The interactive rc of each
# shell is what makes an open terminal find it. bash alone was not enough:
# zsh is the default on macOS, and fish is neither POSIX nor willing to read
# `export PATH=`.
#
# `~/.profile` alone was not enough either. bash reads one login file and stops,
# so a `~/.bash_profile` — which rustup, nvm and bun each create — shadows
# `~/.profile`, and `~/.bashrc` is skipped by a login shell that is not
# interactive. Between them that left `vitrum` off `PATH` in exactly the shell
# someone opens a terminal to, on a machine that had ever installed rust.
#
# A file that refuses the edit is a warning, not a failed install, so the
# per-file result is captured rather than allowed to end the run. `PROFILE_OK`
# carries the one result a caller acts on: whether the login file bash reads
# on a machine with no `~/.bash_profile` took the block.
PROFILE_OK=1
each_rc() {
    PROFILE_OK=1
    "$1" posix "$HOME/.profile" || PROFILE_OK=0
    bash_login=$(bash_login_rc)
    if [ -n "$bash_login" ]; then "$1" posix "$bash_login" || true; fi
    if shell_present bash "$HOME/.bashrc"; then "$1" posix "$HOME/.bashrc" || true; fi
    if shell_present zsh "${ZDOTDIR:-$HOME}/.zshrc"; then "$1" posix "${ZDOTDIR:-$HOME}/.zshrc" || true; fi
    if shell_present fish "$(fish_config)"; then "$1" fish "$(fish_config)" || true; fi
}

# Removes the vitrum block from $1, and the single blank line that precedes it.
# Returns 0 only when a block was really taken out, so callers can report what
# they touched. Everything outside the markers is copied through byte for byte.
rc_block_strip() {
    sf="$1"
    [ -f "$sf" ] || return 1
    if ! grep -qF "$BLOCK_BEGIN" "$sf" 2>/dev/null; then return 1; fi
    st="$sf.vitrum-tmp.$$"
    if awk -v b="$BLOCK_BEGIN" -v e="$BLOCK_END" '
        $0 == b { if (nb > 0) nb--; skip = 1; next }
        $0 == e { skip = 0; next }
        skip { next }
        $0 == "" { nb++; next }
        { while (nb > 0) { print ""; nb-- } print }
        END { while (nb-- > 0) print "" }
    ' "$sf" > "$st" 2>/dev/null && cat "$st" > "$sf" 2>/dev/null; then
        rm -f "$st"
        return 0
    fi
    rm -f "$st"
    warn "could not edit $sf; delete the '$BLOCK_BEGIN' block there by hand"
    return 1
}

# Writes the block into $2 in the syntax named by $1, replacing any block that
# is already there. Re-running the installer, or running it with a different
# --install-dir, leaves exactly one block naming the current directory.
#
# The syntax is `posix`, `fish`, or `shadow` for a `~/.bash_profile` written
# because `~/.profile` refused the write.
#
# A file that did not exist is recorded as `rc-created` rather than `rc`, so
# uninstalling takes it away instead of leaving an empty file behind that
# nobody put there.
rc_block_write() {
    rk="$1"
    rf="$2"
    rd=$(dirname "$rf")
    rkind=existing
    if [ ! -d "$rd" ]; then
        mkdir -p "$rd" 2>/dev/null || { warn "could not create $rd, so $rf was not written"; return 0; }
    fi
    if [ ! -e "$rf" ]; then
        rkind=created
        : > "$rf" 2>/dev/null || { warn "could not create $rf, so PATH and vu are not set for that shell"; return 1; }
    fi
    if [ ! -w "$rf" ]; then
        warn "$rf is not writable, so PATH and vu were not added there"
        return 1
    fi
    rc_block_strip "$rf" >/dev/null 2>&1 || true
    {
        printf '\n%s\n' "$BLOCK_BEGIN"
        if [ "$rk" = fish ]; then
            printf 'if not contains "%s" $PATH\n' "$INSTALL_DIR"
            printf '    set -gx PATH "%s" $PATH\n' "$INSTALL_DIR"
            printf 'end\n'
            printf 'alias vu "vitrum update"\n'
        else
            if [ "$rk" = shadow ]; then
                # bash opens one login file and stops, so this one shadows
                # ~/.profile. Sourcing it first keeps everything in it in
                # force, which is what makes creating this file safe.
                printf '# ~/.profile could not be written, so bash reads its PATH entry here.\n'
                printf 'if [ -r "$HOME/.profile" ]; then . "$HOME/.profile"; fi\n'
            fi
            # Guarded, because an rc is read once per shell and an unguarded
            # prepend grows $PATH by one entry every time a shell nests.
            printf 'case ":$PATH:" in\n'
            printf '    *":%s:"*) ;;\n' "$INSTALL_DIR"
            printf '    *) export PATH="%s:$PATH" ;;\n' "$INSTALL_DIR"
            printf 'esac\n'
            if [ "$rf" != "$HOME/.profile" ]; then
                printf 'alias vu="vitrum update"\n'
            fi
        fi
        printf '%s\n' "$BLOCK_END"
    } >> "$rf" 2>/dev/null || { warn "could not write $rf"; return 1; }
    say "  $rf"
    if [ "$rkind" = created ]; then
        manifest_add rc-created "$rf"
    else
        manifest_add rc "$rf"
    fi
}

rc_block_remove() {
    if rc_block_strip "$2"; then
        say "  $2 (vitrum block)"
        REMOVED=1
    fi
}

# ============================================================
# install manifest
# ============================================================
#
# Uninstalling is not a list of paths in a document for you to retype. Every
# file the installer creates is recorded as it is created, including the icon
# files, whose names come from the binary rather than from this script, so
# `--uninstall` removes what was written on this machine and nothing that
# happened to be sitting next to it.

manifest_add() {
    [ -n "$MANIFEST_NEW" ] || return 0
    printf '%s %s\n' "$1" "$2" >> "$MANIFEST_NEW" 2>/dev/null || true
}

# Merges the previous manifest in, so an install that moved to a new directory
# still knows about the copy it left behind, and commits the result.
manifest_commit() {
    [ -n "$MANIFEST_NEW" ] && [ -f "$MANIFEST_NEW" ] || return 0
    if [ -f "$MANIFEST" ]; then
        while IFS= read -r ml; do
            [ -n "$ml" ] || continue
            if grep -qxF "$ml" "$MANIFEST_NEW"; then continue; fi
            mp="${ml#* }"
            [ -e "$mp" ] || continue
            printf '%s\n' "$ml" >> "$MANIFEST_NEW"
        done < "$MANIFEST"
    fi
    if mkdir -p "$(dirname "$MANIFEST")" 2>/dev/null &&
        cat "$MANIFEST_NEW" > "$MANIFEST" 2>/dev/null; then
        return 0
    fi
    warn "could not write $MANIFEST, so --uninstall will fall back to the default layout"
}

# Empty directories the installer created on its way to a file it removed.
# `rmdir` and not `rm -r`: a directory that still holds anything is somebody
# else's, and is left exactly as it is.
prune_dirs() {
    for pd in \
        "$DATA_DIR"/icons/hicolor/*/apps \
        "$DATA_DIR"/icons/hicolor/* \
        "$DATA_DIR/icons/hicolor" \
        "$DATA_DIR/icons" \
        "$DATA_DIR/applications" \
        "$DATA_DIR/vitrum" \
        "${XDG_CONFIG_HOME:-$HOME/.config}/fish"; do
        if [ -d "$pd" ]; then rmdir "$pd" 2>/dev/null || true; fi
    done
}

remove_file() {
    if [ -e "$1" ] || [ -L "$1" ]; then
        if rm -f "$1" 2>/dev/null; then
            say "  $1"
            REMOVED=1
        else
            warn "could not remove $1"
        fi
    fi
}

remove_tree() {
    if [ -d "$1" ]; then
        if rm -rf "$1" 2>/dev/null; then
            say "  $1"
            REMOVED=1
        else
            warn "could not remove $1"
        fi
    fi
}

# An rc file the installer created holds nothing but the block it was created
# for, so once the block is gone the file is gone too. A file that turned out
# to have something else in it is kept: someone put it there after the install.
rc_file_prune() {
    [ -f "$1" ] || return 0
    if grep -q '[^[:space:]]' "$1" 2>/dev/null; then return 0; fi
    remove_file "$1"
}

# ============================================================
# uninstall
# ============================================================

if [ "$UNINSTALL" = 1 ]; then
    refuse_if_client_running
    REMOVED=0
    say "Removing vitrum."
    if [ -f "$MANIFEST" ]; then
        while IFS= read -r line; do
            [ -n "$line" ] || continue
            kind="${line%% *}"
            path="${line#* }"
            case "$kind" in
                file) remove_file "$path" ;;
                tree) remove_tree "$path" ;;
                rc) rc_block_remove rc "$path" ;;
                rc-created)
                    rc_block_remove rc "$path"
                    rc_file_prune "$path"
                    ;;
                *) warn "ignoring an unreadable manifest line: $line" ;;
            esac
        done < "$MANIFEST"
        rm -f "$MANIFEST"
    else
        say "  no manifest at $MANIFEST, so this removes the default layout"
        remove_file "$INSTALL_DIR/vitrum"
        remove_file "$INSTALL_DIR/vitrum-server"
        remove_file "$DATA_DIR/applications/vitrum.desktop"
        remove_tree "$HOME/Applications/vitrum.app"
        for icon in "$DATA_DIR"/icons/hicolor/*/apps/vitrum.png \
            "$DATA_DIR/icons/vitrum.ico" "$DATA_DIR/icons/vitrum.icns"; do
            remove_file "$icon"
        done
        each_rc rc_block_remove
    fi
    prune_dirs
    if [ "$REMOVED" = 0 ]; then
        die "no vitrum install was found, so nothing was removed" \
            "Looked for the manifest at $MANIFEST and for binaries in $INSTALL_DIR." \
            "If it is installed somewhere else, name it: --uninstall --install-dir=PATH"
    fi
    if running_pid "$INSTALL_DIR/vitrum-server" >/dev/null 2>&1; then
        say ""
        warn "vitrum-server is still running from the copy that was just removed."
        say "  It keeps its sessions until you stop it, and stopping it ends them."
    fi
    say ""
    say "Config and state were left alone; they are listed in docs/configuration.md."
    exit 0
fi

# ============================================================
# tools
# ============================================================

need() {
    command -v "$1" >/dev/null 2>&1 || die "$1 is required and was not found" "$2"
}

case "$BASE_URL" in
    file://*)
        # A local copy of the assets is read, not downloaded, so an air-gapped
        # host with neither curl nor wget can still install. Requiring a
        # downloader here would contradict the advice the failure below gives.
        FETCH="file"
        ;;
    *)
        if command -v curl >/dev/null 2>&1; then
            FETCH="curl"
        elif command -v wget >/dev/null 2>&1; then
            FETCH="wget"
        else
            die "neither curl nor wget is available, so nothing can be downloaded" \
                "Install one of them: 'sudo apt install curl', 'sudo dnf install curl'," \
                "'sudo pacman -S curl' or 'brew install curl'." \
                "Or download the archive and its SHA256SUMS by hand from" \
                "https://github.com/$REPO/releases, put them in one directory, and run" \
                "this script again with --base-url=file:///that/directory --version=X.Y.Z"
        fi
        ;;
esac

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

# ============================================================
# proxy
# ============================================================
#
# A proxy is not an error. Being behind one and not knowing it is: the download
# then fails, or succeeds and hands over a login page, and every symptom points
# at the release. So the proxy is named in the summary, named again in any
# network failure, and rejected up front when it is spelled in a way curl and
# wget cannot use.

PROXY=""
for pv in https_proxy HTTPS_PROXY all_proxy ALL_PROXY http_proxy HTTP_PROXY; do
    eval "pvalue=\${$pv:-}"
    [ -n "$pvalue" ] || continue
    case "$pvalue" in
        http://*/* | http://*[!/] | https://*/* | https://*[!/] | socks5://* | socks5h://* | socks4://*) ;;
        *)
            die "$pv is set to '$pvalue', which is not a URL a proxy can be reached at" \
                "curl and wget read it as scheme://host:port, so a bare host:port is" \
                "treated as a hostname and every download fails with a name lookup." \
                "Set it as http://${pvalue}, or unset $pv."
            ;;
    esac
    PROXY="$pv=$pvalue"
    break
done

fetch() {
    # $1 url, $2 destination. Timeouts, because a proxy that accepts the
    # connection and then says nothing would otherwise hang the install with
    # no output at all.
    #
    # `file://` is copied rather than fetched: wget has no such scheme at all,
    # and a mirror on a local disk is the only way an air-gapped host installs.
    case "$1" in
        file://*)
            cp "${1#file://}" "$2" 2>/dev/null
            return
            ;;
    esac
    if [ "$FETCH" = "curl" ]; then
        curl -fsSL --retry 3 --retry-delay 1 --connect-timeout 30 \
            --speed-limit 1024 --speed-time 60 -o "$2" "$1"
    else
        wget -q --tries=3 --connect-timeout=30 --read-timeout=60 -O "$2" "$1"
    fi
}

fetch_api() {
    # $1 url. Prints the body. GITHUB_TOKEN lifts the anonymous rate limit.
    if [ "$FETCH" = "curl" ]; then
        if [ -n "${GITHUB_TOKEN:-}" ]; then
            curl -fsSL --retry 3 --connect-timeout 30 \
                -H "Authorization: Bearer $GITHUB_TOKEN" \
                -H "X-GitHub-Api-Version: 2022-11-28" "$1"
        else
            curl -fsSL --retry 3 --connect-timeout 30 "$1"
        fi
    else
        if [ -n "${GITHUB_TOKEN:-}" ]; then
            wget -q -O - --tries=3 --connect-timeout=30 \
                --header="Authorization: Bearer $GITHUB_TOKEN" "$1"
        else
            wget -q -O - --tries=3 --connect-timeout=30 "$1"
        fi
    fi
}

# ============================================================
# platform
# ============================================================

# Exactly the four triples `.github/workflows/release.yml` builds and uploads.
# Anything else has no asset to download, so it is told what it is and what to
# do about it, rather than being sent to a URL that answers 404.
case "$os" in
    Linux)
        case "$arch" in
            x86_64 | amd64) TARGET="x86_64-unknown-linux-gnu" ;;
            *)
                die "there is no published build for Linux on $arch" \
                    "Releases carry x86_64 Linux only, so no archive exists to download." \
                    "Build from source on this machine instead:" \
                    "  https://github.com/$REPO/blob/main/CONTRIBUTING.md" \
                    "You will need a WebKitGTK 4.1 development package first."
                ;;
        esac
        # The published Linux build links glibc. On a musl host it would
        # install cleanly and then fail to start with a loader error naming a
        # file nobody has, so the libc is checked here instead.
        libc="glibc"
        for loader in /lib/ld-musl-*.so.1; do
            if [ -e "$loader" ]; then libc="musl"; fi
        done
        if command -v ldd >/dev/null 2>&1; then
            case "$(ldd --version 2>&1 | head -1)" in
                *musl*) libc="musl" ;;
            esac
        fi
        if [ "$libc" = musl ]; then
            die "there is no published build for Linux with musl libc" \
                "This host runs musl (Alpine and Void use it); the release archive is" \
                "linked against glibc and would fail to start with a missing loader." \
                "Build from source on this machine instead:" \
                "  https://github.com/$REPO/blob/main/CONTRIBUTING.md"
        fi
        ;;
    Darwin)
        case "$arch" in
            arm64 | aarch64) TARGET="aarch64-apple-darwin" ;;
            x86_64) TARGET="x86_64-apple-darwin" ;;
            *)
                die "there is no published build for macOS on $arch" \
                    "Releases carry Apple silicon and Intel macOS only." \
                    "Build from source instead: https://github.com/$REPO/blob/main/CONTRIBUTING.md"
                ;;
        esac
        ;;
    *)
        die "this installer supports Linux and macOS; this host reports $os" \
            "On Windows, use install.ps1 from the same repository." \
            "Anywhere else, build from source: https://github.com/$REPO/blob/main/CONTRIBUTING.md"
        ;;
esac

# ============================================================
# preflight
# ============================================================
#
# Everything that can be known before a byte is downloaded is checked before a
# byte is downloaded. Finding out that a directory is read-only, or that the
# machine has no WebKit, after ninety megabytes have crossed a metered link is
# a worse experience than the failure itself.

# The identifiers this distribution answers to, most specific first.
runtime_ids() {
    ri_id=""
    ri_like=""
    if [ -r /etc/os-release ]; then
        ri_id=$(sed -n 's/^ID=//p' /etc/os-release | head -1 | tr -d '"')
        ri_like=$(sed -n 's/^ID_LIKE=//p' /etc/os-release | head -1 | tr -d '"')
    fi
    printf '%s %s' "$ri_id" "$ri_like"
}

# The command this distribution installs packages with, or nothing when it is
# not one this script can name a command for.
runtime_pm() {
    for rm_candidate in $(runtime_ids); do
        case "$rm_candidate" in
            debian | ubuntu | linuxmint | pop | elementary | raspbian | kali)
                printf 'sudo apt install'
                return 0
                ;;
            fedora | rhel | centos | rocky | almalinux)
                printf 'sudo dnf install'
                return 0
                ;;
            arch | manjaro | endeavouros | cachyos)
                printf 'sudo pacman -S'
                return 0
                ;;
            opensuse | opensuse-tumbleweed | opensuse-leap | suse | sles)
                printf 'sudo zypper install'
                return 0
                ;;
            alpine)
                printf 'sudo apk add'
                return 0
                ;;
            void)
                printf 'sudo xbps-install -S'
                return 0
                ;;
            gentoo)
                printf 'sudo emerge'
                return 0
                ;;
            nixos)
                printf 'nix-env -iA'
                return 0
                ;;
        esac
    done
}

# The package that carries the shared library $1 on this distribution, or
# nothing when there is none. "install a WebKit runtime" is not an instruction
# anyone can run, and neither is a package name from another distribution.
#
# A soname with no entry under a distribution is a distribution that does not
# package that soname, not an omission: Arch ships libxdo.so.4 and has nothing
# that provides libxdo.so.3, and naming `xdotool` there would send someone to
# install a package that leaves the binary exactly as broken as it was.
runtime_pkg() {
    rp_lib="$1"
    for rp_candidate in $(runtime_ids); do
        case "$rp_candidate" in
            debian | ubuntu | linuxmint | pop | elementary | raspbian | kali)
                case "$rp_lib" in
                    libwebkit2gtk-4.1.so.0) printf 'libwebkit2gtk-4.1-0' ;;
                    libxdo.so.3) printf 'libxdo3' ;;
                esac
                return 0
                ;;
            fedora | rhel | centos | rocky | almalinux)
                case "$rp_lib" in
                    libwebkit2gtk-4.1.so.0) printf 'webkit2gtk4.1' ;;
                    libxdo.so.3) printf 'xdotool' ;;
                esac
                return 0
                ;;
            arch | manjaro | endeavouros | cachyos)
                case "$rp_lib" in
                    libwebkit2gtk-4.1.so.0) printf 'webkit2gtk-4.1' ;;
                esac
                return 0
                ;;
            opensuse | opensuse-tumbleweed | opensuse-leap | suse | sles)
                case "$rp_lib" in
                    libwebkit2gtk-4.1.so.0) printf 'libwebkit2gtk-4_1-0' ;;
                    libxdo.so.3) printf 'libxdo3' ;;
                esac
                return 0
                ;;
            alpine)
                case "$rp_lib" in
                    libwebkit2gtk-4.1.so.0) printf 'webkit2gtk-4.1' ;;
                    libxdo.so.3) printf 'xdotool' ;;
                esac
                return 0
                ;;
            void)
                case "$rp_lib" in
                    libwebkit2gtk-4.1.so.0) printf 'webkit2gtk' ;;
                    libxdo.so.3) printf 'xdotool' ;;
                esac
                return 0
                ;;
            gentoo)
                case "$rp_lib" in
                    libwebkit2gtk-4.1.so.0) printf 'net-libs/webkit-gtk:4.1' ;;
                    libxdo.so.3) printf 'x11-misc/xdotool' ;;
                esac
                return 0
                ;;
            nixos)
                case "$rp_lib" in
                    libwebkit2gtk-4.1.so.0) printf 'nixpkgs.webkitgtk_4_1' ;;
                    libxdo.so.3) printf 'nixpkgs.xdotool' ;;
                esac
                return 0
                ;;
        esac
    done
}

# One line to paste that installs every package in $@, or a sentence saying
# there is none to paste.
runtime_command() {
    rc_pm=$(runtime_pm)
    if [ -n "$rc_pm" ] && [ $# -gt 0 ]; then
        printf '%s %s' "$rc_pm" "$*"
    else
        printf 'no package on this distribution is known to provide it'
    fi
}

webkit_package() {
    wp_pkg=$(runtime_pkg libwebkit2gtk-4.1.so.0)
    if [ -n "$wp_pkg" ]; then
        runtime_command "$wp_pkg"
    else
        printf 'install your distribution package for libwebkit2gtk-4.1.so.0'
    fi
}

have_webkit() {
    if command -v ldconfig >/dev/null 2>&1; then
        if ldconfig -p 2>/dev/null | grep -q 'libwebkit2gtk-4\.1'; then return 0; fi
    fi
    for wdir in /usr/lib /usr/lib64 /lib /lib64 /usr/local/lib \
        /usr/lib/x86_64-linux-gnu /lib/x86_64-linux-gnu; do
        for wlib in "$wdir"/libwebkit2gtk-4.1.so*; do
            if [ -e "$wlib" ]; then return 0; fi
        done
    done
    return 1
}

# A directory the installer can really write into, rather than one the mode
# bits merely suggest it can: a read-only mount, a full filesystem and an
# immutable directory all pass `-w` and fail the first write.
assert_writable() {
    aw_dir="$1"
    if [ -e "$aw_dir" ] && [ ! -d "$aw_dir" ]; then
        die "$aw_dir exists and is not a directory" \
            "Move it aside, or install somewhere else with --install-dir=PATH."
    fi
    if [ ! -d "$aw_dir" ]; then
        aw_parent=$(dirname "$aw_dir")
        while [ ! -d "$aw_parent" ] && [ "$aw_parent" != "/" ]; do
            aw_parent=$(dirname "$aw_parent")
        done
        mkdir -p "$aw_dir" 2>/dev/null ||
            die "could not create $aw_dir" \
                "$aw_parent is where it stopped: you do not have permission to create" \
                "a directory there, or the filesystem is read-only." \
                "Pick a writable directory with --install-dir=PATH."
    fi
    aw_probe="$aw_dir/.vitrum-write-test.$$"
    if ! (: > "$aw_probe") 2>/dev/null; then
        rm -f "$aw_probe" 2>/dev/null || true
        die "$aw_dir cannot be written to" \
            "The directory exists, and creating a file in it was refused: it is owned" \
            "by another user, mounted read-only, or the filesystem is full." \
            "Pick a writable directory with --install-dir=PATH, or run this as the" \
            "user that owns $aw_dir. Do not run the installer as root to install" \
            "into your own home directory: it would leave root-owned binaries there."
    fi
    rm -f "$aw_probe" 2>/dev/null || true
}

assert_writable "$INSTALL_DIR"
refuse_if_client_running

if [ "$os" = "Linux" ] && [ "$RUNTIME_CHECK" = 1 ]; then
    if ! have_webkit; then
        die "vitrum needs a WebKit runtime and this machine has none" \
            "libwebkit2gtk-4.1.so.0 is vitrum's only system dependency, and without" \
            "it the binary installs and then fails to open a window." \
            "Install it first:" \
            "  $(webkit_package)" \
            "Then run this installer again." \
            "To install anyway, for an image that adds the runtime separately, pass" \
            "--no-runtime-check."
    fi
fi

# A re-install is normal and is stated, so the operator knows the version they
# are leaving as well as the one they are getting.
PREVIOUS=""
if [ -x "$INSTALL_DIR/vitrum" ]; then
    PREVIOUS=$("$INSTALL_DIR/vitrum" --version 2>/dev/null || true)
    [ -n "$PREVIOUS" ] || PREVIOUS="an unreadable build"
fi

# ============================================================
# version
# ============================================================

if [ -n "$BASE_URL" ]; then
    [ -n "$VERSION" ] || die "--base-url needs an explicit version" \
        "A mirror has no releases API to ask, so the version cannot be resolved." \
        "Pass it: sh install.sh 0.1.0 --base-url=$BASE_URL"
elif [ -z "$VERSION" ]; then
    say "Resolving the latest release of $REPO."
    latest=$(fetch_api "https://api.github.com/repos/$REPO/releases/latest") || latest=""
    [ -n "$latest" ] || die_net "could not reach the GitHub releases API" \
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
if [ -n "$BASE_URL" ]; then
    BASE="$BASE_URL"
else
    BASE="https://github.com/$REPO/releases/download/v${VERSION}"
fi

say ""
say "  version      v${VERSION}"
say "  target       ${TARGET}"
say "  archive      ${ARCHIVE}"
say "  install to   ${INSTALL_DIR}"
say "  binaries     vitrum, vitrum-server"
[ -z "$PREVIOUS" ] || say "  replacing    ${PREVIOUS}"
[ -z "$PROXY" ] || say "  proxy        ${PROXY}"
say ""

# ============================================================
# download and verify
# ============================================================

TMPDIR_SELF=$(mktemp -d 2>/dev/null || mktemp -d -t vitrum) ||
    die "could not create a temporary directory" \
        "Check that TMPDIR points somewhere writable."
MANIFEST_NEW="$TMPDIR_SELF/manifest"
: > "$MANIFEST_NEW"

say "Downloading $ARCHIVE."
fetch "$BASE/$ARCHIVE" "$TMPDIR_SELF/$ARCHIVE" ||
    die_net "could not download $BASE/$ARCHIVE" \
        "Check that v${VERSION} is published and carries an asset for ${TARGET}:" \
        "  https://github.com/$REPO/releases/tag/v${VERSION}"

# Why a shape check when there is a digest two steps below: because "checksum
# mismatch" is the wrong answer to "the transfer stopped half way" and to "a
# captive portal sent you its login page". Both are common, neither is a bad
# release, and each has a different thing for the operator to do.
archive_shape() {
    if [ ! -s "$1" ]; then
        printf 'it is empty (0 bytes)'
        return 0
    fi
    as_size=$(wc -c < "$1" | tr -d ' ')
    as_magic=$(head -c 2 "$1" | od -An -tx1 2>/dev/null | tr -d ' \n')
    if [ "$as_magic" != "1f8b" ]; then
        if head -c 512 "$1" | grep -qi -e '<html' -e '<!doctype' -e '<title'; then
            printf 'it is a web page, not an archive (%s bytes)' "$as_size"
        else
            printf 'it is not a gzip archive (%s bytes, first bytes %s)' \
                "$as_size" "${as_magic:-none}"
        fi
        return 0
    fi
    if command -v gzip >/dev/null 2>&1; then
        if ! gzip -t "$1" 2>/dev/null; then
            printf 'it is truncated: the gzip stream ends part way through (%s bytes)' "$as_size"
        fi
    elif ! tar tzf "$1" >/dev/null 2>&1; then
        printf 'it is truncated: the archive ends part way through (%s bytes)' "$as_size"
    fi
}

shape=$(archive_shape "$TMPDIR_SELF/$ARCHIVE")
if [ -n "$shape" ]; then
    die_net "the download of $ARCHIVE did not arrive intact: $shape" \
        "Nothing was installed." \
        "This is the transfer, not the release: retry, and if it keeps stopping" \
        "at the same size, something between you and the download host is cutting" \
        "the connection."
fi

say "Downloading SHA256SUMS."
fetch "$BASE/SHA256SUMS" "$TMPDIR_SELF/SHA256SUMS" ||
    die_net "could not download $BASE/SHA256SUMS" \
        "Every vitrum release publishes it, so a release without one is" \
        "incomplete and must not be installed. Report it at" \
        "https://github.com/$REPO/issues"

if ! head -1 "$TMPDIR_SELF/SHA256SUMS" | grep -Eq '^[0-9a-fA-F]{64}[ *]'; then
    die_net "what came back for SHA256SUMS is not a checksum file" \
        "Its first line is not a digest and a filename, so something answered on" \
        "the release's behalf: a proxy, a captive portal, or a sign-in page." \
        "Nothing was installed."
fi

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
    "installer will not install an unverified archive. Nothing was installed." \
    "Report it at https://github.com/$REPO/issues, or install a version whose" \
    "checksum file lists its own archive: sh install.sh X.Y.Z"

actual=$(sha256_of "$TMPDIR_SELF/$ARCHIVE")
if [ "$actual" != "$expected" ]; then
    die "checksum mismatch for $ARCHIVE; nothing was installed" \
        "expected $expected" \
        "actual   $actual" \
        "The archive is intact but is not the file this release published, so it" \
        "was changed on the way here. Do not use this download. Retry, and if it" \
        "fails again report it at https://github.com/$REPO/issues"
fi
say "Checksum verified."

# ============================================================
# install
# ============================================================

tar xzf "$TMPDIR_SELF/$ARCHIVE" -C "$TMPDIR_SELF" ||
    die "could not unpack $ARCHIVE" \
        "The archive verified, so this is a tar problem, not a corrupt download."

for bin in vitrum vitrum-server; do
    [ -f "$TMPDIR_SELF/$bin" ] ||
        die "$ARCHIVE does not contain $bin" \
            "The release archive is incomplete; both binaries ship together." \
            "Report it at https://github.com/$REPO/issues"
done

# ============================================================
# will it start
# ============================================================
#
# A verified download is not a working install. The archive is the only place
# the truth about this build's runtime dependencies lives, so it is asked
# rather than guessed at: `ldd` names every soname the loader cannot resolve
# and every symbol version the C library is too old for, which are the two
# ways a binary that downloaded perfectly still refuses to start.
#
# Derived from the binary rather than from a table, so a build that stops
# linking something stops being refused for it, and a build that starts
# linking something new is caught the first time anyone installs it.
#
# This runs before a byte is written to the install directory, so a machine
# that cannot run the build keeps the copy it already had.

# Prints one line per unmet dependency of the binary $1: `lib <soname>` for a
# library the loader cannot find, `glibc <version>` for a symbol version the C
# library does not carry.
runtime_report() {
    command -v ldd >/dev/null 2>&1 || return 0
    ldd "$1" 2>&1 | awk '
        /not found/ {
            if (match($0, /GLIBC_[0-9.]+/)) {
                print "glibc " substr($0, RSTART + 6, RLENGTH - 6)
                next
            }
            if ($2 == "=>") { print "lib " $1 }
        }
    '
}

# The greater of two dotted versions. `sort -V` is not on every host, and a
# glibc version is two numbers, so the two numbers are compared.
version_max() {
    printf '%s\n%s\n' "$1" "$2" | awk -F. '
        { if ($1 + 0 > bm || ($1 + 0 == bm && $2 + 0 >= bn)) { bm = $1 + 0; bn = $2 + 0; best = $0 } }
        END { print best }'
}

# The C library version this machine has.
host_glibc() {
    hg=$(getconf GNU_LIBC_VERSION 2>/dev/null || true)
    case "$hg" in
        glibc\ *)
            printf '%s' "${hg#glibc }"
            return 0
            ;;
    esac
    ldd --version 2>&1 | head -1 |
        sed -n 's/.*[^0-9.]\([0-9][0-9]*\.[0-9][0-9]*\).*/\1/p'
}

# Always measured, never always acted on: --no-runtime-check turns the refusal
# into the closing warning, so an image that adds the libraries separately
# still gets told what it is committing to add.
: > "$TMPDIR_SELF/unmet"
for bin in vitrum vitrum-server; do
    runtime_report "$TMPDIR_SELF/$bin" >> "$TMPDIR_SELF/unmet"
done

glibc_need=""
libs_missing=""
while read -r ukind uvalue; do
    case "$ukind" in
        glibc)
            if [ -z "$glibc_need" ]; then
                glibc_need="$uvalue"
            else
                glibc_need=$(version_max "$glibc_need" "$uvalue")
            fi
            ;;
        lib)
            case " $libs_missing " in
                *" $uvalue "*) ;;
                *) libs_missing="${libs_missing:+$libs_missing }$uvalue" ;;
            esac
            ;;
    esac
done < "$TMPDIR_SELF/unmet"

if [ "$RUNTIME_CHECK" = 1 ]; then
    # The C library comes with the distribution release and cannot be installed
    # beside it, so this failure names the two things that do resolve it.
    if [ -n "$glibc_need" ]; then
        die "the published build needs a newer C library than this machine has" \
            "It requires glibc $glibc_need; this machine has $(host_glibc)." \
            "Nothing was installed, and no package fixes this: the C library comes" \
            "with the distribution release." \
            "Upgrade to a distribution release carrying glibc $glibc_need or newer, or" \
            "build from source on this machine, which links against the C library" \
            "you already have:" \
            "  https://github.com/$REPO/blob/main/CONTRIBUTING.md"
    fi

    if [ -n "$libs_missing" ]; then
        # Every missing library in one command, because being told them one
        # install at a time is how three failed installs happen in a row.
        pkgs=""
        unpackaged=""
        for mlib in $libs_missing; do
            mpkg=$(runtime_pkg "$mlib")
            if [ -n "$mpkg" ]; then
                pkgs="${pkgs:+$pkgs }$mpkg"
            else
                unpackaged="${unpackaged:+$unpackaged }$mlib"
            fi
        done
        if [ -z "$unpackaged" ]; then
            die "the published build needs shared libraries this machine does not have" \
                "Missing: $libs_missing" \
                "Nothing was installed. Install them first:" \
                "  $(runtime_command $pkgs)" \
                "Then run this installer again." \
                "To install anyway, for an image that adds them separately, pass" \
                "--no-runtime-check."
        fi
        die "the published build needs shared libraries this distribution does not package" \
            "Missing: $libs_missing" \
            "Of those, $unpackaged has no package here, so there is nothing to install" \
            "that would make this build start. Nothing was installed." \
            "Build from source on this machine, which links against the libraries" \
            "you have:" \
            "  https://github.com/$REPO/blob/main/CONTRIBUTING.md" \
            "Report it at https://github.com/$REPO/issues so the published build" \
            "stops needing it." \
            "To install anyway, pass --no-runtime-check."
    fi
fi

# Staged inside the install directory and renamed into place, one rename each.
# A rename within a directory is atomic and works over a file another process
# is executing, so a running vitrum-server keeps the image it started with
# instead of failing the install with ETXTBSY.
for bin in vitrum vitrum-server; do
    staged="$INSTALL_DIR/.$bin.vitrum-new.$$"
    if ! cat "$TMPDIR_SELF/$bin" > "$staged" 2>/dev/null; then
        rm -f "$staged" 2>/dev/null || true
        die "could not write $bin into $INSTALL_DIR" \
            "There was room and permission a moment ago, so the filesystem filled up" \
            "or the directory changed underneath the install." \
            "Nothing was replaced. Free some space, or use --install-dir=PATH."
    fi
    chmod 755 "$staged"
    if ! mv -f "$staged" "$INSTALL_DIR/$bin" 2>/dev/null; then
        rm -f "$staged" 2>/dev/null || true
        die "could not replace $INSTALL_DIR/$bin" \
            "The file is held open, or the directory does not allow replacing it." \
            "Quit anything running from $INSTALL_DIR and try again, or install" \
            "elsewhere with --install-dir=PATH."
    fi
    manifest_add file "$INSTALL_DIR/$bin"
done

if [ -n "$PREVIOUS" ]; then
    say "Replaced $PREVIOUS with vitrum $VERSION in $INSTALL_DIR."
else
    say "Installed vitrum and vitrum-server into $INSTALL_DIR."
fi

if server_pid=$(running_pid "$INSTALL_DIR/vitrum-server"); then
    warn "vitrum-server (pid $server_pid) is still running the previous build."
    say "  Its sessions are unaffected. It takes the new build when it is next"
    say "  restarted, and restarting it ends every session it holds, so do that"
    say "  when the agents are idle."
fi

# ============================================================
# desktop integration
# ============================================================
#
# The installer finishes the job. A launcher entry, a PATH entry and the `vu`
# shortcut are not follow-up steps for you to paste; they are what installing
# an application means. Every step here is idempotent and skipped by
# --no-integrate.

if [ "$INTEGRATE" = 1 ]; then
    say ""
    say "Setting up."

    each_rc rc_block_write

    # bash opens exactly one login file, and on a machine with no
    # `~/.bash_profile` that file is `~/.profile`. When `~/.profile` refuses
    # the write there is nowhere left for a login shell to pick the binary up,
    # and `command -v vitrum` in a fresh terminal finds nothing on a machine
    # the installer just reported success on. So the block goes into
    # `~/.bash_profile` instead, which bash reads first and which is not there
    # to be overwritten. It sources `~/.profile`, so shadowing it costs
    # nothing, and it is recorded as created, so uninstalling takes it away.
    LOGIN_PATH_OK="$PROFILE_OK"
    if [ "$PROFILE_OK" = 0 ] && [ -z "$(bash_login_rc)" ]; then
        if rc_block_write shadow "$HOME/.bash_profile"; then LOGIN_PATH_OK=1; fi
    fi
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *)
            PATH="$INSTALL_DIR:$PATH"
            export PATH
            ;;
    esac

    # The icon set is drawn by the binary, not shipped beside it. The release
    # archive carries `vitrum` and `vitrum-server` and nothing else, so there
    # is no PNG to unpack and no converter on the machine to make one; the mark
    # is geometry compiled into the client and `vitrum icons` rasterises it.
    #
    # Idempotent: it overwrites the same paths every time, so a second install
    # replaces the set rather than adding to it. The paths it prints are the
    # ones recorded, so the uninstaller removes the set this build wrote rather
    # than a list of names copied into this script.
    icons_written=0
    if [ "$os" = "Linux" ]; then
        if "$INSTALL_DIR/vitrum" icons "$DATA_DIR" > "$TMPDIR_SELF/icons.list" \
            2> "$TMPDIR_SELF/icons.err"; then
            icons_written=1
            while IFS= read -r written; do
                if [ -n "$written" ]; then manifest_add file "$written"; fi
            done < "$TMPDIR_SELF/icons.list"
            say "  $DATA_DIR/icons/hicolor/*/apps/vitrum.png"
            if command -v gtk-update-icon-cache >/dev/null 2>&1; then
                # Recorded only when this install is what created it: an
                # existing cache describes other applications' icons too, and
                # taking it away on uninstall would be taking their picture.
                icon_cache="$DATA_DIR/icons/hicolor/icon-theme.cache"
                had_cache=0
                if [ -e "$icon_cache" ]; then had_cache=1; fi
                gtk-update-icon-cache -q -t -f "$DATA_DIR/icons/hicolor" 2>/dev/null || true
                if [ "$had_cache" = 0 ] && [ -e "$icon_cache" ]; then
                    manifest_add file "$icon_cache"
                fi
            fi
        else
            # Drawing the icons is the first time the installed binary is run,
            # so whatever stopped it is what will stop `vitrum` too. Repeating
            # what it said is the difference between a picture that is missing
            # and a machine that cannot run the build at all.
            warn "could not write the icon set, so the launcher entry has no picture"
            while IFS= read -r iline; do
                [ -n "$iline" ] || continue
                say "  $iline"
            done < "$TMPDIR_SELF/icons.err"
        fi
    fi

    if [ "$os" = "Linux" ]; then
        apps="$DATA_DIR/applications"
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
                # Recorded only when this install is what created it, on the
                # same terms as the icon cache: an existing one indexes other
                # applications' entries, and taking it away on uninstall would
                # take theirs with it.
                mime_cache="$apps/mimeinfo.cache"
                had_mime=0
                if [ -e "$mime_cache" ]; then had_mime=1; fi
                update-desktop-database "$apps" 2>/dev/null || true
                if [ "$had_mime" = 0 ] && [ -e "$mime_cache" ]; then
                    manifest_add file "$mime_cache"
                fi
            fi
            manifest_add file "$apps/vitrum.desktop"
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
            manifest_add tree "$app"
            say "  $app"
            say "  the bundle is unsigned; the first launch needs right-click, then Open"
        else
            warn "could not write $app, so there is no launcher entry"
        fi
    fi
else
    LOGIN_PATH_OK=1
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *) warn "$INSTALL_DIR is not on your PATH, so 'vitrum' will not be found." ;;
    esac
fi

# A login file that refused the edit is warned about where it happened, and
# again here, because by then it has scrolled past a dozen lines of success
# and the next thing this script says is "run 'vitrum'".
if [ "$LOGIN_PATH_OK" = 0 ]; then
    warn "no login file took the PATH entry, so a login shell will not find vitrum."
    say "  Add this to a file your login shell reads, or run it by full path:"
    say "    export PATH=\"$INSTALL_DIR:\$PATH\""
fi

manifest_commit

# --no-runtime-check installs on a machine the build cannot start on. Saying
# what is still missing is the difference between that and pretending the
# install is finished.
if [ "$RUNTIME_CHECK" = 0 ]; then
    if [ -n "$glibc_need" ]; then
        warn "this build needs glibc $glibc_need and this machine has $(host_glibc), so it will not start."
    fi
    if [ -n "$libs_missing" ]; then
        warn "these shared libraries are still missing, so vitrum will not start: $libs_missing"
        tail_pkgs=""
        for mlib in $libs_missing; do
            mpkg=$(runtime_pkg "$mlib")
            if [ -n "$mpkg" ]; then tail_pkgs="${tail_pkgs:+$tail_pkgs }$mpkg"; fi
        done
        say "  $(runtime_command $tail_pkgs)"
    fi
fi

say ""
say "Run 'vitrum', or open it from your app launcher."
say "Update with 'vitrum update', or 'vu' in a new shell."
say "Remove it with 'sh install.sh --uninstall'."
