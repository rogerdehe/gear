# 04 Herdr Adapter

Implementation: [lib.rs](../../crates/gear-herdr/src/lib.rs), [profile.rs](../../crates/gear-herdr/src/profile.rs).
Invariants upheld: an observation failure is not treated as "nothing happened"; an unknown outcome is not treated as success.

## Responsibilities

- Wrap all Herdr calls in one place: call the CLI via subprocess through `HERDR_BIN_PATH` (default `herdr`), with timeouts, JSON parsing, and error classification.
- Socket path: calls may instead go over the Herdr socket through the board-herdr client (MIT, pinned git dependency). Methods it already wraps are called directly; methods it lacks (e.g. `worktree.create` / `worktree.open` / `worktree.remove`) are called through its generic `call(method, params)`. Timeouts, error classification, and `uncertain` handling are the same on either path.
- Explicit session binding: engine calls always carry the project's bound session (`--session <name>`) and do not depend on UI focus.
- Maintain per-CLI profiles: screen-reading signatures, resume method, capability declarations.
- Two layers: `HerdrReadOnly` (also usable from the command line, read-only) and `HerdrActor` (engine only, has side effects).

## Not done

- No business judgement; no retry of side-effecting calls (left to 08 ops to handle as "outcome unknown"); no event subscription (MVP).

## External interface

The `path` returned by `worktree_create` / `worktree_open` is the key of 02 `resources`; GEAR never builds worktree paths itself and keeps no pre-registered pane/worktree pool (17 §4).

All calls: default timeout 8 seconds (`agent_start` 60 seconds); concurrency limit 4 (a semaphore shared across the whole engine); a slow call in one project does not block other projects (08 rotates through projects).

## CLI profiles (overridable by project config)

- **Unattended start is mandatory.** Every agent GEAR starts (captain, lane, temp; fresh or resumed) gets the profile's `unattended_args`. Without them the CLI stops on permission dialogs (Herdr `blocked`) and nothing can run unattended; a profile with empty `unattended_args` can never be `verified`. Model and reasoning effort are not start arguments: they are switched inside the running agent (opencode: `/model`, `/effort`), because a lane may need to change model mid-way (e.g. quota exhausted).
- Session refs: when Herdr does not report `agent_session` for a kind (opencode as of Herdr 0.9.1), the profile names how to find it (opencode: `opencode session list` / `session export <id>` → `info.location.directory` matches the seat's worktree). Verified 2026-10-22: opencode exits on one `ctrl+c` and prints `opencode -s <id>`; `--auto -s <id>` resumes with context intact.
- Built-in profiles are provided only for **probe-verified** CLIs; others must be supplied explicitly in project config and are refused when marked `verified=false`.
- **A prompt typed while the agent works** (07 doorbell, T-130): `pi` and `claude` queue input while a turn runs and read it after the current step; `opencode` and `codex` take it into the running turn. So only an urgent message (or, for the queueing kinds, a captain instruction) is typed into a working agent.
- Screen-reading signatures only match within the tail window; nudge templates are validated at load time to contain no marker (prevents "the nudge text triggering its own detection").
## Agent names (T-134)

Herdr keeps an agent's name separately from the CLI kind, and it clears the name when it re-detects the occupant — a Herdr server restart, a CLI that changes its session. A pane whose agent is unnamed refuses `agent.prompt` (`agent <pane> is not an active named agent`); key input still reaches the pane. In M3.1 the captain and both lanes came back unnamed and lost their first deliveries to it.

- `HerdrRead::agent_names()` reads `agent.list` directly (the board client's typed `AgentInfo` does not carry the name) and returns pane → name, `None` for an unnamed agent (`null` and `""` alike).
- `HerdrAct::agent_rename(pane, name)` is `agent.rename`; a name another pane holds is refused (`agent_name_taken`), never stolen.
- `gear_herdr::ensure_agent_name(herdr, pane, name)` / `ensure_agent_name_with(names, …)` name an agent when it does not carry the name, against a snapshot when one has been read. The engine's tick reads the names once and restores every seat's name (`engine::names`); before every prompt, doorbell or key send it makes sure of that seat's name.
- **Which name** is the seat's own (`names::desired`, T-134 review F1), derived from the store and never from what Herdr reports now: the captain and lanes `<project>-<seat>`, an assigned Temp reviewer its seat's name `<project>-review-<k>` (T-482: the seat is long-lived, the task under review shows in its title), so a warm `review-10` seat's agent keeps `gear-review-10` across the tasks it reviews.

## Result classification

| Herdr returns | Meaning | Upper layer handling |
|---|---|---|
| ok | call succeeded | for prompt: only means written to the terminal (operation SUBMITTED); whether it started is confirmed by linked evidence |
| `agent_blocked` | Agent is in an approval/question screen | do not type; mark the seat as needing human handling |
| `agent_prompt_stalled` / timeout / killed | `uncertain=True` | mark operation UNKNOWN, do not resend, check next round |
| explicit rejection known not executed | definite failure | mark operation FAILED, record reason |
| unknown error, unparseable receipt, connection dropped midway | `uncertain=True` | mark operation UNKNOWN, do not resend |
| cannot connect to Herdr | observation failure | skip all judgements this round, board shows "observation failed" |

## Probes relied on (15)

Per CLI: busy/rate-limit/quota/auth signatures, whether idle is falsely reported while a tool is running, whether `agent_session` is reported, resume arguments and success criterion, exit keys. Environment inheritance of `agent start` after an export in the shell via `pane run`.

## Test points

Fake subprocess: timeout → uncertain; JSON error code mapping; regression samples for classify_screen against markers and nudge text; concurrency limit.

## Captain quota footer (T-37)

`Profile.quota_bar` is separate from conversation-tail quota markers. The verified Claude bar displays **percentage used**: `5h 1%` is healthy and `5h 100%` is exhausted. Only the status region between the bottom input separator and the final permission footer is read, even while the captain is working. The live footer was checked on Claude Code 2.1.309: `Opus 5.5 (1M context) · medium /… (dev)  ·  ctx 63%  ·  5h 4% ↻16:20  ·  7d 73% ↻Oct23`, then `⏵⏵ bypass permissions on (shift+tab to cycle)`.

A project can configure the footer grammar per profile:

```toml
[profiles.claude.quota_bar]
footer_prefix = "⏵⏵ bypass permissions on"
separator_prefix = "─"
windows = ["5h", "7d"]
```

Unknown CLI footer formats stay disabled. Exact HH:MM reset hints use the same local calendar as the status-line renderer and roll over midnight. Missing, malformed or date-only reset hints use `models.quota_reset_h`; multiple exhausted windows wait for the latest deadline. The durable quota episode and wake op keep ordinary captain doorbells quiet, while lanes finish their current tasks and reviews/merges continue. The reset wake is not repeated on an uncertain effect or a delayed view. A previously recorded view, including one from the same second before the wake intent, does not resume dispatch.
