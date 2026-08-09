# Entry points for the things you would otherwise have to remember.
#
# Nothing here is required to build vitrum: `cargo build --release --workspace`
# is the whole build. These are the multi-step jobs, and the README's generated
# tables in particular, where the argument order is the part that goes wrong.

CARGO ?= cargo
PYTHON ?= python3
RUSTFLAGS_STRICT := -D warnings

# The harness runs to feed `readme-perf`. Override any of them to point the
# tables at your own measurement, then run `make readme-perf`.
PROBE_RUN ?= harness/out/probe-20260806T035911Z
MEMORY_RUNS ?= harness/out/memory-20260806T192242Z harness/out/memory-20260806T192658Z
IDLE_RUN ?= harness/out/idle-cpu-20260806T192751Z

.PHONY: help build test clippy gate lanes plan perf-tables perf-tables-check \
	measure package release release-dry-run release-check verify-artifacts \
	check-isa clean

help:
	@echo 'build              release build of every crate, warnings fatal'
	@echo 'test               release test run of every crate'
	@echo 'clippy             advisory lints'
	@echo 'gate               build, then test, exactly as CI does'
	@echo 'measure            run the harness on the measurement host'
	@echo 'perf-tables        snapshot the harness runs and inject docs/performance.md'
	@echo 'perf-tables-check  fail if those tables are stale (CI runs this)'
	@echo 'package            build the release archive and verify its checksum'
	@echo 'verify-artifacts   build the archive and install it through install.sh'
	@echo 'check-isa          disassemble built binaries; fail above the CPU floor'
	@echo 'release-check      every version literal and target triple agrees'
	@echo 'release-dry-run    rehearse a cut in a scratch clone; VERSION=x.y.z'
	@echo 'release            cut it here: bump, changelog, commit, tag; VERSION=x.y.z'
	@echo 'lanes              every worktree, what is uncommitted, what is unpushed'
	@echo 'plan               group open pull requests into non-overlapping waves'
	@echo
	@echo 'a wave is staged, gated once and landed with tools/integrate.py:'
	@echo '  tools/integrate.py stage 56 52 57   merge each onto a staging branch'
	@echo '  tools/integrate.py gate             build and test the wave once'
	@echo '  tools/integrate.py attribute        if red, find which one did it'
	@echo '  tools/integrate.py land --push      move main onto it, nothing squashed'

build:
	RUSTFLAGS='$(RUSTFLAGS_STRICT)' $(CARGO) build --release --workspace --locked

test:
	RUSTFLAGS='$(RUSTFLAGS_STRICT)' $(CARGO) test --release --workspace --locked

clippy:
	$(CARGO) clippy --release --workspace --all-targets

lanes:
	$(PYTHON) tools/integrate.py lanes

plan:
	$(PYTHON) tools/integrate.py plan

gate: build test

# The measurement itself needs the rig: a host with a display it owns and the
# binaries staged onto it. It is deliberately not a dependency of readme-perf,
# because regenerating the tables must not silently re-measure on whatever
# machine happens to be running make.
measure:
	harness/run.sh probe
	harness/run.sh memory 1
	harness/run.sh memory 20
	harness/run.sh idle-cpu 60 20
	@echo 'now point PROBE_RUN, MEMORY_RUNS and IDLE_RUN at the new directories'

perf-tables:
	$(PYTHON) harness/readme_perf.py snapshot \
		--probe $(PROBE_RUN) \
		$(foreach run,$(MEMORY_RUNS),--memory $(run)) \
		--idle $(IDLE_RUN)
	$(PYTHON) harness/readme_perf.py render

perf-tables-check:
	$(PYTHON) harness/readme_perf.py render --check

package:
	./packaging/build-release-asset.sh
	cd dist && sha256sum -c SHA256SUMS

# One command per release, and no prompt in any of them.
#
# `release` stops with the tag made and nothing pushed, because the push is
# the step that cannot be taken back. `release-dry-run` does the whole thing
# in a throwaway clone and proves this tree came out byte-identical, so it is
# the one to run first and it costs nothing to run again.
VERSION ?=
need-version = test -n '$(VERSION)' || { echo 'usage: make $@ VERSION=x.y.z' >&2; exit 1; }

release:
	@$(need-version)
	@tools/release/cut.sh '$(VERSION)'

release-dry-run:
	@$(need-version)
	@tools/release/dry-run.sh '$(VERSION)'

release-check:
	tools/release/versions.sh check
	tools/release/targets.sh check

# Takes the archive that `package` built. Given no argument it checks `dist`,
# which is where that archive lands.
check-isa:
	./tools/release/check-isa.sh $(if $(DIR),$(DIR),dist)

verify-artifacts:
	./tools/release/verify-artifacts.sh

clean:
	$(CARGO) clean
