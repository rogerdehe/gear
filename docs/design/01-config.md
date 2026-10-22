# 01 Config and Project Registration

Files: `gear/config.py`, `gear/cli/admin.py` (init). Budget 100–130 lines.

## Project config (written by the user, registered by `gear admin init`, versioned in the database)

```toml
[project]
id = "oxford"                       # a new product, or the short name of one requirement iterated inside an existing repo (17 §0)
herdr_session = "selyra"          # required, explicit binding
repo = "/path/to/repo"

[quota]
lanes = 4
temps = 2
includes_captain = false

[captain]
pane = "w2:pM"
kind = "codex"
profile = "codex"

[[lanes]]                          # long-lived lane: duty required (17 §1); worktree/pane created by GEAR via Herdr (17 §4), no hand-written paths
name = "heat"                      # → Agent name oxford-heat, branches home/heat, heat/<task>
duty = "Thermal calculation and thermal-related interfaces"
owns = ["src/heat/", "tests/heat/"]
not = ["UI"]
kind = "opencode"
profile = "opencode"

[models]                           # agreed with the user at kickoff (17 §0); ordered: primary, then fallbacks
lanes   = ["minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-v4.1-flash"]
review  = ["deepseek/deepseek-v4.1-flash", "minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-v4-pro"]
captain = []                       # empty = whatever the captain's CLI runs; not switched by GEAR
on_quota = { lanes = "fallback", review = "fallback" }   # or "wait": park until reset, then wake on the same model
quota_reset_h = 5                  # provider plan window, used when the error states no reset time
# a lane may override with its own `models = [...]`

[temps]                            # temp workers: worktree + pane created via Herdr when needed, nothing pre-registered
kind = "opencode"
profile = "opencode"

[conventions]                      # optional, defaults in 17 §11
integration = "dev"
release = "main"
staging = ""

[timing]                           # all optional, defaults used
tick_s = 4
cooldown_s = 90
sample_min = 20
review_timer_min = 45

[profiles.opencode]                # may override a built-in profile (04)
busy_markers = ["esc interrupt"]
```

## Validation (at init; any failure rejects the whole config)

- The session and repo exist, and the `integration` branch exists (read-only queries to Herdr / git). The captain pane must be a usable shell pane; if an old Agent is still running in the pane, the user stops it first — init never types commands into a running Agent. On repeated init, already-onboarded seats only have their current binding checked.
- Long-lived lanes: `duty` and `owns` are required; `owns` must not overlap pairwise; `name` conforms to 17 §2 and `<project>-<name>` ≤ 32 characters. A lane's worktree and pane are not written in config; the engine creates them via `worktree.create` at onboarding (10).
- The profile for each lane's kind is verified (04 `verified=true`); otherwise rejected.
- Number of lanes ≤ quota.lanes.
- `[models]`: every listed model is reachable by the role's agent kind (profile `list_models`, e.g. `opencode models`); `review` holds at least one model that differs from each lane's primary. Engine use: start/onboard on the chain's first entry; on a quota stop move to the next and log it; probe the earlier entry again after `timing.quota_retry_min` (default 60); with `on_quota = "wait"` the seat goes PARKED `until=` the reset time (10) instead and is woken then; reviewers take the first entry ≠ the writer's current model.
- Captain and long-lived lanes: init only registers the seats; **they are restarted and onboarded by GEAR** (03); CLIs that support resume may carry a `resume_ref` to keep the original session context.
- cooldown_s > tick_s.
- The same worktree cannot be held by two seats (after the engine creates it, it is written to 02 resources, keyed by the path Herdr returns, unique across projects).
- Display the **effective values** (including the source of defaults).
- Repeated init: apply only changes; do not recreate seats; changing an existing seat's pane/kind counts as a replacement and goes through 03's re-registration (NEEDS_CHECK).

## Not done

No silent discovery: the kickoff (17 §0) inventories installed agents and models only to *propose* a config, which the user confirms; init itself never infers team composition, and missing items are reported as errors for the user to fill in, never guessed from pane names.
