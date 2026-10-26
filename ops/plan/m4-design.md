# M4 design

Status: draft for alignment with the user (2026-10-26). Every M4 task says "implement §x.y of ops/plan/m4-design.md exactly". Lanes do not design: when this document and the code disagree, or a case is not covered here, the lane stops and asks the captain (`gear send --kind question`), it does not invent a rule.

Words used below:

- **seat** — a pane with one agent and one name; a lane seat (writes code on its branch) or a reviewer seat (reviews a candidate).
- **session** — the agent CLI's own conversation. Herdr reports its id as `agent_session` (`AgentInfo.agent_session`); GEAR stores it as `session_ref`. For pi it is the session file path, for Codex the thread id.
- **round** — one piece of work by one seat: a dev round (attempt 1 of a task), a rework round (attempt n ≥ 2) or a review round (the review of one attempt).
- **Herdr state** — Herdr's agent status: `working`, `idle`, `done`, `blocked`, `unknown`. "Ready" below means `idle` or `done`.

What the code does today (facts the design relies on, from the M4 exploration):

| Fact | Where |
|---|---|
| A lane's session id is on its binding (`bindings.session_ref`), refreshed every tick from Herdr | observe.rs:212-235 |
| A reviewer's session id is in meta `engine.review_session.{project}.{task}`, set once per reviewer start | review.rs:666-675, 745-751 |
| A re-review restarts the reviewer with `resume_args` | review.rs:1122-1144 |
| A reviewer pane is closed after its review; its worktree is kept | release.rs:107-200 |
| A reviewer's checkout is typed into the pane's shell before the agent starts | review.rs:768-799, 1058 |
| A temp seat is bound to one task for life (`seats.task_id`); messages to it show only when `message.task == seat.task_id` | gear-app/src/seats.rs `message_belongs` |
| `attempts.delivered_at` is the time the delivery was intended, not sent | tasks.rs:96 |
| A delivery is confirmed by the lane's check-in view within 3 checks of 4 s | ops.rs:658-685, lib.rs:93 |
| `followup_of` is stored for NEW tasks only and read by nothing | plan.rs:332 |
| `# Decisions` lines are kept only as text in the file and the plan row | draft.rs:219-233 |
| `gear send --kind blocked` is refused ("not available yet") although the help lists it | main.rs:1367-1380 |
| `busy_markers` (screen substrings) decide "busy" | gear-herdr/src/profile.rs:75 |
| No over-estimate check exists; no idle doorbell for the captain exists | engine, doorbell.rs:137 |
| `gear report voyage` does not exist | main.rs:167-180 |

---

## 1. Store: schema v6

One migration, v5 → v6 (`crates/gear-store/src/migrate.rs`, `SCHEMA_VERSION = 6`), by the first M4 task. All later tasks write against these names.

```sql
-- §3: which session each attempt ran in, and what the engine did before the packet
ALTER TABLE attempts ADD COLUMN session_ref TEXT;      -- Herdr session at check-in, updated at hand-in
ALTER TABLE attempts ADD COLUMN session_cli TEXT;      -- the agent kind that ran it (pi, codex, ...)
ALTER TABLE attempts ADD COLUMN session_action TEXT;   -- keep | new | compact | resume | fresh | new-unconfirmed | compact-fallback | resume-fallback
ALTER TABLE attempts ADD COLUMN sent_at INTEGER;       -- when the packet was actually sent through Herdr

-- §1.1: a task keeps its repo, base and gate (B02-01)
ALTER TABLE tasks ADD COLUMN repo TEXT;
ALTER TABLE tasks ADD COLUMN base TEXT;                -- the integration branch
ALTER TABLE tasks ADD COLUMN gate TEXT;

-- §4: rounds and their tokens
CREATE TABLE IF NOT EXISTS rounds (
  project TEXT NOT NULL, task TEXT NOT NULL, attempt INTEGER NOT NULL,
  role TEXT NOT NULL,                 -- dev | rework | review
  seat TEXT NOT NULL, generation INTEGER NOT NULL,
  cli TEXT NOT NULL, model TEXT,
  start_at INTEGER NOT NULL,          -- the engine's first input of this round (§4.1)
  end_at INTEGER,                     -- the next round's start on this seat, or the seat's release
  sessions TEXT NOT NULL DEFAULT '[]',-- JSON list of every session_ref seen on the seat in the window
  active_s INTEGER NOT NULL DEFAULT 0,-- §5.1: time on the clock (pauses excluded)
  paused_s INTEGER NOT NULL DEFAULT 0,
  input INTEGER, output INTEGER, cache_read INTEGER, cache_write INTEGER, reasoning INTEGER,
  cost_usd REAL, cost_source TEXT,    -- agent | prices | none
  counted_at INTEGER,                 -- when the tokens were last read
  PRIMARY KEY (project, task, attempt, role));

-- §6: friction reports
CREATE TABLE IF NOT EXISTS frictions (
  project TEXT NOT NULL, id TEXT NOT NULL,              -- F-<n>, next_id(project,'F')
  task TEXT NOT NULL, attempt INTEGER NOT NULL, role TEXT NOT NULL,
  seat TEXT NOT NULL, body TEXT NOT NULL,
  required INTEGER NOT NULL,          -- 1: the round passed its budget (§6.2)
  created INTEGER NOT NULL,
  closed_at INTEGER, outcome TEXT,    -- §6.4: what the captain did about it
  PRIMARY KEY (project, id));

-- §7: the decision log
CREATE TABLE IF NOT EXISTS decisions (
  project TEXT NOT NULL, key TEXT NOT NULL,             -- sha256 of the normalized line (§7.3)
  seq INTEGER NOT NULL,               -- order of first appearance
  voyage TEXT NOT NULL,
  at TEXT NOT NULL,                   -- the date written on the line, or the apply time
  kind TEXT NOT NULL,                 -- §7.2
  what TEXT NOT NULL, why TEXT, alt TEXT, changes TEXT,
  star INTEGER NOT NULL,              -- 1: worth a look at landing (§7.2)
  source TEXT NOT NULL,               -- plan | gear
  line TEXT,                          -- the plan line, verbatim (source = plan)
  plan_version INTEGER, created INTEGER NOT NULL,
  PRIMARY KEY (project, key));

-- §5.3: a declared wait
-- tasks.wait_on and tasks.wait_cond exist already and are reused:
--   wait_on = 'T-<m>', wait_cond = 'merged'
```

