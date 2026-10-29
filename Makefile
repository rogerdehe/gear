# One target per situation: whoever runs a build or a test picks the right cost
# by name. The machine is shared with another project, so the job counts come
# from tools/build-slot/build-slot.sh, which sizes each run to what is on the
# machine now: no fixed caps, no global config, no locks, no queues (user,
# 2026-10-29; the rule is in tools/build-slot/README.md).

.PHONY: check gate full install

# A run of tests never writes to the system disk (T-412: the disk filled to
# 1 GB free, mostly test scratch directories left in the system temp
# directory). Every target below runs its cargo commands with TMPDIR in the
# checkout, and first deletes what an earlier run left there more than an hour
# ago — a lane killed mid-run leaves its scratch behind.
export TMPDIR := $(CURDIR)/target/tmp
prune-tmp:
	mkdir -p $(TMPDIR)
	find $(TMPDIR) -mindepth 1 -maxdepth 1 -mmin +60 -exec rm -rf {} +

PREFIX ?= $(HOME)/.local

install: ## a source checkout's gear on PATH: release build, then a link in $(PREFIX)/bin
	cargo build --release -p gear-cli
	mkdir -p $(PREFIX)/bin
	ln -sf $(CURDIR)/target/release/gear $(PREFIX)/bin/gear

check: prune-tmp ## the light run for lanes and reviewers: one crate, jobs from build-slot; TESTS=<nextest filter> runs only the tests it matches
	@test -n "$(CRATE)" || { echo "usage: make check CRATE=<crate>"; exit 1; }
	tools/build-slot/build-slot.sh cargo nextest run -p $(CRATE) --no-fail-fast $(if $(strip $(TESTS)),-E '$(TESTS)')

gate: prune-tmp ## the merge gate on the shared machine: workspace build and the whole suite, each sized by build-slot (the test run measures the machine after the build)
	tools/build-slot/build-slot.sh cargo build --workspace --all-targets && tools/build-slot/build-slot.sh scripts/test.sh

full: prune-tmp ## full speed for one person alone on the machine: no caps
	cargo build --workspace --all-targets && scripts/test.sh
