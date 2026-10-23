<!-- gear-plan project=gear base=10 -->
# Decisions
- 2026-10-22 Rolling horizon (user's experience): keep 3–5 independent steps per lane, stable while they run; steer with small corrections. Board lane was down to one step: T-9 moves to board (reuses the board renderer).

# Queue
## T-9 [should] lane=board milestone=M1
goal: The captain's home page (`gear` as captain) starts with a short snapshot before the inbox, reusing the gear-board model: totals; per lane — current task and state, number of ready tasks queued; deliveries not confirmed; temps in use / quota. Read-only, under 15 lines.
accept: cargo test --workspace green; a test renders the captain home with 2 lanes and checks the snapshot lines come before INBOX; run `gear` as captain on this machine and paste the first 15 lines in the report.
deps: -
