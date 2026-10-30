# Handover templates (M6 design input)

Two templates: one for a lane or reviewer whose agent is replaced, one for the captain. Each has three parts, as a colleague's handover would:

1. **Facts the handover must carry.** Things that can be read or checked, which the incoming agent must not have to guess. When the outgoing agent cannot write (its quota is spent, it was stopped after the grace period, it is gone), the engine fills these from the store and git, so they are never missing.
2. **What the outgoing agent summarises from its own conversation.** Judgement only it has: what it tried, why, what it believes. Written in its own words. When it cannot write, the incoming agent reads the previous conversation (session id or transcript path below) and writes this part itself before it works.
3. **Free notes.** Anything else the outgoing agent wants to pass on. Optional.

The incoming agent reads the whole handover before the task packet, checks every fact against the worktree or the store, and says in its first message what it found that differs. It never starts over on work it has not inspected, and never discards uncommitted changes without saying so.

Both templates are written as Markdown, under the voyage folder: `handovers/<seat>-<date>-<time>.md`, committed by whoever writes them.

---

## A. Lane or reviewer handover

### 1. Facts (must; the engine fills them when the agent cannot)

- **Seat, role and task:** seat name, lane or reviewer, task id, attempt, the contract version the attempt adopted, and for a reviewer the job kind (direction check or full review) and the candidate it judges.
- **Why the handover:** model switch on another CLI, agent switch, grace period ended, quota, the user's request.
- **Previous conversation:** CLI, model, session id, transcript path.
- **Worktree:** path, branch, last commit (sha and subject), the last commit pushed to the remote, commits not pushed, `git status` (changed and untracked files), a stash if any.
- **Hand-in state:** handed in or not; if handed in, the candidate and the report path; the open questions to the captain and their answers so far (message ids with their text, not ids alone).
- **Checks:** the acceptance command and its last result (passed, failed, not run), with its time.

### 2. Summary (the outgoing agent writes it; else the incoming agent writes it from the transcript)

- **Where the work stands:** what is done, what is half done, what has not started, in the packet's terms.
- **What was tried and dropped, and why:** so the incoming agent does not repeat a dead end.
- **What it believes about the code:** the cause it found, the places that matter, the trap it fell into.
- **The next step it would take,** concretely (a file, a function, a command).
- **Doubts:** anything it was not sure of, and whether it asked the captain.

### 3. Free notes (optional)

---

## B. Captain handover

### 1. Facts (must; GEAR prints them with `gear captain handover --facts` so the outgoing captain only checks them)

- **Project and voyage:** project id, voyage id and title, goal, done clauses, bar, eta, elapsed, stations and which are reached, releases published.
- **Staffing:** lanes, reviewers, their CLIs and models, role charters, quotas and any temporary raise with its end condition.
- **Board state:** tasks done, in progress, waiting, in review, waiting for the captain's verdict; reworks queued; merges failed or queued.
- **Open items:** every inbox item by id with its text and what it needs.
- **Plan:** the applied plan version, and whether the plan file has edits not applied or not committed.
- **Shared machine:** other projects' captains on the machine and how to reach them; the engine's binary and when it last restarted.

### 2. Summary (the outgoing captain writes it; else the incoming captain writes it from the previous captain's conversation)

- **The user's standing orders this voyage,** in the captain's words (never quoted), with the reason behind each when known.
- **Decisions in flight and why:** what was decided and not yet visible in the plan, and what was rejected.
- **What is fragile right now:** a lane that loops, a reviewer that needs watching, a recovery that has to be repeated, a known engine bug and its workaround.
- **Promises made:** to the user, to other captains (a rebuild, a release, an answer).
- **The next three things to do,** in order.
- **Lessons this voyage** not yet in lessons.md.

### 3. Free notes (optional)

---

## How each handover starts

- **Lane or reviewer, agent switch at a safe point:** the engine asks the outgoing agent to write the handover, then exits it.
- **Lane or reviewer, switch at once:** the engine tells the agent to wrap up, commit, push and write the handover; after the grace period (10 minutes by default) it exits the agent and writes part 1 itself where it is missing.
- **Captain, on the user's request:** the outgoing captain writes it from `gear captain handover --facts`, then starts the new captain.
- **Captain, `gear captain take` from a pane the user started:** the engine asks the previous captain for its handover; when no answer comes in a bounded time, the new captain gets part 1 from GEAR and the previous session to read for part 2.
- **Model switch on the same CLI:** no handover; the conversation stays.
