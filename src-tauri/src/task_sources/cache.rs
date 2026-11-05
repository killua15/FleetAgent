//! The Tasks panel's memory of every board.
//!
//! The Tasks screen reads a board when you open it and forgets it when you
//! leave. The panel next to the terminals is different: it's open while you
//! work, so it has to stay current without you pressing ↻ — and when a task
//! lands on you or moves column while you're heads-down, that's worth a line in
//! the bell. Both need the previous answer to compare against, which is what
//! this cache is.
//!
//! In memory on purpose. A board is cheap to read again, and persisting it
//! would only buy "what changed while Hangar was closed" — which the board
//! itself already shows better than a diff would.

use super::{all_sources, fetch_source, Task};
use crate::db::DbConnection;
use crate::notifications::{self, Target};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// One source as the panel last saw it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedSource {
    pub source_id: String,
    pub label: String,
    pub kind: String,
    /// What the last *successful* read returned. A failed read keeps these, so
    /// an expired token or a flaky network doesn't empty the panel.
    pub tasks: Vec<Task>,
    /// Why the last read failed, if it did.
    pub error: Option<String>,
    /// When the tasks above were read (RFC 3339). `None` until the first read
    /// succeeds.
    pub fetched_at: Option<String>,
}

#[derive(Default)]
pub struct TaskCache {
    sources: Mutex<HashMap<String, CachedSource>>,
    /// Held for a whole refresh, so the poll and a ↻ never diff against the
    /// same previous answer and announce the same change twice.
    refreshing: Mutex<()>,
}

impl TaskCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn snapshot(&self) -> Vec<CachedSource> {
        let mut list: Vec<CachedSource> = self.sources.lock().map(|m| m.values().cloned().collect()).unwrap_or_default();
        list.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
        list
    }
}

/// What happened to one task between two reads of its board.
#[derive(Debug, PartialEq)]
pub(crate) enum TaskChange<'a> {
    /// It wasn't on the board before: newly assigned, or newly created.
    New(&'a Task),
    /// Same task, different status.
    Moved { task: &'a Task, from: Option<String> },
}

/// The changes between two reads of one board. A task that **left** isn't one:
/// it was closed or unassigned, and either way it's no longer yours to act on.
pub(crate) fn changes<'a>(before: &[Task], after: &'a [Task]) -> Vec<TaskChange<'a>> {
    let before: HashMap<&str, &Task> = before.iter().map(|t| (t.external_id.as_str(), t)).collect();
    after
        .iter()
        .filter_map(|task| match before.get(task.external_id.as_str()) {
            None => Some(TaskChange::New(task)),
            Some(old) if old.status != task.status => Some(TaskChange::Moved { task, from: old.status.clone() }),
            Some(_) => None,
        })
        .collect()
}

/// Past this many changes in one read, one summary line instead of one each —
/// a board reorganised in bulk shouldn't bury everything else in the bell.
const SUMMARY_OVER: usize = 3;

/// The notification lines for one board's changes: `(title, body, target id)`.
pub(crate) fn change_lines(source_id: &str, label: &str, changes: &[TaskChange]) -> Vec<(String, String, String)> {
    if changes.len() > SUMMARY_OVER {
        let keys: Vec<&str> = changes
            .iter()
            .map(|c| match c {
                TaskChange::New(t) | TaskChange::Moved { task: t, .. } => t.key.as_str(),
            })
            .take(6)
            .collect();
        let more = if changes.len() > keys.len() { ", …" } else { "" };
        return vec![(format!("{} task changes · {label}", changes.len()), format!("{}{more}", keys.join(", ")), source_id.to_string())];
    }
    changes
        .iter()
        .map(|change| match change {
            TaskChange::New(t) => (format!("New task · {label}"), format!("{} {}", t.key, t.title), format!("{source_id}:{}", t.external_id)),
            TaskChange::Moved { task: t, from } => (
                format!("Task moved · {label}"),
                format!("{} {}: {} → {}", t.key, t.title, from.as_deref().unwrap_or("—"), t.status.as_deref().unwrap_or("—")),
                format!("{source_id}:{}", t.external_id),
            ),
        })
        .collect()
}

