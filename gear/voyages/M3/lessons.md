# M3 lessons

What M3 taught, recorded as it happened. A lesson holds for this voyage; only what the release review distils into the shipped skill (`skills/gear/`) carries over. Numbers are the ones lessons had in the old shared file, so earlier references still find them.

7d. **A failed review is settled in the plan's `# Verdicts`, never by a reply** (M3, 2026-10-25). The captain replied "rework F1+F2" to two review fails and resolved them, following the home page's `next: gear reply … --resolve`; nothing was settled, the tasks stayed verifying, and a lane found it while waiting on one. The command GEAR suggests must be the one that does the job.

18. **Development never touches the production store, nor acts as a seat.** Tests run with a scratch store and HOME, GEAR_CTX blanked, no real `gh` and git limited to local paths (`.cargo/config.toml`), and push nothing: a lane's GEAR_CTX reached a test hook in M3 and sent the captain 300+ urgent items, and a trial team's test run full of `git push` crossed a company red line.

19. **Two writers push the integration branch** (the engine's merges, the captain's plan commits): the captain holds its pushes while a merge is in flight (M3: two merges lost to the captain's own pushes).

21. **The plan's allowance is the ceiling.** The captain never sets GEAR_ADMIN, never registers a project, never starts an agent outside the plan's lanes and reviewers; live acceptance runs on its own lanes. (M3: 11 agents, three self-made captains, one froze on a dialog.)

22. **When every lane waits, the captain checks on its own.** A silent stall is found by the captain, not by the user. (M3: two reviews never started for an hour.)

23. **Fix what happened, and build only what the user agreed.** A trial's red line was the gate's own `git push`; the captain built a push allow-list instead, never agreed, which then blocked the fleet's own pushes and was removed (M3).

24. **A removal lands whole.** Deleting a command while its caller still ships breaks the integration branch: T-106 removed `push-check` while the engine still installed the hook that called it, and every push failed until the hook went too (M3).
