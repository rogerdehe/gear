# M5.2 backlog

- 2026-10-30: a reviewer whose direction check is closed by the lane's hand-in is not told; review-9 kept checking T-442, its `gear send` was refused ("does not hold the direction job"), and it spent minutes looking for the cause in the main repo (guardrail M-6417). Candidate: when a hand-in closes a running direction check, the engine sends the reviewer one line "the direction check of T-<n> is closed by the hand-in; end your turn".
