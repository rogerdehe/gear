<!-- gear-plan project=gear base=41 -->
# Decisions
- 2026-10-23 Root cause of the review-copy saga (copies on dev, then deletions, then merge conflicts, then a script exception — all reverted): GEAR stores a review's findings as a PATH on the review branch, so once the reviewer is released the findings are hard to reach. Fix at the root: the store keeps the findings text itself. No review copies on dev, ever.

# Queue
## T-15 [must] lane=data milestone=M1 required
deps: -

## NEW [must] lane=data milestone=M1 required
goal: A review's findings live in GEAR's store, not only as a file on a review branch. Purpose: a lane doing rework must read the findings from GEAR (rework note / task packet), whatever happened to the reviewer, its worktree or its branch; nobody copies review files around. `gear send --kind review ... "<findings path or text>"`: when the argument is a path to a file in the reviewer's cwd, store the file's TEXT (bounded, e.g. 64 KiB, with a note if cut); otherwise store the text. settle_review puts that text into the rework note; `gear --show T-n` shows the latest review's findings for a reworked task. Environment: the reviewer runs the command in its own worktree; the engine settles concurrently.
accept: cargo test --workspace green; tests: a review handed in as a path stores the file content; the rework note and the task packet contain the findings text (not a path); a missing file -> BAD_FORMAT, nothing stored. Real proof: hand in a scratch review with the built binary on a scratch store and show the packet.
deps: -
