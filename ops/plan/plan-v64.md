<!-- gear-plan project=gear base=63 -->
# Decisions
- 2026-10-23 MiniMax's Token Plan ran out on all three lanes. The engine's quota fallback (T-15) exits the agent and restarts it with the model in the environment — which no longer applies since --standalone was dropped, so runtime came back on MiniMax, hit the limit again and was parked for 5 h. Changing a model must not close the agent: T-33 (switch in place with `/model`, verified on the status bar) moves into M1, at the head of data, and the quota fallback uses it instead of exit+restart.

# Queue
## T-33 [must] lane=data milestone=M1 required
goal: Change an agent's model IN PLACE, never by closing it. For OpenCode: `/model`, type the model's picker filter, Enter, answer the variant dialog when there is one, then read the status bar until it shows the model (the filter must be specific: "DeepSeek V4.1 Flash OpenCode Go", not the free tier — lesson L-29); retry once, then flag the seat. Used (a) right after an OpenCode lane/reviewer starts when its status bar is not on the chain's model, and (b) by the quota fallback (T-15): on a provider limit the engine waits for the agent to stop (it may keep retrying; a double Esc interrupts a retry loop), switches the model in place, and delivers "Continue T-<n>"; no exit, no restart, the conversation stays. The model's picker filter and status-bar text come from config (`[models]` entries may carry them; the opencode profile knows the steps). Reference: ops/bin/switch-model.sh and `gear --skill agent:opencode`. Environment: never type into a working agent; the engine may restart mid-switch (re-read the status bar, do not retype blindly).
accept: cargo test --workspace green; FakeHerdr tests: wrong model -> the key sequence then a status-bar check; variant dialog answered; already on the model -> nothing sent; a quota fallback switches in place (no exit keys, no agent_start) and delivers one continue; two failed tries -> flagged. Real proof: the exact commands for the captain to watch one in-place switch.
deps: -

## T-31 [must] lane=data milestone=M1 required
deps: -

## T-23 [must] lane=data milestone=M1 required
deps: -
