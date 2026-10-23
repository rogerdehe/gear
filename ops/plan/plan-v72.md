<!-- gear-plan project=gear base=71 -->
# Decisions
- 2026-10-23 T-26 review: only R1 breaks the chain (a new user cannot start the captain). R2-R4 are labels and wording: cut under the FASTEST bar, logged in the M2 backlog.
- 2026-10-23 The runtime lane stood idle while T-32 waited behind T-26 on board. T-32 is engine/profile code that needs T-31's profiles, not the README: it moves to runtime and no longer waits for T-26.

# Queue
## T-32 [must] lane=runtime milestone=M1 required
deps: T-31

# Verdicts
- T-31 attempt=2 contract=1 candidate=718fdd4f0844de6a9ec1955297f54be13be5278d pass evidence=ops/reviews/T-31.md
- T-26 attempt=1 contract=2 candidate=be4ddd8b9810436a87d724c6457a5910601a884f fail rework="Captain's fix, one round — do exactly this, nothing else from the review. README.md, between init and the plan step: add a short 'Start the captain' step (8-12 lines with one labelled command block): in a Herdr shell pane in the project directory, with gear on PATH, start the chosen captain CLI with GEAR_CTX=<project.id>:captain:1 in its environment (and GEAR_DB if customized); use that pane and kind in gear.toml; run init from another shell without GEAR_CTX; then tell the captain to read `gear --skill` and write and apply the first plan. Verify the plan-apply step on a scratch store; label starting a real agent 'not verifiable here' if you did not run it. Do not touch R2-R4. Hand in when done."
