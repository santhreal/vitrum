#!/bin/sh
# Fail if a shipped binary can execute an instruction the target CPU floor does
# not promise.
#
#   check-isa.sh <path>...        binaries, or directories to search
#
# Exit 0 clean, 1 something above the floor, 2 the check could not run.
#
# WHY THIS EXISTS
#
# A published binary must run on every machine its triple claims. That is not
# automatic. A compiler asked to build for the machine it is running on emits
# instructions that machine has, and every target this project publishes is
# built on a runner of its own architecture, so a build that detects instead of
# obeying makes the instruction set of a release a property of the runner.
#
# libghostty-vt-sys 0.2.1 did that: it passed -Dtarget to zig only when
# cross-compiling. Built on a Zen 5 desktop the library carried 5581 AVX-512
# instructions; pinned, it carries none. On a CPU without AVX-512 the
# difference is SIGILL on first use, with no message.
#
# WHERE THE FLOOR IS, AND WHY IT IS NOT BASELINE
#
# The floor is AVX2 on x86-64 and armv8.2-a on aarch64, not the architecture
# baseline, and the reason is measurable rather than a preference. Ghostty
# vendors highway and simdutf; Rust pulls in memchr and its relatives. All of
# them compile several kernels and pick one at run time from CPUID. Their
# AVX2 code is present in every build and is never executed on a machine that
# cannot run it.
#
# The two builds prove the distinction. Pinned to `baseline` and unpinned, the
# static library carries the same 3662 AVX2 instructions and differs only in
# AVX-512: 0 against 5581. So AVX2 in these artifacts is dispatched. A gate set
# at the true baseline would fire on every build ever made here, and a gate
# that always fires is a gate somebody deletes.
#
# WHAT THIS CONCEDES
#
# A runner whose CPU is above the floor but at or below AVX2 could host-detect
# without being caught. That is a narrow window — GitHub's x86 runners have
# had AVX2 since Broadwell — and the pin, not this gate, is what closes it.
# This is the backstop that notices when the pin is lost, and it notices on
# exactly the runners where losing it does damage.
#
# It also cannot tell guarded code from unguarded code. It approximates: if a
# binary contains above-floor vector code and executes no CPUID at all, there
# is nothing that could be doing the guarding, and that is reported.
#
# Above the floor is therefore not proof of host detection, and this gate has
# already found the other kind. `rav1e`, an AV1 encoder reached through
# `image`'s default features, put 278 CPUID-dispatched AVX-512 instructions in
# vitrum.exe: code that would not have faulted anywhere. The finding was still
# correct, because the dependency had no business being linked, but a reader
# who assumes a lost pin will go looking in the wrong place. The failure text
# at the bottom describes both, and the symbol table separates them.

set -eu

die() { printf 'check-isa: %s\n' "$*" >&2; exit 2; }
fail() { printf 'check-isa: %s\n' "$*" >&2; status=1; }

