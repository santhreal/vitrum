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
# The case that matters most is `sme tile with a mnemonic`. The fix for the
# false positive was to require corroboration, and the way that fix goes wrong
# is by swallowing real SME too. That case MUST still fail.

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

# The exact arm64 mac failure. SVE is dispatched from HWCAP by highway at every
# pinned CPU, so it says nothing about the pin and the floor concedes it.
run 'dispatched sve passes' 0 '1 binaries clean' \
    'mach-o arm64' \
'100564700: 	ptrue	p0.b, vl16
100564704: 	fnmls	z7.h, p3/m, z27.h, z13.h'

# A tile operand with no instruction that names one. Nothing can address ZA
# without an SME mnemonic, so those four bytes were a literal pool.
run 'lone tile operand is decoded data' 0 'decoded data, not code' \
    'mach-o arm64' \
'100564700: 	mov	za0.s[w12, 0], p0/m, z1.s
100564704: 	add	x0, x1, x2'

# THE ONE THAT MUST NOT REGRESS. Same operand, now with a mnemonic that only
# an SME compiler emits.
run 'sme tile with a mnemonic fails' 1 'above the armv8.2-a with dispatched SVE floor' \
    'mach-o arm64' \
'100000000: 	smstart	za
100000004: 	mov	za0.s[w12, 0], p0/m, z1.s'

# SME streaming mode is self-evidencing on its own, with no tile operand.
run 'sme streaming mode fails' 1 'above the armv8.2-a with dispatched SVE floor' \
    'mach-o arm64' \
'100000000: 	smstart	sm
100000004: 	add	x0, x1, x2'

# i8mm and bf16 are armv8.6 and never need streaming mode, so the mnemonic is
# the whole evidence. A pin that stopped reaching a compiler shows up here.
run 'bf16 mnemonic fails' 1 'above the armv8.2-a with dispatched SVE floor' \
    'mach-o arm64' \
'100000000: 	bfdot	v0.2s, v1.4h, v2.4h
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
