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

.PHONY: help build test clippy gate readme-perf readme-perf-check measure package clean

help:
	@echo 'build              release build of every crate, warnings fatal'
	@echo 'test               release test run of every crate'
	@echo 'clippy             advisory lints'
	@echo 'gate               build, then test, exactly as CI does'
	@echo 'measure            run the harness on the measurement host'
	@echo 'readme-perf        snapshot the harness runs and inject README tables'
	@echo 'readme-perf-check  fail if the README tables are stale (CI runs this)'
	@echo 'package            build the release archive and verify its checksum'

build:
	RUSTFLAGS='$(RUSTFLAGS_STRICT)' $(CARGO) build --release --workspace --locked

test:
	RUSTFLAGS='$(RUSTFLAGS_STRICT)' $(CARGO) test --release --workspace --locked

clippy:
	$(CARGO) clippy --release --workspace --all-targets

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

readme-perf:
	$(PYTHON) harness/readme_perf.py snapshot \
		--probe $(PROBE_RUN) \
		$(foreach run,$(MEMORY_RUNS),--memory $(run)) \
		--idle $(IDLE_RUN)
	$(PYTHON) harness/readme_perf.py render

readme-perf-check:
	$(PYTHON) harness/readme_perf.py render --check

package:
	./packaging/build-release-asset.sh
	cd dist && sha256sum -c SHA256SUMS

clean:
	$(CARGO) clean
