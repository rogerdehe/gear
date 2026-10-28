-- GEAR schema v1 (design/02). Applied by Store::open when meta.schema_version is absent.
-- All times are unix epoch seconds. All ids are TEXT.

CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY, herdr_session TEXT NOT NULL, config_toml TEXT NOT NULL,
  config_version INTEGER NOT NULL, quota_lanes INTEGER NOT NULL, quota_temps INTEGER NOT NULL,
  quota_includes_captain INTEGER NOT NULL, plan_version INTEGER NOT NULL DEFAULT 0,
  goal TEXT, created INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS id_seq (project TEXT NOT NULL, kind TEXT NOT NULL, next INTEGER NOT NULL,
  PRIMARY KEY (project, kind));

CREATE TABLE IF NOT EXISTS milestones (project TEXT NOT NULL, id TEXT NOT NULL, title TEXT NOT NULL,
  scope TEXT, state TEXT NOT NULL, PRIMARY KEY (project, id));

-- The findings of a review that block (T-23): the ids the reviewer named
-- with `--blocking`. The findings themselves stay in the review record; this
-- says which of them go back to the lane as rework.
CREATE TABLE IF NOT EXISTS review_blocks (project TEXT NOT NULL, task TEXT NOT NULL,
  attempt INTEGER NOT NULL, ids TEXT NOT NULL, created INTEGER NOT NULL,
  PRIMARY KEY (project, task, attempt));

-- A milestone as a voyage (T-22): the one-sentence `done` it is judged by, the
-- quality `bar` of its phase, the planned `eta`, and where it departed from —
-- the time its first task was created and the tasks it had then. `milestones`
-- above is the old (unused) shape; a store without migrations gets a new table.
CREATE TABLE IF NOT EXISTS voyages (project TEXT NOT NULL, milestone TEXT NOT NULL,
  title TEXT NOT NULL DEFAULT '',
  -- NULL until the plan describes the voyage: a milestone whose tasks predate
  -- its first `# Milestone` block still has a departure (T-22, review R1/R2).
  done TEXT, bar TEXT, eta TEXT,
  departure INTEGER, departure_tasks INTEGER, departure_estimate_min INTEGER, updated INTEGER NOT NULL,
  PRIMARY KEY (project, milestone));

CREATE TABLE IF NOT EXISTS seats (
  id TEXT PRIMARY KEY,                 -- '<project>:<name>'
  project TEXT NOT NULL, role TEXT NOT NULL, name TEXT NOT NULL, profile TEXT NOT NULL,
  generation INTEGER NOT NULL DEFAULT 0,   -- current active generation (0 = never onboarded)
  state TEXT NOT NULL, state_since INTEGER NOT NULL, state_reason TEXT,
  park_until INTEGER, park_task TEXT, hold_reason TEXT,
  task_id TEXT,                        -- temp: the one task it is bound to for life
  worktree TEXT);                      -- resources.path of the worktree Herdr created for it

CREATE TABLE IF NOT EXISTS bindings (
  seat TEXT NOT NULL, generation INTEGER NOT NULL, session TEXT NOT NULL, pane_id TEXT NOT NULL,
  kind TEXT NOT NULL, session_ref TEXT,
  status TEXT NOT NULL,                -- pending | active | void
  handover_id TEXT, created INTEGER NOT NULL,
  PRIMARY KEY (seat, generation));

-- Worktrees created through Herdr (conventions 17 §4); path is what Herdr returned.
CREATE TABLE IF NOT EXISTS resources (path TEXT PRIMARY KEY, project TEXT NOT NULL,
  branch TEXT NOT NULL, workspace_id TEXT,  -- NULL while not open in Herdr
  holder_seat TEXT, state TEXT NOT NULL, last_task TEXT, note TEXT, updated INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS plans (project TEXT NOT NULL, version INTEGER NOT NULL, draft TEXT NOT NULL,
  applied INTEGER NOT NULL, by_seat TEXT, by_gen INTEGER, PRIMARY KEY (project, version));

CREATE TABLE IF NOT EXISTS tasks (
  project TEXT NOT NULL, id TEXT NOT NULL, rank INTEGER NOT NULL, state TEXT NOT NULL,
  priority TEXT NOT NULL, lane TEXT, milestone TEXT, required INTEGER NOT NULL DEFAULT 0,
  contract_version INTEGER NOT NULL, deps TEXT NOT NULL DEFAULT '[]', followup_of TEXT,
  wait_on TEXT, wait_cond TEXT, rework_note TEXT,
  -- The planned time of the task in minutes (T-38): the plan's `estimate:`
  -- field. NULL for a task that has none (an older task may lack it).
  estimate_min INTEGER,
  -- The GitHub issue a task came from (T-79): the plan's `issue: #<n>`. NULL
  -- for a task with none.
  issue INTEGER,
  -- v6 (§1 of gear/voyages/M4/design.md): the task keeps its own repo, base and
  -- gate, copied from the lane's config when it is created. A reader that
  -- finds NULL falls back to the lane's current config (old tasks behave as
  -- today). `rework_estimate_min` is the `estimate=<min>` of the fail verdict
  -- that asked for the rework (§5.1).
  repo TEXT, base TEXT, gate TEXT,
  rework_estimate_min INTEGER,
  updated INTEGER NOT NULL, PRIMARY KEY (project, id));

