<!-- gear-plan project=gear base=60 -->
# Milestone M1: Self-hosting loop, delivered as v0.1.0
done: a hand-in is reviewed and merged or sent back without the captain's hands (automatic settlement on, made safe by T-12 and T-23; reviewers on warm seats, T-18), and a new user without Rust installs GEAR from the v0.1.0 GitHub release and runs a captain + lanes + reviewers project with Claude Code, Codex, OpenCode or Grok in any role
bar: delivery: block only what breaks the loop, loses data, or stops a new user from installing and running; log the rest
eta: 31h

# Milestone M2: Keep the captain on course
done: any captain in any project is held to the goal by GEAR itself — milestone done/bar/eta, the captain's gate, drift and land-or-extend reviews, the voyage report; plus quota handling and the reliable model switch
bar: product: block wrong behaviour in normal operation and data loss; log polish
eta: 12h

# Decisions
- 2026-10-23 Voyages need done/bar/eta (T-22). M1 had no planned duration at departure; re-baselined honestly: about 28h elapsed + about 3h left after the cut = 31h. Bug found: adding a task to an old milestone with no voyage row fails (NOT NULL voyages.done) instead of being grandfathered — logged in m2-backlog.

# Queue
## NEW [must] lane=data milestone=M2 required
goal: The engine sets an OpenCode agent's model through `/model`, verified on the status bar — the reliable way (env + --standalone was dropped: private servers exited on their own, L-56). After an OpenCode lane or reviewer starts (and after a quota fallback), if its status bar does not show the wanted model: send `/model`, type the model's picker filter, Enter, answer the variant dialog when the model has variants (e.g. MiniMax-M3 -> thinking, DeepSeek V4 Pro -> high), Esc out of anything unexpected, then read the status bar until it shows the model; retry once, then flag the seat (captain stall item). The profile carries the per-kind steps (opencode: picker; codex/grok/claude: start flags, nothing to do). Reference: the manual script ops/bin/switch-model.sh and `gear --skill agent:opencode`. Purpose: lanes run on the model of the project's chain, not on whatever the shared OpenCode service last used. Environment: never type into a working agent; the engine ticks concurrently and may restart mid-switch (re-check the status bar, do not retype blindly).
accept: cargo test --workspace green; FakeHerdr tests: wrong model -> the key sequence, then the status bar check; variant dialog answered; already on the model -> nothing sent; working agent -> nothing sent; two failed tries -> flagged. Real proof: the exact commands for the captain to watch one switch on a lane.
deps: -
