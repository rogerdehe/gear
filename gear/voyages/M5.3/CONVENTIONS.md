# Working conventions

GEAR runs this repository's task, review and merge workflow. Start with `gear`,
read the assigned packet with `gear --show T-<n>`, and report through `gear send`.
Use the exact result command in the packet, then end the turn.

The shared rules are [default working conventions](../../../skills/gear/references/conventions.md),
also available as `gear --skill conventions`. Review guidance is `gear --skill review`;
[REVIEW.md](REVIEW.md) is retained as this repository's review record format.

Everything of one voyage lives in its folder `gear/voyages/<M>/` (`<M>` is the task's milestone): `plan.md`, `backlog.md`, the writer's report `reports/T-<n>.md`, the review `reviews/T-<n>.md`, and this voyage's `CONVENTIONS.md`, `REVIEW.md`, `gear.toml`, `lessons.md` and `stopgaps.md`. Nothing the captain writes spans voyages: the next voyage starts from a copy of these rules, adapted at its kickoff; only what the release review puts into the skill carries over.

## Test time (user's red line)

The whole test suite — every test of the repository together (`scripts/test.sh`), build excluded — must finish in under 300 seconds; that is the red line, never crossed. The target: 60 s passes, 30 s is good, 10 s is excellent. There are no "heavy" tests in this project.

- A test never sleeps or polls on the wall clock: time is injected (`now`, a fake clock), waits are on a channel or a condition.
- No real network, no real `git push`, no stress loops (`for i in $(seq 20)`), no `--test-threads` tuning in an acceptance.
- A git or process fixture is built once per test file and kept tiny.
- A task that adds tests keeps the total under the line; a hand-in that pushes it over fails review.
- Run tests with `scripts/test.sh` (parallel; it prints every test's time and a warning for each test over 10 s). While working, run only the crate you change: `cargo nextest run -p <crate>`. Run the whole suite once, right before handing in. The merge gate runs `scripts/test.sh` too.
- A test over 10 s gets a warning. The only exception is a line in `scripts/slow-tests.allow` (test, its own max, why it cannot be faster). A lane never adds one itself: it asks the captain (`gear send --kind question`) with the reason. The captain refuses by default and makes the test faster instead; only a one-in-ten-thousand case gets in. For this project the list stays empty: nothing here needs more than 10 s. A listed test still warns past its own max.

## The last step of every voyage

Every voyage plan of GEAR ends with this task, planned at kickoff: read the voyage's lessons (the voyage's `lessons.md`, its plan's Decisions, what went to the backlog) against the shipped skill (`skills/gear/`); fix what contradicts, is stale or no longer matches the code by rewriting or deleting — never by piling on rules; then release. Nothing found means nothing to change.