The migration fills `tasks.repo/base/gate` of every existing task from its lane's current config, and leaves the new attempt columns NULL (old attempts have no session).

### 1.1 A task keeps its repo, base and gate

At task creation (`plan.rs` insert), `repo/base/gate` are copied from the lane's config. Review, merge, the packet and the board read them from the task, never from the lane's current config. A lane rebound to another repo (§10.6) leaves its old tasks where they were.

---

## 2. Agent profiles: new fields

`crates/gear-herdr/src/profile.rs`, struct `Profile` and its `[profiles.<kind>]` patch. Values for the built-in kinds; each value marked *live* is confirmed in one live run on the real agent by the profile task (lesson 12) and corrected if the run says otherwise.

| Field | Type | pi | codex | claude | opencode |
|---|---|---|---|---|---|
| `new_session` | slash command, typed | `/new` | `/new` | `/clear` | `/new` |
| `compact` | slash command, typed; empty = none | `/compact` | `/compact` | `/compact` | `/compact` |
| `resume_args` | exists today | `--session {session}` | `resume {session}` | `--resume {session}` | `-s {session}` |
| `usage_log` | which reader of §4.3 | `pi-jsonl` | `codex-rollout` | `claude-jsonl` | `opencode-db` |
| `context_window_k` | per model, optional, in `[[models]]` entries | from the model config | | | |

Slash commands are **typed** (`send_text` then `enter`), never pasted with `agent_prompt` (lesson 13: a pasted `/new` is a chat message).

There is no in-pane resume: every CLI's `/resume` without an argument opens a picker, which the engine never drives. A resume is always: exit the agent (`exit_keys`), start it again in the same pane with `resume_args` (§3.4).

`busy_markers` no longer decides "busy" (§10.2). It stays in the profile for nothing; remove the field's use, keep parsing it so old configs load.

---

## 3. Seats and sessions

### 3.1 One seat model

A lane seat and a reviewer seat are the same thing: a pane, an agent, a name. The differences are only:

| | lane seat | reviewer seat |
|---|---|---|
| work | its own queue of tasks | review rounds, any task |
| worktree | `home/<lane>`, writes `<lane>/t<n>` | `home/review-<k>`, checks out the candidate |
| when idle | stays (keeps its pane, worktree, build cache) | released by the rule in §3.6 |

A reviewer seat is no longer closed after its review. It goes to `Standby` (idle, agent alive) and can take the next review.

### 3.2 When the session is chosen

Before the engine sends a round's packet, it picks one **session action** by the table in §3.3 and records it in `attempts.session_action` (lane rounds) or the round row (review rounds). The captain never chooses it. The order is fixed:

