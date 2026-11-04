# Postmortem: accepted work that never landed, and the branch pile-up (2026-11-04)

Severity: P-1. Owner: the GEAR maintainer. GEAR caused it.

## What happened

In the selyra session (Prometheus, Lawrence), 63 tasks were done with an accepted review while their accepted code was not on `dev`. The real landing units were 23 heads; the rest was nested or carried. GitHub held 155 branches, about 140 of them task and review branches. Many had sat for days.

## Why GEAR let it happen

1. **"Done" meant "accepted", not "landed".** A task went Done when its review passed. A failed merge left it Done. Progress counts, stations and the captain's view all read Done.
2. **The lost-work scan had exceptions.** It skipped a carried task while its carrier was unmerged, a rule added in v0.6.37 with `carried-by`. It also skipped a task whose failed-merge item was open. Carriers were rebundled for hours (T-579 → T-595 → T-597 → T-603), so their carried tasks were never reported.
3. **The reports could be ignored.** The scan sent ordinary items to the captain every few minutes and never escalated. Open items piled up (M-8859, M-8864, M-8889).
4. **Branches were only removed on a task's own merge.** A carried task's branches stayed when its carrier landed. A done task whose merge failed kept its branches indefinitely. Gate candidate refs leaked when a gate was interrupted.

## Fixed already

- v0.6.47: the lost scan has no exceptions for failed merges, and its item is urgent. A paused project still records a running merge.
- v0.6.48: a carried task is waited for only while its carrier is active, and for at most 2 hours after acceptance. A remerge of landed work records the merge.
- Cleanup: 136 stale Prometheus/Lawrence branches were moved to `refs/archive/` (nothing deleted). 1215 stale local branches were removed, with a full bundle backup. History was rewritten so that dev+main+staging is about 80 MB.

## Remediation (to build in M6.2, in this order)

1. **Done = landed.** A task whose review passed and whose work is not on the integration branch is shown and counted as *landing*, not done. Done is set when its merge lands, or when its carrier's merge lands. Progress, stations, voyage completion and `gear queue` count landed work only. Dependencies already wait for merged work (T-236).
2. **A landing deadline with escalation.** Accepted work lands within 2 hours. The urgent item goes to the captain at the deadline. At twice the deadline, the board shows a red *unlanded work* banner that names the tasks, and the next doorbell to the captain leads with it. No path to quiet it except landing the work or dropping it in the plan.
3. **Carried work is bounded.** `carried-by` is accepted only when the carrier is ready or running. When the carrier lands, the carried tasks' task and review branches are deleted with it (`land_carried`).
4. **Branch lifetime.** The branch sweep removes a task's branches as soon as its work is on the integration branch, whatever landed it. A task branch older than 3 hours whose work has not landed is an urgent item. GEAR never deletes unlanded work; it reports it.
5. **One landed-work check.** Every report of progress or loss reads the same check: is the accepted candidate an ancestor of the integration branch? That replaces task state and per-path flags.
6. **The maintainer's own rule** (memory: done-means-landed): progress is reported from landed state, never from task state.
