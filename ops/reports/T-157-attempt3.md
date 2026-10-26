# T-157 — the board's view 2 (Graph), attempt 3 (the conflict-only round)

Scope: T-157 attempt 3 — the conflict-only round: `origin/dev` merged into
`deck/t157`, with both modules kept.

Branch: `deck/t157` (the same branch as the earlier attempts; code @ the
hand-in commit)

## What changed

Nothing but the merge: `origin/dev` carries the idle reasons of T-155 and
this branch carries the graph view of T-157, and the two wanted the same lines
of the board's module list. The conflict is resolved by keeping **both**:

- `crates/gear-board/src/lib.rs`: `pub mod graph;` and `pub mod idle;`;
- `crates/gear-board/tests/it/main.rs`: `mod graph_contract;` and
  `mod idle_contract;`.

## Verification

- `cargo build --workspace --all-targets` → clean, **no warning**.
- `cargo test -p gear-board` → **48 passed, 0 failed** (the graph's 7 tests
  and the idle reasons' 13 among them).
- `scripts/test.sh` → **1058 tests run, 1058 passed, 0 skipped**.
- The tests over 10 s that the suite warns about are the machine (the load
  average was 52–72 while the suite ran): the git-heavy engine tests, which
  this branch does not touch.

## Open questions

None.

## Out of scope

Everything else: the graph view and the task panel are as in attempt 2.
