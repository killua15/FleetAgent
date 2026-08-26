use rusqlite::Connection;
use std::sync::Mutex;
use tauri::AppHandle;

pub struct DbConnection(pub Mutex<Connection>);

pub(crate) const SCHEMA: &str = include_str!("schema.sql");

/// Opens the database, applies the schema, and migrates it forward.
///
/// Returns an error rather than panicking. It used to panic, and the failure
/// mode was as bad as it gets: a `SIGABRT` inside the Objective-C launch
/// notification, before any window existed. The user sees a crash reporter and
/// nothing else — no message, no log pointer, no way to tell "your database is
/// from a newer version" apart from "your disk is full".
///
/// Still refuses to start on a half-applied migration: every command past this
/// point assumes the schema it was written against, and limping along in a
/// shape nothing expects corrupts data instead of failing.
pub fn init(app: &AppHandle) -> Result<Connection, String> {
    let dir = crate::app_data::root(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let db_path = dir.join(crate::app_data::DB_FILE);

    let conn = Connection::open(&db_path).map_err(|e| format!("could not open {}: {e}", db_path.display()))?;
    conn.execute_batch("PRAGMA foreign_keys = ON;").map_err(|e| format!("could not enable foreign keys: {e}"))?;
    // WAL and a relaxed sync, set before the schema so every write from here on
    // gets them.
    //
    // The default — `journal_mode = delete`, `synchronous = FULL` — makes each
    // statement outside a transaction create a journal file and fsync it. That
    // is the safest possible setting and it costs about 0.3 ms per row, which
    // nobody notices until something writes thousands of them while holding the
    // one lock the whole app shares (see `usage_meter::record_turns`).
    //
    // WAL also lets a reader run while a writer commits, which matters here for
    // a reason specific to this app: the UI thread runs synchronous commands
    // that read this database, and it used to sit behind every background
    // write.
    //
    // `NORMAL` rather than `FULL`: in WAL that risks losing the last commits in
    // a power cut, never a corrupt database. What is at stake is a few seconds
    // of token accounting, which the next transcript read puts back.
    //
    // Not fatal if it fails — an older SQLite or a filesystem that won't do WAL
    // should still get an app, just a slower one.
    if let Err(e) = conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;") {
        log::warn!("could not enable WAL, continuing on the slower default: {e}");
    }
    conn.execute_batch(SCHEMA).map_err(|e| format!("could not apply the schema: {e}"))?;
    run_migrations(&conn)?;
    Ok(conn)
}

/// One numbered, recorded schema change. `version` is what gets written to
/// `PRAGMA user_version` once it succeeds, so each migration runs exactly
/// once per database, ever.
struct Migration {
    version: i64,
    name: &'static str,
    run: fn(&Connection) -> Result<(), String>,
    /// Whether the runner should wrap `run` in its own transaction. False
    /// only for `legacy_baseline`, which manages its own transactions and
    /// toggles `PRAGMA foreign_keys` (a no-op inside a transaction, so it
    /// has to stay outside one).
    transactional: bool,
}

/// Every schema change, in order. **Append only** — never renumber, never
/// edit a migration that has shipped: a database that already recorded
/// version N will never re-run it, so an edited migration silently applies to
/// new installs and not existing ones, which is exactly the class of drift
/// this table exists to prevent. To change something, add a new entry.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "legacy_baseline",
        run: legacy_baseline,
        transactional: false,
    },
    Migration {
        version: 2,
        name: "accounts_exhausted_at",
        run: accounts_exhausted_at,
        transactional: true,
    },
    Migration {
        version: 3,
        name: "open_terminals",
        run: open_terminals_table,
        transactional: true,
    },
    Migration {
        version: 4,
        name: "pull_requests_project_anchored",
        run: pull_requests_project_anchored,
        // Rebuilds a table other tables reference, so it has to toggle
        // `PRAGMA foreign_keys` — a no-op inside a transaction. It runs its
        // own BEGIN/COMMIT instead.
        transactional: false,
    },
    Migration {
        version: 5,
        name: "account_usage_samples",
        run: account_usage_samples_table,
        transactional: true,
    },
    Migration {
        version: 6,
        name: "open_terminals_display_name",
        run: open_terminals_display_name,
        transactional: true,
    },
    Migration {
        version: 7,
        name: "queued_runs",
        run: queued_runs_table,
        transactional: true,
    },
    Migration {
        version: 8,
        name: "quota_budgets_and_team_accounts",
        run: quota_budgets_and_team_accounts,
        transactional: true,
    },
    Migration {
        version: 9,
        name: "allow_kimi_provider",
        run: allow_kimi_provider,
        // Rebuilds tables other tables reference, so it toggles PRAGMAs and
        // runs its own transaction.
        transactional: false,
    },
    Migration {
        version: 10,
        name: "project_git_identity",
        run: project_git_identity_table,
        transactional: true,
    },
    Migration {
        version: 11,
        name: "task_sources",
        run: task_sources_tables,
        transactional: true,
    },
    Migration {
        version: 12,
        name: "turn_usage_from_terminals",
        run: turn_usage_from_terminals,
        // Rebuilds a table with foreign keys of its own, so it toggles the
        // PRAGMAs and runs its own transaction — same care as 4 and 9.
        transactional: false,
    },
    Migration {
        version: 13,
        name: "paired_device",
        run: paired_device_table,
        transactional: true,
    },
    Migration {
        version: 14,
        name: "allow_grok_provider",
        run: allow_grok_provider,
        // Rebuilds tables other tables reference, so it toggles PRAGMAs and
        // runs its own transaction — same care as 4, 9 and 12.
        transactional: false,
    },
    Migration {
        version: 15,
        name: "project_preferences",
        run: project_preferences_table,
        transactional: true,
    },
    Migration {
        version: 16,
        name: "project_skills",
        run: project_skills_table,
        transactional: true,
    },
    Migration {
        version: 17,
        name: "always_on_skills",
        run: always_on_skills_table,
        transactional: true,
    },
    Migration {
        version: 18,
        name: "skill_sources",
        run: skill_sources_table,
        transactional: true,
    },
    Migration {
        version: 19,
        name: "code_graphs",
        run: code_graphs_tables,
        transactional: true,
    },
    Migration {
        version: 20,
        name: "graph_groups",
        run: graph_groups_tables,
        transactional: true,
    },
    Migration {
        version: 21,
        name: "turn_usage_graph_marker",
        run: turn_usage_graph_marker,
        transactional: true,
    },
    Migration {
        version: 22,
        name: "pull_requests_forge_and_host",
        run: pull_requests_forge_and_host,
        transactional: true,
    },
    Migration {
        version: 23,
        name: "forge_instances",
        run: forge_instances_table,
        transactional: true,
    },
    Migration {
        version: 24,
        name: "pr_host_checks",
        run: pr_host_checks_table,
        transactional: true,
    },
    Migration {
        version: 25,
        name: "account_login_checks",
        run: account_login_checks_table,
        transactional: true,
    },
    Migration {
        version: 26,
        name: "turn_usage_is_a_ledger",
        // Manages its own transaction: rebuilding a table needs
        // `PRAGMA foreign_keys=OFF` around it, which is a no-op inside one.
        run: turn_usage_is_a_ledger,
        transactional: false,
    },
    Migration {
        version: 27,
        name: "account_provider_email",
        run: account_provider_email,
        transactional: true,
    },
    Migration {
        version: 28,
        name: "allow_opencode_provider",
        run: allow_opencode_provider,
        transactional: false,
    },
    Migration {
        version: 29,
        name: "open_terminal_minimized",
        run: open_terminal_minimized,
        transactional: true,
    },
    // Was 28 on its own branch, where it landed at the same number as the
    // opencode one. Renumbered on the way in, never after: append only.
    Migration {
        version: 30,
        name: "graph_index_policy",
        run: graph_index_policy,
        transactional: true,
    },
];

/// Whether `table` already has a column named `column`.
///
/// Needed because `schema.sql` declares the current shape for a fresh database
/// *and* every migration still runs against it, so an `ALTER TABLE ADD COLUMN`
/// for a column `schema.sql` already created would fail with *duplicate column
/// name* and take startup down with it. Checking beats `legacy_baseline`'s
/// blanket error-swallowing: only the one expected collision is tolerated, and
/// a genuine failure still reports.
fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| format!("could not inspect {table}: {e}"))?;
    let mut names = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("could not read {table} columns: {e}"))?;
    Ok(names.any(|name| name.as_deref() == Ok(column)))
}

/// Gives account exhaustion its own timestamp, separate from `updated_at`.
///
/// Existing `exhausted` rows are deliberately left with `exhausted_at` NULL
/// rather than backfilled from `updated_at`. Those flags come from the headless
/// rate-limit path and may be days old, with `updated_at` since overwritten by
/// unrelated edits — so their real age is not recoverable. Recovery reads NULL
/// as expired, which clears them on the next pass and lets a real `/usage`
/// check re-establish the truth, instead of preserving a benching nobody can
/// date or justify.
fn accounts_exhausted_at(conn: &Connection) -> Result<(), String> {
    if has_column(conn, "accounts", "exhausted_at")? {
        return Ok(());
    }
    conn.execute_batch("ALTER TABLE accounts ADD COLUMN exhausted_at TEXT;")
        .map_err(|e| format!("could not add accounts.exhausted_at: {e}"))
}

/// Says which forge and which instance a tracked change request lives on.
///
/// The defaults are correct by construction rather than by guess: before this
/// column existed the only thing that could be in the table was a github.com
/// pull request, because that was the only thing Hangar could open or discover.
///
/// **No `CHECK` on either column, deliberately.** SQLite can't add one via
/// `ALTER TABLE`, so getting it would mean rebuilding the table with
/// `pr_review_comments`' foreign key pointing at it — real risk, bought for
/// nothing: `forge` is written from exactly one place (`Forge::as_str`), so a
/// wrong value there is a compile-visible bug, not data drift. It also matters
/// that the inline tests apply `SCHEMA` directly and never run these
/// migrations, which makes any divergence between the two paths invisible to
/// the whole suite — keeping them identical is worth more than the constraint.
///
/// The index is replaced rather than kept: `(repo_owner, repo_name, number)`
/// stopped discriminating the moment two hosts could hold the same project
/// path. Correctness comes from the `WHERE` clauses that now filter on `host`;
/// this just makes them cheap.
fn pull_requests_forge_and_host(conn: &Connection) -> Result<(), String> {
    if !has_column(conn, "pull_requests", "forge")? {
        conn.execute_batch("ALTER TABLE pull_requests ADD COLUMN forge TEXT NOT NULL DEFAULT 'github';")
            .map_err(|e| format!("could not add pull_requests.forge: {e}"))?;
    }
    if !has_column(conn, "pull_requests", "host")? {
        conn.execute_batch("ALTER TABLE pull_requests ADD COLUMN host TEXT NOT NULL DEFAULT 'github.com';")
            .map_err(|e| format!("could not add pull_requests.host: {e}"))?;
    }
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_pull_requests_repo_number;
         CREATE INDEX IF NOT EXISTS idx_pull_requests_host_repo_number ON pull_requests(host, repo_owner, repo_name, number);",
    )
    .map_err(|e| format!("could not re-key the pull_requests index on host: {e}"))
}

/// The instances the user has declared. Same shape `schema.sql` declares for a
/// fresh database, so this is a no-op there and creates the table on an existing
/// one.
fn forge_instances_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS forge_instances (
           host       TEXT PRIMARY KEY,
           kind       TEXT NOT NULL CHECK (kind IN ('github','gitlab','git_only')),
           scheme     TEXT NOT NULL DEFAULT 'https' CHECK (scheme IN ('http','https')),
           label      TEXT NOT NULL DEFAULT '',
           created_at TEXT NOT NULL
         );",
    )
    .map_err(|e| format!("could not create forge_instances: {e}"))
}

