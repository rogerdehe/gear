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

-- Where a repo of the project may be pushed (T-73): the destinations
-- `gear admin init` froze from the repo's own `origin` remote, normalised as
-- host + path. One row per destination; a repo with no row may not be pushed
-- to at all. The list is read again on every re-init.
CREATE TABLE IF NOT EXISTS push_destinations (
  project TEXT NOT NULL,
  repo TEXT NOT NULL,           -- '' for the project root, else the [[repos]] name
  destination TEXT NOT NULL,    -- normalised host + path
  created INTEGER NOT NULL,
  PRIMARY KEY (project, repo, destination));

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
  updated INTEGER NOT NULL, PRIMARY KEY (project, id));

CREATE TABLE IF NOT EXISTS contracts (project TEXT NOT NULL, task TEXT NOT NULL, version INTEGER NOT NULL,
  goal TEXT NOT NULL, accept TEXT NOT NULL, evidence_files TEXT NOT NULL DEFAULT '[]',
  evidence_due_min INTEGER, body TEXT NOT NULL, created INTEGER NOT NULL,
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
  PRIMARY KEY (project, task, n));

CREATE TABLE IF NOT EXISTS messages (
  project TEXT NOT NULL, id TEXT NOT NULL, thread TEXT NOT NULL, task TEXT, attempt INTEGER,
  sender TEXT NOT NULL, sender_gen INTEGER NOT NULL, recipient TEXT NOT NULL,
  kind TEXT NOT NULL, urgent INTEGER NOT NULL DEFAULT 0, body TEXT NOT NULL, reply_to TEXT,
  needs_ack INTEGER NOT NULL DEFAULT 0, state TEXT NOT NULL,
  wait_on TEXT, wait_until INTEGER, result TEXT,
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
