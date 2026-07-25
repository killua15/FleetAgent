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
  updated_at       TEXT NOT NULL,
  -- When `status` became 'exhausted', and nothing else. `updated_at` cannot
  -- serve this: any unrelated write (repointing a config dir, editing a cap)
  -- refreshes it, which silently pushed the cooldown-based recovery back by
  -- however long ago that edit was. NULL whenever the account isn't benched —
  -- and also on a flag inherited from before this column existed, whose age is
  -- unknowable and which recovery therefore treats as expired.
  exhausted_at     TEXT
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

-- One row per PR a session pushed/opened via `gh` (or discovered later —
-- see `discover_open_prs_for_project`). `last_comment_seen_at` is just "the
-- last time this PR was successfully checked" — a display/debug timestamp,
-- not a dedup key. Which comments are "new" is decided by
-- `pr_review_comments.external_id` instead (see its own comment for why).
-- Lifecycle: a row lives here until either GitHub reports it merged (at
-- which point `check_pr_comments_for` deletes it outright — merged is
-- terminal, nothing left to review) or it's closed without merging, in
-- which case the row (and its comments) stick around with `state='closed'`
-- since a closed PR can still be reopened.
CREATE TABLE IF NOT EXISTS pull_requests (
  id                    TEXT PRIMARY KEY,
  session_id            TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  repo_owner            TEXT NOT NULL,
  repo_name             TEXT NOT NULL,
  number                INTEGER NOT NULL,
  url                   TEXT NOT NULL,
  state                 TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open','closed','merged')),
  last_comment_seen_at  TEXT,
  -- The error from the most recent failed `check_pr_comments_impl` run for
  -- this PR (a `gh` call failing — permissions, SSO, rate limit, whatever) —
  -- NULL once a check succeeds. Without this, a PR whose check is silently
  -- failing every time looks identical to one with genuinely nothing new.
  last_check_error      TEXT,
  -- GitHub's own `updatedAt` as of the last successful check — a cheap
  -- freshness signal. A real check first asks `gh` for just state+updatedAt;
  -- if updatedAt hasn't moved since this value, the expensive comment fetch
  -- (a second `gh` call, plus a REST call for inline comments) is skipped
  -- entirely, so a "check now" over PRs with nothing new stays fast instead
  -- of re-downloading everything every time.
  last_seen_updated_at  TEXT,
  -- The PR's actual head branch (`headRefName`) — known immediately for a
  -- PR Fleet itself opened, backfilled lazily (one `gh pr view` call) for a
  -- discovered one the first time "Send to agent" needs it. This is what
  -- lets `resolve_session_for_pr_branch` check out the PR's real branch
  -- instead of trusting whatever session happens to be tied to the row.
  head_branch           TEXT,
  created_at            TEXT NOT NULL
);

-- Every PR comment/review Fleet has ever fetched from someone other than the
-- logged-in `gh` user, persisted so the PR review inbox screen survives
-- restarts and doesn't depend on a panel being open when the poller finds
-- one. `resolved_at` is NULL for anything still pending in the inbox.
-- `path`/`line` are set only for inline diff comments (from
-- `gh api .../pulls/{n}/comments`) — NULL for general PR/review comments
-- (from `gh pr view --json comments,reviews`), which have no file context.
-- `external_id` is GitHub's own id for the comment/review — the real "have
-- we already surfaced this one" key (see `RawPrComment`'s doc comment in
-- commands/pull_requests.rs for why this replaced a timestamp watermark).
CREATE TABLE IF NOT EXISTS pr_review_comments (
  id                  TEXT PRIMARY KEY,
  pull_request_id     TEXT NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
  author              TEXT NOT NULL,
  body                TEXT NOT NULL,
  comment_created_at  TEXT NOT NULL,
  path                TEXT,
  line                INTEGER,
  external_id         TEXT,
  resolution          TEXT CHECK (resolution IN ('sent','dismissed')),
  resolved_at         TEXT,
  created_at          TEXT NOT NULL
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

-- Which terminals were open when Fleet last closed, so they can be reopened
-- on the next launch (and, when they carry a Claude session id, resumed).
-- A mirror of the in-memory `TerminalRegistry`: `open_terminal` inserts,
-- `close_terminal` (the user closing one) deletes, but `kill_all` (Fleet
-- shutting down) deliberately does NOT — that's what tells "forget this" apart
-- from "remember this across a restart". `source_id` is a project or session
-- id per `target_kind`, so it has no single FK; restore validates the target
-- (and account, and transcript) and drops dead rows instead.
CREATE TABLE IF NOT EXISTS open_terminals (
  id           TEXT PRIMARY KEY,
  target_kind  TEXT NOT NULL CHECK (target_kind IN ('project','session')),
  source_id    TEXT NOT NULL,
  account_id   TEXT,
  session_uuid TEXT,
  created_at   TEXT NOT NULL
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
CREATE INDEX IF NOT EXISTS idx_pull_requests_session ON pull_requests(session_id);
CREATE INDEX IF NOT EXISTS idx_pr_review_comments_pr ON pr_review_comments(pull_request_id);