/// When each forge instance was last polled for open PRs, and what went wrong
/// if anything did.
///
/// Exists so the PR screen can paint from the database and still be honest
/// about how old what it's showing is. Without it, "no open PRs on this
/// instance" and "never managed to ask" look identical, which is the worst
/// thing a local-first screen can do.
fn pr_host_checks_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS pr_host_checks (
           host       TEXT PRIMARY KEY,
           checked_at TEXT NOT NULL,
           error      TEXT
         );",
    )
    .map_err(|e| format!("could not create pr_host_checks: {e}"))
}

/// The subscription's own email on each account.
///
/// The only identifier that means the same thing on two machines: `accounts.id`
/// is a UUID minted per install, and `name` is whatever the user typed there —
/// "David Personal 1" here, something else on the other Mac. Neither can join
/// "the same subscription" across installs, which is what a per-profile usage
/// report needs.
///
/// A plain column rather than a CHECK change, so no rebuild. Filled by the
/// login check, which already asks the CLI and receives the email in
/// `AuthStatus` — a field whose own comment says it was "simply discarded".
fn account_provider_email(conn: &Connection) -> Result<(), String> {
    if has_column(conn, "accounts", "provider_email")? {
        return Ok(());
    }
    conn.execute_batch("ALTER TABLE accounts ADD COLUMN provider_email TEXT NOT NULL DEFAULT '';")
        .map_err(|e| format!("could not add accounts.provider_email: {e}"))
}

/// Turns `turn_usage` from a child table into a ledger.
///
/// It hung off three parents with `ON DELETE CASCADE`, so spend history was
/// destroyed by ordinary use: deleting one account on the database this was
/// written against removed **3,331 of 5,798 rows — 57% of the history** — with
/// no warning, and deleting a session did the same for its share. A project
/// deletion was gentler and still lossy: `SET NULL` kept the row and threw away
/// which project it was, so every deleted project collapsed into one "no
/// project" bucket.
///
/// A ledger cannot work that way. What was spent happened, and no later edit to
/// the workspace can un-happen it. So:
///
/// - the names are **denormalized onto the row** at write time, because a name
///   resolved by `JOIN` at read time dies with its parent;
/// - the ids stay, downgraded to `SET NULL`, useful for joining against live
///   things and never again load-bearing for identity;
/// - and `machine_id`/`machine_name` are recorded too, so the same history can
///   later be told apart per machine — the reason `bridge::session` already
///   mints a stable machine id.
///
/// The 12-step rebuild, same as `allow_kimi_provider` and for the same reason:
/// SQLite cannot alter a foreign key. Existing rows keep their names by
/// resolving the joins one last time, while they still resolve.
fn turn_usage_is_a_ledger(conn: &Connection) -> Result<(), String> {
    let already: bool = conn
        .query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name='turn_usage'", [], |row| row.get::<_, String>(0))
        .map(|sql| sql.contains("project_name"))
        .unwrap_or(false);
    if already {
        return Ok(());
    }

    let _ = conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;");
    let result = conn.execute_batch(
        "BEGIN TRANSACTION;
         ALTER TABLE turn_usage RENAME TO turn_usage_pre_ledger;
         CREATE TABLE turn_usage (
           id                    TEXT PRIMARY KEY,
           session_id            TEXT REFERENCES sessions(id) ON DELETE SET NULL,
           account_id            TEXT REFERENCES accounts(id) ON DELETE SET NULL,
           project_id            TEXT REFERENCES projects(id) ON DELETE SET NULL,
           block_id              TEXT REFERENCES session_blocks(id) ON DELETE SET NULL,
           -- Written once, never resolved again. This is what survives the
           -- delete button.
           project_name          TEXT NOT NULL DEFAULT '',
           account_name          TEXT NOT NULL DEFAULT '',
           provider              TEXT NOT NULL DEFAULT '',
           machine_id            TEXT NOT NULL DEFAULT '',
           machine_name          TEXT NOT NULL DEFAULT '',
           model                 TEXT,
           provider_message_id   TEXT,
           input_tokens          INTEGER NOT NULL,
           output_tokens         INTEGER NOT NULL,
           cache_read_tokens     INTEGER,
           cache_creation_tokens INTEGER,
           cost_usd              REAL,
           created_at            TEXT NOT NULL,
           with_graph            INTEGER NOT NULL DEFAULT 0
         );
         INSERT INTO turn_usage (id, session_id, account_id, project_id, block_id,
                                 project_name, account_name, provider, machine_id, machine_name,
                                 model, provider_message_id, input_tokens, output_tokens,
                                 cache_read_tokens, cache_creation_tokens, cost_usd, created_at, with_graph)
           SELECT tu.id, tu.session_id, tu.account_id, tu.project_id, tu.block_id,
                  COALESCE(p.name, ''), COALESCE(a.name, ''), COALESCE(a.provider, ''), '', '',
                  tu.model, tu.provider_message_id, tu.input_tokens, tu.output_tokens,
                  tu.cache_read_tokens, tu.cache_creation_tokens, tu.cost_usd, tu.created_at, tu.with_graph
             FROM turn_usage_pre_ledger tu
             LEFT JOIN projects p ON p.id = tu.project_id
             LEFT JOIN accounts a ON a.id = tu.account_id;
         DROP TABLE turn_usage_pre_ledger;
         CREATE UNIQUE INDEX IF NOT EXISTS idx_turn_usage_message ON turn_usage(provider_message_id) WHERE provider_message_id IS NOT NULL;
         CREATE INDEX IF NOT EXISTS idx_turn_usage_account ON turn_usage(account_id, created_at);
         CREATE INDEX IF NOT EXISTS idx_turn_usage_session ON turn_usage(session_id);
         -- New: the report reads by time range across the whole ledger, which
         -- the account-prefixed index above cannot serve.
         CREATE INDEX IF NOT EXISTS idx_turn_usage_created ON turn_usage(created_at);
         COMMIT;",
    );
    let outcome = match result {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(format!("could not turn turn_usage into a ledger: {e}"))
        }
    };
    let _ = conn.execute_batch("PRAGMA legacy_alter_table=OFF; PRAGMA foreign_keys=ON;");
    outcome
}

/// Whether each account's provider login still works, as of when it was asked.
///
/// Not a column on `accounts`: `status` carries a
/// `CHECK (status IN ('healthy','cooling_down','exhausted'))`, and SQLite can't
/// widen a CHECK without rebuilding the table — a real risk to run against
/// everyone's database for a fact that isn't part of what an account *is*.
/// Being logged out is a property of the credential on disk, not of the row.
///
/// `error` is kept apart from `logged_in` on purpose: a probe that failed
/// because the CLI wasn't reachable is not the same as one that answered "no",
/// and treating them alike would raise the banner on a machine that is merely
/// offline.
fn account_login_checks_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS account_login_checks (
           account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
           checked_at TEXT NOT NULL,
           logged_in  INTEGER NOT NULL,
           error      TEXT
         );",
    )
    .map_err(|e| format!("could not create account_login_checks: {e}"))
}

/// Remembers which terminals were open so they can be reopened next launch.
/// Same shape `schema.sql` declares for a fresh database; `CREATE TABLE IF NOT
/// EXISTS` makes it a no-op there and creates it on an existing database.
fn open_terminals_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS open_terminals (
           id           TEXT PRIMARY KEY,
           target_kind  TEXT NOT NULL CHECK (target_kind IN ('project','session')),
           source_id    TEXT NOT NULL,
           account_id   TEXT,
           session_uuid TEXT,
           created_at   TEXT NOT NULL
         );",
    )
    .map_err(|e| format!("could not create open_terminals: {e}"))
}

/// Per-project git identity and credential reference. Same shape `schema.sql`
/// declares for a fresh database.
fn project_git_identity_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS project_git_identity (
           project_id     TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
           user_name      TEXT,
           user_email     TEXT,
           auth_user      TEXT,
           credential_ref TEXT,
           updated_at     TEXT NOT NULL
         );",
    )
    .map_err(|e| format!("could not create project_git_identity: {e}"))
}

/// Per-project overrides of the agent preferences that otherwise come from the
/// global defaults in `app_settings`.
///
/// A `NULL` column means **inherit**, which is why every one of them is
/// nullable and there is no companion "use the global" boolean: one column can
/// only be in one of the two states, so a second field could contradict it.
///
/// Deliberately not in `schema.sql` — that file is applied to existing
/// databases *before* any migration runs, so anything new declared there and
/// referenced by a migration takes startup down on older installs.
fn project_preferences_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS project_preferences (
           project_id            TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
           conversation_language TEXT,
           comment_language      TEXT
         );",
    )
    .map_err(|e| format!("could not create project_preferences: {e}"))
}

/// Which of the user's skills each project turns on.
///
/// Only the *selection* lives here. The skills themselves are real folders on
/// disk (`<app_data>/skills/<name>/SKILL.md`), because the Agent Skills
/// standard defines a skill as a folder that may also carry `scripts/`,
/// `references/` and `assets/` — a markdown column would close that door on
/// day one.
///
/// Keyed by name rather than by an id for the same reason: the folder name
/// *is* the skill's identity and its slash command. A row holding an id would
/// be a second census of the directory, and the two drift apart the moment
/// anyone touches the disk.
fn project_skills_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS project_skills (
           project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
           skill_name TEXT NOT NULL,
           PRIMARY KEY (project_id, skill_name)
         );",
    )
    .map_err(|e| format!("could not create project_skills: {e}"))
}

/// Skills that apply everywhere, without picking them project by project.
///
/// These reach the CLI by a different road than `project_skills`: they are
/// linked into each account's own config directory (`$CLAUDE_CONFIG_DIR/skills`,
/// `$CODEX_HOME/skills`, …), where every CLI already looks without needing a
/// flag. That is what makes them work on providers whose flags Hangar hasn't
/// verified.
///
/// No `account_id` column on purpose: "always on" means every account and every
/// project. Wanting a skill on the work account but not the personal one is a
/// third dimension nobody has asked for, and adding it later needs no migration
/// of what's here.
fn always_on_skills_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS always_on_skills (
           skill_name TEXT PRIMARY KEY
         );",
    )
    .map_err(|e| format!("could not create always_on_skills: {e}"))
}

/// GitHub repositories you can install skills from.
///
/// Same shape as `task_sources`: register a source, browse what's in it, pull
/// what you want. And like the GitHub task source, it holds no credential —
/// `gh` brings its own login, which is also why a private team repo works.
///
/// `repo` is UNIQUE and stored normalised to `owner/repo`, so pasting a URL and
/// typing the short form can't produce two entries for the same place.
/// Graphify knowledge graphs (`GRAPHIFY_TRD.md` §5.4).
///
/// A graph hangs off a **scope**, not a project: one project, or a group of
/// them. Phase 1 only writes `'project'`, but the column is here from the start
/// because adding it later means a second migration over a primary key.
///
/// `graph_commits` is a table rather than a column on `graphs` because a group
/// graph was built over N commits, one per member, and it is stale the moment
/// *any* of them moves (§5.5). One column could only ever record one of them.
///
/// Not in `schema.sql`, deliberately: that file is applied to existing
/// databases before any migration runs, so declaring something there and
/// referencing it from a migration takes startup down on older installs.
fn code_graphs_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS graphs (
           scope_kind  TEXT NOT NULL,
           scope_id    TEXT NOT NULL,
           graph_path  TEXT NOT NULL,
           built_at    TEXT,
           node_count  INTEGER NOT NULL DEFAULT 0,
           edge_count  INTEGER NOT NULL DEFAULT 0,
           status      TEXT NOT NULL DEFAULT 'never_built',
           last_error  TEXT,
           PRIMARY KEY (scope_kind, scope_id)
         );

         CREATE TABLE IF NOT EXISTS graph_commits (
           scope_kind  TEXT NOT NULL,
           scope_id    TEXT NOT NULL,
           project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
           commit_sha  TEXT NOT NULL,
           PRIMARY KEY (scope_kind, scope_id, project_id)
         );",
    )
    .map_err(|e| format!("could not create the graph tables: {e}"))
}

