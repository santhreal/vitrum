#!/bin/sh
# Fail if a shipped Linux binary cannot be LOADED on the oldest system its
# archive claims to support.
#
#   check-abi.sh [--expect-elf N] <path>...   binaries, or directories to search
#
# Exit 0 clean, 1 something the loader on an older system cannot satisfy, 2 the
# check could not run.
#
# WHY THIS EXISTS
#
# `check-isa.sh` asks whether a binary can EXECUTE on the machines its triple
# promises. This asks the earlier question: whether the dynamic loader will
# open it at all. Both failures are silent at build time, both arrive as a
# refusal on somebody else's machine, and neither is visible on the runner that
# produced the artifact, because the runner satisfies everything it built
# against by definition.
#
# Two things decide it, and this checks both.
#
# THE GLIBC FLOOR
#
# A dynamically linked binary records the SYMBOL VERSION it was linked against,
# not the oldest one that would do. Build `posix_spawn` on a host whose glibc
# offers the pidfd form and the binary requires `GLIBC_2.39` forever, even
# though nothing in the source asked for it. The published 0.1.2 Linux archive
# did exactly that: built on ubuntu-latest, both binaries required GLIBC_2.39
# and died with `version GLIBC_2.39 not found` on Debian 12, Ubuntu 22.04 and
# RHEL 9 — which is most of the installed base.
#
# The floor is 2.28. It is not a preference: it is the newest floor that still
# reaches RHEL 8 (2.28), Debian 10 (2.28), Ubuntu 20.04 (2.31), Debian 12
# (2.36), Ubuntu 22.04 (2.35) and RHEL 9 (2.34). Moving it to 2.34 would drop
# two of those and buy nothing, because the build already produces 2.28.
#
# The Linux artifact is built with `cargo-zigbuild` against
# `x86_64-unknown-linux-gnu.2.28`, which links zig's stubs for that release, so
# the floor is a property of the build rather than of the runner. This gate is
# what notices when that is lost — a runner change, a toolchain bump that
# reintroduces a pidfd symbol, or someone running plain `cargo build` for the
# release.
#
# THE SONAME SET
#
# Every `DT_NEEDED` entry is a file the target machine must already have, under
# exactly that name. A soname is a promise about a distribution's package set,
# and one wrong entry is a binary that will not start anywhere the name differs.
#
# vitrum shipped with `libxdo.so.3` in that list, from an optional default
# feature of `muda` that nothing in this product uses. Arch ships only
# `libxdo.so.4`, so the client could not start there at all. It is not a
# theoretical class: OpenSSL reached the same list through a websocket crate's
# default features, and `libssl.so.3` does not exist on Debian 10 or RHEL 8.
#
# So the list is an ALLOWLIST and not a denylist. A new entry of any kind is
# red until somebody writes it down here with the reason it is safe to require,
# which is the only form of this check that survives the next dependency.

set -eu

die() { printf 'check-abi: %s\n' "$*" >&2; exit 2; }
fail() { printf 'check-abi: %s\n' "$*" >&2; status=1; }

# The oldest glibc a published Linux binary may require. See above.
GLIBC_FLOOR=2.28

# libstdc++ arrives through the C++ in the terminal engine. 3.4.25 is GCC 8,
# which is what RHEL 8 and Debian 10 carry, so it is the same floor stated in
# the other library's versioning scheme.
GLIBCXX_FLOOR=3.4.25
CXXABI_FLOOR=1.3.11

