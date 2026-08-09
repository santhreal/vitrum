#!/bin/sh
# Fixtures for check-isa.sh.
#
#   check-isa-test.sh        runs every case, exits 1 on the first mismatch
#
# WHY THIS EXISTS
#
# check-isa.sh is the gate that decides whether a release ships. Both times it
# has been wrong it was wrong in CI, on a runner nobody can attach to, after a
# forty-minute build:
#
#   - It read the architecture out of `architecture:`, which only ELF and PE
#     carry, so every macOS binary was skipped and both mac legs died on "no
#     binaries found to disassemble".
#   - It failed the arm64 mac leg on a single `fnmls z7.h, p3/m, z27.h, z13.h`
#     in a stripped __text, on a target whose CPUs have no SVE at all. Four
#     bytes of literal pool decoded as an instruction.
#
# Neither needed a real binary to catch, only a disassembler saying those
# words. So each case here stubs `OBJDUMP` and asserts the exit status and the
# sentence, which is the whole contract the release workflow depends on.
#
# The case that matters most is `arm sve with a control instruction`. The fix
# for the false positive was to require corroboration, and the way that fix
# goes wrong is by swallowing real SVE too. That case carries the exact
# instruction from the failure above and MUST still fail.

set -eu

script=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/check-isa.sh
[ -f "$script" ] || { echo "check-isa-test: no check-isa.sh beside me" >&2; exit 2; }

work=$(mktemp -d) || exit 2
trap 'rm -rf "$work"' EXIT INT TERM

failures=0
cases=0

# run <name> <expected-rc> <expected-substring> <file-format> <disassembly>
run() {
    name=$1 want_rc=$2 want_text=$3 fmt=$4 dis=$5
    cases=$((cases + 1))

    bin=$work/bin
    : >"$bin"

    stub=$work/objdump
    {
        echo '#!/bin/sh'
        # `--file-headers` anywhere in the arguments means the header query.
        echo 'for a in "$@"; do'
        echo '    if [ "$a" = "--file-headers" ]; then'
        printf "        printf '%%s:\\\\tfile format %s\\\\n' \"\$1\"\n" "$fmt"
        echo '        exit 0'
        echo '    fi'
        echo 'done'
        echo "cat <<'DISASM'"
        printf '%s\n' "$dis"
        echo 'DISASM'
    } >"$stub"
    chmod +x "$stub"

    out=$(OBJDUMP=$stub "$script" "$bin" 2>&1) && rc=0 || rc=$?

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

# A Mach-O binary is read at all. Keying on `architecture:` skipped these and
# reported "no binaries found to disassemble" with a clean exit path.
run 'mach-o arm64 is disassembled' 0 '1 binaries clean' \
    'mach-o arm64' \
'100000000: 	add	x0, x1, x2
100000004: 	fmla	v0.4s, v1.4s, v2.4s'

# The exact arm64 mac failure: one SVE-shaped operand, nothing establishing a
# vector length. Not code.
run 'lone sve operand is decoded data' 0 'decoded data, not code' \
    'mach-o arm64' \
'100564700: 	fnmls	z7.h, p3/m, z27.h, z13.h
100564704: 	add	x0, x1, x2'

# THE ONE THAT MUST NOT REGRESS. Same instruction, now with a control
# instruction establishing a predicate. That is a compiler emitting SVE.
run 'sve with a control instruction fails' 1 'above the armv8.2-a floor' \
    'mach-o arm64' \
'100000000: 	ptrue	p0.b, vl16
100000004: 	fnmls	z7.h, p3/m, z27.h, z13.h'

# SME streaming mode is self-evidencing too, with no z or p operand anywhere.
run 'sme streaming mode fails' 1 'above the armv8.2-a floor' \
    'mach-o arm64' \
'100000000: 	smstart	sm
100000004: 	add	x0, x1, x2'

# Corroboration is aarch64-only. AVX-512 has no equivalent of a vector length
# to establish, so one %zmm is one too many.
run 'a single avx-512 operand fails' 1 'above the AVX2 floor' \
    'mach-o 64-bit x86-64' \
'1000: 	vinsertf64x2	$0x2, %xmm1, %zmm0, %zmm0
1004: 	cpuid'

# The floor sits at AVX2 because the libraries emitting it dispatch from CPUID.
# A binary with the code and no CPUID has nothing doing the dispatch.
run 'avx2 with no cpuid fails' 1 'nothing can be dispatching them' \
    'elf64-x86-64' \
'1000: 	vpaddd	%ymm0, %ymm1, %ymm2
1004: 	ret'

run 'avx2 behind cpuid passes' 0 'nothing above AVX2' \
    'elf64-x86-64' \
'1000: 	cpuid
1004: 	vpaddd	%ymm0, %ymm1, %ymm2'

if [ "$failures" -gt 0 ]; then
    printf 'check-isa-test: %s of %s cases failed\n' "$failures" "$cases" >&2
    exit 1
fi
printf 'check-isa-test: %s cases pass\n' "$cases"
