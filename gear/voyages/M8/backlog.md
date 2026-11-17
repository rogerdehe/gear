# Backlog

The next voyage's candidates. Its goal is agreed at its kickoff.

## Candidates
- **A task registered as already done hits `max_task_min`** (Prometheus M5, 2026-11-11): work done before the voyage was registered was refused for its real estimate (120 > 60); the captain raised the cap. Decide at kickoff whether the cap applies to a captain-alone project at all, since it sizes packets for lanes.

Moved to M7.3 (2026-11-17): a spent Claude lane (T-711), the missing seat branch (T-710), the restart request key (T-709), the done sentence once on the home page (T-708), named NEW blocks for deps (T-707), squash and rebase, the parallel-change check, the records merge files, integrated-by with voyage files only, the records-only landing. The idle-voyage count was fixed in v0.7.24.

## Postponed from M7.1 (2026-11-09: M7.1 closed with these open; check each against the code at kickoff)
- **A lane can leave a started task and take the next one** (M7.1, 2026-11-07): mast's T-678 and T-679 stayed Started from 01:59/02:01 with no commits while mast worked T-681 to T-683 (their packets said to stop if T-677's function was not there yet); the snapshot showed "T-678 working" and the fleet sat idle until the idle-voyage item rang. A lane that hands in another task while an attempt of its own is still Started should make that attempt's state visible to the captain (an item naming it), not hide it as "working".
- **`merge::tests::a_merge_that_lands_closes_the_red_item` spins forever when its runner dies** (M7.1 landing, 2026-11-07, M-8886; also M6 F-338): three copies from the removed M6.3 hull worktree ran 15h at 10% CPU each with parent pid 1 until the new orphan item named them and the captain stopped them. Find the loop that never ends without its parent (a wait on a merge worker or a channel) and give it a deadline.
- Infra refusals of the merge gate page the captain and need a hand `- remerge` (Lawrence M3.1, 2026-11-08: GitHub dispatch 422, unreadable artifact, 2-3 of 11 refusals). Retry a merge once by itself when the gate's failure is an infra one, before the item rings.
- A gate-only failure of an accepted task becomes a new carrier task with its own full review (Lawrence M3.1, 2026-11-08: timing-cap jitter, repo-wide invariant tests a lane's check never runs; ~25-40 min each on the critical path). Allow a gate fix-up attempt on the accepted task: its lane pushes the fix to the same branch and a light review reads only the new diff.
