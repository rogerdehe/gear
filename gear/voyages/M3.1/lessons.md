# M3.1 lessons

What M3.1 taught, recorded as it happened. A lesson holds for this voyage; only what the release review distils into the shipped skill (`skills/gear/`) carries over. Numbers are the ones lessons had in the old shared file, so earlier references still find them.

25. **Consent is gathered once, at kickoff.** Every config re-apply a voyage will need (a lane switching CLI, a new reviewer) is named at kickoff, and the user's yes covers it; the captain never asks for it again mid-voyage. (M3.1: the captain asked again for the deck → pi re-apply the user had already agreed.)

26. **A merged engine fix runs only after a rebuild.** The engine keeps running the binary it started with; after a merge that changes GEAR's own behavior, rebuild and restart it between merges. (M3.1: T-118 had landed, but the old engine still sent a rework packet without the captain's rework text, and a lane did the rework the captain had cut.)

27. **Never flood the machine, never kill a program while it starts.** On macOS each new binary's first run waits for a system check; four builds and test runs at once (nextest starts one process per test) queued thousands, and every process killed while waiting (a timeout probe, a `gear` that spawned `/bin/ps` per ancestor) left a dead request the checker kept retrying until the whole machine stalled (M3.1, 2026-10-26). One build and test run at a time per machine; no external program on every `gear` call; diagnose without timeout-killing fresh programs.
