# 04 Herdr Adapter

Files: `gear/herdr/client.py`, `gear/herdr/profiles.py`. Budget 220–280 lines.
Invariants upheld: an observation failure is not treated as "nothing happened"; an unknown outcome is not treated as success.

## Responsibilities

- Wrap all Herdr calls in one place: call the CLI via subprocess through `HERDR_BIN_PATH` (default `herdr`), with timeouts, JSON parsing, and error classification.
- Socket path: calls may instead go over the Herdr socket through the vendored board-herdr client (`third_party/board-herdr`). Methods it already wraps are called directly; methods it lacks (e.g. `worktree.create` / `worktree.open` / `worktree.remove`) are called through its generic `call(method, params)`. Timeouts, error classification, and `uncertain` handling are the same on either path.
- Explicit session binding: engine calls always carry the project's bound session (`--session <name>`) and do not depend on UI focus.
- Maintain per-CLI profiles: screen-reading signatures, resume method, capability declarations.
- Two layers: `HerdrReadOnly` (also usable from the command line, read-only) and `HerdrActor` (engine only, has side effects).

## Not done

- No business judgement; no retry of side-effecting calls (left to 08 ops to handle as "outcome unknown"); no event subscription (MVP).

## External interface

```python
class HerdrResult(NamedTuple):
    ok: bool
    data: dict | None
    error: str | None        # Herdr JSON error code, e.g. agent_blocked / agent_prompt_stalled / timeout
    uncertain: bool          # timeout/process killed: the action may have taken effect

class HerdrReadOnly:
    def __init__(self, session: str, timeout_s: float = 8): ...
    def agent_list(self) -> HerdrResult                     # all Agents in the whole session
    def agent_get(self, target: str) -> HerdrResult
    def agent_read(self, target: str, lines: int = 16) -> HerdrResult   # --source recent-unwrapped
    def pane_get(self, pane_id: str) -> HerdrResult

class HerdrActor(HerdrReadOnly):
    def prompt(self, target: str, text: str) -> HerdrResult          # no --wait; whether it started is confirmed by the next observation round
    def pane_run(self, pane_id: str, command: str) -> HerdrResult   # set GEAR_CTX in the shell before onboarding
    def agent_start(self, name: str, kind: str, pane_id: str, args: list[str]) -> HerdrResult
    def send_keys(self, target: str, *keys: str) -> HerdrResult      # used to make an Agent exit (profile-specified)
    def pane_close(self, pane_id: str) -> HerdrResult
    def pane_title(self, pane_id: str, title: str) -> HerdrResult    # report-metadata --source gear --title
    def notify(self, title: str, body: str) -> HerdrResult
    def worktree_create(self, repo_cwd: str, branch: str, base: str, label: str) -> HerdrResult
        # socket worktree.create; data = {workspace_id, pane_id, path}
        # lane: once at onboarding, branch home/<lane>, base = integration branch (dev), label <project>-<lane>;
        #   task branches <lane>/<task-id>-<slug> are then switched inside this worktree
        # temp worker: once per new task, branch = its task branch (17 §3), base dev, label <project>-<task-id>
    def worktree_open(self, repo_cwd: str, branch: str, label: str) -> HerdrResult
        # socket worktree.open --branch <task-branch>; data = {workspace_id, pane_id, path}
        # temp worker returning for rework/resume to its original worktree
    def worktree_remove(self, workspace_id: str) -> HerdrResult
        # socket worktree.remove; removes the checkout and the Herdr workspace, keeps the branch (verified)
        # temp worker: only after its result is merged
```

The `path` returned by `worktree_create` / `worktree_open` is the key of 02 `resources`; GEAR never builds worktree paths itself and keeps no pre-registered pane/worktree pool (17 §4).

All calls: default timeout 8 seconds (`agent_start` 60 seconds); concurrency limit 4 (a semaphore shared across the whole engine); a slow call in one project does not block other projects (08 rotates through projects).

## CLI profile (`profiles.py` + overridable by project config)

```python
@dataclass
class Profile:
    kind: str                          # opencode / claude / codex / pi … (Herdr kind)
    busy_markers: list[str]            # appearing at the screen tail means still running, e.g. opencode: "esc interrupt"
    ratelimit_markers: list[str]       # e.g. opencode: "Error: Rate limit exceeded"
    quota_markers: list[str]           # quota-exhausted signatures (determined by probes)
    auth_markers: list[str]
    tail_lines: int = 16
    confirm_rounds: int = 2            # only counts after N consecutive rounds match
    session_ref: bool = False          # whether Herdr reports agent_session
    unattended_args: list[str]         # REQUIRED, e.g. opencode: ["--auto"]; passed on every start/resume
    resume_args: list[str] | None = None   # e.g. claude: ["--resume", "{session}"]; None = not supported
    exit_keys: list[str] = ["ctrl+c"]  # key sequence to make the Agent exit (used before closing the session)
    verified: bool = False             # profiles not verified by probes may not take over real projects
```

- **Unattended start is mandatory.** Every agent GEAR starts (captain, lane, temp; fresh or resumed) gets the profile's `unattended_args`. Without them the CLI stops on permission dialogs (Herdr `blocked`) and nothing can run unattended; a profile with empty `unattended_args` can never be `verified`. Model and reasoning effort are not start arguments: they are switched inside the running agent (opencode: `/model`, `/effort`), because a lane may need to change model mid-way (e.g. quota exhausted).
- Session refs: when Herdr does not report `agent_session` for a kind (opencode as of Herdr 0.9.1), the profile names how to find it (opencode: `opencode session list` / `session export <id>` → `info.location.directory` matches the seat's worktree). Verified 2026-10-22: opencode exits on one `ctrl+c` and prints `opencode -s <id>`; `--auto -s <id>` resumes with context intact.
- Built-in profiles are provided only for **probe-verified** CLIs; others must be supplied explicitly in project config and are refused when marked `verified=false`.
- Screen-reading signatures only match within the tail window; nudge templates are validated at load time to contain no marker (prevents "the nudge text triggering its own detection").
- Matching logic (pure function, easy to test):

```python
def classify_screen(profile: Profile, tail: str) -> set[str]   # {"busy","ratelimit","quota","auth"}
```

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
