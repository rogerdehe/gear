# Handover — <seat> (<lane or reviewer>) — <task>

<!--
Template for a seat whose agent is switched (a new CLI, or a model only
another CLI serves). The engine writes part 1 and commits the document under
the voyage folder as `handovers/<seat>-<date>-<time>.md`; the outgoing agent
writes parts 2 and 3 while it wraps up. When the outgoing agent cannot write,
the incoming agent reads the session named in part 1 and writes part 2 itself
before it works. The incoming agent's packet starts with these facts and
points here; it must not start over or discard work it has not inspected.

A model switch on the same CLI needs no handover: the conversation stays.
Delete these instructions when you fill the template.
-->

## 1. Facts (the engine fills these)

- **Seat, role and task:** seat name, lane or reviewer, task id, attempt, and the contract version the attempt adopted.
- **Why the handover:** model switch on another CLI, agent switch, grace period ended, quota, the user's request.
- **Previous conversation:** CLI, model, session id or transcript path.
- **Incoming agent:** CLI and model.
- **Worktree:** path, branch, last commit (sha and subject), the upstream branch and how many commits are not pushed (with their subjects), `git status` (changed and untracked files), a stash if any.
- **Hand-in state:** handed in or not; if handed in, the candidate and the report path; the open questions to the captain and their answers so far.
- **Checks:** the acceptance command and its last result (passed, failed, not run), with its time.

## 2. Summary (the outgoing agent writes it; else the incoming agent writes it from the session)

- **Where the work stands:** what is done, what is half done, what has not started, in the packet's terms.
- **What was tried and dropped, and why:** so the incoming agent does not repeat a dead end.
- **What it believes about the code:** the cause it found, the places that matter, the trap it fell into.
- **The next step it would take,** concretely (a file, a function, a command).
- **Doubts:** anything it was not sure of, and whether it asked the captain.

## 3. Free notes (optional)

Anything else worth passing on.