# Every soname a published Linux binary may require, and why each is safe.
#
#   The loader family, the C runtime, and the compiler runtimes. Present on
#   every glibc system by definition.
#     ld-linux-x86-64.so.2 libc.so.6 libm.so.6 libdl.so.2 libpthread.so.0
#     librt.so.1 libutil.so.1 libgcc_s.so.1 libstdc++.so.6
#
#   The last four of those are the libraries glibc 2.34 absorbed into libc.
#   Building at the 2.28 floor puts them back in the list, which is correct:
#   a system old enough to need the floor is a system that still ships them,
#   and one new enough not to keeps a stub for exactly this.
#
#   The webview and its toolkit, which `install.sh` refuses to install without
#   and `docs/install.md` names per distribution. These are the product's one
#   real system dependency.
#     libwebkit2gtk-4.1.so.0 libjavascriptcoregtk-4.1.so.0 libsoup-3.0.so.0
#     libgtk-3.so.0 libgdk-3.so.0 libgdk_pixbuf-2.0.so.0 libcairo.so.2
#     libcairo-gobject.so.2 libpango-1.0.so.0 libpangocairo-1.0.so.0
#     libharfbuzz.so.0 libatk-1.0.so.0 libgio-2.0.so.0 libgobject-2.0.so.0
#     libglib-2.0.so.0
#
#   X11, reached through the toolkit and through the window backend.
#     libX11.so.6 libXi.so.6 libXcursor.so.1 libXrandr.so.2 libXext.so.6
#     libXfixes.so.3 libxkbcommon.so.0 libxcb.so.1
#
# Anything else is a finding. Notably absent, and deliberately: libxdo.so.N,
# libssl.so.N and libcrypto.so.N.
SONAME_ALLOWLIST='
ld-linux-x86-64.so.2
ld-linux-aarch64.so.1
libc.so.6
libm.so.6
libdl.so.2
libpthread.so.0
librt.so.1
libutil.so.1
libgcc_s.so.1
libstdc++.so.6
libwebkit2gtk-4.1.so.0
libjavascriptcoregtk-4.1.so.0
libsoup-3.0.so.0
libgtk-3.so.0
libgdk-3.so.0
libgdk_pixbuf-2.0.so.0
libcairo.so.2
libcairo-gobject.so.2
libpango-1.0.so.0
libpangocairo-1.0.so.0
libharfbuzz.so.0
libatk-1.0.so.0
libgio-2.0.so.0
libgobject-2.0.so.0
libglib-2.0.so.0
libX11.so.6
libXi.so.6
libXcursor.so.1
libXrandr.so.2
libXext.so.6
libXfixes.so.3
libxkbcommon.so.0
libxcb.so.1
'

expect_elf=0
case "${1:-}" in
    --expect-elf)
        [ $# -ge 2 ] || die '--expect-elf needs a count'
        expect_elf=$2
        shift 2
        ;;
esac
[ $# -gt 0 ] || die 'nothing to check'

# GNU readelf and llvm-readelf print the two sections this reads in the same
# shape. Either will do, and one of them is on every runner that builds a
# Linux archive.
if [ -n "${READELF:-}" ]; then
    readelf_bin=$READELF
elif command -v readelf >/dev/null 2>&1; then
    readelf_bin=readelf
elif command -v llvm-readelf >/dev/null 2>&1; then
    readelf_bin=llvm-readelf
else
    die 'no readelf; install binutils or the llvm-tools rustup component'
fi

# `sort -V` orders 2.9 below 2.28, which plain string or numeric comparison
# both get wrong.
above() {
    [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -1)" = "$1" ]
}

status=0
checked=0

# `dist` holds archives, not binaries, and a gate pointed at it that finds no
# ELF file reports that there was nothing to check and exits 0. That is the
# shape of a gate that has never once looked at what ships, so a tarball is
# opened and its members are checked. A `.zip` is left alone: the only ones
# this project produces are the Windows archives, and a PE binary is not an
# ELF loader's problem.
unpacked=
cleanup() { [ -n "$unpacked" ] && rm -rf "$unpacked"; return 0; }
trap cleanup EXIT INT TERM

listed=$(
    for path in "$@"; do
        if [ -d "$path" ]; then
            find "$path" -type f
        elif [ -f "$path" ]; then
            printf '%s\n' "$path"
        else
            printf 'check-abi: no such path: %s\n' "$path" >&2
            exit 2
        fi
    done
) || exit 2
[ -n "$listed" ] || die 'no files to check'

files=
for entry in $listed; do
    case "$entry" in
        *.tar.gz | *.tgz)
            [ -n "$unpacked" ] || unpacked=$(mktemp -d)
            into="$unpacked/$(basename "$entry")"
            mkdir -p "$into"
            tar -xzf "$entry" -C "$into" ||
                die "$entry could not be opened"
            members=$(find "$into" -type f)
            [ -z "$members" ] || files="$files
$members"
            ;;
        *)
            files="$files
