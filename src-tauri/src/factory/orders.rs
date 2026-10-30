//! The queue: the project's orders, in the order ▶ takes them. They come from
//! the project's own list or repo (never the whole workspace), from a TRD the
//! planner broke down, or typed by hand; an order waits for the ones it
//! depends on to be merged.

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    pub id: String,
    pub key: String,
    pub title: String,
    pub description: String,
    /// Acceptance criteria: each one becomes a test.
    pub criteria: Vec<String>,
    /// Keys of the orders this one waits for.
    pub deps: Vec<String>,
    pub position: i64,
    /// `todo`, `running`, `waiting_limit`, `waiting_human`, `stopped`,
    /// `failed`, `delivered` (MR open) or `merged`.
    pub status: String,
    /// `provider`, `trd` or `manual`.
    pub origin: String,
    pub ext_id: Option<String>,
    pub ext_url: Option<String>,
    pub touches_ui: bool,
    /// The brain rules the planner said apply.
    pub rules: Vec<String>,
    /// The brain version it froze when it started.
    pub brain_version: Option<i64>,
    pub created_at: String,
    /// Dependencies not merged yet; empty when it can run.
    #[serde(default)]
    pub blocked_by: Vec<String>,
}

/// An order whose work is in the base branch — what a dependency waits for.
pub fn is_done(status: &str) -> bool {
    status == "merged"
}

const COLS: &str = "id, key, title, description, criteria, deps, position, status, origin, ext_id, ext_url, touches_ui, rules, brain_version, created_at";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Order> {
    let list = |i: usize| -> rusqlite::Result<Vec<String>> { Ok(serde_json::from_str(&r.get::<_, String>(i)?).unwrap_or_default()) };
    Ok(Order {
        id: r.get(0)?,
        key: r.get(1)?,
        title: r.get(2)?,
        description: r.get(3)?,
        criteria: list(4)?,
        deps: list(5)?,
        position: r.get(6)?,
        status: r.get(7)?,
        origin: r.get(8)?,
        ext_id: r.get(9)?,
        ext_url: r.get(10)?,
        touches_ui: r.get(11)?,
        rules: list(12)?,
        brain_version: r.get(13)?,
        created_at: r.get(14)?,
        blocked_by: Vec::new(),
    })
}

/// Every order of the project, in queue order, with what blocks each.
pub fn list(conn: &Connection, project_id: &str) -> Result<Vec<Order>, String> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM factory_orders WHERE project_id = ?1 ORDER BY position, created_at")).map_err(|e| e.to_string())?;
    let mut orders: Vec<Order> = stmt.query_map(params![project_id], from_row).map_err(|e| e.to_string())?.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    let done: std::collections::HashSet<String> = orders.iter().filter(|o| is_done(&o.status)).map(|o| o.key.clone()).collect();
    let known: std::collections::HashSet<String> = orders.iter().map(|o| o.key.clone()).collect();
    for o in &mut orders {
        // A dependency that isn't in this project's queue can't be waited for.
        o.blocked_by = o.deps.iter().filter(|d| known.contains(*d) && !done.contains(*d)).cloned().collect();
    }
    Ok(orders)
}

pub fn get(conn: &Connection, id: &str) -> Result<Order, String> {
    let project: String = conn.query_row("SELECT project_id FROM factory_orders WHERE id = ?1", params![id], |r| r.get(0)).map_err(|_| "esa orden ya no existe".to_string())?;
    list(conn, &project)?.into_iter().find(|o| o.id == id).ok_or_else(|| "esa orden ya no existe".to_string())
}

/// What ▶ takes next: the first order still to do whose dependencies are in.
pub fn next(conn: &Connection, project_id: &str) -> Result<Option<Order>, String> {
    Ok(list(conn, project_id)?.into_iter().find(|o| o.status == "todo" && o.blocked_by.is_empty()))
}

