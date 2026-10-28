# M2 lessons

What M2 taught, recorded as it happened. A lesson holds for this voyage; only what the release review distils into the shipped skill (`skills/gear/`) carries over. Numbers are the ones lessons had in the old shared file, so earlier references still find them.

Items 1-17 and 20 were distilled at M2's landing (2026-10-25) from the incident log of M1 and M2 (`docs/lessons.md` L-1.., in git history until 2026-10-25).

1. **Every wait in the wake-up and delivery path needs a time limit and an exit.** Four times the captain or a reviewer went silent because something waited for proof that could never come (a doorbell for an item no view would show, a review packet "in flight" forever, a finished voyage nothing rang for). Ask of every wait: what if the thing it waits for never happens?

2. **Anything that stops work must ring the captain** — never only a log line or an info message. A parked lane, a stuck reviewer, a finished voyage.

3. **A stop is a status, not an outcome.** A quota limit, a server error and a finished turn all look like "idle/done". Read the screen.

4. **Serial traps hide in reasonable safety choices** ("wait until reviewed / merged / decided"). For each wait: who is idle meanwhile, and must they be?

5. **An agent that is busy is never given work** — so a lane must end its turn after a hand-in, and the captain must never prompt a lane with work by hand.

6. **The safety net must exist before the risk.** A quota fallback that is itself a pending task is not a fallback; kickoff refuses a policy the engine cannot carry out.

7. **The captain judges findings against the voyage's bar**; reviewers give the smallest exact fix; one round is the norm. Copying reviewer severities into rework cost M1 36 rounds for 28 tasks; judging them cost M2 3 rounds for 17.

8. **The bar and the done-sentence are written in the plan, chosen by the user at kickoff** — not kept in anyone's memory.

9. **Estimates must be calibrated** from the first tasks; human-pace estimates were 10× the agents' real time and made every pace signal meaningless. Read pace by wall clock too.

10. **The early drift signals were about flow, not time:** parallel changes on the same files, queues running low, a lane busy without a task, a voyage finished but not landed.

11. **Conventions before details.** Where files live and how voyages are separated is decided in the MVP; a reviewer's edge case is not.

12. **Code that drives an agent's UI is accepted only after one live run on a real agent.** Fake-terminal tests passed while the real screen differed (a box edge before the status bar; a pasted `/model` sent as a chat message).

13. **Typed vs pasted input differ.** Slash commands and picker filters are typed; messages are pasted.

14. **Prefer the agent's own API over keystrokes** (OpenCode's session API sets model and effort per session).

15. **Name models by full id**; a display prefix picks the wrong provider or a free tier.

16. **Look before typing into a pane**; whatever is typed into an agent's pane becomes a prompt.

17. **A gate over 300 s is cut.** One test binary per crate (macOS scans each new binary on first run).

20. **A hand merge cleans up like the engine's merge** (task and review branches, local and remote).