/// Whether a scope's graph may index documents, and with which model.
///
/// On `graphs` and not on `projects` and `graph_groups` separately: the setting
/// is per **scope**, which is exactly what this table is already keyed by, so
/// one pair of columns covers a project graph and a group graph instead of two
/// pairs that could disagree.
///
/// `'code_only'` as the default is the whole point — every existing row and
/// every new one keeps §3's promise that nothing leaves the machine, and only
/// an explicit change opts a scope out of it. `doc_backend` stays NULL until
/// someone picks one; `graph::DEFAULT_DOC_BACKEND` decides what NULL means, so
/// the answer can change without a migration.
///
/// Survives a rebuild because `mark_building` upserts only `status` and
/// `graph_path`.
fn graph_index_policy(conn: &Connection) -> Result<(), String> {
    if !has_column(conn, "graphs", "index_policy")? {
        conn.execute_batch("ALTER TABLE graphs ADD COLUMN index_policy TEXT NOT NULL DEFAULT 'code_only';")
            .map_err(|e| format!("could not add graphs.index_policy: {e}"))?;
    }
    if !has_column(conn, "graphs", "doc_backend")? {
        conn.execute_batch("ALTER TABLE graphs ADD COLUMN doc_backend TEXT;")
            .map_err(|e| format!("could not add graphs.doc_backend: {e}"))?;
    }
    Ok(())
}

/// Projects the user has marked as one system (`GRAPHIFY_TRD.md` §5.6).
///
/// The same shape as `memory_groups`, and deliberately **not** the same table.
/// "What context do these share?" and "what code gets indexed together?" are
/// two questions: coupling them would make changing a memory group trigger a
/// graph rebuild, and make one scope impossible without the other.
///
/// `bridge_count` is cached on the group because it is the number the list
/// screen shows for every group at once, and recomputing it means parsing a
/// multi-megabyte graph per row. `-1` distinguishes "never built" from a real,
/// meaningful zero — a group whose members share nothing (§5.7).
fn graph_groups_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS graph_groups (
           id           TEXT PRIMARY KEY,
           name         TEXT NOT NULL,
           bridge_count INTEGER NOT NULL DEFAULT -1,
           created_at   TEXT NOT NULL
         );

         CREATE TABLE IF NOT EXISTS graph_group_projects (
           group_id   TEXT NOT NULL REFERENCES graph_groups(id) ON DELETE CASCADE,
           project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
           PRIMARY KEY (group_id, project_id)
         );",
    )
    .map_err(|e| format!("could not create the graph group tables: {e}"))
}

/// Whether a turn ran with a graph available (`GRAPHIFY_TRD.md` Phase 3).
///
/// One column, on the table that already records what every turn cost in
/// input, output, cache and dollars. That is the whole point of Differential
/// B: the saving is *measured against the user's own work*, not asserted — and
/// measuring it needs nothing more than knowing which side of the line each
/// turn fell on.
fn turn_usage_graph_marker(conn: &Connection) -> Result<(), String> {
    if has_column(conn, "turn_usage", "with_graph")? {
        return Ok(());
    }
    conn.execute("ALTER TABLE turn_usage ADD COLUMN with_graph INTEGER NOT NULL DEFAULT 0", [])
        .map(|_| ())
        .map_err(|e| format!("could not add turn_usage.with_graph: {e}"))
}

fn skill_sources_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS skill_sources (
           id         TEXT PRIMARY KEY,
           repo       TEXT NOT NULL UNIQUE,
           label      TEXT NOT NULL,
           created_at TEXT NOT NULL
         );",
    )
    .map_err(|e| format!("could not create skill_sources: {e}"))
}

/// Makes `turn_usage` able to record what a **terminal** spent.
///
/// It was built for headless turns, so `session_id` was NOT NULL — but a
/// terminal opened on a project has no session row, and that is where the real
/// work happens. It also had no idea which account paid, which is the only
/// question anyone actually asks of it.
///
/// `provider_message_id` is the important one: the numbers come from re-reading
/// a transcript the CLI is appending to, and without a unique key a restart
/// would count yesterday's turns again and quietly double the week's spend.
fn turn_usage_from_terminals(conn: &Connection) -> Result<(), String> {
    let already: bool = conn
        .query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name='turn_usage'", [], |row| row.get::<_, String>(0))
        .map(|sql| sql.contains("provider_message_id"))
        .unwrap_or(false);
    // The indexes are created below whichever path this takes: a fresh database
    // gets the new shape from `schema.sql` and never rebuilds, so creating them
    // only inside the rebuild would leave new installs without the unique index
    // that stops a re-read from double-counting.
    if already {
        return ensure_turn_usage_indexes(conn);
    }

    // A modern ALTER ... RENAME rewrites *other* tables' foreign keys to follow
    // the rename, leaving them pointing at a table this is about to drop. Same
    // trap migration 4 fell into.
    let _ = conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;");
    let result = (|| -> Result<(), String> {
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE turn_usage_new (
               id                    TEXT PRIMARY KEY,
               session_id            TEXT REFERENCES sessions(id) ON DELETE CASCADE,
               account_id            TEXT REFERENCES accounts(id) ON DELETE CASCADE,
               project_id            TEXT REFERENCES projects(id) ON DELETE SET NULL,
               block_id              TEXT REFERENCES session_blocks(id) ON DELETE SET NULL,
               model                 TEXT,
               provider_message_id   TEXT,
               input_tokens          INTEGER NOT NULL,
               output_tokens         INTEGER NOT NULL,
               cache_read_tokens     INTEGER,
               cache_creation_tokens INTEGER,
               cost_usd              REAL,
               created_at            TEXT NOT NULL
             );
             INSERT INTO turn_usage_new (id, session_id, account_id, project_id, block_id, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, cost_usd, created_at)
               SELECT tu.id, tu.session_id, s.account_id, s.project_id, tu.block_id, tu.input_tokens, tu.output_tokens, tu.cache_read_tokens, tu.cache_creation_tokens, tu.cost_usd, tu.created_at
               FROM turn_usage tu LEFT JOIN sessions s ON s.id = tu.session_id;
             DROP TABLE turn_usage;
             ALTER TABLE turn_usage_new RENAME TO turn_usage;
             COMMIT;",
        )
        .map_err(|e| format!("could not rebuild turn_usage: {e}"))
    })();
    let _ = conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA legacy_alter_table=OFF;");
    result?;
    ensure_turn_usage_indexes(conn)
}

fn ensure_turn_usage_indexes(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_turn_usage_message ON turn_usage(provider_message_id) WHERE provider_message_id IS NOT NULL;
         CREATE INDEX IF NOT EXISTS idx_turn_usage_account ON turn_usage(account_id, created_at);",
    )
    .map_err(|e| format!("could not index turn_usage: {e}"))
}

/// El dispositivo móvil emparejado con este Mac. Misma forma que declara
/// `schema.sql` para una base nueva.
fn paired_device_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS paired_device (
           id             TEXT PRIMARY KEY,
           name           TEXT NOT NULL,
           level          TEXT NOT NULL CHECK (level IN ('watch','steer','launch')),
           credential_ref TEXT NOT NULL,
           paired_at      TEXT NOT NULL,
           last_seen_at   TEXT,
           push_token     TEXT
         );",
    )
    .map_err(|e| format!("could not create paired_device: {e}"))
}

/// Where task rows come from: a configured source, and which project each one
/// feeds. See `schema.sql` for why the per-tool detail lives in `config_json`
/// instead of a column per tool.
fn task_sources_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_sources (
           id             TEXT PRIMARY KEY,
           kind           TEXT NOT NULL CHECK (kind IN ('github', 'clickup')),
           label          TEXT NOT NULL,
           credential_ref TEXT,
           config_json    TEXT NOT NULL DEFAULT '{}',
           created_at     TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS task_source_links (
           project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
           source_id       TEXT NOT NULL REFERENCES task_sources(id) ON DELETE CASCADE,
           scope_id        TEXT,
           branch_template TEXT,
           PRIMARY KEY (project_id, source_id)
         );",
    )
    .map_err(|e| format!("could not create the task source tables: {e}"))
}

/// Adds `kimi` to the providers an account may have.
///
/// It's a `CHECK` constraint, and SQLite can't alter one — the only way is the
/// 12-step rebuild. Same `legacy_alter_table` care as migration 4: `sessions`
/// and several other tables reference `accounts`, and a modern rename would
/// rewrite their foreign keys to follow it and leave them pointing at a table
/// that's about to be dropped.
fn allow_kimi_provider(conn: &Connection) -> Result<(), String> {
    let already: bool = conn
        .query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name='accounts'", [], |row| row.get::<_, String>(0))
        .map(|sql| sql.contains("'kimi'"))
        .unwrap_or(false);
    if already {
        return Ok(());
    }

    let _ = conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;");
    let result = conn.execute_batch(
        "BEGIN TRANSACTION;
         ALTER TABLE accounts RENAME TO accounts_old_provider_migration;
         CREATE TABLE accounts (
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
           exhausted_at     TEXT,
           team_account_id  TEXT
         );
         INSERT INTO accounts SELECT id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, monthly_cap, status, created_at, updated_at, exhausted_at, team_account_id
           FROM accounts_old_provider_migration;
         DROP TABLE accounts_old_provider_migration;

         ALTER TABLE rotation_policies RENAME TO rotation_policies_old_provider_migration;
         CREATE TABLE rotation_policies (
           project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
           provider   TEXT NOT NULL CHECK (provider IN ('claude','codex','gemini','kimi')),
           policy     TEXT NOT NULL DEFAULT 'manual' CHECK (policy IN ('auto','round_robin','manual')),
           PRIMARY KEY (project_id, provider)
         );
         INSERT INTO rotation_policies SELECT project_id, provider, policy FROM rotation_policies_old_provider_migration;
         DROP TABLE rotation_policies_old_provider_migration;
         COMMIT;",
    );
    let outcome = match result {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(format!("could not widen the provider constraint: {e}"))
        }
    };
    let _ = conn.execute_batch("PRAGMA legacy_alter_table=OFF; PRAGMA foreign_keys=ON;");
    outcome
}

/// Adds `grok` to the providers an account may have — same 12-step rebuild as
/// `allow_kimi_provider`, for the same reason (SQLite can't alter a `CHECK`).
fn allow_grok_provider(conn: &Connection) -> Result<(), String> {
    let already: bool = conn
        .query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name='accounts'", [], |row| row.get::<_, String>(0))
        .map(|sql| sql.contains("'grok'"))
        .unwrap_or(false);
    if already {
        return Ok(());
    }

    let _ = conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;");
    let result = conn.execute_batch(
        "BEGIN TRANSACTION;
         ALTER TABLE accounts RENAME TO accounts_old_provider_migration;
         CREATE TABLE accounts (
           id               TEXT PRIMARY KEY,
           provider         TEXT NOT NULL CHECK (provider IN ('claude','codex','gemini','kimi','grok')),
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
           exhausted_at     TEXT,
           team_account_id  TEXT
         );
         INSERT INTO accounts SELECT id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, monthly_cap, status, created_at, updated_at, exhausted_at, team_account_id
           FROM accounts_old_provider_migration;
         DROP TABLE accounts_old_provider_migration;

         ALTER TABLE rotation_policies RENAME TO rotation_policies_old_provider_migration;
         CREATE TABLE rotation_policies (
           project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
           provider   TEXT NOT NULL CHECK (provider IN ('claude','codex','gemini','kimi','grok')),
           policy     TEXT NOT NULL DEFAULT 'manual' CHECK (policy IN ('auto','round_robin','manual')),
           PRIMARY KEY (project_id, provider)
         );
         INSERT INTO rotation_policies SELECT project_id, provider, policy FROM rotation_policies_old_provider_migration;
         DROP TABLE rotation_policies_old_provider_migration;
         COMMIT;",
    );
    let outcome = match result {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(format!("could not widen the provider constraint: {e}"))
        }
    };
    let _ = conn.execute_batch("PRAGMA legacy_alter_table=OFF; PRAGMA foreign_keys=ON;");
    outcome
}

