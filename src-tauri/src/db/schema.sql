-- Fleet SQLite schema. Applied idempotently on startup (CREATE TABLE IF NOT EXISTS).

CREATE TABLE IF NOT EXISTS accounts (
  id               TEXT PRIMARY KEY,
  provider         TEXT NOT NULL CHECK (provider IN ('claude','codex','gemini')),
  billing          TEXT NOT NULL CHECK (billing IN ('sub','key')),
  name             TEXT NOT NULL,
  scope            TEXT NOT NULL CHECK (scope IN ('work','personal')),
  plan_label       TEXT NOT NULL DEFAULT '',
  config_dir_path  TEXT NOT NULL,
  credential_ref   TEXT NOT NULL,
  monthly_cap      REAL,
  status           TEXT NOT NULL DEFAULT 'healthy' CHECK (status IN ('healthy','cooling_down','exhausted')),
  created_at       TEXT NOT NULL,
  updated_at       TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS projects (
  id                     TEXT PRIMARY KEY,
  name                   TEXT NOT NULL,
  path                   TEXT NOT NULL,
  -- Last "setup command" used when creating a subsession worktree for this
  -- project (e.g. `npm install`), remembered so the next subsession dialog
  -- can default to it instead of starting blank every time.
  default_setup_command  TEXT,
  created_at             TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS project_allowed_accounts (
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  allowed    INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (project_id, account_id)
);

CREATE TABLE IF NOT EXISTS rotation_policies (
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  provider   TEXT NOT NULL CHECK (provider IN ('claude','codex','gemini')),
  policy     TEXT NOT NULL DEFAULT 'manual' CHECK (policy IN ('auto','round_robin','manual')),
  PRIMARY KEY (project_id, provider)
);

CREATE TABLE IF NOT EXISTS rotation_events (
  id             TEXT PRIMARY KEY,
  project_id     TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  from_account_id TEXT REFERENCES accounts(id) ON DELETE SET NULL,
  to_account_id   TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  reason         TEXT NOT NULL,
  created_at     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS memory_groups (
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS memory_group_projects (
  group_id   TEXT NOT NULL REFERENCES memory_groups(id) ON DELETE CASCADE,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  PRIMARY KEY (group_id, project_id)
);

CREATE TABLE IF NOT EXISTS memory_files (
  id         TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  filename   TEXT NOT NULL,
  content_md TEXT NOT NULL DEFAULT '',
  updated_at TEXT NOT NULL
);

-- Small singleton key/value store for app-wide preferences that don't
-- warrant their own table (e.g. the preferred reviewer account) — read the
-- whole table into a struct rather than adding a one-off table per setting.
CREATE TABLE IF NOT EXISTS app_settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS agent_nodes (
  id         TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  parent_id  TEXT REFERENCES agent_nodes(id) ON DELETE CASCADE,
  role       TEXT NOT NULL,
  prompt     TEXT NOT NULL DEFAULT '',
  model      TEXT NOT NULL,
  account_id TEXT REFERENCES accounts(id) ON DELETE SET NULL,
  depth      INTEGER NOT NULL DEFAULT 0 CHECK (depth <= 3),
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
  id                 TEXT PRIMARY KEY,
  -- User-chosen display name for this session, distinct from the project
  -- it belongs to (e.g. "fix-billing-rounding" for project "casablanca").
  name               TEXT NOT NULL DEFAULT '',
  project_id         TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  node_id            TEXT REFERENCES agent_nodes(id) ON DELETE SET NULL,
  -- Subsessions: a session branched off another to work in parallel on the
  -- same project via its own git worktree (or copied folder). CASCADE so
  -- deleting a session takes its whole subsession subtree with it.
  parent_session_id  TEXT REFERENCES sessions(id) ON DELETE CASCADE,
  -- Absolute path of the subsession's isolated working directory, NULL for
  -- an ordinary (non-branched) session.
  worktree_path      TEXT,
  -- Branch git created for it — only set when worktree_kind = 'linked'.
  worktree_branch    TEXT,
  -- 'linked' = real `git worktree` off the project's existing repo.
  -- 'copy'   = project had no git repo, so the folder was copied and
  --            `git init`-ed fresh instead (no shared history).
  worktree_kind      TEXT CHECK (worktree_kind IN ('linked','copy')),
  -- CASCADE (not RESTRICT): deleting an account must delete its sessions
  -- too, rather than blocking the account delete outright.
  account_id         TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  model              TEXT NOT NULL,
  effort             TEXT NOT NULL DEFAULT 'high' CHECK (effort IN ('low','medium','high','xhigh','max')),
  mode               TEXT NOT NULL DEFAULT 'ask_edits' CHECK (mode IN ('plan','ask_edits','auto_edit','yolo')),
  status             TEXT NOT NULL DEFAULT 'waiting' CHECK (status IN ('waiting','running','reviewing','done')),
  pid                INTEGER,
  -- The provider CLI's own conversation/session id (e.g. Claude Code's
  -- `--resume` id), captured from its first response so follow-up turns
  -- continue the same conversation instead of starting fresh each time.
  provider_session_id TEXT,
  started_at         TEXT NOT NULL,
  ended_at           TEXT
);

CREATE TABLE IF NOT EXISTS session_blocks (
  id         TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  kind       TEXT NOT NULL CHECK (kind IN ('you','think','edit','cmd','agent','sum','rev','out')),
  text       TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS git_checkpoints (
  id          TEXT PRIMARY KEY,
  session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  turn_number INTEGER NOT NULL,
  commit_ref  TEXT NOT NULL,
  created_at  TEXT NOT NULL
);

-- Captures the real before/after content of an Edit/Write/MultiEdit tool call
-- (turn_runner's parsed session_blocks only keep a human-readable "Edit path"
-- summary, deliberately dropping the raw diff) so a later diff view has
-- something real to render instead of just the file path that changed.
CREATE TABLE IF NOT EXISTS file_edits (
  id         TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  block_id   TEXT REFERENCES session_blocks(id) ON DELETE SET NULL,
  file_path  TEXT NOT NULL,
  tool_name  TEXT NOT NULL,
  old_text   TEXT,
  new_text   TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS turn_usage (
  id                    TEXT PRIMARY KEY,
  session_id            TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  block_id              TEXT REFERENCES session_blocks(id) ON DELETE SET NULL,
  input_tokens          INTEGER NOT NULL,
  output_tokens         INTEGER NOT NULL,
  cache_read_tokens     INTEGER,
  cache_creation_tokens INTEGER,
  cost_usd              REAL,
  created_at            TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_id);
-- idx_sessions_parent is created in db::run_migrations instead of here: on an
-- existing database, CREATE TABLE IF NOT EXISTS above is a no-op that does
-- NOT retroactively add parent_session_id, so creating this index here
-- unconditionally would crash startup for any pre-existing install (the
-- ALTER TABLE ADD COLUMN migrations that add the column only run after this
-- whole schema batch succeeds).
CREATE INDEX IF NOT EXISTS idx_session_blocks_session ON session_blocks(session_id);
CREATE INDEX IF NOT EXISTS idx_agent_nodes_project ON agent_nodes(project_id);
CREATE INDEX IF NOT EXISTS idx_agent_nodes_parent ON agent_nodes(parent_id);
CREATE INDEX IF NOT EXISTS idx_file_edits_session ON file_edits(session_id);
CREATE INDEX IF NOT EXISTS idx_turn_usage_session ON turn_usage(session_id);