CREATE TABLE IF NOT EXISTS contracts (project TEXT NOT NULL, task TEXT NOT NULL, version INTEGER NOT NULL,
  goal TEXT NOT NULL, accept TEXT NOT NULL, evidence_files TEXT NOT NULL DEFAULT '[]',
  evidence_due_min INTEGER, body TEXT NOT NULL, created INTEGER NOT NULL,
  -- v7 (T-272): the reason this task is mechanical — the gate alone proves the
  -- change, so the review is skipped. NULL: reviewed as before.
  mechanical TEXT,
  PRIMARY KEY (project, task, version));

CREATE TABLE IF NOT EXISTS attempts (
  project TEXT NOT NULL, task TEXT NOT NULL, n INTEGER NOT NULL, seat TEXT NOT NULL,
  generation INTEGER NOT NULL, contract_version INTEGER NOT NULL, adopted_version INTEGER NOT NULL,
  pending_update INTEGER, state TEXT NOT NULL, candidate TEXT, report TEXT, safe_point TEXT,
  -- The moment the attempt row was created (its old name); the delivery time
  -- is `delivered_at` below.
  started INTEGER NOT NULL, updated INTEGER NOT NULL,
  -- The time of every step of the attempt (T-38). `updated` is overwritten at
  -- every step, so it cannot say when a step happened; these six columns can.
  -- Each step writes its own column and never another's: a step that has not
  -- happened yet is NULL. A database from before this change gains the
  -- columns through the store's migration (they are nullable, so an older
  -- binary keeps working on it).
  delivered_at INTEGER, started_at INTEGER, handed_in_at INTEGER,
  verdict_at INTEGER, settled_at INTEGER, merged_at INTEGER,
  -- v6 (§1): which session the attempt ran in, and what the engine did before
  -- the packet. `session_action` is one of keep | new | compact | resume |
  -- fresh | new-unconfirmed | compact-fallback | resume-fallback.
  session_ref TEXT, session_cli TEXT, session_action TEXT, sent_at INTEGER,
  PRIMARY KEY (project, task, n));

CREATE TABLE IF NOT EXISTS messages (
  project TEXT NOT NULL, id TEXT NOT NULL, thread TEXT NOT NULL, task TEXT, attempt INTEGER,
  sender TEXT NOT NULL, sender_gen INTEGER NOT NULL, recipient TEXT NOT NULL,
  kind TEXT NOT NULL, urgent INTEGER NOT NULL DEFAULT 0, body TEXT NOT NULL, reply_to TEXT,
  needs_ack INTEGER NOT NULL DEFAULT 0, state TEXT NOT NULL,
  wait_on TEXT, wait_until INTEGER, result TEXT,
  -- v6 (§5.2): the structured fields of a progress answer, JSON
  -- {"left": <min>, "blocker": "<text>"}.
  meta TEXT,
  created INTEGER NOT NULL, resolved INTEGER, PRIMARY KEY (project, id));
CREATE INDEX IF NOT EXISTS messages_recipient ON messages (project, recipient, state);

-- What a `gear` home/show call actually displayed (design/13). Evidence for op confirmation.
CREATE TABLE IF NOT EXISTS views (
  project TEXT NOT NULL, seat TEXT NOT NULL, generation INTEGER NOT NULL, ts INTEGER NOT NULL,
  shown_msgs TEXT NOT NULL DEFAULT '[]', shown_ops TEXT NOT NULL DEFAULT '[]',
  shown_task TEXT, shown_attempt INTEGER, shown_contract INTEGER);
CREATE INDEX IF NOT EXISTS views_seat ON views (seat, generation, ts);

