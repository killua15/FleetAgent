use rusqlite::Connection;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

pub struct DbConnection(pub Mutex<Connection>);

pub(crate) const SCHEMA: &str = include_str!("schema.sql");

pub fn init(app: &AppHandle) -> Connection {
    let dir = app
        .path()
        .app_data_dir()
        .expect("resolve app data dir");
    std::fs::create_dir_all(&dir).expect("create app data dir");
    let db_path = dir.join("fleet.db");

    let conn = Connection::open(db_path).expect("open sqlite database");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("enable foreign keys");
    conn.execute_batch(SCHEMA).expect("apply schema");
    run_migrations(&conn);
    conn
}

/// Small idempotent ALTER TABLE migrations for columns added after a table
/// already existed on disk — CREATE TABLE IF NOT EXISTS above won't add
/// them to a pre-existing dev database. Errors are ignored (column already
/// present is the expected steady state).
fn run_migrations(conn: &Connection) {
    let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN provider_session_id TEXT;");
    let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN name TEXT NOT NULL DEFAULT '';");
    let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN effort TEXT NOT NULL DEFAULT 'high';");
    let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN parent_session_id TEXT REFERENCES sessions(id) ON DELETE CASCADE;");
    let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN worktree_path TEXT;");
    let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN worktree_branch TEXT;");
    let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN worktree_kind TEXT;");
    let _ = conn.execute_batch("ALTER TABLE projects ADD COLUMN default_setup_command TEXT;");
    let _ = conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_session_id);");
    migrate_sessions_account_fk_to_cascade(conn);
    repair_dangling_sessions_fk(conn);
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
            eprintln!("repair_dangling_sessions_fk: session_blocks repair failed, rolling back: {e}");
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
            eprintln!("repair_dangling_sessions_fk: git_checkpoints repair failed, rolling back: {e}");
            let _ = conn.execute_batch("ROLLBACK;");
        }
    }

    let _ = conn.execute_batch("PRAGMA foreign_keys=ON;");
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
    let _ = conn.execute_batch("PRAGMA foreign_keys=OFF;");
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
        eprintln!("sessions FK migration failed, rolling back: {e}");
        let _ = conn.execute_batch("ROLLBACK;");
    }
    let _ = conn.execute_batch("PRAGMA foreign_keys=ON;");
}

#[cfg(test)]
mod tests {
    use super::*;

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
        run_migrations(&conn);

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

    /// Rebuilds the exact scenario that shipped in early dev databases: a
    /// `sessions` table with the old `ON DELETE RESTRICT`, holding a real
    /// row for an account — then confirms migration + cascading delete
    /// both work, with no data loss for unrelated rows.
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
}