/// Whether a restored terminal comes back folded.
///
/// The state existed before this column, in `localStorage` keyed by terminal id
/// — and it could never have worked: `restore_terminals` drops the rows and
/// reopens each terminal with a *fresh* id, so every key in that map pointed at
/// something that no longer existed and all panels came back expanded. Keeping
/// it next to the row that survives the restart is the fix; the key is the
/// problem, not the storage.
///
/// Additive, so no table rebuild: there is no `CHECK` to alter here, unlike the
/// provider constraint above.
fn open_terminal_minimized(conn: &Connection) -> Result<(), String> {
    if has_column(conn, "open_terminals", "minimized")? {
        return Ok(());
    }
    conn.execute_batch("ALTER TABLE open_terminals ADD COLUMN minimized INTEGER NOT NULL DEFAULT 0;")
        .map_err(|e| format!("could not add open_terminals.minimized: {e}"))
}

/// Adds `opencode` to the providers an account may have.
///
/// The same 12-step rebuild as `allow_grok_provider`, for the same reason:
/// SQLite cannot alter a `CHECK`, so the table is recreated and copied. Both
/// tables carry the constraint, and missing the second one is the interesting
/// failure — accounts would save fine and rotation would refuse the provider
/// later, far from the cause.
fn allow_opencode_provider(conn: &Connection) -> Result<(), String> {
    let already: bool = conn
        .query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name='accounts'", [], |row| row.get::<_, String>(0))
        .map(|sql| sql.contains("'opencode'"))
        .unwrap_or(false);
    if already {
        return Ok(());
    }

    let _ = conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;");
    let result = conn.execute_batch(
        "BEGIN TRANSACTION;
         ALTER TABLE accounts RENAME TO accounts_old_provider_migration;
         CREATE TABLE accounts (
           id               TEXT PRIMARY KEY,
           provider         TEXT NOT NULL CHECK (provider IN ('claude','codex','opencode','gemini','kimi','grok')),
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
           exhausted_at     TEXT,
           team_account_id  TEXT,
           provider_email   TEXT NOT NULL DEFAULT ''
         );
         INSERT INTO accounts SELECT id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, monthly_cap, status, created_at, updated_at, exhausted_at, team_account_id, provider_email
           FROM accounts_old_provider_migration;
         DROP TABLE accounts_old_provider_migration;

         ALTER TABLE rotation_policies RENAME TO rotation_policies_old_provider_migration;
         CREATE TABLE rotation_policies (
           project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
           provider   TEXT NOT NULL CHECK (provider IN ('claude','codex','opencode','gemini','kimi','grok')),
           policy     TEXT NOT NULL DEFAULT 'manual' CHECK (policy IN ('auto','round_robin','manual')),
           PRIMARY KEY (project_id, provider)
         );
         INSERT INTO rotation_policies SELECT project_id, provider, policy FROM rotation_policies_old_provider_migration;
         DROP TABLE rotation_policies_old_provider_migration;
         COMMIT;",
    );
    let outcome = match result {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(format!("could not widen the provider constraint: {e}"))
        }
    };
    let _ = conn.execute_batch("PRAGMA legacy_alter_table=OFF; PRAGMA foreign_keys=ON;");
    outcome
}

/// Per-project spending caps, and the link from a local account to its row in
/// a shared team pool. Both are additive; neither changes existing behaviour
/// until something is configured.
fn quota_budgets_and_team_accounts(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS quota_budgets (
           project_id  TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
           weekly_pct  REAL NOT NULL CHECK (weekly_pct > 0 AND weekly_pct <= 100),
           updated_at  TEXT NOT NULL
         );",
    )
    .map_err(|e| format!("could not create quota_budgets: {e}"))?;
    if !has_column(conn, "accounts", "team_account_id")? {
        conn.execute_batch("ALTER TABLE accounts ADD COLUMN team_account_id TEXT;")
            .map_err(|e| format!("could not add accounts.team_account_id: {e}"))?;
    }
    Ok(())
}

/// Work that waits for quota instead of failing against it. Same shape
/// `schema.sql` declares for a fresh database.
fn queued_runs_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS queued_runs (
           id           TEXT PRIMARY KEY,
           project_id   TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
           prompt       TEXT NOT NULL,
           branch_name  TEXT NOT NULL,
           account_id   TEXT REFERENCES accounts(id) ON DELETE SET NULL,
           state        TEXT NOT NULL DEFAULT 'queued' CHECK (state IN ('queued','running','done','failed','cancelled')),
           terminal_id  TEXT,
           session_id   TEXT REFERENCES sessions(id) ON DELETE SET NULL,
           attempts     INTEGER NOT NULL DEFAULT 0,
           last_error   TEXT,
           created_at   TEXT NOT NULL,
           started_at   TEXT,
           finished_at  TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_queued_runs_state ON queued_runs(state, created_at);",
    )
    .map_err(|e| format!("could not create queued_runs: {e}"))
}

/// Lets a panel carry a name the user typed. Needed once terminals are listed
/// in a sidebar: two panels on the same project read identically there.
fn open_terminals_display_name(conn: &Connection) -> Result<(), String> {
    if has_column(conn, "open_terminals", "display_name")? {
        return Ok(());
    }
    conn.execute_batch("ALTER TABLE open_terminals ADD COLUMN display_name TEXT;")
        .map_err(|e| format!("could not add open_terminals.display_name: {e}"))
}

/// Keeps the `/usage` readings the poller was already taking. Same shape
/// `schema.sql` declares for a fresh database.
fn account_usage_samples_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS account_usage_samples (
           id           TEXT PRIMARY KEY,
           account_id   TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
           session_pct  REAL,
           weekly_pct   REAL,
           context_pct  REAL,
           sampled_at   TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_account_usage_samples ON account_usage_samples(account_id, sampled_at);",
    )
    .map_err(|e| format!("could not create account_usage_samples: {e}"))
}

/// Re-anchors `pull_requests` on the project instead of a session, so a PR
/// opened by *someone else* can be tracked at all.
///
/// Two things made that impossible: `session_id` was `NOT NULL`, and the inbox
/// reached the project by joining through `sessions`. A PR of another person's
/// has no session, and inventing one (which discovery used to do — "the
/// project's most recent session", an apaño its own doc comment admitted to)
/// puts a stranger's PR under a session that never touched it.
///
/// SQLite can't relax a NOT NULL or change an FK action in place, so this is
/// the standard 12-step rebuild, same as `migrate_sessions_account_fk_to_cascade`.
/// Existing rows keep everything they had: `project_id` is backfilled through
/// the session they were anchored to, and `relation` becomes `mine` — which is
/// exactly what they all were, since discovery only ever looked for
/// `--author @me`.
///
/// A row whose session vanished (an FK left dangling by an older, buggier
/// schema) has no project to backfill from and is dropped: it can't be shown on
/// any screen, and re-discovery will bring the PR back properly if it's still
/// open.
fn pull_requests_project_anchored(conn: &Connection) -> Result<(), String> {
    if has_column(conn, "pull_requests", "project_id")? {
        return Ok(());
    }

    // `pr_review_comments` references `pull_requests`, and a modern
    // `ALTER TABLE ... RENAME` rewrites other tables' FKs to follow the rename —
    // which is exactly how `repair_dangling_sessions_fk` came to exist. With
    // `legacy_alter_table` on, the rename touches nothing but the renamed table,
    // and the new `pull_requests` created right after slots straight back in
    // under the name the FK still names. Both PRAGMAs are no-ops inside a
    // transaction, so they sit outside it.
    let _ = conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;");
    let result = conn.execute_batch(
        "BEGIN TRANSACTION;
         ALTER TABLE pull_requests RENAME TO pull_requests_old_anchor_migration;
         CREATE TABLE pull_requests (
           id                    TEXT PRIMARY KEY,
           project_id            TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
           session_id            TEXT REFERENCES sessions(id) ON DELETE SET NULL,
           repo_owner            TEXT NOT NULL,
           repo_name             TEXT NOT NULL,
           number                INTEGER NOT NULL,
           url                   TEXT NOT NULL,
           state                 TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open','closed','merged')),
           title                 TEXT,
           author_login          TEXT,
           relation              TEXT NOT NULL DEFAULT 'mine' CHECK (relation IN ('mine','review','other')),
           last_comment_seen_at  TEXT,
           last_check_error      TEXT,
           last_seen_updated_at  TEXT,
           head_branch           TEXT,
           base_branch           TEXT,
           created_at            TEXT NOT NULL
         );
         INSERT INTO pull_requests (id, project_id, session_id, repo_owner, repo_name, number, url, state, relation, last_comment_seen_at, last_check_error, last_seen_updated_at, head_branch, created_at)
           SELECT pr.id, s.project_id, pr.session_id, pr.repo_owner, pr.repo_name, pr.number, pr.url, pr.state, 'mine', pr.last_comment_seen_at, pr.last_check_error, pr.last_seen_updated_at, pr.head_branch, pr.created_at
           FROM pull_requests_old_anchor_migration pr
           JOIN sessions s ON s.id = pr.session_id;
         DROP TABLE pull_requests_old_anchor_migration;
         CREATE INDEX IF NOT EXISTS idx_pull_requests_repo_number ON pull_requests(repo_owner, repo_name, number);
         COMMIT;",
    );
    let outcome = match result {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(format!("could not re-anchor pull_requests on the project: {e}"))
        }
    };
    let _ = conn.execute_batch("PRAGMA legacy_alter_table=OFF; PRAGMA foreign_keys=ON;");
    outcome
}

/// Applies every migration newer than the database's recorded
/// `user_version`, then records the new version — all inside one transaction
/// per migration, so a failure leaves the database exactly where it was
/// rather than half-migrated with a version that lies about it.
///
/// Replaces an ad-hoc "run every ALTER TABLE on every startup and ignore all
/// errors" loop. That worked only because *duplicate column name* happened to
/// be the expected steady-state error, which meant a genuinely broken
/// migration was indistinguishable from an already-applied one — and it gave
/// no way to add a migration that isn't naturally idempotent.
pub(crate) fn run_migrations(conn: &Connection) -> Result<(), String> {
    apply(conn, MIGRATIONS)
}

/// The runner itself, taking the migration list as a parameter so tests can
/// drive it with deliberately broken migrations without touching the real
/// `MIGRATIONS` table.
fn apply(conn: &Connection, migrations: &[Migration]) -> Result<(), String> {
    let current: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| format!("could not read schema version: {e}"))?;

    for migration in migrations {
        if migration.version <= current {
            continue;
        }
        log::info!("applying migration {} ({})", migration.version, migration.name);

        if migration.transactional {
            conn.execute_batch("BEGIN TRANSACTION;").map_err(|e| e.to_string())?;
        }
        let result = (migration.run)(conn).and_then(|()| {
            // `user_version` lives in the database header and participates in
            // the surrounding transaction, so version and schema commit or
            // roll back together — they can never disagree.
            conn.execute_batch(&format!("PRAGMA user_version = {};", migration.version))
                .map_err(|e| format!("could not record schema version: {e}"))
        });

        match result {
            Ok(()) => {
                if migration.transactional {
                    conn.execute_batch("COMMIT;").map_err(|e| e.to_string())?;
                }
            }
            Err(e) => {
                if migration.transactional {
                    let _ = conn.execute_batch("ROLLBACK;");
                }
                log::error!("migration {} ({}) failed: {e}", migration.version, migration.name);
                return Err(format!("migration {} ({}) failed: {e}", migration.version, migration.name));
            }
        }
    }
    Ok(())
}