/// Puts the orders in the given order; the ones not listed keep theirs, after.
pub fn reorder(conn: &Connection, project_id: &str, ids: &[String]) -> Result<(), String> {
    for (i, id) in ids.iter().enumerate() {
        conn.execute("UPDATE factory_orders SET position = ?3 WHERE id = ?1 AND project_id = ?2", params![id, project_id, i as i64]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn set_status(conn: &Connection, id: &str, status: &str) -> Result<(), String> {
    conn.execute("UPDATE factory_orders SET status = ?2, updated_at = ?3 WHERE id = ?1", params![id, status, Utc::now().to_rfc3339()]).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn freeze_brain(conn: &Connection, id: &str, version: i64) -> Result<(), String> {
    conn.execute("UPDATE factory_orders SET brain_version = ?2 WHERE id = ?1 AND brain_version IS NULL", params![id, version]).map_err(|e| e.to_string())?;
    Ok(())
}

fn next_position(conn: &Connection, project_id: &str) -> Result<i64, String> {
    conn.query_row("SELECT COALESCE(MAX(position), -1) + 1 FROM factory_orders WHERE project_id = ?1", params![project_id], |r| r.get(0)).map_err(|e| e.to_string())
}

/// A new order at the end of the queue.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewOrder {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub criteria: Vec<String>,
    #[serde(default)]
    pub deps: Vec<String>,
    pub origin: String,
    #[serde(default)]
    pub ext_id: Option<String>,
    #[serde(default)]
    pub ext_url: Option<String>,
    #[serde(default)]
    pub touches_ui: bool,
    #[serde(default)]
    pub rules: Vec<String>,
}

pub fn insert(conn: &Connection, project_id: &str, o: &NewOrder) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let j = |v: &Vec<String>| serde_json::to_string(v).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO factory_orders (id, project_id, key, title, description, criteria, deps, position, status, origin, ext_id, ext_url, touches_ui, rules, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'todo', ?9, ?10, ?11, ?12, ?13, ?14, ?14)",
        params![id, project_id, o.key, o.title, o.description, j(&o.criteria), j(&o.deps), next_position(conn, project_id)?, o.origin, o.ext_id, o.ext_url, o.touches_ui, j(&o.rules), now],
    )
    .map_err(|e| e.to_string())?;
    Ok(id)
}

/// `Citas Vet` → `VET`: the prefix of the keys the Factory hands out.
pub fn key_prefix(project_name: &str) -> String {
    let word = project_name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).last().unwrap_or("ORD");
    let p: String = word.chars().filter(|c| c.is_ascii_alphanumeric()).take(3).collect::<String>().to_uppercase();
    if p.is_empty() { "ORD".into() } else { p }
}

/// The next free `<PREFIX>-N`.
pub fn next_key(conn: &Connection, project_id: &str, prefix: &str) -> Result<String, String> {
    let mut stmt = conn.prepare("SELECT key FROM factory_orders WHERE project_id = ?1").map_err(|e| e.to_string())?;
    let keys: Vec<String> = stmt.query_map(params![project_id], |r| r.get(0)).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
    let n = keys.iter().filter_map(|k| k.strip_prefix(&format!("{prefix}-"))?.parse::<i64>().ok()).max().unwrap_or(0) + 1;
    Ok(format!("{prefix}-{n}"))
}