CREATE TABLE IF NOT EXISTS operations (
  project TEXT NOT NULL, id TEXT NOT NULL, seat TEXT NOT NULL, generation INTEGER NOT NULL,
  kind TEXT NOT NULL, task TEXT, attempt INTEGER, ref_msg TEXT, plan_version INTEGER,
  payload TEXT NOT NULL DEFAULT '{}', state TEXT NOT NULL, result TEXT,
  check_budget INTEGER NOT NULL DEFAULT 0, created INTEGER NOT NULL, updated INTEGER NOT NULL,
  PRIMARY KEY (project, id));
CREATE INDEX IF NOT EXISTS operations_seat ON operations (seat, state);

CREATE TABLE IF NOT EXISTS verdicts (project TEXT NOT NULL, task TEXT NOT NULL, attempt INTEGER NOT NULL,
  contract_version INTEGER NOT NULL, candidate TEXT NOT NULL, passed INTEGER NOT NULL,
  evidence TEXT, by_seat TEXT NOT NULL, created INTEGER NOT NULL);

-- The verdict a temp reviewer handed in for one attempt (T-2). One record per
-- attempt; the findings hold a path or the text itself.
CREATE TABLE IF NOT EXISTS review_verdicts (project TEXT NOT NULL, task TEXT NOT NULL,
  attempt INTEGER NOT NULL, passed INTEGER NOT NULL, findings TEXT NOT NULL,
  by_seat TEXT NOT NULL, created INTEGER NOT NULL,
  PRIMARY KEY (project, task, attempt));

CREATE TABLE IF NOT EXISTS reviews (project TEXT NOT NULL, id TEXT NOT NULL, cause TEXT NOT NULL,
  state TEXT NOT NULL, detail TEXT, decided TEXT, plan_version INTEGER, checkpoint INTEGER,
  reminders INTEGER NOT NULL DEFAULT 0, created INTEGER NOT NULL, updated INTEGER NOT NULL,
  PRIMARY KEY (project, id));

CREATE TABLE IF NOT EXISTS samples (project TEXT NOT NULL, seat TEXT NOT NULL, task TEXT, ts INTEGER NOT NULL,
  head TEXT, status_digest TEXT, content_digest TEXT, complete INTEGER NOT NULL, evidence TEXT);

CREATE TABLE IF NOT EXISTS work_checks (project TEXT NOT NULL, task TEXT NOT NULL, ts INTEGER NOT NULL,
  base_head TEXT, content_digest TEXT, judgement TEXT NOT NULL, next_check INTEGER, by_seat TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS requests (id TEXT PRIMARY KEY, args_hash TEXT NOT NULL, result TEXT NOT NULL,
  created INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS audit (ts INTEGER NOT NULL, project TEXT, actor TEXT NOT NULL,
  action TEXT NOT NULL, detail TEXT);

-- v6 (§1 of gear/voyages/M4/design.md). The tables of the M4 schema: the rounds
-- and their tokens, the stretches a seat could not work, the friction reports
-- and the captain's decision log.

-- One round of work by one seat (§4): a dev round (attempt 1 of a task), a
-- rework round (attempt n >= 2) or a review round (the review of one
-- attempt). A round owns every agent record on its seat from `start_at` until
-- `end_at`; there is no other rule (§4.1).
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

-- Every stretch a seat could not work (§5.4): a quota park, a declared wait.
-- The key carries the kind: the two stretches of one seat — a quota park and
-- a declared wait — are independent and may overlap.
CREATE TABLE IF NOT EXISTS seat_pauses (
  project TEXT NOT NULL, seat TEXT NOT NULL,
  kind TEXT NOT NULL,                 -- quota | wait
  reason TEXT,                        -- provider and window, or the task waited for
  start_at INTEGER NOT NULL, end_at INTEGER,   -- end_at NULL while it lasts
  PRIMARY KEY (project, seat, kind, start_at));

-- The friction reports (§6): what cost a round time the captain could fix.
CREATE TABLE IF NOT EXISTS frictions (
  project TEXT NOT NULL, id TEXT NOT NULL,              -- F-<n>, next_id(project,'F')
  task TEXT NOT NULL, attempt INTEGER NOT NULL, role TEXT NOT NULL,
  seat TEXT NOT NULL, body TEXT NOT NULL,
  required INTEGER NOT NULL,          -- 1: the round passed its budget (§6.2)
  created INTEGER NOT NULL,
  closed_at INTEGER, outcome TEXT,    -- §6.4: what the captain did about it
  PRIMARY KEY (project, id));

-- The decision log (§7): every decision the captain made, keyed by the hash
-- of the normalized line, so an apply is idempotent (§7.3).
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
