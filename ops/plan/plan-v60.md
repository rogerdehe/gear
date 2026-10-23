<!-- gear-plan project=gear base=59 -->
# Decisions
- 2026-10-23 USER: the remaining M1 estimate (6h) is too much. Cut: T-23 keeps only blocking/logged findings + the round cap (stale captain items re-alert -> M2); T-32 shrinks because T-31 (in progress) makes profiles configurable — what is left (a built-in grok profile from the SOP, exit by `/exit`, pre-trusting the worktree, `--model` for claude) moves to board after T-26; runtime finishes T-18 (as is) and T-12. One review round per task; only a P0 sends work back. Target: about 3 hours.

# Queue
## T-23 [must] lane=data milestone=M1 required
goal: The captain's gate in the automatic flow — core only. (1) The engine-built review packet includes the task's milestone `done:` and `bar:` (T-22) and the rule "every finding is blocking (name the part of the done-sentence or bar it breaks) or logged; fail only for a failed mechanical fact, a P0, or a blocking finding". (2) `gear send --kind review` takes `--blocking <ids>`; settlement sends back only the blocking findings in the rework note, the rest become one information item for the captain; a fail with no blocking finding and no failed fact settles as pass. (3) Rework rounds capped by `[conventions].review_max_rounds` (default 3): past the cap, one captain action item and the task stays Verifying. Environment: concurrent ticks; a restart between storing a review and settling must not double-settle.
accept: cargo test --workspace green; tests for (1), (2) only-logged -> pass + info item, blocking -> rework lists only it, (3) the cap, and the restart case.
deps: -

## T-32 [must] lane=board milestone=M1 required
goal: Grok and Claude Code as lanes/reviewers on top of T-31's configurable profiles: a built-in `grok` profile from skills/gear/references/agents/grok.md (unattended `--always-approve`, busy `Ctrl+c:cancel`, resume `-r {session}`, session_ref true, exit by the `/exit` command — add `exit_command` to the profile and use it wherever an agent is exited), the engine pre-trusts a grok worktree in `~/.grok/trusted_folders.toml` before starting it (append only), model at start: grok `-m <model>`, claude `--model <model>`. Keep it small.
accept: cargo test --workspace green; FakeHerdr tests: a grok lane start (args), its exit sends `/exit`, the trust entry written once into a temp HOME; a claude lane start carries `--model`.
deps: T-31, T-26