1. The seat's agent is Herdr-ready (`idle`/`done`), and no op is pending on the seat. Otherwise the dispatch waits for the next tick, as today.
2. The round row is created, `start_at = now` (§4.1).
3. The session action runs (§3.4).
4. The packet is sent (`agent_prompt` by agent name, §10.1); `attempts.sent_at = now`.
5. The check-in (the agent's `gear --show T-n` view) confirms the delivery (§10.3). At check-in, `attempts.session_ref` and `session_cli` are set from the binding's current `session_ref` and kind. At hand-in (`gear send --kind result`) `session_ref` is set again (the agent may have compacted or been restarted inside the round).

### 3.3 The rule table

"Current" = the seat's `bindings.session_ref` now. "Fresh" = the agent was started by this dispatch (its session has no earlier work). Rules are checked top to bottom; the first match wins.

**Lane round (task T, attempt n).** "Fresh" applies only after the rule is chosen: a `new` on a just-started agent is `fresh`; a `resume` is never replaced by `fresh`.

| # | Case | Action |
|---|---|---|
| L1 | — | (rules L2-L7 first decide what the round needs; when the seat has no live agent, the dispatch starts it: with `resume_args` when the rule says `resume`, else plainly and the action becomes `fresh`) |
| L2 | n ≥ 2 (rework) and attempt n−1 ran on this seat with the same CLI, and its `session_ref` = current | `keep` |
| L3 | n ≥ 2 and attempt n−1 ran on this seat with the same CLI, `session_ref` ≠ current | `resume` (attempt n−1's session) |
| L4 | n ≥ 2, any other case (another seat, another CLI, no session recorded) | `new` |
| L5 | n = 1, T has `followup-of: F`, F's last accepted attempt ran on this seat with the same CLI, its `session_ref` = current | `keep`, then `compact` if the context is large (§3.5) |
| L6 | n = 1, followup of F as in L5 but `session_ref` ≠ current | `resume` (F's session), then `compact` if large |
| L7 | n = 1, any other case | `new` |

A rework (L2-L4) starts only after the lane's current round has handed in: that is today's rule (a busy agent is never given work) and it stays.

When L4 or L7 applies to a follow-up or rework whose session cannot be used, the packet already carries what the lane needs: the rework text and the review reports (today), and for a follow-up a line `This task follows T-F; read its report: gear --show T-F`.

**Review round (task T, attempt n):**

| # | Case | Seat | Action |
|---|---|---|---|
| V1 | an earlier review round of T exists on seat X with the same CLI, and X is `Standby` or `Released` | X | `keep` if X's current session = that round's session, else `resume` it (a Released X is started with `resume_args`) |
| V2 | any other case | any `Standby` reviewer seat, else a `Released` one, else a new seat (temps quota) | `new` (`fresh` if just started) |

A review never keeps or resumes another task's session, so its judgment is independent of other work.

**Other moments:**

| # | Case | Action |
|---|---|---|
| S1 | the agent died or was restarted while its round is open (engine restart, crash) | `resume` the round's session, same round (no new round row) |
| S2 | the lane's kind changes in place (§10.5) to another CLI | the next round starts `fresh` on the new CLI |
| S3 | same CLI, another model (pi `--model`) | `resume` the open round's session with the new model args |
| S4 | a quota park ends (§9) | `keep`: the engine sends `Continue T-n: your quota is back.` to the same session |

`followup-of` becomes live: plan apply sets it on existing tasks too (today only NEW), and the field is checked to name an existing task.

**Who decides "related".** Only the captain, once, when it writes the task: the parsed field `followup-of: T-F` in the task's block. The engine never reads the goal text, never matches keywords, never guesses. No field = unrelated = `new` (the safe default: at worst the lane re-reads code; it never carries unrelated context). Rework and re-review need no field: the attempt number says it. Everything after the field (which seat and session ran F, whether it is current, how large the context is) is a fact the engine reads from the store.

The captain's guide says when to write it, in one line: the task changes the code the earlier task just wrote and needs that task's reasoning, not only its result. A task that only uses an earlier task's result (calls its API) is not a follow-up. `followup-of` is not `deps`: `deps` orders tasks, `followup-of` carries a session.

### 3.4 How each action runs

All waits are bounded; every timeout has the fallback written here.

- **keep**: nothing; go to the packet.
- **fresh**: nothing; go to the packet.
- **new**: type `new_session` + enter. Wait until Herdr reports the agent ready again **and** the binding's `session_ref` changed, at most 30 s. If the id did not change in 30 s, record `session_action = 'new-unconfirmed'`, continue to the packet, and add one info item for the captain (`session:<seat>`): "`/new` did not change the session id of <seat>; check the profile". (Live check in the profile task: does Herdr report a new `agent_session` after `/new`, and does the agent keep its Herdr name? If Herdr keeps the old id, the profile task adds a fallback: the newest session file for the pane's cwd.)
- **compact**: note the time T0, type `compact` + enter. It has finished when Herdr reports the agent ready **and** either Herdr reported `done` after T0 or the session log (§4.3) has a compaction or assistant record after T0 (a compaction can start and end between two 4 s ticks, so `working` may never be seen). Wait at most 5 min. On timeout: go on with the packet in the same session (keep the context, never lose it) and record `session_action = 'compact-fallback'`.
- **resume S**: send `exit_keys`; wait until the pane shows no agent (Herdr `agent_get` → none), at most 30 s; start the agent in the same pane and cwd with its normal start args plus `resume_args` (`{session}` = S) — the same path as onboarding, `naming.rs` gives it its name; wait for ready, at most 60 s. On any failure: start it without `resume_args` and record `session_action = 'resume-fallback'`.

### 3.5 "The context is large"

After the previous round, the engine reads the last assistant record of the current session (§4.3). Context = its input + cache_read + cache_write tokens. Large = context ≥ 50% of the model's `context_window_k × 1000`. If either number is unknown, it is large (compact is the safe choice).

### 3.6 Reviewer seats: keep, release, quota

- After its verdict, a reviewer seat goes to `Standby` (agent alive, pane open). Its job binding ends: `seats.task_id = NULL`.
- Each tick, with W = number of lane seats whose attempt is `Started` (working on a task):
  - while the Standby reviewers are more than W, the one idle longest is released (pane closed, binding void, seat `Released`, worktree kept);
  - when W = 0, every Standby reviewer is released at once (no lane works, so no hand-in can come; a hand-in that does come starts a reviewer again in seconds).
- The captain may release one at once: `# Staff` line `- release <reviewer>` (the verb exists for lanes; it now accepts a reviewer in Standby). The captain's guide says: when you are idle and judge that no hand-in comes within 10 min, release the idle reviewers.
- The temps quota counts seats with a live agent or starting: `Active`, `Standby`, `Starting`. `Released`, `Unknown` with no agent, and retired seats never count.
- A seat in `Starting` whose job is no longer under review (the attempt was failed, a newer attempt exists, the task is not Verifying) is cancelled: its start op is voided, its pane closed if any, the seat `Released` (B03-05).

### 3.7 A reviewer's worktree before its round

The engine prepares the reviewer's worktree itself, with `git -C <path>`; nothing is typed into a pane (today's checkout line in the shell goes away):

1. `git -C <path> status --porcelain`. If anything is listed: `git -C <path> stash push --include-untracked -m "gear: before review of T-<n> attempt <a>"`, and add one info item for the captain naming the stash (evidence is kept, never cleaned).
2. If `review/t<n>` is checked out in another worktree (`git worktree list --porcelain`):
   - its holder is a reviewer seat with a live agent on a job still under review → do not take it; this round waits (next tick);
   - otherwise → in the holder: step 1, then `git -C <holder> checkout --detach`.
3. `git -C <path> fetch origin`, then `git -C <path> checkout -f -B review/t<n> <candidate>`.
4. In the same transaction as the review delivery op: `seats.task_id = T`, so messages to this seat about T show on its home page (the B03 "invisible reply" bug).

### 3.8 Messages to a reviewer seat carry the task

- `gear reply M-n` copies the original message's `task` and `attempt` onto the reply.
- `gear send --kind instruction --to <reviewer>` without `--task` takes the seat's current `task_id`; with no current job it is refused: "<reviewer> has no review job; nothing would show".

---

## 4. Rounds and tokens

### 4.1 One ownership rule

A round owns every agent record on its seat from its `start_at` until its `end_at`. `end_at` = the `start_at` of the next round on the same seat, or the seat's release, whichever comes first. There is no other rule and no "between rounds" bucket: what a seat does after a hand-in (answering a doorbell, say) belongs to the round it handed in.

`start_at` is set before the session action (§3.2 step 2), so a `/compact` or `/new` and its cost belong to the round it prepares.

### 4.2 Worked examples

1. **A rework resumes its session.** T-7 attempt 1 on hull, session A, 10:00-10:40; hull then works T-8 (session B) 10:41-11:10; the review fails T-7 at 11:05; at 11:11 attempt 2 starts with `resume A`. Rounds: T-7/1 owns 10:00-10:41 on hull, T-8/1 10:41-11:11, T-7/2 from 11:11. Session A's records before 11:11 are never counted twice: the window decides, not the session.
2. **Two tasks back to back in one session.** T-9 then T-10 (follow-up, `keep`). T-9 owns its records until T-10's `start_at`; T-10's first request re-reads T-9's context, and that cost is T-10's, which is what it really cost.
3. **Compact.** T-10's round starts at 12:00, `/compact` runs 12:00-12:01, packet at 12:01: the compact's summary request is T-10's.
4. **Re-review.** Review of T-7/1 on review-1, session R, 10:41-10:55; review-1 then reviews T-8/1 with `new` (session Q) 11:11-11:20; T-7/2's review on review-1 `resume R` from 11:30. Each review round owns its own window.
5. **A model switch inside a round** (S3): the same window; records carry their own model, and §4.4 prices each record by its own model.

### 4.3 Reading the records (per `usage_log`)

The tick that closes a round (and any read of an open round: board, report) sums the records of every session in `rounds.sessions` with a timestamp in the window. `rounds.sessions` gets every `session_ref` the seat's binding had during the window (observe.rs adds a new value to the open round's list when it changes).

