<!-- gear-plan project=gear base=56 -->
# Milestone M1: Self-hosting loop, delivered as v0.1.0
Done when: (1) a task a lane hands in is reviewed and then merged into dev or sent back without the captain's hands — automatic settlement ON by default, made safe by the engine merge (T-12) and the captain's gate (T-23: blocking findings tied to the bar go back, the rest are logged, a round cap goes to the captain); reviewers on warm seats (T-18); (2) a new user without Rust installs GEAR from a GitHub release and runs a captain + lanes + reviewers project with Claude Code, Codex, OpenCode or Grok in any role (T-26, T-27, T-30, T-31, T-32); (3) dev is merged into main and tagged v0.1.0.

# Decisions
- 2026-10-23 Automatic settlement is switched off (engine `auto_settle`, L-57) only until T-12 and T-23 are merged and verified; then it is on by default. Principle (PRD §10): mechanical and reversible steps go to the engine (pass -> merge behind a hard gate; objective failures -> rework); judgement that changes scope stays with the captain (only blocking findings tied to the bar become rework; a round cap escalates). T-23 moves into M1: it is the precondition of the M1 sentence.

# Queue
## T-31 [must] lane=data milestone=M1 required
deps: -

## T-23 [must] lane=data milestone=M1 required
deps: -
