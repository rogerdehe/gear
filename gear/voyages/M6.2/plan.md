# Goal
Make the captain handover smooth: handovers between captains (another model, another CLI, another person's pane) will happen often across the projects that use GEAR, and a broken handover takes the whole project down with it. M6.2 fixes what the first real handover (Prometheus, 2026-11-02, Codex → Claude) exposed, and gets each fix into the projects' hands. Further non-handover issues the user reports become tasks here for the lanes.

# Milestone M6.2: a smooth captain handover
done: (1) a captain handed over with `gear captain handover --pane` learns it from GEAR alone: the new pane carries the captain's name, the old pane no longer does, and one urgent item names the handover document with its path, branch and commit; (2) `gear captain handover --facts` is short enough to read in one tool read and every fact in it is true: lanes summarized, releases read correctly, no stale instruction about an older handover, and a plan file named clean, draft or stale; (3) each fix is released as v0.6.x and is live for every project on this machine, and each captain is told; (4) before landing, this voyage's lessons are read against the skill GEAR ships and the skill is iterated, nothing added for its own sake
bar: DELIVERABLE: an error in normal use blocks the task; edge cases outside normal use are logged
eta: 4h

# Decisions
- 2026-11-02 03:00 scope: M6.2 is the captain handover (user): the backlog's eight findings from the Prometheus handover (outgoing Codex captain, incoming Claude captain, the GEAR captain's own look); handover fixes are made by the captain itself, not by lanes, because the handover is too critical to pass through a packet (user); other issues the user reports later become lane tasks
- 2026-11-02 03:00 scope: taken now: (a) `--pane` sends nothing to the new captain; (b) the captain's agent name stays on the old pane; (c) the handover document's path, branch and commit are named to the new captain; (d) `--facts` prints every lane's full owns list; (e) facts that are wrong: releases from a checkout without tags, an older handover request read as an instruction, a stale plan shown as a draft; logged, not taken yet: the home page after a landing says nothing about the next voyage (the store keeps no landed state; weigh after (a)-(e) ship)

# Queue
