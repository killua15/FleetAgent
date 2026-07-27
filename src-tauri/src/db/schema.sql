-- Hangar SQLite schema. Applied idempotently on startup (CREATE TABLE IF NOT EXISTS).

CREATE TABLE IF NOT EXISTS accounts (
  id               TEXT PRIMARY KEY,
  provider         TEXT NOT NULL CHECK (provider IN ('claude','codex','gemini','kimi')),
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
  exhausted_at     TEXT,
  -- This account's row in a shared team pool, when the user has joined one.
  -- The pool never holds credentials: it records that the account exists and
  -- who is using it right now, so two people don't spend the same subscription
  -- at once. NULL for a purely local account, which is all of them by default.
  team_account_id  TEXT
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
  provider   TEXT NOT NULL CHECK (provider IN ('claude','codex','gemini','kimi')),
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
  -- The project whose repo this PR belongs to. The real anchor: a PR opened by
  -- someone else has no session behind it at all, and grouping the inbox by
  -- project through `sessions` is what used to make such a PR unrepresentable.
  project_id            TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  -- The session that produced this PR, when Hangar itself opened it. NULL for
  -- a discovered PR — including every PR of someone else's. SET NULL rather
  -- than CASCADE: the PR still exists on GitHub after its session is deleted.
  session_id            TEXT REFERENCES sessions(id) ON DELETE SET NULL,
  repo_owner            TEXT NOT NULL,
  repo_name             TEXT NOT NULL,
  number                INTEGER NOT NULL,
  url                   TEXT NOT NULL,
  state                 TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open','closed','merged')),
  title                 TEXT,
  -- The PR author's GitHub login, so a PR that isn't yours says whose it is.
  author_login          TEXT,
  -- What this PR is to the logged-in `gh` user, recomputed on every discovery
  -- pass because it genuinely changes: 'mine' (I opened it), 'review'
  -- (review-requested from me, or assigned to me), 'other' (just open on a
  -- repo I work on). Only 'mine' is polled for review comments — see
  -- `poll_all_open_prs`.
  relation              TEXT NOT NULL DEFAULT 'mine' CHECK (relation IN ('mine','review','other')),
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
  -- PR Hangar itself opened, backfilled lazily (one `gh pr view` call) for a
  -- discovered one the first time "Send to agent" needs it. This is what
  -- lets `resolve_session_for_pr_branch` check out the PR's real branch
  -- instead of trusting whatever session happens to be tied to the row.
  head_branch           TEXT,
  -- The branch the PR targets (`baseRefName`) — what a review diffs against.
  base_branch           TEXT,
  created_at            TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_pull_requests_repo_number ON pull_requests(repo_owner, repo_name, number);

-- Every PR comment/review Hangar has ever fetched from someone other than the
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

-- Which terminals were open when Hangar last closed, so they can be reopened
-- on the next launch (and, when they carry a Claude session id, resumed).
-- A mirror of the in-memory `TerminalRegistry`: `open_terminal` inserts,
-- `close_terminal` (the user closing one) deletes, but `kill_all` (Hangar
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
  -- A name the user typed for this panel, overriding the project or session
  -- name. Two terminals on the same project are otherwise indistinguishable in
  -- a list, which is exactly when a sidebar makes them worth naming.
  display_name TEXT,
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

-- Every `/usage` reading the poller takes, kept instead of thrown away.
--
-- The poller has always scraped the real 5h/weekly/context percentages every
-- five minutes and cached them in memory, where each sample overwrote the last
-- and the whole history died with the process. One number tells you where you
-- are; a series tells you how fast you're burning and when you'll run out —
-- which is the thing no other agent orchestrator can compute, because none of
-- them models accounts at all.
--
-- Cheap by construction: one row per account per five minutes, only for
-- accounts with a terminal open.
CREATE TABLE IF NOT EXISTS account_usage_samples (
  id           TEXT PRIMARY KEY,
  account_id   TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  -- All three are nullable: `/usage` is a TUI-only built-in and its output is
  -- parsed leniently, so a reading can legitimately be partial.
  session_pct  REAL,
  weekly_pct   REAL,
  context_pct  REAL,
  sampled_at   TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_account_usage_samples ON account_usage_samples(account_id, sampled_at);

-- Work waiting for quota instead of failing against it.
--
-- The loudest documented complaint about Claude Code is hitting the usage limit
-- mid-task and losing the session. Every tool in this category, including this
-- one until now, lets you walk into that wall. A queued run waits for an
-- account with headroom and starts itself when one appears.
--
-- `branch_name` is NOT NULL on purpose: unattended work always gets its own
-- worktree. A prompt that runs at 3am while you sleep does not touch your main
-- checkout, and that isn't a preference to configure.
CREATE TABLE IF NOT EXISTS queued_runs (
  id           TEXT PRIMARY KEY,
  project_id   TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  prompt       TEXT NOT NULL,
  branch_name  TEXT NOT NULL,
  -- A specific account, or NULL for "any the project is allowed to use".
  account_id   TEXT REFERENCES accounts(id) ON DELETE SET NULL,
  state        TEXT NOT NULL DEFAULT 'queued' CHECK (state IN ('queued','running','done','failed','cancelled')),
  -- The terminal and worktree session this run took over once it started.
  terminal_id  TEXT,
  session_id   TEXT REFERENCES sessions(id) ON DELETE SET NULL,
  attempts     INTEGER NOT NULL DEFAULT 0,
  last_error   TEXT,
  created_at   TEXT NOT NULL,
  started_at   TEXT,
  finished_at  TEXT
);

CREATE INDEX IF NOT EXISTS idx_queued_runs_state ON queued_runs(state, created_at);

-- Caps on how much of the quota pool one project may spend.
--
-- Attribution is honest about its limits: usage is sampled per *account*, so
-- when two projects share an account at the same time, splitting the burn
-- between them is an estimate and the UI says so. With team leases the same
-- number becomes exact, because a lease records precisely who held which
-- account between which times.
--
-- Enforcement blocks *new* work only. A run already in flight is never killed:
-- stopping an agent mid-edit leaves a worktree in a state nobody asked for.
CREATE TABLE IF NOT EXISTS quota_budgets (
  project_id  TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
  -- Share of the weekly pool this project may spend, 1-100. The weekly window
  -- is the one worth budgeting: the 5-hour one refills over lunch.
  weekly_pct  REAL NOT NULL CHECK (weekly_pct > 0 AND weekly_pct <= 100),
  updated_at  TEXT NOT NULL
);
