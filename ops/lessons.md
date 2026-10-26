# Lessons (internal)

What building GEAR with GEAR taught us. Internal notes for whoever develops GEAR — not shipped. Rules that also hold for users live in the skill (`skills/gear/`). The incident-by-incident log this was distilled from is in git history (`docs/lessons.md`, until 2026-10-25).

## Never stop

1. **Every wait in the wake-up and delivery path needs a time limit and an exit.** Four times the captain or a reviewer went silent because something waited for proof that could never come (a doorbell for an item no view would show, a review packet "in flight" forever, a finished voyage nothing rang for). Ask of every wait: what if the thing it waits for never happens?
2. **Anything that stops work must ring the captain** — never only a log line or an info message. A parked lane, a stuck reviewer, a finished voyage.
3. **A stop is a status, not an outcome.** A quota limit, a server error and a finished turn all look like "idle/done". Read the screen.
4. **Serial traps hide in reasonable safety choices** ("wait until reviewed / merged / decided"). For each wait: who is idle meanwhile, and must they be?
5. **An agent that is busy is never given work** — so a lane must end its turn after a hand-in, and the captain must never prompt a lane with work by hand.
6. **The safety net must exist before the risk.** A quota fallback that is itself a pending task is not a fallback; kickoff refuses a policy the engine cannot carry out.

7a. **An interface between two tasks is a contract, not a dependency** (M2.1, 2026-10-25). The captain made T-53 wait for T-52 because T-53 used T-52's API — the API was already written out in T-52's goal, so nothing stopped T-53 from starting. The engine lane stood idle from the first minute, and the captain's own snapshot showed it (`engine · no current task · 1 queued`) and was ignored; the user had to point it out. A `deps:` is for work that cannot start without the other's result; shared names are fixed in both contracts and the later task merges the integration branch in. The same day it happened again inside one lane (T-54 waited for T-52, which sat in review): a follow-up in the same lane builds on the lane's own branch, never on a dep. The contract lets a task start, not hand in: code that calls another lane's new API compiles only after that task lands (M3: T-74 and T-80 waited on T-73, T-78, T-79), so the producing task goes first in its lane and the caller goes after independent work. At every apply, every lane must have a task it can start now.

7b. **A dialog at an agent's start is a stop nobody sees** (M2.1). A Codex reviewer sat on the "Update available" dialog and nothing rang the captain. Turn such dialogs off in the profile's start args, and ring the captain for any seat blocked for minutes.


7c. **Self-hosting runs the build it was started with** (M2.1). The engine ran a binary from before the voyage, so merged fixes were not live and a stopgap was removed too early. Rebuild and restart after the merges you need live — and only when no merge op is in flight (a restart interrupted T-58's merge; it recovered).

7d. **A failed review is settled in the plan's `# Verdicts`, never by a reply** (M3, 2026-10-25). The captain replied "rework F1+F2" to two review fails and resolved them, following the home page's `next: gear reply … --resolve`; nothing was settled, the tasks stayed verifying, and a lane found it while waiting on one. The command GEAR suggests must be the one that does the job.

## Holding the course

7. **The captain judges findings against the voyage's bar**; reviewers give the smallest exact fix; one round is the norm. Copying reviewer severities into rework cost M1 36 rounds for 28 tasks; judging them cost M2 3 rounds for 17.
8. **The bar and the done-sentence are written in the plan, chosen by the user at kickoff** — not kept in anyone's memory.
9. **Estimates must be calibrated** from the first tasks; human-pace estimates were 10× the agents' real time and made every pace signal meaningless. Read pace by wall clock too.
10. **The early drift signals were about flow, not time:** parallel changes on the same files, queues running low, a lane busy without a task, a voyage finished but not landed.
11. **Conventions before details.** Where files live and how voyages are separated is decided in the MVP; a reviewer's edge case is not.

## Driving agents

12. **Code that drives an agent's UI is accepted only after one live run on a real agent.** Fake-terminal tests passed while the real screen differed (a box edge before the status bar; a pasted `/model` sent as a chat message).
13. **Typed vs pasted input differ.** Slash commands and picker filters are typed; messages are pasted.
14. **Prefer the agent's own API over keystrokes** (OpenCode's session API sets model and effort per session).
15. **Name models by full id**; a display prefix picks the wrong provider or a free tier.
16. **Look before typing into a pane**; whatever is typed into an agent's pane becomes a prompt.

## Engineering the project

17. **A gate over 300 s is cut.** One test binary per crate (macOS scans each new binary on first run).
18. **Development never touches the production store, nor acts as a seat.** Tests run with a scratch store and HOME, GEAR_CTX blanked, no real `gh` and git limited to local paths (`.cargo/config.toml`), and push nothing: a lane's GEAR_CTX reached a test hook in M3 and sent the captain 300+ urgent items, and a trial team's test run full of `git push` crossed a company red line.
19. **Two writers push the integration branch** (the engine's merges, the captain's plan commits): the captain holds its pushes while a merge is in flight (M3: two merges lost to the captain's own pushes).
20. **A hand merge cleans up like the engine's merge** (task and review branches, local and remote).
21. **The plan's allowance is the ceiling.** The captain never sets GEAR_ADMIN, never registers a project, never starts an agent outside the plan's lanes and reviewers; live acceptance runs on its own lanes. (M3: 11 agents, three self-made captains, one froze on a dialog.)
22. **When every lane waits, the captain checks on its own.** A silent stall is found by the captain, not by the user. (M3: two reviews never started for an hour.)
23. **Fix what happened, and build only what the user agreed.** A trial's red line was the gate's own `git push`; the captain built a push allow-list instead, never agreed, which then blocked the fleet's own pushes and was removed (M3).
24. **A removal lands whole.** Deleting a command while its caller still ships breaks the integration branch: T-106 removed `push-check` while the engine still installed the hook that called it, and every push failed until the hook went too (M3).