[ $# -gt 0 ] || die 'nothing to check'

# llvm-objdump reads ELF, Mach-O and PE and knows every architecture, so one
# disassembler covers all four published targets. GNU objdump is built for one
# target family and cannot read a Mach-O arm64 binary on an x86 host.
if [ -n "${OBJDUMP:-}" ]; then
    objdump=$OBJDUMP
elif command -v llvm-objdump >/dev/null 2>&1; then
    objdump=llvm-objdump
elif sysroot=$(rustc --print sysroot 2>/dev/null) &&
     found=$(ls "$sysroot"/lib/rustlib/*/bin/llvm-objdump 2>/dev/null | head -1) &&
     [ -n "$found" ]; then
    objdump=$found
elif command -v objdump >/dev/null 2>&1; then
    objdump=objdump
else
    die 'no llvm-objdump; install the llvm-tools rustup component'
fi

# Above AVX2 on x86-64, matched on operands rather than mnemonics wherever a
# register class gives it away:
#
#   %zmm        512-bit vectors: AVX-512 in any of its forms.
#   %k0-%k7     AVX-512 mask registers, which also catch EVEX-encoded
#               128- and 256-bit instructions that carry no %zmm operand.
#   %tmm        AMX tile registers.
#
# The named ones are AVX-512-era instructions that can appear without a mask
# or a 512-bit operand, and the scalar extensions that arrived alongside them.
X86_ABOVE_FLOOR='%zmm|%k[0-7]|%tmm|	(vpternlog|vpconflict|vplzcnt|vpcompress|vpexpand|vscatter|vrange|vreduce|vfixupimm|vgetmant|vgetexp|vrcp14|vrsqrt14|vpmultishift|vpopcnt|vpshld|vpshrd|vpdpbusd|vpdpwssd|vcvtne2ps|vdpbf16ps|v4fmadd|v4fnmadd|vp4dpwssd|kmov|kand|kor|kxor|knot|kadd|kshift|kunpck|ktest|kortest)|	(sha512|sm3|sm4|aesenc256|aesdec256)|	(tile|tdp|tld|tst)[a-z]+|	(cldemote|serialize|senduipi|hreset|xsavec|xsaves)'
# Above armv8.2-a on aarch64. The Apple cores in the mac runners are 8.4 and
# newer, so the risk is the same one x86 has: a build reading the core it is
# on and emitting SVE, SME or the matrix extensions.
#
#   z0-z31 / p0-p15   SVE and SVE2 vector and predicate registers.
#   ptrue, whilelo    SVE control instructions.
#   smstart, rdsvl    SME streaming mode.
#   smmla, bfdot      i8mm and bf16, armv8.6.
ARM_ABOVE_FLOOR='	(ptrue|whilel[eot]|whileg[et]|rdvl|addvl|addpl|setffr|rdffr|smstart|smstop|rdsvl|addsvl|addspl|zero	)|[	,]z[0-9]+\.|[	,]p[0-9]+[/.]|	(smmla|usmmla|ummla|bfdot|bfmmla|bfcvt|bfmlal)|	(ld64b|st64b|cpyf|setp|mops)'

status=0
checked=0

files=$(
    for path in "$@"; do
        if [ -d "$path" ]; then
            find "$path" -type f -perm -u+x
        elif [ -f "$path" ]; then
            printf '%s\n' "$path"
        else
            printf 'check-isa: no such path: %s\n' "$path" >&2
            exit 2
        fi
    done
)
[ -n "$files" ] || die 'no files to check'

for file in $files; do
    # A tarball or a text file alongside the binaries is not a defect, so skip
    # anything the disassembler does not recognise rather than failing on it.
    #
    # Match on the `file format` line rather than on `architecture:`. Only ELF
    # and PE get an `architecture:` line; for Mach-O llvm-objdump prints a Mach
    # header table instead, so keying on it skipped every macOS binary, left
    # `checked` at zero and failed both mac legs with "no binaries found to
    # disassemble". Every object format carries `file format`.
    fmt=$($objdump --file-headers "$file" 2>/dev/null |
        sed -n 's/.*file format \(.*\)/\1/p' | head -1) || fmt=
    [ -n "$fmt" ] || continue

    case "$fmt" in
        *x86-64*|*x86_64*)  pattern=$X86_ABOVE_FLOOR; floor='AVX2' ;;
        *arm64*|*aarch64*)  pattern=$ARM_ABOVE_FLOOR; floor='armv8.2-a' ;;
        *) die "unknown object format '$fmt' in $file; extend the floor table" ;;
    esac

    checked=$((checked + 1))

    # `--no-show-raw-insn` keeps encoded bytes out of the text, so a byte
    # sequence that happens to spell one of these names cannot match.
    text=$($objdump -d --no-show-raw-insn "$file" 2>/dev/null) || text=
    [ -n "$text" ] || { fail "$file: could not be disassembled"; continue; }

    hits=$(printf '%s\n' "$text" | grep -E -c "$pattern" || true)
    if [ "$hits" -gt 0 ]; then
        fail "$file: $hits instructions above the $floor floor"
        printf '%s\n' "$text" | grep -E "$pattern" | head -5 | sed 's/^/    /' >&2
        printf '    (first 5 of %s)\n' "$hits" >&2
        # An address names nothing anyone can act on, and the next question a
        # failure here always asks is which dependency did it. `objdump -d`
        # already prints a symbol header before each function, so attribute
        # every hit to the one it falls under and report the worst offenders.
        # A mangled Rust symbol carries its crate, which is the answer.
        CHECK_ISA_PATTERN=$pattern
        export CHECK_ISA_PATTERN
        printf '%s\n' "$text" | awk '
            /^[0-9a-fA-F]+ </ {
                sym = $0
                sub(/^[0-9a-fA-F]+ </, "", sym)
                sub(/>:.*$/, "", sym)
                next
            }
            $0 ~ ENVIRON["CHECK_ISA_PATTERN"] { n[sym]++ }
            END { for (s in n) printf "%8d  %s\n", n[s], s }
        ' | sort -rn | head -10 | sed 's/^/    /' >&2
        printf '    (functions carrying them, worst first)\n' >&2
        continue
    fi

    # Everything at or below the floor is allowed because the libraries that
    # emit it choose at run time from CPUID. If a binary carries that code and
    # never executes CPUID, nothing is choosing, and the assumption this floor
    # rests on does not hold for it.
    case "$fmt" in
        *x86-64*|*x86_64*)
            wide=$(printf '%s\n' "$text" | grep -c '%ymm' || true)
            guards=$(printf '%s\n' "$text" | grep -cE '	cpuid' || true)
            if [ "$wide" -gt 0 ] && [ "$guards" -eq 0 ]; then
                fail "$file: $wide AVX2 instructions and no CPUID anywhere; nothing can be dispatching them"
                continue
            fi
            printf '  ok  %s: %s, nothing above %s, %s AVX2 behind %s CPUID sites\n' \
                "$(basename "$file")" "$fmt" "$floor" "$wide" "$guards"
            ;;
        *)
            printf '  ok  %s: %s, nothing above the %s floor\n' \
                "$(basename "$file")" "$fmt" "$floor"
            ;;
    esac
done

[ "$checked" -gt 0 ] || die 'no binaries found to disassemble'

if [ "$status" -eq 0 ]; then
    printf 'check-isa: %s binaries clean\n' "$checked"
else
    cat >&2 <<'EOF'

A published binary carries instructions the CPU floor does not promise, so it
will SIGILL on a machine without them. There are two causes, and the symbol
table above tells them apart.

The hits are spread across everything: a build let a compiler detect the
builder's CPU instead of being told the target. Check that the pinned target
and CPU reach every compiler in the build, including the ones run by build
scripts, and confirm by re-running this rather than by reading the flags.

The hits sit in one dependency: it ships above-floor code of its own and
dispatches it from CPUID, so it will not actually SIGILL. Decide whether that
dependency belongs in the binary at all. `rav1e`, an AV1 encoder reached
through `image`'s default features, put 278 AVX-512 instructions in vitrum.exe
this way, and the answer was that a terminal multiplexer does not ship a video
encoder.

A stripped PE has no symbols to attribute to, so on Windows the table collapses
to `.text` and the delta between the two shipped binaries is the better signal.
EOF
fi

exit "$status"
