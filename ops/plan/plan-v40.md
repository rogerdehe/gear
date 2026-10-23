<!-- gear-plan project=gear base=39 -->
# Decisions
- 2026-10-23 The engine now settles verdicts itself (T-13): T-12 and T-15 were sent back without the captain. Side effect: the rework gets every finding, not the captain's scoped list (m2-backlog: a way for the captain to scope an automatic rework).
- 2026-10-23 T-15 failed three times on the same point (delayed exit taken for the new agent) and on merging dev; quota restart is engine internals. It goes back to runtime after T-18, with a narrow brief.

# Queue
## T-18 [must] lane=runtime milestone=M1 required
deps: T-17

## T-15 [must] lane=runtime milestone=M1 required
goal: Quota policy in the engine (PRD §9, kickoff) — rework brief, captain scope. Read first: git show origin/dev:ops/reviews/T-15.md (F3, F4) and the board lane's branch board/t15 (start from it, it has the rest). Do ONLY: (1) merge current origin/dev first (the release module landed at the same place in lib.rs; keep both); (2) after sending the exit keys, the switch completes only when the pane shows a shell (agent_get returns no agent) — an agent still there is the OLD one; then export the next model and start with resume args; (3) quota turns never flag the seat. Everything else in the reviews is out of scope.
accept: cargo test --workspace green merged with dev; engine tests: exit sent and the old agent still present on the next tick -> no start, no adoption; old agent gone -> export + start with resume args + one continue delivery; quota turns do not flag. Environment: one engine restart between exit and start (test it).
deps: T-18
