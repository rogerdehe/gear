# T-173 attempt 2

F1: the reserved OpenCode binding now receives the resume session before launch; resumed starts skip fresh session creation, and the model gate operates on that existing session. The API-success regression checks the single existing-session argument, absence of `session.create`, the targeted `session.switchModel`, and no second resume.

F2: each new round session records its owning CLI in round/session metadata. Counting groups sessions by their recorded CLI, uses each CLI's usage reader, combines tokens and complete costs, and preserves unknown results for missing logs. Existing rounds without metadata retain their original CLI as the fallback. A Pi-to-Codex restart regression uses both readable logs and checks combined tokens/cost and missing-log behavior.

Validation: `cargo test -p gear-engine --lib` passed 239 existing tests. Full `scripts/test.sh` was run for the revised candidate; the hand-in carries its result.