/// Everything that shipped before schema versioning existed, collapsed into
/// migration 1. Deliberately keeps the old "ignore every error" behaviour:
/// these statements ran unconditionally on every startup for the whole life
/// of the project, so a database out there may be in any state between "none
/// of this applied" and "all of it applied", and *duplicate column name* is a
/// legitimate outcome rather than a failure. Every migration added from here
/// on runs exactly once and reports its errors for real.
fn legacy_baseline(conn: &Connection) -> Result<(), String> {
    let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN provider_session_id TEXT;");
    let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN name TEXT NOT NULL DEFAULT '';");
    let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN effort TEXT NOT NULL DEFAULT 'high';");
    let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN parent_session_id TEXT REFERENCES sessions(id) ON DELETE CASCADE;");
    let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN worktree_path TEXT;");
    let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN worktree_branch TEXT;");
    let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN worktree_kind TEXT;");
    let _ = conn.execute_batch("ALTER TABLE projects ADD COLUMN default_setup_command TEXT;");
    let _ = conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_session_id);");
    let _ = conn.execute_batch("ALTER TABLE pr_review_comments ADD COLUMN path TEXT;");
    let _ = conn.execute_batch("ALTER TABLE pr_review_comments ADD COLUMN line INTEGER;");
    let _ = conn.execute_batch("ALTER TABLE pr_review_comments ADD COLUMN external_id TEXT;");
    let _ = conn.execute_batch("ALTER TABLE pull_requests ADD COLUMN last_check_error TEXT;");
    let _ = conn.execute_batch("ALTER TABLE pull_requests ADD COLUMN last_seen_updated_at TEXT;");
    let _ = conn.execute_batch("ALTER TABLE pull_requests ADD COLUMN head_branch TEXT;");
    migrate_sessions_account_fk_to_cascade(conn);
    repair_dangling_sessions_fk(conn);
    Ok(())
}

/// SQLite silently rewrites *other* tables' foreign-key definitions to
/// follow a table through `ALTER TABLE ... RENAME TO`. The FK-cascade
/// migration above renames `sessions` to a temp name and back — which
/// means `session_blocks`/`git_checkpoints` (both `REFERENCES sessions`)
/// got their FK silently repointed at the temp name, and never got
/// rewritten back once that temp table was dropped and a fresh `sessions`
/// table took its place. Left unfixed, every insert into those tables
/// fails with "no such table: main.sessions_old_fk_migration" — this is
/// what actually caused every earlier "message doesn't save" symptom this
/// project hit, not a one-off crash. Detects and rebuilds any table still
/// pointing at the dead name; safe/no-op once repaired.
fn repair_dangling_sessions_fk(conn: &Connection) {
    const GHOST: &str = "sessions_old_fk_migration";

    let references_ghost = |table: &str| -> bool {
        conn.query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |row| row.get::<_, String>(0),
        )
        .map(|sql| sql.contains(GHOST))
        .unwrap_or(false)
    };

    let needs_session_blocks = references_ghost("session_blocks");
    let needs_git_checkpoints = references_ghost("git_checkpoints");
    if !needs_session_blocks && !needs_git_checkpoints {
        return;
    }

    let _ = conn.execute_batch("PRAGMA foreign_keys=OFF;");

    if needs_session_blocks {
        let result = conn.execute_batch(
            "BEGIN TRANSACTION;
             ALTER TABLE session_blocks RENAME TO session_blocks_fk_repair;
             CREATE TABLE session_blocks (
               id         TEXT PRIMARY KEY,
               session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
               kind       TEXT NOT NULL CHECK (kind IN ('you','think','edit','cmd','agent','sum','rev','out')),
               text       TEXT NOT NULL,
               created_at TEXT NOT NULL
             );
             INSERT INTO session_blocks SELECT id, session_id, kind, text, created_at FROM session_blocks_fk_repair;
             DROP TABLE session_blocks_fk_repair;
             CREATE INDEX IF NOT EXISTS idx_session_blocks_session ON session_blocks(session_id);
             COMMIT;",
        );
        if let Err(e) = result {
            log::error!("repair_dangling_sessions_fk: session_blocks repair failed, rolling back: {e}");
            let _ = conn.execute_batch("ROLLBACK;");
        }
    }

    if needs_git_checkpoints {
        let result = conn.execute_batch(
            "BEGIN TRANSACTION;
             ALTER TABLE git_checkpoints RENAME TO git_checkpoints_fk_repair;
             CREATE TABLE git_checkpoints (
               id          TEXT PRIMARY KEY,
               session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
               turn_number INTEGER NOT NULL,
               commit_ref  TEXT NOT NULL,
               created_at  TEXT NOT NULL
             );
             INSERT INTO git_checkpoints SELECT id, session_id, turn_number, commit_ref, created_at FROM git_checkpoints_fk_repair;
             DROP TABLE git_checkpoints_fk_repair;
             COMMIT;",
        );
        if let Err(e) = result {
            log::error!("repair_dangling_sessions_fk: git_checkpoints repair failed, rolling back: {e}");
            let _ = conn.execute_batch("ROLLBACK;");
        }
    }

    let _ = conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA legacy_alter_table=OFF;");
}