/// Brings the provider's tasks into the queue: new ones at the end, known
/// ones (by their id in the provider) with their title and body refreshed.
/// Returns how many were new.
pub fn sync_provider(conn: &Connection, project_id: &str, tasks: &[crate::task_sources::Task]) -> Result<usize, String> {
    let mut added = 0;
    for t in tasks {
        let existing: Option<String> = conn
            .query_row("SELECT id FROM factory_orders WHERE project_id = ?1 AND ext_id = ?2", params![project_id, t.external_id], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        match existing {
            Some(id) => {
                conn.execute("UPDATE factory_orders SET title = ?2, description = ?3, ext_url = ?4 WHERE id = ?1", params![id, t.title, t.description, t.url]).map_err(|e| e.to_string())?;
            }
            None => {
                let key = if t.key.chars().all(|c| c.is_ascii_digit()) { format!("#{}", t.key) } else { t.key.clone() };
                insert(
                    conn,
                    project_id,
                    &NewOrder { key, title: t.title.clone(), description: t.description.clone(), origin: "provider".into(), ext_id: Some(t.external_id.clone()), ext_url: Some(t.url.clone()), ..Default::default() },
                )?;
                added += 1;
            }
        }
    }
    Ok(added)
}

pub fn set_external(conn: &Connection, id: &str, ext_id: &str, ext_url: &str) -> Result<(), String> {
    conn.execute("UPDATE factory_orders SET ext_id = ?2, ext_url = ?3 WHERE id = ?1", params![id, ext_id, ext_url]).map_err(|e| e.to_string())?;
    Ok(())
}

/// Orders that live only in Hangar and nobody started — what «Conectar un
/// proveedor más adelante» exports.
pub fn unstarted_local(conn: &Connection, project_id: &str) -> Result<Vec<Order>, String> {
    Ok(list(conn, project_id)?.into_iter().filter(|o| o.ext_id.is_none() && o.status == "todo").collect())
}

/// The order as a task body for the provider.
pub fn task_body(o: &Order) -> String {
    let mut body = o.description.clone();
    if !o.criteria.is_empty() {
        body.push_str("\n\n**Criterios de aceptación**\n");
        for c in &o.criteria {
            body.push_str(&format!("- [ ] {c}\n"));
        }
    }
    if !o.deps.is_empty() {
        body.push_str(&format!("\nDepende de: {}\n", o.deps.join(", ")));
    }
    if !o.rules.is_empty() {
        body.push_str(&format!("\nReglas del brain: {}\n", o.rules.join(", ")));
    }
    body.push_str(&format!("\n_{} · Hangar Factory_", o.key));
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = crate::factory::store::tests::conn();
        c.execute_batch("INSERT INTO projects VALUES ('p2', 'otro', '/repos/otro');").unwrap();
        c
    }

    fn add(c: &Connection, project: &str, key: &str, deps: &[&str]) -> String {
        insert(c, project, &NewOrder { key: key.into(), title: key.into(), deps: deps.iter().map(|d| d.to_string()).collect(), origin: "trd".into(), ..Default::default() }).unwrap()
    }

    #[test]
    fn the_queue_is_the_projects_in_order_and_waits_for_its_dependencies() {
        let c = conn();
        let v1 = add(&c, "p1", "VET-1", &[]);
        add(&c, "p1", "VET-2", &["VET-1"]);
        let v3 = add(&c, "p1", "VET-3", &["VET-1"]);
        add(&c, "p2", "OTR-1", &[]);
        assert_eq!(list(&c, "p1").unwrap().len(), 3, "only this project's");
        assert_eq!(next(&c, "p1").unwrap().unwrap().key, "VET-1");

        set_status(&c, &v1, "running").unwrap();
        assert!(next(&c, "p1").unwrap().is_none(), "VET-2 and VET-3 wait for VET-1");
        set_status(&c, &v1, "delivered").unwrap();
        assert!(next(&c, "p1").unwrap().is_none(), "an open MR isn't in the base branch yet");
        set_status(&c, &v1, "merged").unwrap();
        assert_eq!(next(&c, "p1").unwrap().unwrap().key, "VET-2");

        let ids: Vec<String> = list(&c, "p1").unwrap().into_iter().map(|o| o.id).collect();
        reorder(&c, "p1", &[ids[0].clone(), v3.clone(), ids[1].clone()]).unwrap();
        assert_eq!(next(&c, "p1").unwrap().unwrap().key, "VET-3", "▶ respects the order you dragged");
    }

    #[test]
    fn keys_and_provider_tasks() {
        let c = conn();
        assert_eq!(key_prefix("Citas Vet"), "VET");
        assert_eq!(key_prefix("citas-vet"), "VET");
        assert_eq!(next_key(&c, "p1", "VET").unwrap(), "VET-1");
        add(&c, "p1", "VET-1", &[]);
        add(&c, "p1", "VET-7", &[]);
        assert_eq!(next_key(&c, "p1", "VET").unwrap(), "VET-8");

        let task = |id: &str, title: &str| crate::task_sources::Task {
            external_id: id.into(),
            key: id.into(),
            title: title.into(),
            description: String::new(),
            url: format!("https://github.com/a/b/issues/{id}"),
            status: None,
            status_kind: None,
            branch_hint: None,
            source_id: "s".into(),
            source_kind: "github".into(),
            container: None,
            suggested_branch: String::new(),
        };
        assert_eq!(sync_provider(&c, "p1", &[task("12", "Login")]).unwrap(), 1);
        assert_eq!(sync_provider(&c, "p1", &[task("12", "Login con email"), task("13", "Logout")]).unwrap(), 1);
        let all = list(&c, "p1").unwrap();
        let login = all.iter().find(|o| o.key == "#12").unwrap();
        assert_eq!((login.title.as_str(), login.origin.as_str()), ("Login con email", "provider"));
        assert_eq!(unstarted_local(&c, "p1").unwrap().len(), 2, "VET-1 and VET-7 live only in Hangar");
    }
}
