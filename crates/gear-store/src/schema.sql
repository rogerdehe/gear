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

CREATE TABLE IF NOT EXISTS seats (
  id TEXT PRIMARY KEY,                 -- '<project>:<name>'
  project TEXT NOT NULL, role TEXT NOT NULL, name TEXT NOT NULL, profile TEXT NOT NULL,
  generation INTEGER NOT NULL DEFAULT 0,   -- current active generation (0 = never onboarded)
  state TEXT NOT NULL, state_since INTEGER NOT NULL, state_reason TEXT,
  park_until INTEGER, park_task TEXT, hold_reason TEXT,
  task_id TEXT,                        -- temp: the one task it is bound to for life
  slot TEXT);

CREATE TABLE IF NOT EXISTS bindings (
  seat TEXT NOT NULL, generation INTEGER NOT NULL, session TEXT NOT NULL, pane_id TEXT NOT NULL,
  kind TEXT NOT NULL, session_ref TEXT,
  status TEXT NOT NULL,                -- pending | active | void
  handover_id TEXT, created INTEGER NOT NULL,
  PRIMARY KEY (seat, generation));

CREATE TABLE IF NOT EXISTS resources (path TEXT PRIMARY KEY, project TEXT NOT NULL,
  holder_seat TEXT, state TEXT NOT NULL, last_task TEXT, note TEXT, updated INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS slots (project TEXT NOT NULL, id TEXT NOT NULL, pane_id TEXT NOT NULL,
  worktree TEXT NOT NULL, state TEXT NOT NULL, PRIMARY KEY (project, id));

CREATE TABLE IF NOT EXISTS plans (project TEXT NOT NULL, version INTEGER NOT NULL, draft TEXT NOT NULL,
  applied INTEGER NOT NULL, by_seat TEXT, by_gen INTEGER, PRIMARY KEY (project, version));

CREATE TABLE IF NOT EXISTS tasks (
  project TEXT NOT NULL, id TEXT NOT NULL, rank INTEGER NOT NULL, state TEXT NOT NULL,
  priority TEXT NOT NULL, lane TEXT, milestone TEXT, required INTEGER NOT NULL DEFAULT 0,
  contract_version INTEGER NOT NULL, deps TEXT NOT NULL DEFAULT '[]', followup_of TEXT,
  wait_on TEXT, wait_cond TEXT, rework_note TEXT, updated INTEGER NOT NULL,
  PRIMARY KEY (project, id));

CREATE TABLE IF NOT EXISTS contracts (project TEXT NOT NULL, task TEXT NOT NULL, version INTEGER NOT NULL,
  goal TEXT NOT NULL, accept TEXT NOT NULL, evidence_files TEXT NOT NULL DEFAULT '[]',
  evidence_due_min INTEGER, body TEXT NOT NULL, created INTEGER NOT NULL,
  PRIMARY KEY (project, task, version));

CREATE TABLE IF NOT EXISTS attempts (
  project TEXT NOT NULL, task TEXT NOT NULL, n INTEGER NOT NULL, seat TEXT NOT NULL,
  generation INTEGER NOT NULL, contract_version INTEGER NOT NULL, adopted_version INTEGER NOT NULL,
  pending_update INTEGER, state TEXT NOT NULL, candidate TEXT, report TEXT, safe_point TEXT,
  started INTEGER NOT NULL, updated INTEGER NOT NULL, PRIMARY KEY (project, task, n));

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
