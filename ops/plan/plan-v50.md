<!-- gear-plan project=gear base=49 -->
# Decisions
- 2026-10-23 The low-queue warning (T-7) fired for board and runtime. Rolling horizon: runtime gets the fresh-session mechanism (M2 backlog; two lanes already ran out of context); T-20 moves from data (deepest queue) to board.

# Queue
## NEW [must] lane=runtime milestone=M2 required
goal: Fresh lane sessions at task boundaries. Purpose: a long-lived lane keeps one session for every task; after many tasks and reworks its context fills (board reached 62% and asked to stop; runtime was auto-compacted and stopped), and its work degrades. At a task boundary — the lane is idle, it has no started/delivered attempt (everything handed in), and its context is above `[timing].fresh_session_pct` (default 50) read from its status bar (OpenCode: `620.2K (62%)`; per kind in the profile, a new `context_pattern`) — the engine exits the agent (profile exit keys), waits until the pane shows a shell (the old agent gone), starts a NEW session in the same pane with the same env (no resume args), and only then dispatches the next task; the delivery must be confirmed (the first prompt right after a start can be lost: deliver after the agent is idle and ready, and resend once if not confirmed). Environment: the engine ticks concurrently and may restart between exit and start (do not start twice; do not type into an agent still present); one seat failing never stops the tick.
accept: cargo test --workspace green; engine tests with FakeHerdr: at the boundary above the threshold -> exit, wait for shell, start without resume, then one delivery; below the threshold or with a started attempt -> nothing; restart between exit and start -> one start; an unconfirmed first delivery is sent once more. Real proof: the exact commands for the captain to watch it happen on a lane.
deps: -

## T-20 [should] lane=board milestone=M2
deps: -
