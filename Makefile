# One target per situation: whoever runs a build or a test picks the right cost
# by name. The machine is shared with another project, so the caps live in the
# targets, not in machine settings: no global config, no locks, no queues
# (user, 2026-10-28).

.PHONY: check gate full install

PREFIX ?= $(HOME)/.local

install: ## a source checkout's gear on PATH: release build, then a link in $(PREFIX)/bin
	cargo build --release -p gear-cli
	mkdir -p $(PREFIX)/bin
	ln -sf $(CURDIR)/target/release/gear $(PREFIX)/bin/gear

check: ## the light run for lanes and reviewers: one crate, 2 build jobs, 2 test threads; TESTS=<nextest filter> runs only the tests it matches
	@test -n "$(CRATE)" || { echo "usage: make check CRATE=<crate>"; exit 1; }
	CARGO_BUILD_JOBS=2 NEXTEST_TEST_THREADS=2 cargo nextest run -p $(CRATE) --no-fail-fast $(if $(strip $(TESTS)),-E '$(TESTS)')

gate: ## the merge gate on the shared machine: workspace build and the whole suite, 4 jobs
	CARGO_BUILD_JOBS=4 cargo build --workspace --all-targets && CARGO_BUILD_JOBS=4 NEXTEST_TEST_THREADS=4 scripts/test.sh

full: ## full speed for one person alone on the machine: no caps
	cargo build --workspace --all-targets && scripts/test.sh
