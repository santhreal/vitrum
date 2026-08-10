#!/bin/sh
# Fixtures for check-abi.sh.
#
#   check-abi-test.sh        runs every case, exits 1 on the first mismatch
#
# WHY THIS EXISTS
#
# check-abi.sh is the gate that decides whether a release ships, and like its
# sibling it has been wrong in CI, on a runner nobody can attach to, after a
# build that already succeeded:
#
#   - It resolved readelf at the top and died when there was none. macOS has
#     no readelf and no ELF for it to read, so both mac legs failed on
#     "no readelf; install binutils", the release never got its fourth
#     archive, and a finished build published nothing.
#   - Every readelf invocation discards stderr and reads an empty answer as
#     "this binary asks for nothing". A READELF naming a binary that does not
#     exist therefore reported every archive clean. A gate that passes when
#     its tool is missing is worse than no gate, because it is believed.
#
# Neither needed a real binary to catch, only a stub saying those words. Each
# case stubs READELF and asserts the exit status and the sentence, which is
# the whole contract the release workflow depends on.
#
# The case that matters most is `mach-o only, no readelf`. That is the macOS
# leg of every release, and the way a fix for it goes wrong is by letting a
# missing tool wave a real ELF through, which `elf present, readelf missing`
# holds down.

set -eu

script=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/check-abi.sh
[ -f "$script" ] || { echo "check-abi-test: no check-abi.sh beside me" >&2; exit 2; }

work=$(mktemp -d) || exit 2
trap 'rm -rf "$work"' EXIT INT TERM

failures=0
cases=0

# An ELF header is the only thing the gate uses to decide a file is its
# business, so four bytes is a whole fixture.
elf() { printf '\177ELF\002\001\001\000rest' >"$1"; }
macho() { printf '\317\372\355\376rest' >"$1"; }

# stub_readelf <name> <version-rc> <needs> <sonames>
#
# `--version` decides whether the tool is usable at all. `-V` prints the
# version-needs section and `-d` the dynamic section, in the shapes the gate
# greps for. Each stub gets its own path: one shared path meant the last stub
# written was the one every case ran, and the cases still passed for the
# wrong reason.
stub_readelf() {
    stub=$work/readelf-$1
    shift
    {
        echo '#!/bin/sh'
        echo 'case "$1" in'
        printf '    --version) exit %s ;;\n' "$1"
        echo 'esac'
        echo 'for a in "$@"; do'
        echo '    case "$a" in'
        echo '        -V)'
        echo "            cat <<'NEEDS'"
        printf '%s\n' "$2"
        echo 'NEEDS'
        echo '            exit 0 ;;'
        echo '        -d)'
        echo "            cat <<'DYN'"
        printf '%s\n' "$3"
        echo 'DYN'
        echo '            exit 0 ;;'
        echo '    esac'
        echo 'done'
        echo 'exit 0'
    } >"$stub"
    chmod +x "$stub"
    printf '%s' "$stub"
}

# A PATH holding everything the gate runs except a readelf, so that "there is
# no readelf on this machine" is a real condition rather than an environment
# variable the gate could route around. Setting READELF to the empty string
# does not express it: the gate falls through to `command -v readelf`, finds
# the one this machine has, and the case passes for the wrong reason.
sandbox=$work/bin
mkdir -p "$sandbox"
for tool in sh find dd od tr basename dirname mktemp rm tar grep sed sort tail head cat; do
    real=$(command -v "$tool" 2>/dev/null) || continue
    ln -sf "$real" "$sandbox/$tool"
done
if [ -n "$(PATH=$sandbox command -v readelf 2>/dev/null)" ]; then
    echo 'check-abi-test: the sandbox PATH still has a readelf' >&2
    exit 2
fi

# run <name> <expected-rc> <expected-substring> <readelf> <dir>
#
# A readelf of `NONE` runs the gate on the sandbox PATH with READELF unset,
# which is the macOS runner: no such tool anywhere.
run() {
    name=$1 want_rc=$2 want_text=$3 re=$4 dir=$5
    cases=$((cases + 1))

    if [ "$re" = NONE ]; then
        out=$(env -u READELF PATH="$sandbox" "$script" "$dir" 2>&1) && rc=0 || rc=$?
    else
        out=$(READELF=$re "$script" "$dir" 2>&1) && rc=0 || rc=$?
    fi

    if [ "$rc" -ne "$want_rc" ]; then
        printf 'FAIL %s: exit %s, wanted %s\n%s\n' "$name" "$rc" "$want_rc" "$out" >&2
        failures=$((failures + 1))
        return
    fi
    case "$out" in
        *"$want_text"*) printf 'ok   %s\n' "$name" ;;
        *)
            printf 'FAIL %s: output did not contain %s\n%s\n' \
                "$name" "$want_text" "$out" >&2
            failures=$((failures + 1))
            ;;
    esac
}

good=$(stub_readelf good 0 'GLIBC_2.17
GLIBC_2.28' ' 0x0001 (NEEDED)  Shared library: [libc.so.6]')

# THE macOS LEG. Mach-O binaries, no readelf on the machine at all, and the
# answer is that there was nothing here to answer for.
mac=$work/mac
mkdir -p "$mac"
macho "$mac/vitrum"
printf 'notes\n' >"$mac/README"
run 'mach-o only, no readelf' 0 'nothing to check' NONE "$mac"

# THE ONE THAT MUST NOT REGRESS. The same missing tool, now with an ELF in
# front of it. Skipping the check here would ship a binary nobody read.
mixed=$work/mixed
mkdir -p "$mixed"
macho "$mixed/vitrum-darwin"
elf "$mixed/vitrum-linux"
run 'elf present, readelf missing' 2 'no readelf' NONE "$mixed"
run 'elf present, readelf does not run' 2 'does not run' /nonexistent/readelf "$mixed"

# A version above the floor is the failure this gate was written for.
high=$(stub_readelf high 0 'GLIBC_2.17
GLIBC_2.39' ' 0x0001 (NEEDED)  Shared library: [libc.so.6]')
linux=$work/linux
mkdir -p "$linux"
elf "$linux/vitrum"
run 'glibc above the floor fails' 1 'above the GLIBC_2.28 floor' "$high" "$linux"

# A soname no supported distribution ships is the other half of the gate. This
# is the libxdo the 0.1.2 archive shipped with.
xdo=$(stub_readelf xdo 0 'GLIBC_2.17' ' 0x0001 (NEEDED)  Shared library: [libxdo.so.3]')
run 'an unlisted soname fails' 1 'not a soname this product may depend on' "$xdo" "$linux"

run 'a clean elf passes' 0 'load on a 2.28 system' "$good" "$linux"

if [ "$failures" -gt 0 ]; then
    printf 'check-abi-test: %s of %s cases failed\n' "$failures" "$cases" >&2
    exit 1
fi
printf 'check-abi-test: %s cases pass\n' "$cases"