/// Reads every configured board (or just `only`), updates the cache, announces
/// what changed since the previous read, and returns the whole cache. Blocking.
pub(crate) fn refresh<R: Runtime>(app: &AppHandle<R>, only: Option<&str>) -> Result<Vec<CachedSource>, String> {
    let db = app.state::<DbConnection>();
    let cache = app.state::<TaskCache>();
    let _one_at_a_time = cache.refreshing.lock().map_err(|e| e.to_string())?;

    let sources = all_sources(&db)?;
    // A source removed in Settings leaves the panel with it.
    if only.is_none() {
        if let Ok(mut map) = cache.sources.lock() {
            map.retain(|id, _| sources.iter().any(|s| &s.id == id));
        }
    }

    for source in sources.iter().filter(|s| only.map_or(true, |id| id == s.id)) {
        let fetched = fetch_source(&db, &source.id);
        let now = chrono::Utc::now().to_rfc3339();

        let mut announce = Vec::new();
        if let Ok(mut map) = cache.sources.lock() {
            let entry = map.entry(source.id.clone()).or_insert_with(|| CachedSource {
                source_id: source.id.clone(),
                label: source.label.clone(),
                kind: source.kind.clone(),
                tasks: Vec::new(),
                error: None,
                fetched_at: None,
            });
            entry.label = source.label.clone();
            match fetched {
                Ok(tasks) => {
                    // Only against a real previous read: the first read of a
                    // launch is the baseline, not forty "new task" lines.
                    if entry.fetched_at.is_some() {
                        announce = change_lines(&source.id, &source.label, &changes(&entry.tasks, &tasks));
                    }
                    entry.tasks = tasks;
                    entry.error = None;
                    entry.fetched_at = Some(now);
                }
                Err(e) => {
                    log::warn!("task poll: {e}");
                    entry.error = Some(e);
                }
            }
        }

        for (title, body, target) in announce {
            notifications::record(app, notifications::kind::TASK, |s| s.notify_task_changes, &title, &body, Target::new("task", target));
        }
        let _ = app.emit("tasks-updated", &source.id);
    }

    Ok(cache.snapshot())
}

/// What the panel has, without going to the network.
#[tauri::command]
pub fn list_cached_tasks(cache: tauri::State<TaskCache>) -> Vec<CachedSource> {
    cache.snapshot()
}

/// Reads the boards again now — every one, or just `source_id`.
#[tauri::command]
pub async fn refresh_tasks<R: Runtime>(app: AppHandle<R>, source_id: Option<String>) -> Result<Vec<CachedSource>, String> {
    crate::blocking::run_blocking(move || refresh(&app, source_id.as_deref())).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, status: &str) -> Task {
        Task {
            external_id: id.to_string(),
            key: format!("K-{id}"),
            title: format!("Task {id}"),
            description: String::new(),
            url: String::new(),
            status: Some(status.to_string()),
            status_kind: None,
            branch_hint: None,
            source_id: "s".to_string(),
            source_kind: "clickup".to_string(),
            container: None,
            suggested_branch: String::new(),
        }
    }

    #[test]
    fn an_unchanged_board_announces_nothing() {
        let board = vec![task("1", "open"), task("2", "in progress")];
        assert!(changes(&board, &board.clone()).is_empty());
    }

    #[test]
    fn a_new_task_and_a_moved_one_are_both_reported() {
        let before = vec![task("1", "open"), task("2", "open")];
        let after = vec![task("1", "open"), task("2", "verificar"), task("3", "open")];
        let found = changes(&before, &after);
        assert_eq!(found, vec![TaskChange::Moved { task: &after[1], from: Some("open".to_string()) }, TaskChange::New(&after[2])]);

        let lines = change_lines("s", "Acme", &found);
        assert_eq!(lines[0].0, "Task moved · Acme");
        assert_eq!(lines[0].1, "K-2 Task 2: open → verificar");
        assert_eq!(lines[0].2, "s:2");
        assert_eq!(lines[1].0, "New task · Acme");
    }

    #[test]
    fn a_task_that_left_the_board_is_not_a_change() {
        let before = vec![task("1", "open"), task("2", "open")];
        let after = vec![task("1", "open")];
        assert!(changes(&before, &after).is_empty());
    }

    #[test]
    fn a_bulk_change_is_one_summary_line() {
        let after: Vec<Task> = (1..=8).map(|i| task(&i.to_string(), "open")).collect();
        let found = changes(&[], &after);
        let lines = change_lines("s", "GitHub", &found);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].0, "8 task changes · GitHub");
        assert!(lines[0].1.ends_with(", …"));
        assert_eq!(lines[0].2, "s");
    }
}