$entry"
            ;;
    esac
done
[ -n "$files" ] || die 'no files to check'

for file in $files; do
    # A Mach-O or PE binary from another leg of the release, a tarball, or a
    # text file beside the binaries. This gate is about the ELF loader, so
    # anything else is skipped rather than failed.
    head=$(dd if="$file" bs=4 count=1 2>/dev/null | od -An -tx1 | tr -d ' \n')
    [ "$head" = "7f454c46" ] || continue
    checked=$((checked + 1))
    name=$(basename "$file")

    # The version-needs section lists exactly what this binary asks another
    # library to provide, which is the question the loader answers at startup.
    needs=$("$readelf_bin" -V "$file" 2>/dev/null |
        grep -o '[A-Z_]\{1,\}_[0-9][0-9.]*' | sort -u)

    for prefix in GLIBC GLIBCXX CXXABI; do
        case "$prefix" in
            GLIBC) floor=$GLIBC_FLOOR ;;
            GLIBCXX) floor=$GLIBCXX_FLOOR ;;
            CXXABI) floor=$CXXABI_FLOOR ;;
        esac
        highest=$(printf '%s\n' "$needs" |
            sed -n "s/^${prefix}_\([0-9][0-9.]*\)\$/\1/p" | sort -V | tail -1)
        [ -n "$highest" ] || continue
        if above "$highest" "$floor"; then
            fail "$name requires ${prefix}_${highest}, above the ${prefix}_${floor} floor"
            printf '%s\n' "$needs" |
                while IFS= read -r entry; do
                    case "$entry" in "${prefix}_"*) ;; *) continue ;; esac
                    above "${entry##*_}" "$floor" || continue
                    printf '        %s   <- above the floor\n' "$entry" >&2
                done
            # Which symbols forced it, because the answer is usually two of
            # them and they are usually nothing the source asked for.
            "$readelf_bin" --dyn-syms -W "$file" 2>/dev/null |
                grep -o "[A-Za-z_][A-Za-z0-9_]*@${prefix}_${highest}" |
                sed 's/@.*//; s/^/        /' | sort -u | head -20 >&2
        else
            printf '  ok  %s: highest %s_%s, at or below %s_%s\n' \
                "$name" "$prefix" "$highest" "$prefix" "$floor"
        fi
    done

    for soname in $("$readelf_bin" -d "$file" 2>/dev/null |
        sed -n 's/.*NEEDED.*\[\(.*\)\]/\1/p'); do
        allowed=0
        for known in $SONAME_ALLOWLIST; do
            [ "$soname" = "$known" ] && allowed=1 && break
        done
        if [ "$allowed" -eq 0 ]; then
            fail "$name requires $soname, which is not a soname this product may depend on"
        fi
    done
done

if [ "$checked" -lt "$expect_elf" ]; then
    die "expected at least $expect_elf ELF binaries, found $checked"
fi

if [ "$checked" -eq 0 ]; then
    printf 'check-abi: no ELF binaries here, nothing to check\n'
    exit 0
fi

if [ "$status" -eq 0 ]; then
    printf 'check-abi: %s ELF binaries load on a %s system\n' "$checked" "$GLIBC_FLOOR"
else
    cat >&2 <<EOF

A published Linux binary will not start on a system this project claims to
support. Two causes, and the lines above say which.

A version above the floor: the binary was linked against a newer glibc than
the one it must run on. Every published Linux archive is built with
\`cargo-zigbuild --target x86_64-unknown-linux-gnu.$GLIBC_FLOOR\`; a plain
\`cargo build\` on the runner takes the runner's glibc instead, and the symbols
listed above are the ones that carried it in.

A soname that is not on the list: a dependency now requires a library file by
name on every user's machine. Decide whether it belongs in the binary at all
before adding it here. Both entries this gate was written for were reached
through a default feature nothing in this product uses.
EOF
fi

exit "$status"
