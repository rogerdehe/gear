<!-- gear-plan project=gear base=11 -->
# Decisions
- 2026-10-22 All three lanes hit the MiniMax Token Plan limit (2056) at once; the engine flagged them and stopped nudging, and the user switched them to DeepSeek V4.1 Flash by hand. Quota handling must be the engine's job (never-stop): T-15 goes to the head of runtime's queue after T-5.

# Queue
## T-5 [must] lane=runtime milestone=M1 required
deps: T-1

## NEW [must] lane=runtime milestone=M1 required
goal: Quota policy in the engine (PRD §9, kickoff). When observation shows a lane's quota marker (profile `quota_markers`, e.g. OpenCode "Token Plan" / "(2056)") for `confirm_rounds` ticks, apply `[models].on_quota.lanes`: `fallback` -> restart the lane on the next model of its chain: exit the agent (profile exit_keys), export the new model in the pane (the T-1 env line), start again with the profile's resume args and the session ref so the conversation continues, then deliver "Continue T-<n>: run `gear` first"; when the chain is exhausted, or the policy is `wait` -> park the seat until now + `quota_reset_h` and resume then the same way on the first model. Record the switch in the engine log and as an information line to the captain. Never count quota-error turns as "no progress" (today they exhausted the nudge budget and the lane was flagged).
accept: cargo test --workspace green; engine tests with FakeHerdr: quota marker on 2 ticks -> exit, env line with the next model, agent_start with resume args, one continue delivery; chain exhausted -> parked with until = reset; quota turns do not flag the seat.
deps: T-1

## T-12 [must] lane=runtime milestone=M1 required
deps: -

## T-7 [must] lane=runtime milestone=M1 required
deps: -