/// Early dev builds created `sessions.account_id` with `ON DELETE RESTRICT`,
/// which silently blocked deleting any account that ever had a session —
/// rebuilds the table in place (SQLite's standard "12-step" schema-change
/// procedure) so existing databases pick up the `ON DELETE CASCADE` fix
/// without losing data. No-ops once already migrated.
fn migrate_sessions_account_fk_to_cascade(conn: &Connection) {
    let needs_migration: bool = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='sessions'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map(|sql| sql.contains("ON DELETE RESTRICT"))
        .unwrap_or(false);
    if !needs_migration {
        return;
    }

    // The ADD COLUMN migrations for name/effort/provider_session_id/
    // parent_session_id/worktree_* already ran (see run_migrations) by the
    // time this executes, so the old table being rebuilt here already has
    // them — carry them over rather than silently dropping that data.
    // PRAGMAs are no-ops inside a transaction, so these run outside the
    // BEGIN/COMMIT that does the actual table rebuild.
    //
    // `legacy_alter_table` is not optional here, and its absence was a real
    // latent bug: a modern `ALTER TABLE ... RENAME` rewrites *other* tables'
    // foreign keys to follow the rename, so `turn_usage.session_id` came out
    // pointing at `sessions_old_fk_migration` — a table this very statement is
    // about to drop. Nothing noticed until deleting an account cascaded into
    // `turn_usage` and hit "no such table". Migration 4 learned this the same
    // way; this call site never got the fix.
    let _ = conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;");
    let result = conn.execute_batch(
        "BEGIN TRANSACTION;
         ALTER TABLE sessions RENAME TO sessions_old_fk_migration;
         CREATE TABLE sessions (
           id                   TEXT PRIMARY KEY,
           name                 TEXT NOT NULL DEFAULT '',
           project_id           TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
           node_id              TEXT REFERENCES agent_nodes(id) ON DELETE SET NULL,
           parent_session_id    TEXT REFERENCES sessions(id) ON DELETE CASCADE,
           worktree_path        TEXT,
           worktree_branch      TEXT,
           worktree_kind        TEXT CHECK (worktree_kind IN ('linked','copy')),
           account_id           TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
           model                TEXT NOT NULL,
           effort               TEXT NOT NULL DEFAULT 'high' CHECK (effort IN ('low','medium','high','xhigh','max')),
           mode                 TEXT NOT NULL DEFAULT 'ask_edits' CHECK (mode IN ('plan','ask_edits','auto_edit','yolo')),
           status               TEXT NOT NULL DEFAULT 'waiting' CHECK (status IN ('waiting','running','reviewing','done')),
           pid                  INTEGER,
           provider_session_id  TEXT,
           started_at           TEXT NOT NULL,
           ended_at             TEXT
         );
         INSERT INTO sessions (id, name, project_id, node_id, parent_session_id, worktree_path, worktree_branch, worktree_kind, account_id, model, effort, mode, status, pid, provider_session_id, started_at, ended_at)
           SELECT id, name, project_id, node_id, parent_session_id, worktree_path, worktree_branch, worktree_kind, account_id, model, effort, mode, status, pid, provider_session_id, started_at, ended_at
           FROM sessions_old_fk_migration;
         DROP TABLE sessions_old_fk_migration;
         CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_id);
         CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_session_id);
         COMMIT;",
    );
    if let Err(e) = result {
        log::error!("sessions FK migration failed, rolling back: {e}");
        let _ = conn.execute_batch("ROLLBACK;");
    }
    let _ = conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA legacy_alter_table=OFF;");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_version(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap()
    }

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn
    }

    /// Spend that happened stays happened, whatever gets deleted afterwards.
    ///
    /// This is the whole point of migration 26. `turn_usage` used to hang off
    /// three parents with `ON DELETE CASCADE`, and on a real database deleting
    /// **one account removed 3,331 of 5,798 rows — 57% of the history** — with
    /// no warning. A project deletion was subtler and still lossy: the row
    /// survived and lost its name, so every deleted project collapsed into one
    /// anonymous bucket.
    #[test]
    fn deleting_a_project_or_an_account_never_deletes_what_it_spent() {
        let conn = fresh_db();
        conn.execute_batch(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
               VALUES ('acc','claude','sub','Personal','personal','','/tmp/x','acc','healthy','now','now');
             INSERT INTO projects (id, name, path, created_at) VALUES ('proj','Fleet','/tmp/fleet','now');
             INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, started_at)
               VALUES ('sess','s','proj','acc','opus','high','ask_edits','waiting','now');
             INSERT INTO turn_usage (id, session_id, account_id, project_id, project_name, account_name, provider,
                                     input_tokens, output_tokens, cost_usd, created_at)
               VALUES ('t1','sess','acc','proj','Fleet','Personal','claude', 10, 5, 1.25, '2026-08-01T10:00:00Z');",
        )
        .unwrap();

        // The three deletions a user can perform, in the order that used to be
        // most destructive.
        conn.execute_batch("DELETE FROM sessions WHERE id='sess'; DELETE FROM projects WHERE id='proj'; DELETE FROM accounts WHERE id='acc';").unwrap();

        let (rows, project, account, cost): (i64, String, String, f64) = conn
            .query_row("SELECT COUNT(*), MAX(project_name), MAX(account_name), MAX(cost_usd) FROM turn_usage", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap();

        assert_eq!(rows, 1, "the ledger line must survive every parent being deleted");
        assert_eq!(project, "Fleet", "and still know which project spent it");
        assert_eq!(account, "Personal", "and which account paid");
        assert_eq!(cost, 1.25);

        // The ids are released, which is what lets the report mark the row as
        // archived rather than pretending the project is still there.
        let orphaned: i64 = conn
            .query_row("SELECT COUNT(*) FROM turn_usage WHERE project_id IS NULL AND account_id IS NULL AND session_id IS NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(orphaned, 1, "the links go, the facts stay");
    }

    #[test]
    fn migrations_record_the_latest_version_and_are_idempotent() {
        let conn = fresh_db();
        assert_eq!(schema_version(&conn), 0, "a brand new database starts unversioned");

        run_migrations(&conn).unwrap();
        let latest = MIGRATIONS.last().unwrap().version;
        assert_eq!(schema_version(&conn), latest);

        // The whole point of the version stamp: a second run does nothing.
        run_migrations(&conn).unwrap();
        assert_eq!(schema_version(&conn), latest);
    }

    /// The append-only rule in `MIGRATIONS`' doc comment is only enforceable
    /// if versions are actually strictly increasing and start at 1 — a
    /// duplicate or out-of-order entry would silently skip a migration on
    /// every database that had already passed that number.
    #[test]
    fn migration_versions_are_strictly_increasing_from_one() {
        let mut expected = 1;
        for migration in MIGRATIONS {
            assert_eq!(migration.version, expected, "migration '{}' is out of order", migration.name);
            expected += 1;
        }
    }

    /// Both upgrade paths for `accounts.exhausted_at`: a database created before
    /// the column existed has to gain it, and a fresh one — where `schema.sql`
    /// already declared it — has to survive the same migration running anyway,
    /// which is what every install does on its first launch.
    #[test]
    fn the_exhausted_at_migration_adds_the_column_and_tolerates_it_already_existing() {
        let legacy = Connection::open_in_memory().unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE accounts (
                   id TEXT PRIMARY KEY, provider TEXT NOT NULL, billing TEXT NOT NULL, name TEXT NOT NULL,
                   scope TEXT NOT NULL, plan_label TEXT NOT NULL DEFAULT '', config_dir_path TEXT NOT NULL,
                   credential_ref TEXT NOT NULL, monthly_cap REAL, status TEXT NOT NULL DEFAULT 'healthy',
                   created_at TEXT NOT NULL, updated_at TEXT NOT NULL
                 );",
            )
            .unwrap();
        assert!(!has_column(&legacy, "accounts", "exhausted_at").unwrap(), "the fixture has to start without the column for this to prove anything");

        accounts_exhausted_at(&legacy).unwrap();
        assert!(has_column(&legacy, "accounts", "exhausted_at").unwrap(), "an existing database must gain the column");

        let fresh = fresh_db();
        assert!(has_column(&fresh, "accounts", "exhausted_at").unwrap(), "schema.sql must declare it for new databases");
        accounts_exhausted_at(&fresh).expect("re-running where schema.sql already added it must be a no-op, not a startup failure");
    }

    /// The table has to appear on a database that predates it, survive the
    /// migration running again, and be a no-op on a fresh database where
    /// `schema.sql` already declared it — the three shapes every table
    /// migration here has to hold.
    #[test]
    fn the_forge_instances_table_is_created_once_and_then_idempotent() {
        let legacy = Connection::open_in_memory().unwrap();
        let declared = |conn: &Connection| {
            conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='forge_instances'", [], |r| r.get::<_, i64>(0)).unwrap() == 1
        };
        assert!(!declared(&legacy), "the fixture has to start without it for this to prove anything");

        forge_instances_table(&legacy).unwrap();
        assert!(declared(&legacy));
        forge_instances_table(&legacy).expect("running it twice must not fail startup");

        let fresh = Connection::open_in_memory().unwrap();
        fresh.execute_batch(SCHEMA).unwrap();
        assert!(declared(&fresh), "schema.sql must declare it for new databases");
        forge_instances_table(&fresh).expect("a no-op where schema.sql already created it");
    }

    /// `git_only` is a declaration the CHECK constraint has to accept — it is
    /// the whole reason the column isn't just the two forges — and anything
    /// outside the three has to be refused rather than stored and later
    /// misread.
    #[test]
    fn forge_instances_accepts_the_three_kinds_and_nothing_else() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();

        for kind in ["github", "gitlab", "git_only"] {
            conn.execute(
                "INSERT INTO forge_instances (host, kind, created_at) VALUES (?1, ?2, 'now')",
                rusqlite::params![format!("{kind}.example.com"), kind],
            )
            .unwrap_or_else(|e| panic!("{kind} must be storable: {e}"));
        }

        conn.execute("INSERT INTO forge_instances (host, kind, created_at) VALUES ('x.example.com','bitbucket','now')", [])
            .expect_err("a kind no adapter speaks must not reach the table");
    }

    /// Every row that could exist before these columns did was a github.com
    /// pull request, so the defaults are a fact rather than a guess — and an
    /// existing row must read back that way without being touched.
    #[test]
    fn the_forge_migration_defaults_existing_rows_to_github_and_is_idempotent() {
        let legacy = Connection::open_in_memory().unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE pull_requests (
                   id TEXT PRIMARY KEY, project_id TEXT NOT NULL, session_id TEXT,
                   repo_owner TEXT NOT NULL, repo_name TEXT NOT NULL, number INTEGER NOT NULL,
                   url TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'open',
                   relation TEXT NOT NULL DEFAULT 'mine', created_at TEXT NOT NULL
                 );
                 CREATE INDEX idx_pull_requests_repo_number ON pull_requests(repo_owner, repo_name, number);
                 INSERT INTO pull_requests (id, project_id, repo_owner, repo_name, number, url, created_at)
                 VALUES ('pr1','proj1','killua15','Hangar',1,'https://github.com/killua15/Hangar/pull/1','now');",
            )
            .unwrap();
        assert!(!has_column(&legacy, "pull_requests", "forge").unwrap(), "the fixture has to start without the column for this to prove anything");

        pull_requests_forge_and_host(&legacy).unwrap();

        let (forge, host): (String, String) =
            legacy.query_row("SELECT forge, host FROM pull_requests WHERE id = 'pr1'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((forge.as_str(), host.as_str()), ("github", "github.com"), "an existing row is a github.com PR by construction");

        let index_names: Vec<String> = legacy
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'pull_requests'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(index_names.iter().any(|n| n == "idx_pull_requests_host_repo_number"), "the host-keyed index must replace the old one: {index_names:?}");
        assert!(!index_names.iter().any(|n| n == "idx_pull_requests_repo_number"), "the old index no longer discriminates and must go: {index_names:?}");

        let fresh = fresh_db();
        assert!(has_column(&fresh, "pull_requests", "host").unwrap(), "schema.sql must declare it for new databases");
        pull_requests_forge_and_host(&fresh).expect("re-running where schema.sql already added it must be a no-op, not a startup failure");
    }

    /// A failing migration must leave both the schema *and* the recorded
    /// version untouched, so the next launch retries it from a known state
    /// instead of skipping it forever against a half-changed database.
    #[test]
    fn a_failing_transactional_migration_rolls_back_and_does_not_bump_the_version() {
        fn good(conn: &Connection) -> Result<(), String> {
            conn.execute_batch("CREATE TABLE applied_first (id TEXT);").map_err(|e| e.to_string())
        }
        fn bad(conn: &Connection) -> Result<(), String> {
            conn.execute_batch("CREATE TABLE half_applied (id TEXT);").map_err(|e| e.to_string())?;
            conn.execute_batch("THIS IS NOT SQL;").map_err(|e| e.to_string())
        }
        let migrations = [
            Migration { version: 1, name: "good", run: good, transactional: true },
            Migration { version: 2, name: "bad", run: bad, transactional: true },
        ];

        let conn = fresh_db();
        let result = apply(&conn, &migrations);

        assert!(result.is_err(), "a broken migration must surface as an error, not be swallowed");
        assert_eq!(schema_version(&conn), 1, "version must stay at the last migration that fully succeeded");
        assert!(
            conn.query_row("SELECT 1 FROM sqlite_master WHERE name='applied_first'", [], |r| r.get::<_, i64>(0)).is_ok(),
            "the migration that succeeded must stay applied"
        );
        assert!(
            conn.query_row("SELECT 1 FROM sqlite_master WHERE name='half_applied'", [], |r| r.get::<_, i64>(0)).is_err(),
            "the failed migration's partial work must be rolled back"
        );
    }

    /// Reproduces a real production crash: an existing database created
    /// before the subsession/worktree columns existed. `CREATE TABLE IF NOT
    /// EXISTS` in schema.sql is a no-op against that pre-existing table, so
    /// any unconditional `CREATE INDEX ... ON sessions(parent_session_id)`
    /// left directly in schema.sql (rather than in `run_migrations`, after
    /// the `ALTER TABLE ADD COLUMN` that actually adds the column) crashes
    /// `conn.execute_batch(SCHEMA)` on every subsequent app launch. This
    /// drives the exact call sequence `init` uses — `execute_batch(SCHEMA)`
    /// then `run_migrations` — against a hand-built old-shape database.
    #[test]
    fn execute_batch_schema_does_not_crash_on_a_pre_subsession_database() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        // The exact `sessions`/`projects` shape from before parent_session_id/
        // worktree_*/default_setup_command existed.
        conn.execute_batch(
            "CREATE TABLE projects (
               id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL, created_at TEXT NOT NULL
             );
             CREATE TABLE accounts (
               id TEXT PRIMARY KEY, provider TEXT NOT NULL, billing TEXT NOT NULL, name TEXT NOT NULL,
               scope TEXT NOT NULL, plan_label TEXT NOT NULL DEFAULT '', config_dir_path TEXT NOT NULL,
               credential_ref TEXT NOT NULL, monthly_cap REAL, status TEXT NOT NULL DEFAULT 'healthy',
               created_at TEXT NOT NULL, updated_at TEXT NOT NULL
             );
             CREATE TABLE sessions (
               id TEXT PRIMARY KEY, name TEXT NOT NULL DEFAULT '',
               project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
               account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
               model TEXT NOT NULL, effort TEXT NOT NULL DEFAULT 'high',
               mode TEXT NOT NULL DEFAULT 'ask_edits', status TEXT NOT NULL DEFAULT 'waiting',
               pid INTEGER, provider_session_id TEXT, started_at TEXT NOT NULL, ended_at TEXT
             );",
        )
        .unwrap();

        // This must NOT panic — this is the exact call `db::init` makes.
        conn.execute_batch(SCHEMA).expect("schema batch must succeed against a pre-existing old-shape database");
        run_migrations(&conn).expect("migrations must succeed against a pre-existing old-shape database");

        let sessions_sql: String = conn
            .query_row("SELECT sql FROM sqlite_master WHERE name='sessions'", [], |r| r.get(0))
            .unwrap();
        assert!(sessions_sql.contains("parent_session_id"), "parent_session_id must exist after migration");
        assert!(sessions_sql.contains("worktree_path"), "worktree_path must exist after migration");

        let projects_sql: String = conn
            .query_row("SELECT sql FROM sqlite_master WHERE name='projects'", [], |r| r.get(0))
            .unwrap();
        assert!(projects_sql.contains("default_setup_command"), "default_setup_command must exist after migration");

        // The index migration itself must have actually run and be usable.
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('acc1','claude','sub','Test','work','','','acc1','healthy','now','now')",
            [],
        ).unwrap();
        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES ('proj1','Test','/tmp','now')", []).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, project_id, account_id, model, effort, mode, status, started_at)
             VALUES ('sess1','proj1','acc1','opus','high','ask_edits','waiting','now')",
            [],
        ).unwrap();
        let found: String = conn
            .query_row("SELECT id FROM sessions WHERE parent_session_id IS NULL AND id = 'sess1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(found, "sess1");
    }

    /// Reproduces the real production crash this guards against: a
    /// `pull_requests` table from before the GitLab `host`/`forge` columns
    /// existed. `CREATE TABLE IF NOT EXISTS` in schema.sql is a no-op against
    /// that pre-existing table, so an unconditional `CREATE INDEX ...
    /// ON pull_requests(host, ...)` left directly in schema.sql (rather than
    /// in `pull_requests_forge_and_host`, after the `ALTER TABLE ADD COLUMN`
    /// that actually adds `host`) crashed `conn.execute_batch(SCHEMA)` on
    /// every launch of an install predating migration 22 — before
    /// `run_migrations` ever got a chance to run and fix it forward. Also
    /// confirms a pre-existing PR row survives the migration untouched.
    #[test]
    fn execute_batch_schema_does_not_crash_on_a_pre_host_pull_requests_table() {
        let conn = Connection::open_in_memory().unwrap();

        // The exact `pull_requests` shape from before `host`/`forge` existed.
        conn.execute_batch(
            "CREATE TABLE pull_requests (
               id                    TEXT PRIMARY KEY,
               project_id            TEXT NOT NULL,
               session_id            TEXT,
               repo_owner            TEXT NOT NULL,
               repo_name             TEXT NOT NULL,
               number                INTEGER NOT NULL,
               url                   TEXT NOT NULL,
               state                 TEXT NOT NULL DEFAULT 'open',
               title                 TEXT,
               author_login          TEXT,
               relation              TEXT NOT NULL DEFAULT 'mine',
               last_comment_seen_at  TEXT,
               last_check_error      TEXT,
               last_seen_updated_at  TEXT,
               head_branch           TEXT,
               base_branch           TEXT,
               created_at            TEXT NOT NULL
             );
             INSERT INTO pull_requests (id, project_id, repo_owner, repo_name, number, url, created_at)
             VALUES ('pr1', 'proj1', 'killua15', 'Fleet-Agent', 42, 'https://github.com/killua15/Fleet-Agent/pull/42', 'now');",
        )
        .unwrap();

        // This must NOT fail — this is the exact call sequence `db::init` makes.
        conn.execute_batch(SCHEMA).expect("schema batch must succeed against a pre-host pull_requests table");
        run_migrations(&conn).expect("migrations must succeed against a pre-host pull_requests table");

        let sql: String =
            conn.query_row("SELECT sql FROM sqlite_master WHERE name='pull_requests'", [], |r| r.get(0)).unwrap();
        assert!(sql.contains("host"), "host must exist after migration");
        assert!(sql.contains("forge"), "forge must exist after migration");

        let indexed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_pull_requests_host_repo_number'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(indexed, 1, "the host-keyed index must exist after migrating an old database");

        let (host, forge, repo_owner): (String, String, String) = conn
            .query_row("SELECT host, forge, repo_owner FROM pull_requests WHERE id = 'pr1'", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(host, "github.com", "pre-existing rows must default to github.com, not lose data");
        assert_eq!(forge, "github");
        assert_eq!(repo_owner, "killua15", "pre-existing row data must survive the migration untouched");
    }

    /// Rebuilds the exact scenario that shipped in early dev databases: a
    /// `sessions` table with the old `ON DELETE RESTRICT`, holding a real
    /// row for an account — then confirms migration + cascading delete
    /// both work, with no data loss for unrelated rows.
    /// `schema.sql` is applied to **existing** databases too, before any
    /// migration runs — so nothing in it may name a column a migration
    /// introduces. An index on `turn_usage.provider_message_id` did, and every
    /// launch of an older install aborted inside `setup()` with a panic across
    /// the Objective-C boundary: the app died before drawing a window.
    #[test]
    fn schema_sql_applies_cleanly_to_a_database_that_predates_it() {
        let conn = Connection::open_in_memory().unwrap();
        // `turn_usage` exactly as it shipped before migration 12.
        conn.execute_batch(
            "CREATE TABLE turn_usage (
               id                    TEXT PRIMARY KEY,
               session_id            TEXT NOT NULL,
               block_id              TEXT,
               input_tokens          INTEGER NOT NULL,
               output_tokens         INTEGER NOT NULL,
               cache_read_tokens     INTEGER,
               cache_creation_tokens INTEGER,
               cost_usd              REAL,
               created_at            TEXT NOT NULL
             );",
        )
        .unwrap();

        conn.execute_batch(SCHEMA).expect("schema.sql must not fail against a database that predates it");
        run_migrations(&conn).expect("migrations must carry that database forward");

        // And the unique key that stops a re-read double-counting has to exist
        // on this path too, not only on a fresh install.
        let indexed: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_turn_usage_message'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(indexed, 1, "the dedupe index must exist after migrating an old database");
    }

    #[test]
    fn migrates_old_restrict_schema_and_then_cascades_on_delete() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();

        // Simulate the old (buggy) schema by rebuilding sessions with RESTRICT.
        conn.execute_batch(
            "PRAGMA foreign_keys=OFF;
             DROP TABLE sessions;
             CREATE TABLE sessions (
               id TEXT PRIMARY KEY,
               project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
               node_id TEXT REFERENCES agent_nodes(id) ON DELETE SET NULL,
               account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE RESTRICT,
               model TEXT NOT NULL,
               mode TEXT NOT NULL DEFAULT 'ask_edits',
               status TEXT NOT NULL DEFAULT 'waiting',
               pid INTEGER,
               provider_session_id TEXT,
               started_at TEXT NOT NULL,
               ended_at TEXT
             );
             PRAGMA foreign_keys=ON;",
        )
        .unwrap();

        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('acc1','claude','sub','Test','work','','/tmp/acc1','acc1','healthy','now','now')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at) VALUES ('proj1','Test','/tmp','now')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, project_id, account_id, model, mode, status, started_at)
             VALUES ('sess1','proj1','acc1','opus','ask_edits','waiting','now')",
            [],
        ).unwrap();

        // Pre-migration: this is the bug — deleting the account is blocked.
        let blocked = conn.execute("DELETE FROM accounts WHERE id = 'acc1'", []);
        assert!(blocked.is_err(), "expected the old RESTRICT constraint to block the delete");

        // run_migrations always adds these via ALTER TABLE before the FK
        // rebuild runs — mirror that ordering so the rebuild has real
        // name/effort/parent/worktree columns to carry over, same as in production.
        conn.execute_batch("ALTER TABLE sessions ADD COLUMN name TEXT NOT NULL DEFAULT '';").unwrap();
        conn.execute_batch("ALTER TABLE sessions ADD COLUMN effort TEXT NOT NULL DEFAULT 'high';").unwrap();
        conn.execute_batch("ALTER TABLE sessions ADD COLUMN parent_session_id TEXT REFERENCES sessions(id) ON DELETE CASCADE;").unwrap();
        conn.execute_batch("ALTER TABLE sessions ADD COLUMN worktree_path TEXT;").unwrap();
        conn.execute_batch("ALTER TABLE sessions ADD COLUMN worktree_branch TEXT;").unwrap();
        conn.execute_batch("ALTER TABLE sessions ADD COLUMN worktree_kind TEXT;").unwrap();
        conn.execute("UPDATE sessions SET name = 'my-session' WHERE id = 'sess1'", []).unwrap();

        migrate_sessions_account_fk_to_cascade(&conn);

        let sql: String = conn
            .query_row("SELECT sql FROM sqlite_master WHERE name='sessions'", [], |r| r.get(0))
            .unwrap();
        assert!(sql.contains("ON DELETE CASCADE"));
        assert!(!sql.contains("ON DELETE RESTRICT"));

        let (preserved_id, preserved_name): (String, String) = conn
            .query_row("SELECT id, name FROM sessions WHERE id = 'sess1'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(preserved_id, "sess1");
        assert_eq!(preserved_name, "my-session", "name column must survive the FK rebuild, not be dropped");

        conn.execute("DELETE FROM accounts WHERE id = 'acc1'", []).expect("delete should now cascade cleanly");
        let remaining: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0)).unwrap();
        assert_eq!(remaining, 0, "session should have been cascade-deleted with its account");
    }

    #[test]
    fn fresh_schema_already_has_cascade_and_migration_is_a_no_op() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();

        migrate_sessions_account_fk_to_cascade(&conn);

        let sql: String = conn
            .query_row("SELECT sql FROM sqlite_master WHERE name='sessions'", [], |r| r.get(0))
            .unwrap();
        assert!(sql.contains("ON DELETE CASCADE"));
    }

    /// Reproduces the real bug found via the user's own error logs:
    /// `emit_block: ... no such table: main.sessions_old_fk_migration`.
    /// Running the FK-cascade migration renames `sessions` away and back,
    /// and SQLite silently repoints `session_blocks`/`git_checkpoints`'
    /// FKs at the temp name along the way — leaving them dangling once
    /// that temp table is dropped. This is why every message a user sent
    /// silently failed to save on the live database, for real, in
    /// production, this whole time.
    #[test]
    fn repairs_dangling_fk_left_by_the_cascade_migration_and_keeps_data() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();

        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('acc1','claude','sub','Test','work','','','acc1','healthy','now','now')",
            [],
        ).unwrap();
        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES ('proj1','Test','/tmp','now')", []).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, started_at)
             VALUES ('sess1','s','proj1','acc1','','high','ask_edits','waiting','now')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO session_blocks (id, session_id, kind, text, created_at) VALUES ('blk1','sess1','you','hello','now')",
            [],
        ).unwrap();

        // Reproduce the exact corruption: rename sessions away and back,
        // same as migrate_sessions_account_fk_to_cascade does.
        conn.execute_batch(
            "PRAGMA foreign_keys=OFF;
             ALTER TABLE sessions RENAME TO sessions_old_fk_migration;
             CREATE TABLE sessions (
               id TEXT PRIMARY KEY, name TEXT NOT NULL DEFAULT '', project_id TEXT NOT NULL,
               account_id TEXT NOT NULL, model TEXT NOT NULL, effort TEXT NOT NULL DEFAULT 'high',
               mode TEXT NOT NULL DEFAULT 'ask_edits', status TEXT NOT NULL DEFAULT 'waiting',
               pid INTEGER, provider_session_id TEXT, started_at TEXT NOT NULL, ended_at TEXT
             );
             INSERT INTO sessions SELECT id, name, project_id, account_id, model, effort, mode, status, pid, provider_session_id, started_at, ended_at FROM sessions_old_fk_migration;
             DROP TABLE sessions_old_fk_migration;
             PRAGMA foreign_keys=ON;",
        ).unwrap();

        // Confirm the bug is really reproduced before fixing it.
        let broken = conn.execute(
            "INSERT INTO session_blocks (id, session_id, kind, text, created_at) VALUES ('blk2','sess1','you','world','now')",
            [],
        );
        assert!(broken.is_err(), "expected the dangling FK to block this insert, reproducing the real bug");

        repair_dangling_sessions_fk(&conn);

        conn.execute(
            "INSERT INTO session_blocks (id, session_id, kind, text, created_at) VALUES ('blk2','sess1','you','world','now')",
            [],
        ).expect("insert must succeed once the dangling FK is repaired");

        let texts: Vec<String> = conn
            .prepare("SELECT text FROM session_blocks ORDER BY created_at, id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(texts, vec!["hello".to_string(), "world".to_string()], "pre-existing block must survive the repair");
    }

    /// Widening a CHECK means rebuilding the table, and `accounts` is
    /// referenced by half the schema — the migration has to leave every one of
    /// those foreign keys pointing at a table that exists, which is the trap
    /// this file already paid for once.
    #[test]
    fn kimi_becomes_a_valid_provider_without_breaking_what_references_accounts() {
        let conn = fresh_db();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('k1','kimi','key','Kimi','personal','','','k1','healthy','now','now')",
            [],
        )
        .expect("kimi must be a valid provider after the migration");

        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES ('p1','P','/tmp','now')", []).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, started_at)
             VALUES ('s1','s','p1','k1','','high','ask_edits','waiting','now')",
            [],
        )
        .expect("a session referencing the rebuilt accounts table must still insert");

        conn.execute("INSERT INTO rotation_policies (project_id, provider, policy) VALUES ('p1','kimi','auto')", [])
            .expect("rotation policies were rebuilt too, or a kimi pool couldn't have a policy");

        // And deleting the account still cascades, which is what the FK is for.
        conn.execute("DELETE FROM accounts WHERE id = 'k1'", []).unwrap();
        let left: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0, "ON DELETE CASCADE must survive the rebuild");
    }

    /// Same rebuild, run again for `grok` — the two migrations widen the same
    /// two `CHECK` constraints, so a bug that broke one silently and not the
    /// other (a stale idempotency check, a column dropped from one rebuild but
    /// not the other) needs its own coverage rather than assuming kimi's
    /// passing is enough.
    /// The previous test proves the *schema* accepts opencode, and proves
    /// nothing about the migration: `fresh_db` applies the new `SCHEMA`, so
    /// `allow_opencode_provider` sees the provider already there and returns
    /// early. The path that actually runs on someone's existing database is the
    /// one that was broken, so it gets rebuilt here on purpose.
    #[test]
    fn the_migration_widens_an_existing_database_without_losing_rows() {
        let conn = fresh_db();
        run_migrations(&conn).unwrap();

        // Put the two tables back on their pre-opencode shape, with a row in
        // each, exactly as a database from before this branch looks.
        conn.execute_batch(
            "PRAGMA foreign_keys=OFF;
             PRAGMA legacy_alter_table=ON;
             DROP TABLE accounts;
             CREATE TABLE accounts (
               id               TEXT PRIMARY KEY,
               provider         TEXT NOT NULL CHECK (provider IN ('claude','codex','gemini','kimi','grok')),
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
               exhausted_at     TEXT,
               team_account_id  TEXT,
               provider_email   TEXT NOT NULL DEFAULT ''
             );
             DROP TABLE rotation_policies;
             CREATE TABLE rotation_policies (
               project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
               provider   TEXT NOT NULL CHECK (provider IN ('claude','codex','gemini','kimi','grok')),
               policy     TEXT NOT NULL DEFAULT 'manual' CHECK (policy IN ('auto','round_robin','manual')),
               PRIMARY KEY (project_id, provider)
             );
             PRAGMA legacy_alter_table=OFF;
             PRAGMA foreign_keys=ON;",
        )
        .unwrap();

        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES ('p1','P','/tmp','now')", []).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, monthly_cap, status, created_at, updated_at, provider_email)
             VALUES ('c1','claude','sub','Work','work','Max','~/.work','c1',42.5,'healthy','then','then','me@example.com')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO rotation_policies (project_id, provider, policy) VALUES ('p1','claude','round_robin')", []).unwrap();

        // Before: exactly the error that was reported.
        assert!(
            conn.execute(
                "INSERT INTO accounts (id, provider, billing, name, scope, config_dir_path, credential_ref, status, created_at, updated_at)
                 VALUES ('o0','opencode','key','OC','personal','~/.oc','o0','healthy','now','now')",
                [],
            )
            .is_err(),
            "the old shape must reject opencode, or this test is not reproducing the bug"
        );

        allow_opencode_provider(&conn).expect("the migration must run on a real pre-opencode database");

        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('o1','opencode','key','OC','personal','~/.oc','o1','healthy','now','now')",
            [],
        )
        .expect("after the migration opencode must be accepted");
        conn.execute("INSERT INTO rotation_policies (project_id, provider, policy) VALUES ('p1','opencode','auto')", [])
            .expect("rotation_policies must have been widened too");

        // Every column of the old row, not just the ones the CHECK mentions.
        let (cap, plan, email, created): (f64, String, String, String) = conn
            .query_row("SELECT monthly_cap, plan_label, provider_email, created_at FROM accounts WHERE id = 'c1'", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .expect("the pre-existing account must survive the rebuild");
        assert_eq!((cap, plan.as_str(), email.as_str(), created.as_str()), (42.5, "Max", "me@example.com", "then"));

        let policy: String = conn
            .query_row("SELECT policy FROM rotation_policies WHERE project_id='p1' AND provider='claude'", [], |r| r.get(0))
            .expect("the pre-existing rotation policy must survive too");
        assert_eq!(policy, "round_robin");

        // And it is idempotent, because migrations get re-run on every start
        // until their version is recorded.
        allow_opencode_provider(&conn).expect("running it twice must be harmless");
    }

    /// Reported as "I minimise every panel, reopen Hangar, and they are all
    /// open again". The state *was* being saved — in `localStorage`, keyed by
    /// terminal id — and could never have survived: restore reopens each
    /// terminal with a fresh id, so every key pointed at nothing. It lives on
    /// the row now, which is the thing that outlives the restart.
    #[test]
    fn a_folded_panel_is_remembered_on_its_own_row() {
        let conn = fresh_db();
        run_migrations(&conn).unwrap();
        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES ('p1','P','/tmp','now')", []).unwrap();

        // A row written before the column existed: it must read as "open",
        // never as folded, or an update would fold panels nobody folded.
        conn.execute(
            "INSERT INTO open_terminals (id, target_kind, source_id, created_at) VALUES ('t1','project','p1','now')",
            [],
        )
        .unwrap();
        let fresh: i64 = conn.query_row("SELECT minimized FROM open_terminals WHERE id='t1'", [], |r| r.get(0)).unwrap();
        assert_eq!(fresh, 0, "an existing terminal must default to unfolded");

        conn.execute("UPDATE open_terminals SET minimized = 1 WHERE id = 't1'", []).unwrap();
        let folded: i64 = conn.query_row("SELECT minimized FROM open_terminals WHERE id='t1'", [], |r| r.get(0)).unwrap();
        assert_eq!(folded, 1);

        // Idempotent: migrations re-run until their version is recorded.
        open_terminal_minimized(&conn).expect("running it twice must be harmless");
        let still: i64 = conn.query_row("SELECT minimized FROM open_terminals WHERE id='t1'", [], |r| r.get(0)).unwrap();
        assert_eq!(still, 1, "re-running the migration must not reset what the user set");
    }

    /// The constraint this was reported against: creating an opencode account
    /// failed with `CHECK constraint failed: provider IN (...)`, because the
    /// provider was wired everywhere except the two tables that police it.
    ///
    /// `rotation_policies` is the half that would have been missed: an account
    /// saves fine without it, and rotation refuses the provider much later,
    /// nowhere near the cause.
    #[test]
    fn opencode_becomes_a_valid_provider_and_the_rebuild_keeps_the_rows() {
        let conn = fresh_db();
        run_migrations(&conn).unwrap();

        // A row that predates the rebuild, to prove the copy carries data over
        // rather than quietly starting an empty table.
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at, provider_email)
             VALUES ('c1','claude','sub','Work','work','','~/.work','c1','healthy','now','now','me@example.com')",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('o1','opencode','key','OC','personal','','~/.oc','o1','healthy','now','now')",
            [],
        )
        .expect("opencode must be a valid provider — this is the reported failure");

        let email: String = conn
            .query_row("SELECT provider_email FROM accounts WHERE id = 'c1'", [], |r| r.get(0))
            .expect("the pre-existing row must survive the table rebuild");
        assert_eq!(email, "me@example.com", "the rebuild must copy every column, not just the ones it names in the CHECK");

        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES ('p1','P','/tmp','now')", []).unwrap();
        conn.execute("INSERT INTO rotation_policies (project_id, provider, policy) VALUES ('p1','opencode','auto')", [])
            .expect("rotation_policies carries the same constraint and must have been rebuilt too");

        conn.execute(
            "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, started_at)
             VALUES ('s1','s','p1','o1','','high','ask_edits','waiting','now')",
            [],
        )
        .expect("a session referencing the rebuilt accounts table must still insert");

        conn.execute("DELETE FROM accounts WHERE id = 'o1'", []).unwrap();
        let left: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0, "ON DELETE CASCADE must survive the rebuild");
    }

    #[test]
    fn grok_becomes_a_valid_provider_without_breaking_what_references_accounts() {
        let conn = fresh_db();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('g1','grok','sub','Grok','personal','','','g1','healthy','now','now')",
            [],
        )
        .expect("grok must be a valid provider after the migration");

        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES ('p1','P','/tmp','now')", []).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, started_at)
             VALUES ('s1','s','p1','g1','','high','ask_edits','waiting','now')",
            [],
        )
        .expect("a session referencing the rebuilt accounts table must still insert");

        conn.execute("INSERT INTO rotation_policies (project_id, provider, policy) VALUES ('p1','grok','auto')", [])
            .expect("rotation policies were rebuilt too, or a grok pool couldn't have a policy");

        conn.execute("DELETE FROM accounts WHERE id = 'g1'", []).unwrap();
        let left: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0, "ON DELETE CASCADE must survive the rebuild");
    }

    /// A database on the pre-migration-4 shape: `pull_requests` anchored to a
    /// session, no project of its own. Rebuilt in the test rather than taken
    /// from `SCHEMA`, which now declares the *new* shape.
    fn db_with_session_anchored_prs() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=OFF;
             PRAGMA legacy_alter_table=ON;
             DROP TABLE pull_requests;
             CREATE TABLE pull_requests (
               id                    TEXT PRIMARY KEY,
               session_id            TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
               repo_owner            TEXT NOT NULL,
               repo_name             TEXT NOT NULL,
               number                INTEGER NOT NULL,
               url                   TEXT NOT NULL,
               state                 TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open','closed','merged')),
               last_comment_seen_at  TEXT,
               last_check_error      TEXT,
               last_seen_updated_at  TEXT,
               head_branch           TEXT,
               created_at            TEXT NOT NULL
             );
             PRAGMA legacy_alter_table=OFF;
             PRAGMA foreign_keys=ON;",
        )
        .unwrap();

        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('acc1','claude','sub','Test','work','','','acc1','healthy','now','now')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES ('proj1','Test','/tmp','now')", []).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, started_at)
             VALUES ('sess1','s','proj1','acc1','opus','high','ask_edits','waiting','now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO pull_requests (id, session_id, repo_owner, repo_name, number, url, state, head_branch, created_at)
             VALUES ('pr1','sess1','killua15','Hangar',7,'https://github.com/killua15/Hangar/pull/7','open','feat/x','now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO pr_review_comments (id, pull_request_id, author, body, comment_created_at, external_id, created_at)
             VALUES ('c1','pr1','alice','fix this','now','gh-1','now')",
            [],
        )
        .unwrap();
        conn
    }

    /// The migration must carry every tracked PR over, find its project through
    /// the session it used to hang off, and label it `mine` — which is what all
    /// of them were, since discovery only ever asked for `--author @me`.
    #[test]
    fn re_anchoring_pull_requests_keeps_them_and_backfills_their_project() {
        let conn = db_with_session_anchored_prs();
        run_migrations(&conn).unwrap();

        let (project_id, session_id, relation): (String, Option<String>, String) = conn
            .query_row("SELECT project_id, session_id, relation FROM pull_requests WHERE id = 'pr1'", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .expect("the tracked PR must survive the rebuild");
        assert_eq!(project_id, "proj1");
        assert_eq!(session_id.as_deref(), Some("sess1"), "a PR Hangar opened keeps the session that produced it");
        assert_eq!(relation, "mine");
    }

    /// The whole point of the migration: a PR with no session at all — anyone
    /// else's — is now representable.
    #[test]
    fn a_pull_request_with_no_session_can_be_tracked_after_the_migration() {
        let conn = db_with_session_anchored_prs();
        run_migrations(&conn).unwrap();

        conn.execute(
            "INSERT INTO pull_requests (id, project_id, session_id, repo_owner, repo_name, number, url, state, author_login, relation, created_at)
             VALUES ('pr2','proj1',NULL,'killua15','Hangar',8,'https://github.com/killua15/Hangar/pull/8','open','alice','review','now')",
            [],
        )
        .expect("a PR opened by someone else has no session, and must still insert");
    }

    /// The trap this file already paid for once: rebuilding a table that
    /// others reference can leave their FK pointing at a name that no longer
    /// exists, and every later insert fails. `pr_review_comments` references
    /// `pull_requests`, so it must still work — and its rows must survive.
    #[test]
    fn re_anchoring_leaves_pr_comments_pointing_at_a_table_that_exists() {
        let conn = db_with_session_anchored_prs();
        run_migrations(&conn).unwrap();

        conn.execute(
            "INSERT INTO pr_review_comments (id, pull_request_id, author, body, comment_created_at, external_id, created_at)
             VALUES ('c2','pr1','bob','and this','now','gh-2','now')",
            [],
        )
        .expect("inserting a comment must still work after pull_requests is rebuilt");

        let comments: i64 = conn.query_row("SELECT COUNT(*) FROM pr_review_comments WHERE pull_request_id = 'pr1'", [], |r| r.get(0)).unwrap();
        assert_eq!(comments, 2, "the pre-existing comment must survive alongside the new one");

        let sql: String = conn.query_row("SELECT sql FROM sqlite_master WHERE name='pr_review_comments'", [], |r| r.get(0)).unwrap();
        assert!(!sql.contains("anchor_migration"), "the FK must not have followed the rebuild's temporary name");
    }

    /// Deleting a session no longer takes its PR with it — the PR is the
    /// project's, and it still exists on GitHub. Deleting the project does.
    #[test]
    fn a_pull_request_outlives_its_session_but_not_its_project() {
        let conn = db_with_session_anchored_prs();
        run_migrations(&conn).unwrap();

        conn.execute("DELETE FROM sessions WHERE id = 'sess1'", []).unwrap();
        let session_id: Option<String> = conn.query_row("SELECT session_id FROM pull_requests WHERE id = 'pr1'", [], |r| r.get(0)).unwrap();
        assert_eq!(session_id, None, "the PR stays, orphaned of its session");

        conn.execute("DELETE FROM projects WHERE id = 'proj1'", []).unwrap();
        let left: i64 = conn.query_row("SELECT COUNT(*) FROM pull_requests", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0, "deleting the project takes its PRs");
    }
}
