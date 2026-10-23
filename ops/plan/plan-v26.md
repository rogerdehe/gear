<!-- gear-plan project=gear base=25 -->
# Decisions
- 2026-10-23 Board lane ran dry (all three of its tasks in review): T-7 moves from runtime to board. Review temps go back to warm seats (17 §8a): T-5 made one fresh worktree per task, so every review cold-builds the Rust workspace (~0.8 GB, minutes); the captain also removed the old warm seats by mistake. T-18 restores them, after T-17 on the same lane (same file).

# Queue
## T-7 [must] lane=board milestone=M1 required
deps: -

## T-17 [must] lane=runtime milestone=M1 required
deps: -

## T-15 [must] lane=runtime milestone=M1 required
deps: -

## NEW [must] lane=runtime milestone=M1 required
goal: Review temps use warm seats (conventions 17 §8a). Purpose: a review should not rebuild the whole Rust workspace from nothing — keep each seat's worktree and build cache. Shape: `[quota].temps` = N seats, each a long-lived Herdr worktree on branch `home/review-<k>` (label `<project>-review-<k>`), created once on first need (worktree_open if it exists, else worktree_create — as onboarding does for lanes). A review job takes a free seat: the agent of the previous job is exited (profile exit keys), the seat's checkout switches to the job's branch `review/t<n>` created at the candidate (`git switch -C review/t<n> <candidate>` in the pane before the agent starts), and a NEW agent session starts (clean context) named `<project>-review-t<n>`; a re-review of the same task resumes that job's session. Releasing a job (T-17) exits the agent and keeps the worktree and its cache; it never removes the seat's worktree.
accept: cargo test --workspace green. Environment: the engine ticks concurrently and may restart between any two steps; one seat failing must not stop the tick. Adversarial (one test each): two jobs in a row on one seat reuse the same worktree (one worktree_open/create in total); restart between the exit of the old agent and the start of the new one (no double start, the job still starts); the seat's checkout has uncommitted changes from the last job (they are discarded before switching — a reviewer never commits there except on review/t<n>). Real-environment proof: on this machine, two FakeHerdr-free runs are not possible for the lane — write the exact commands the captain should run to watch the second review start without a cold build.
deps: T-17