| `usage_log` | Where | One record | Fields |
|---|---|---|---|
| `pi-jsonl` | the file named by the session ref | a line with `type = "message"`, `message.role = "assistant"`, `message.usage` | `input`, `output`, `cacheRead`, `cacheWrite`, `reasoning`, `cost.total`; time `timestamp` |
| `codex-rollout` | `~/.codex/sessions/YYYY/MM/DD/rollout-*-<thread>.jsonl` (search back 14 days) | a line `type = "event_msg"`, `payload.type = "token_count"`, `payload.info` not null | delta of `info.total_token_usage` against the previous such line in the same file: `input_tokens`, `cached_input_tokens`, `cache_write_input_tokens`, `output_tokens`, `reasoning_output_tokens`; time `timestamp` |
| `claude-jsonl` | `~/.claude/projects/*/<session>.jsonl` | a line `type = "assistant"`, `message.usage` | `input_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens`, `output_tokens`; **counted once per `message.id`** (one message is written on several lines) |
| `opencode-db` | `~/.local/share/opencode/opencode.db`, table `session_message` | a row of the session whose JSON `data` has `tokens` | `tokens.input/output/reasoning/cache.read/cache.write`, `cost`; time `time.completed` |

Codex's input count includes cached input: input = `input_tokens − cached_input_tokens` so the columns never double count. A file that cannot be read leaves the token columns NULL (unknown), never 0.

### 4.4 Cost

- A record that carries its cost (pi) → that cost, `cost_source = agent`.
- Else the `[prices."<provider/model>"]` table in the project config: `input`, `output`, `cache_read`, `cache_write`, USD per 1M tokens → `cost_source = prices`.
- Else `cost_usd = NULL`, `cost_source = none`. Tokens are still counted.

Subscription plans (MiniMax, Codex) get the API-equivalent price: the number says what the work would cost at list price, which is what a light procedure for small tasks needs (M5).

### 4.5 Where it shows

`gear report time` gets token and cost columns per task and per round; the board's view 5 (`ops/plan/m4-board/`); the voyage report totals (§7.5).

---

## 5. The estimate clock, the overrun question, and declared waits

### 5.1 The clock

Each round has a budget:

| Round | Budget |
|---|---|
| dev (attempt 1) | the task's `estimate_min` |
| rework (attempt n ≥ 2) | the `estimate=<min>` written on the fail verdict that asked for it (`# Verdicts` fail lines take an optional `estimate=`); without one, max(10, ⌈estimate_min / 2⌉) |
| review | measured: 1.5 × the median active time of the last 10 finished review rounds on the same reviewer model, at least 10 min; with fewer than 3 such rounds, 30 min |

