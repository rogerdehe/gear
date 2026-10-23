<!-- gear-plan project=gear base=52 -->
# Decisions
- 2026-10-23 USER: the M1 delivery must carry everything a new user needs to run GEAR, not just the binary. Found: review packets point at `ops/REVIEW.md` (only in this repo); no captain guide (plan draft format, kickoff, daily loop); no `agent:claude` SOP although the captain is usually Claude Code; skills can only be printed. T-30 (M1) makes GEAR self-contained; the README (T-26) follows it.

# Queue
## NEW [must] lane=board milestone=M1 required
goal: GEAR carries the knowledge it needs; nothing depends on files of the gear repo. (1) `gear --skill review`: the review rules from ops/REVIEW.md made project-neutral (mechanical acceptance, findings and severities, verdict judged against the task's milestone done-sentence and bar, the template, delivering on review/t<n>); the engine's review packet (crates/gear-engine/src/review.rs) points at `gear --skill review` instead of `ops/REVIEW.md`. (2) `gear --skill captain`: the captain's guide — kickoff (`gear admin kickoff`, agreeing done/bar/eta, lanes, models, quota policy with the user, no popups after kickoff), the plan draft format (milestone, queue blocks and fields, verdicts, review conclusions — design/05 made concise, with one complete example), the daily loop (`gear`, `gear reply`, `gear plan apply`, keeping 2–3 valuable steps per lane, holding the course). (3) `gear --skill agent:claude`: the Claude Code SOP (start flags `--dangerously-skip-permissions`, resume `--resume <id>`, session id from Herdr, exit, the status line). (4) `gear admin install-skill --agent claude|codex|opencode`: writes the skill (SKILL.md plus the references as files) where that CLI loads skills (verify each location on this machine; print where it wrote). Keep each text short: only what the role must do and the real special cases.
accept: cargo test --workspace green; `gear --skill` lists the new topics; a test asserts the review packet names `gear --skill review` and no `ops/` path; `install-skill` tested into a temp HOME for each agent. Real proof: in a scratch git repo with no gear files, follow `gear --skill captain` far enough to write and apply a plan on a scratch store; report anything that needed a file from this repo.
deps: -

## T-26 [must] lane=board milestone=M1 required
deps: T-30
