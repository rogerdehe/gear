# Backlog

M6.2: the captain handover must be smooth (user, 2026-11-02): handovers will happen often between projects, and a broken handover takes the whole project down. Simple fixes the captain may make itself.

## Captain handover: trial feedback (Prometheus, 2026-11-02)
First real handover between two CLIs (Codex captain, pane w2:p1M, gen 5 → Claude, pane w2:p1Q, gen 6) with `gear captain handover --pane`; the user asked to collect the trial's feedback for optimization.
- **The new captain is never told (normal-use error, take first).** `--pane` only rebinds the seat; nothing is delivered to the target pane, so a fresh agent sits idle with no idea it is the captain until some later doorbell, and that doorbell does not name the handover document either. The GEAR captain had to prompt it by hand. The rebind should deliver one start packet: you are captain of <project> at generation n, read <handover doc>, then `gear`. captain.md (line ~183) says nothing about it today.
- **`--facts` is too long to read (≈32K tokens).** Most of it is the full `owns` lists of retired lanes; a normal tool read truncated the summary, and the outgoing captain had to redirect to a file and read it in sections. Summarize retired lanes in one line each with a reference; keep part 1 to what the incoming captain acts on.
- **Plan line contradicts itself.** It printed "clean" and "edits not applied" together for a tracked plan file at base=597 while the store had applied 806. That file is stale, not a draft ready to apply; applying it would be wrong. Show the three states apart: clean (file = applied), draft (uncommitted edits on the applied base), stale (file base behind the store: regenerate, do not apply).
- Still to collect: the incoming captain's own view once it has worked a few turns (what it missed in the document, what it had to find out alone).
