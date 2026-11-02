# Backlog

M6.2: the captain handover must be smooth (user, 2026-11-02): handovers will happen often between projects, and a broken handover takes the whole project down. Simple fixes the captain may make itself.

## Captain handover: trial feedback (Prometheus, 2026-11-02)
First real handover between two CLIs (Codex captain, pane w2:p1M, gen 5 → Claude, pane w2:p1Q, gen 6) with `gear captain handover --pane`; the user asked to collect the trial's feedback for optimization.
- **The new captain is never told (normal-use error, take first).** `--pane` only rebinds the seat; nothing is delivered to the target pane, so a fresh agent sits idle with no idea it is the captain until some later doorbell, and that doorbell does not name the handover document either. The GEAR captain had to prompt it by hand. The rebind should deliver one start packet: you are captain of <project> at generation n, read <handover doc>, then `gear`. captain.md (line ~183) says nothing about it today.
- **`--facts` is too long to read (≈32K tokens).** Most of it is the full `owns` lists of retired lanes; a normal tool read truncated the summary, and the outgoing captain had to redirect to a file and read it in sections. Summarize retired lanes in one line each with a reference; keep part 1 to what the incoming captain acts on.
- **Plan line contradicts itself.** It printed "clean" and "edits not applied" together for a tracked plan file at base=597 while the store had applied 806. That file is stale, not a draft ready to apply; applying it would be wrong. Show the three states apart: clean (file = applied), draft (uncommitted edits on the applied base), stale (file base behind the store: regenerate, do not apply).
- Still to collect: the incoming captain's own view once it has worked a few turns (what it missed in the document, what it had to find out alone).
- **Incoming captain's view (Prometheus, Claude, gen 6, 2026-11-02):**
  - Nothing from GEAR at the rebind (confirms the first item above); the GEAR captain's hand-sent message was its only start.
  - **The rebind leaves the agent name on the old pane.** Its first `gear` showed M-7067 "name prometheus-captain held by pane w2:p1M"; it cleared the name by hand after `herdr agent list`. The rebind should move the captain's agent name to the new pane (or clear it from the old one) itself.
  - Part 1 facts were wrong or misleading, and part 2 had to correct them: "Releases published: none tagged in this checkout" (production runs 66feb01a4: tags read from the wrong checkout); "Previous captain's handover: none … write part 2 yourself" (that very document is the handover); the stale-plan line above.
  - The home page after a landing shows the landed voyage and its retired lanes, and nothing about the next voyage's state (M4 not departed, its backlog on the notes branch); it learned that only from the document.
  - The handover document lives on the notes branch, not in the root checkout, so it needed the commit id from the message to find it. The seat or the home page should name the document's path, branch and commit.
  - What took longest: reading past the part 1 path dump. Checking facts (ls-remote, worktree, herdr list) was quick.
