<!-- gear-plan project=gear base=43 -->
# Milestone M1: Self-hosting loop
Done when ONE sentence holds: a task a lane hands in is reviewed and then merged into dev or sent back, without the captain's hands, within about 30 minutes, with reviewers on warm seats (`gear-review-<k>` workspaces, `gear-review-t<n>` agents). Remaining: T-12 (merge), T-18 (warm seats). Every finding is judged only against that sentence.

# Decisions
- 2026-10-23 M1 closing (captain, after the user's reflection on drift): M1 = T-12 + T-18 only. One more review round each; only P0 (wrong code merged, lost work) or a break of the M1 sentence sends them back — everything else is logged in m2-backlog and the captain passes the task. T-15 (quota) and T-21 (findings text in the store) move to the head of M2. Review slots go to M1 first.

# Queue
## T-18 [must] lane=runtime milestone=M1 required
deps: -

## T-12 [must] lane=runtime milestone=M1 required
deps: -

## T-15 [must] lane=data milestone=M2
deps: -

## T-21 [must] lane=board milestone=M2
deps: -