`active_s` = (now, or the round's hand-in time once handed in; for a review, its verdict time) − the round's check-in time (`started_at`; for a review, its delivery confirmation) − `paused_s`. The clock pauses (adds to `paused_s`) while the seat is quota-parked (§9) or the task is in a declared wait (§5.3).

### 5.2 The overrun question

Only while the attempt is `Started` or `Stopped` (or the review round has no verdict): when `active_s` reaches the budget, the engine sends the seat an at-once message (it interrupts a working agent, as urgent messages do today) with exactly this text:

```
GEAR: T-<n> has reached its estimate (<budget> min; <active> min so far).
Answer with one command, then keep working:
gear send --kind progress --task T-<n> --attempt <a> --contract <c> --left <minutes> --blocker "<what stops you, or none>" "<done so far; what is left>"
```

The engine fills `<a>` and `<c>` with the open attempt's numbers, so the lane copies the line as it is. `gear send --kind progress` gets two flags: `--left <minutes>` (integer) and `--blocker <text>`. The reply becomes one captain item on thread `overrun:T-<n>` (one per task, updated, never duplicated): urgent when `--blocker` is not `none`, else normal. The item's text shows budget, active time, the answer, and the lane's `--left`.

- The next question comes at `active_s ≥ budget + max(10, left)` minutes, with the same text.
- No answer within 10 min → the item becomes urgent with "no answer"; the next question comes 15 min later.
- A progress reply counts as progress for the stall logic: an asked and answering lane is never "stuck".
- The captain helps through the item (`gear reply M-n -m …`, which reaches the lane at once): split the task, make the contract clearer, give a permission, or raise the estimate (a plan edit of `estimate:`; a raised estimate moves the next question).

### 5.3 A declared wait (B03-09)

A lane whose task cannot finish until another task lands says so:

```
gear send --kind blocked --task T-<n> --attempt <a> --contract <c> --on T-<m> "<what is done; what waits for T-m>"
```

- Accepted only when T-m exists and is not Done. Sets `tasks.wait_on = 'T-<m>'`, `wait_cond = 'merged'`; the attempt stays `Started`; the lane ends its turn.
- While waiting: no nudges, no stall item, the clock paused, the lane shown as "waiting for T-m" (board, `gear queue`). One info item for the captain.
- When T-m is merged: `wait_on` is cleared and the lane gets at once `GEAR: T-<m> is merged (<commit>). Continue T-<n>.` in the same session (`keep`).
- The captain can end a wait from the plan by editing the task (any contract change clears `wait_on`).
- `blocked` is removed from nowhere: the help keeps listing it, now it works.

---

## 6. Friction reports

### 6.1 What a lane or reviewer reports

At hand-in, an optional flag on `gear send --kind result` and `gear send --kind review`:

```
--friction "<what cost you time that the captain could fix>"
```

What belongs there (the packet lists these words): a permission it lacked and worked around, a test or build that took far too long, an unclear contract, a missing tool or fixture, a step that cost much time. What does not: the work itself, or "none".

### 6.2 When it is required

When the round's `active_s` is over its budget (§5.1), `--friction` is required. Without it the hand-in is refused with:

```
ERROR FRICTION_REQUIRED: T-<n> took <active> min against <budget> min. Add --friction "<what made it slow>" and send again.
```

The packet says so at delivery: `Budget: <budget> min. Past it, your hand-in must carry --friction "…"`.

### 6.3 What GEAR adds

Each report is stored with `required`, and the board and `gear friction` show it next to the round's own facts: budget, active time, paused time, rounds so far, tokens (§4).

### 6.4 The captain

- **When**: the captain is idle = it has no actionable item (`actionable_for` empty). When it is idle and unread (`closed_at IS NULL`) reports exist, the engine rings once with one item on thread `friction` listing them ("3 friction reports: F-4 F-5 F-7. Run gear friction."). It rings again only when a new report arrives and the captain is idle again; never more than once per 30 min.
- The `friction` item closes itself when every report it lists is closed; a new report while it is open updates its list.
- **Reading**: `gear friction` lists open reports with their facts; `gear friction --all` includes closed ones.
- **Closing**: `gear friction close F-<n> -m "<outcome>"`, outcome one of: fixed (what), lesson (which), not worth it (why). The landing review (§7.5) lists all reports with their outcomes; open ones are marked.

---

## 7. The captain's decision log

### 7.1 The line

In `# Decisions`, a decision is one line:

```
- <YYYY-MM-DD HH:MM> <kind>: <what> | why: <reason> | else: <the alternative not taken> | changes: <scope/time/quality/risk>
```

`why:` is required; `else:` and `changes:` are optional. `- tail: …` and `- station M-n: …` keep their own forms and are also logged (kind `tail`, `station`). The user's corrections at landing are written `- <date> user: <text>` (kind `user`).

### 7.2 Kinds and stars

| kind | for |
|---|---|
| `scope` | a task added, moved between lanes, a contract rewritten |
| `cut` | a task cancelled or moved to the next voyage |
| `split` | a task split |
| `override` | a pass against a failed review |
| `logged` | a finding logged instead of reworked |
| `rework` | a fail with rework |
| `estimate` | an estimate changed |
| `staff` | park, hold, release, retire, rebind |
| `model`, `config` | a model or config change |
| `hand-fix` | anything changed by hand outside GEAR |
| `note` | anything else |
| `tail`, `station`, `user` | see §7.1 |

**The star is the captain's, and it is rare.** Everything is logged; the user reads only the starred lines at landing. A decision gets `★` (written right after the dash: `- ★ 2026-10-27 10:05 cut: …`) only when it changed the course, that is one of:

1. what the voyage delivers: a part of the done sentence or a promised function is dropped, changed or added;
2. the quality the user will meet: a known defect in a delivered function is accepted (an override or a logged finding on it);
3. the user's own resources or contract: a model, a cost, a config or a permission the user did not set at kickoff;
4. the time: the eta moves by more than 20%.

Nothing else is starred: rework rounds, estimates, staff moves, cuts of nice-to-have tasks, fixes by hand that changed no delivered behavior. GEAR's own entries (§7.4) are never starred; the captain stars the decision line it writes about them when it meets the rule. The landing review shows the starred lines, then one line with the count of the others by kind.

A line that does not match §7.1 (and is not tail/station) is kept as kind `note` without a star, and apply prints `warning: decision line <k> is not structured; logged as a note`.

### 7.3 Apply is idempotent

Every apply re-reads every line. A line's key = sha256 of the line with whitespace collapsed. A key already in `decisions` is skipped; a new key is inserted with the next `seq`. The table is append-only: editing an applied line makes a new decision (the old one stays); deleting a line removes nothing. Re-applying an unchanged file changes nothing.

### 7.4 Decisions GEAR records itself (`source = gear`)

| Event | kind | what | why |
|---|---|---|---|
| `# Verdicts` pass where the review failed | `override` | `T-n attempt a passed over a failed review` | the verdict line's `why="…"`, **required** for this case (apply refuses without it: `BAD_FORMAT: a pass over a failed review needs why="…"`) |
| `# Verdicts` fail | `rework` | the rework text | — |
| a task `state: cancelled` | `cut` | `T-n cancelled` | the matching plan decision line's why if one names T-n, else `(no why written)` |
| a `# Staff` line applied | `staff` | the line | — |
| `gear admin init` re-applies a config | `config` | `config v<k>` | — |

### 7.5 Landing

`gear report voyage [--json]` (new, captain only, read-only) prints, in order:

1. The voyage report lines (today's `VoyageReport::lines`).
2. The milestone's `done:` text, clause by clause (the captain marks met / not met in its own landing decision lines).
3. Time and tokens: totals, per lane, and the five tasks with the highest actual/estimate.
4. The ★ decisions in full; the others as one count per kind (`--all` prints them).
5. Friction reports with outcomes; open ones marked.
6. Rework: tasks with more than one round.

The landing item (station.rs:65-77) says instead:

```
Voyage <M> is ready to land: all <done> tasks are done (elapsed <e> / eta <eta>).
1. Run `gear report voyage` and write the voyage report into # Decisions.
2. Review it with the user first: the report, the ★ decisions (only those), the friction reports that led to a change. Write each correction as `- <date> user: <text>`.
3. Only then ask about reclaiming the lanes and the release.
Close this with `station <id>: land`.
```

The captain's guide (`skills/gear/references/captain.md`, `course.md`) changes by subtraction: the decision line format replaces the free-form advice; the landing paragraph points to the three steps.

---

## 8. The board and `gear queue`

### 8.1 The board

Build `gear board` to match the prototype `ops/plan/m4-board/prototype.py` (run it: `python3 ops/plan/m4-board/prototype.py`) and its README. The prototype is the contract for layout, views, keys and wording; the data comes from the store. Nothing on the board repeats Herdr (no pane lines, no messaging).

### 8.2 Why a lane is idle

One function in gear-app, used by the board, `gear queue` and §8.4: `idle_reason(lane) -> IdleReason`, first match:

| Reason | When | Shown |
|---|---|---|
| `Quota { until }` | the seat is quota-parked | `quota until 15:12` |
| `Parked { until }`, `Held { reason }` | seat state | `parked until …` / `held: …` |
| `Waiting { on }` | a declared wait (§5.3) | `waiting for T-m (lane, stage)` |
| `DepWait { task, on }` | the lane's next queued task has a dep not Done | `next T-x waits for T-y (lane, stage)` |
| `Undelivered { task }` | the next task is ready and not delivered | `next T-x is ready but not delivered` |
| `NoWork` | nothing left in its queue | `nothing left planned` |

### 8.3 `gear queue [--json]`

Captain's plain view, read from the store, never from the plan file:

```
hull  working T-143 38/60m  · left 3 (125m): T-148 35m, T-153 30m needs T-143, T-160 60m
deck  working T-144 22/45m  · left 4 (175m): …
keel  idle: next T-150 waits for T-142 (hull, review) · left 1 (40m)
mast  idle: nothing left planned · left 0
review-1 reviewing T-142 · review-2 standby 3m
```

`gear plan show T-n` prints one task's block as it would stand in the plan file, so the captain edits a block without reading the whole file.

### 8.4 Imbalance item

When a lane is idle with `NoWork` while another lane has two or more tasks not started (ready or dep-waiting) and more than 60 min of estimates left, one captain item on thread `balance` (updated, not repeated; closed when the state ends): `mast has nothing left; deck has 7 tasks (315m) left: move some (lane ownership allowing) or plan more for mast.`

---

## 9. Quota from the providers

### 9.1 Sources

The screen is not the source of quota facts. Each provider's quota is read from its API, configured per provider:

```toml
[[quota.sources]]
provider = "minimax-cn"            # the provider part of model ids
reader = "minimax"                 # minimax | opencode-go | codex
key_file = "~/.pi/agent/auth.json" # a JSON file holding the API key
key_path = "minimax-cn.key"        # dotted path inside it (pi's auth.json holds {"<provider>": {"type", "key"}})
```

| reader | Request | Facts |
|---|---|---|
| `minimax` | `GET https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains`, Bearer key | per entry of `model_remains`: `current_interval_remaining_percent`, `end_time` (ms) for the 5 h window; `current_weekly_remaining_percent`, `weekly_end_time` |
| `opencode-go` | `GET https://opencode.ai/zen/go/v1/usage`, Bearer key | `usage.rolling/weekly/monthly`: `percent` (used), `resetsAt` |
| `codex` | the newest `token_count` line of the reviewer's rollout (§4.3) | `rate_limits.primary/secondary`: `used_percent`, `window_minutes`, `resets_at` (s) |

The engine reads each source every 5 min and after any quota signal, and stores the last reading (meta `quota.reading.<provider>`, JSON with the time read). A reading older than 15 min is shown as stale.

### 9.2 A quota stop

A quota marker on the screen (the profile's `quota_markers`) or a provider error is only a **signal** to read the source at once.

- The source says a window is spent (0% left) → the seat is parked until that window's reset time + 1 min, the round's clock paused; at the reset the engine sends `Continue T-n: your quota is back.` in the same session (§3.3 S4). With M4's config (lanes on MiniMax only, no fallback), this is the whole policy; the 5 h wait is real and is the live test of this mechanism.
- The source says quota is left → not a quota stop: a short rate limit or an error. The seat waits 5 min, then the same `Continue` message; a second time in a row becomes a captain item ("<seat>: provider error, not quota").
- The source cannot be read → today's behavior: park `quota_reset_h` hours.

The board's quota block and `gear queue` show the last readings.

---

## 10. Reliability fixes

### 10.1 Never type into the wrong thing (B03-03)

- `HerdrAct::prompt` targets the live **agent name**, never the pane id: Herdr then refuses when no agent of that name runs (`agent_not_ready`), instead of a race typing into a shell.
- `pane_run` (shell commands, today only before an agent start) runs only after `agent_get(pane)` reports no agent. With §3.7 the reviewer checkout no longer types into a pane at all.
- When Herdr refuses a prompt by name, the engine calls `agent_get(pane)`: an agent of the seat's kind is there → it lost its name (M3.1); `naming.rs` renames it and the prompt is retried next tick, nothing is restarted. No agent in the pane → the op fails at once as `agent-gone` (not "submitted") and the seat goes to the restart path (§3.3 S1).

### 10.2 Busy is Herdr's state (pi `Working` in prose)

Whether an agent is busy is Herdr's state (`working`) only. Only `idle` and `done` are ready: `working`, `blocked` and `unknown` are never ready — the engine never delivers, types a command or sends a session action to an agent in those states (`unknown` is common for pi while it works). Screen text never makes an agent busy. `profile.rs` `classify` keeps quota, blocked-dialog and error markers; the `busy` class is removed from it, and every caller that treated "busy marker" as busy reads Herdr's state instead.

### 10.3 Delivery confirmation (late views, results from Delivered)

- The confirmation window is wall clock: `timing.delivery_grace_s`, default 180, instead of 3 checks of 4 s.
- A check-in view of the attempt by its seat that arrives after the Deliver op failed confirms it anyway (Delivered → Started) and closes the stall item.
- A hand-in for an attempt still `Delivered`, sent by the attempt's own seat and generation, is accepted: Delivered → Started → Submitted in one transaction.
- The engine never creates a Deliver op for an attempt that is already Started.

### 10.4 A lane with no task answers and stops

A doorbell to a lane with no open attempt says: `GEAR: <n> item(s) for you: <ids>. Run gear, answer them, then end your turn. You have no task now: do not continue earlier work.`

### 10.5 A lane's kind changes in place

A config re-apply that changes a lane's kind or model list takes effect at the lane's next safe point (no open round, or an open round restarted with §3.3 S2/S3): exit the old agent, start the new kind in the same pane and worktree, name kept, `target/` untouched. No retire, no new worktree.

### 10.6 Rebind a lane (B02-01)

`# Staff` line `- rebind <lane> repo=<name> owns=[<path>, …]`. Apply records it; while the lane has open attempts (not terminal) the engine waits and apply prints `rebind <lane>: waits for T-a, T-b`. Then: the lane's config repo and owns change, its worktree becomes the new repo's `home/<lane>` (created by the usual onboarding path), its next round is `fresh`. Old tasks keep their repo (§1.1). The captain's guide: a lane whose repo has no ready work is rebound at once.

### 10.7 The other fixes, one line each

| Id | Fix |
|---|---|
| A1 | The engine talks to each project's own Herdr session (its registered socket, as `gear captain take`) and reconnects after that server restarts. |
| A4 (T-121) | `gear admin stop` waits while a merge op is intended, executing or unknown (up to 10 min, naming the task); `--now` stops at once. |
| A5 (T-89) | Starting a busy agent never blocks a tick: the retry moves to the next tick. |
| A6 (T-69 R2) | covered by §3.7 step 2. |
| A7 | A push race: re-merge onto the new tip; when the merged tree equals the gated tree, push without running the gate again. |
| A8 (T-134 F2/F3) | A captain name collision creates the captain item; teardown names an unnamed leftover before prompting it. |
| A9 | T-124 (the rereview test independent of load), T-127 F4 (bounded FIFO handshakes), F5 (no 10 ms sleep), T-128 F1 (board PTY test reads by poll). |
| B2 (T-70) | One open item per (thread, fact); it closes itself when its cause is gone. |
| B3 (T-66) | The run-dry warning is checked every tick, not only at apply. |
| B4 | Station projection per lane: each lane's open estimates × its own pace; the slowest lane sets the finish and is named. |
| B5 (T-120) | Used `# Staff` lines are removed after apply; a failed merge's `next:` names `- remerge T-n`; apply prints the lanes a tail silences. |
| B6 | `- tail: <lanes> until=T-n` / `until=<RFC3339>` ends by itself. |
| B7 | `gear send --kind instruction --ack` requires the recipient's `gear send --kind ack --reply-to M-n`; until then the captain's item stays open, reminded after 10 min. |
| B9 | `gear whoami`: the real pane, the seat bound to it, the store, and why a claim fails. |
| B03-07 | `gear reply` help states that the default resolves; `gear reply M-n --reopen` reopens an item resolved by mistake. |
| B03-08 | A rework hand-in whose candidate does not contain attempt n−1's candidate is refused, naming that commit; `gear --show T-n` prints the worktree's branch and HEAD next to the task's branch. |
| B03 config | The home page warns when the project's config file differs from the applied config version. |
| B03-01 | A pass verdict may say `integrated-by=T-m`: the task is marked merged by T-m's merge (refused unless T-m is merged and the candidate's tree is in the integration branch). |
| B03-04/10 | After a hand-in the reply says: `Handed in. End your turn: the next task comes by itself; do not push to <lane>/t<n> again.` |

---

## 11. GitHub PR-mode landing

Today: `merge = "pr"` is refused by config (config.rs:1040-1049); the engine's merge (merge.rs `merge_one_locked`) fetches, merges the candidate and `review/t<n>` into `home/merge`, runs the gate, pushes the integration branch and deletes the task and review branches; the forge only opens a draft claim PR per task (key `{task}:claim`, head `<lane>/t<n>`), comments, and closes the issue. `Forge::merge` exists (`gh pr merge <n> --squash|--merge`, readies a draft first) but nothing calls it.

### 11.1 Config

`[github] merge = "pr"` is accepted (remove the refusal and flip its two tests: gear-app github_contract.rs:88-109, gear-cli admin_init_contract.rs:129-154). `merge_method = "squash" | "merge"` (exists, default squash).

### 11.2 The landing, step by step

For a passed task in a repo with `merge = "pr"`, the Merge op runs:

1. Steps 1-8 of today's `merge_one_locked` unchanged: fetch, reset to `origin/<integration>`, merge the candidate and `origin/review/t<n>` with `--no-ff`, conflict checks, the gate. A failure here fails the op exactly as today.
2. Instead of pushing the integration branch: put the review commits on the PR head. In a scratch checkout of the candidate: `git merge --no-ff origin/review/t<n>` (when the review branch has commits not in the candidate), then `git push origin HEAD:refs/heads/<lane>/t<n>` (a fast-forward of the PR head). Record `head = <that sha>`.
3. The Merge op goes to `Submitted` with result `{"mode":"pr","pr":<number>,"head":"<sha>"}` and the engine inserts a forge op key `{task}:{n}:land`, payload `{"forge":"land","pr":<n>,"head":"<sha>","method":"squash|merge"}`. Nothing is deleted.
4. Sync runs the land op: `gh pr merge <n> --repo <r> --squash|--merge --match-head-commit <head>` (a new argument of `Forge::merge`), so GitHub merges exactly the gated head. Success → the forge op is Confirmed with `{"merge_commit":"<sha>"}` read by `gh pr view <n> --json state,mergeCommit`.
5. The next engine tick sees the Merge op `Submitted` and its land op Confirmed: Merge op Confirmed (`merged {task} into {integration} by PR #<n>: <merge commit>`), `merged_at`, `close_branches` as today (the task branch and `review/t<n>`), waits and failed items resolved.

### 11.3 Failures

| Case | Result |
|---|---|
| no Confirmed claim op for the task (no PR) | Merge op Failed: `no pull request for T-n: the claim PR was never opened` → captain item, as a merge failure today |
| `gh pr merge` refused (not mergeable, required checks, branch protection) | land op Failed with gh's message; Merge op Failed with it → captain item naming `- remerge T-n` |
| the land call timed out or the engine restarted (op Unknown) | sync reads `gh pr view <n> --json state,mergeCommit,headRefOid` (new `Forge::pr_state`): `MERGED` → Confirmed with its commit; `OPEN` with the same head → run the merge again; anything else → Failed with the state |

`Forge::pr_state` also reconciles an interrupted claim (T-91 F4): an Unknown claim op looks for an open PR with head `<lane>/t<n>` (`gh pr list --head <branch> --json number,url`) and adopts it instead of staying quarantined.

### 11.4 What PR mode does not promise

The local gate ran on the integration tip of its own moment. If the base moves before GitHub merges, GitHub's own checks and branch protection are the gate for the combination; GEAR does not re-gate. The github reference says so in one line.

---

## 12. Live checks the captain settles before dispatch

The captain runs these itself before the tasks that depend on them are delivered (no lane is asked to find them out), and writes each answer into this section and the profile table of §2.

1. pi, Codex, Claude: after typing `new_session`, does Herdr's `agent_session` change, and is the agent's Herdr name kept?
2. Same after `compact`: does Herdr report `working` then ready, and does the session id stay?
3. pi `--session <file>` and Codex `resume <id>` started in the same pane: does the agent come back with its history and is it recognized by Herdr?
4. The MiniMax and OpenCode Go usage endpoints with the configured keys (one call each).

---

## 13. Interfaces between lanes

Tasks of different lanes meet only at these names. A task writes against them before the other side has landed (lesson 7a), and merges dev in when it lands. A change to a name here is a captain decision, never a lane's.

### 13.1 Store (`crates/gear-store`, deck)

```rust
// TaskRow gains: pub repo: Option<String>, pub base: Option<String>, pub gate: Option<String>
// AttemptRow gains: pub session_ref: Option<String>, pub session_cli: Option<String>,
//                   pub session_action: Option<String>, pub sent_at: Option<Ts>
impl Tx<'_> {
    pub fn set_attempt_session(&self, project: &str, task: &str, n: i64, session_ref: &str, cli: &str) -> GearResult<()>;
    pub fn set_attempt_action(&self, project: &str, task: &str, n: i64, action: &str) -> GearResult<()>;
    pub fn set_attempt_sent(&self, project: &str, task: &str, n: i64, at: Ts) -> GearResult<()>;

    pub fn insert_round(&self, r: &NewRound) -> GearResult<()>;                 // fails if the key exists
    pub fn open_round_of_seat(&self, seat: &str) -> GearResult<Option<RoundRow>>; // end_at IS NULL
    pub fn close_round(&self, key: &RoundKey, end_at: Ts) -> GearResult<()>;
    pub fn add_round_session(&self, key: &RoundKey, session_ref: &str) -> GearResult<()>; // no duplicates
    pub fn add_round_pause(&self, key: &RoundKey, secs: i64) -> GearResult<()>;
    pub fn set_round_active(&self, key: &RoundKey, active_s: i64) -> GearResult<()>;
    pub fn set_round_tokens(&self, key: &RoundKey, t: &RoundTokens, at: Ts) -> GearResult<()>;
    pub fn rounds_of_task(&self, project: &str, task: &str) -> GearResult<Vec<RoundRow>>;
    pub fn rounds_of_project(&self, project: &str) -> GearResult<Vec<RoundRow>>;

    pub fn insert_friction(&self, f: &NewFriction) -> GearResult<String>;       // returns "F-<n>"
    pub fn open_frictions(&self, project: &str) -> GearResult<Vec<FrictionRow>>;
    pub fn frictions_of_project(&self, project: &str) -> GearResult<Vec<FrictionRow>>;
    pub fn close_friction(&self, project: &str, id: &str, outcome: &str, at: Ts) -> GearResult<()>;

    pub fn insert_decision_if_new(&self, d: &NewDecision) -> GearResult<bool>; // false: key exists
    pub fn decisions_of(&self, project: &str, voyage: &str) -> GearResult<Vec<DecisionRow>>;
}
pub struct RoundKey { pub project: String, pub task: String, pub attempt: i64, pub role: String }
pub struct NewRound { pub key: RoundKey, pub seat: String, pub generation: i64, pub cli: String, pub model: Option<String>, pub start_at: Ts }
pub struct RoundTokens { pub input: Option<i64>, pub output: Option<i64>, pub cache_read: Option<i64>,
    pub cache_write: Option<i64>, pub reasoning: Option<i64>, pub cost_usd: Option<f64>, pub cost_source: String }
// RoundRow, FrictionRow, DecisionRow: one field per column of §1, same names.
// NewFriction / NewDecision: the columns of §1 without id/seq/created.
```

### 13.2 Usage and quota readers (new crate `crates/gear-usage`, deck)

```rust
pub enum UsageLog { PiJsonl, CodexRollout, ClaudeJsonl, OpencodeDb }       // profile `usage_log`
pub struct Usage { pub input: i64, pub output: i64, pub cache_read: i64, pub cache_write: i64,
                   pub reasoning: i64, pub cost_usd: Option<f64>, pub records: i64 }
/// Sum of the records of one session with from <= time < to (§4.3). None: the log cannot be read.
pub fn usage(log: UsageLog, session_ref: &str, from: Ts, to: Ts) -> Option<Usage>;
/// input + cache_read + cache_write of the last assistant record (§3.5). None: unknown.
pub fn last_context(log: UsageLog, session_ref: &str) -> Option<i64>;

pub enum QuotaReader { Minimax, OpencodeGo, Codex }
pub struct Window { pub name: String, pub used_percent: f64, pub resets_at: Option<Ts> }
pub struct Reading { pub provider: String, pub windows: Vec<Window>, pub read_at: Ts }
/// One call (§9.1). HTTP through `curl -sS --max-time 15` (no HTTP crate in the workspace).
pub fn read_quota(reader: QuotaReader, key: &str, codex_session: Option<&str>, now: Ts) -> Result<Reading, String>;
```

### 13.3 Profiles (`crates/gear-herdr/src/profile.rs`, deck)

`Profile` gains `pub new_session: String`, `pub compact: String` (empty = none), `pub usage_log: Option<String>` (the names of §2, kebab case); `classify` returns no busy class (§10.2).

### 13.4 Engine: sessions (`crates/gear-engine/src/session.rs`, hull)

```rust
pub enum Prep { Ready, Wait }       // Ready: send the packet now; Wait: try again next tick
/// §3.2 steps 2-3 for a lane round: creates the round row on the first call, chooses the
/// action (§3.3), runs it one step per tick (§3.4), records attempts.session_action.
pub fn prepare_lane(store: &mut Store, herdr: &mut dyn HerdrAct, cfg: &EngineCfg, seat: &SeatRow,
                    task: &str, attempt: i64, now: Ts) -> GearResult<Prep>;
/// The same for a review round; the reviewer worktree (§3.7) is prepared inside.
pub fn prepare_review(store: &mut Store, herdr: &mut dyn HerdrAct, cfg: &EngineCfg, seat: &SeatRow,
                      task: &str, attempt: i64, now: Ts) -> GearResult<Prep>;
```

Keel's dispatch (lanes) and hull's review start call these before the packet; on `Ready` they send it and call `store.set_attempt_sent`.

### 13.5 Engine: rounds and waits (keel)

`crates/gear-engine/src/rounds.rs`: `pub fn tick(store, herdr, cfg, now)` adds session refs to open rounds, closes rounds (§4.1), counts tokens of closed rounds (§4.3-4.4). `crates/gear-engine/src/overrun.rs`: `pub fn tick(...)` (§5.1-5.2). `crates/gear-engine/src/waits.rs`: `pub fn tick(...)` (§5.3). All three are called from `lib.rs::tick`.

### 13.6 Idle reasons (`crates/gear-board/src/idle.rs`, deck)

```rust
pub enum IdleReason { Quota { until: Ts }, Parked { until: Option<Ts> }, Held { reason: String },
    Waiting { on: String }, DepWait { task: String, on: String }, Undelivered { task: String }, NoWork }
pub fn idle_reason(tx: &Tx, project: &str, lane: &str, now: Ts) -> GearResult<Option<IdleReason>>; // None: working
pub fn describe(r: &IdleReason, tx: &Tx, project: &str) -> String; // the "Shown" texts of §8.2
```
gear-board is below gear-app, so the board, `gear queue` (mast) and the imbalance item (keel) all use this one function.
