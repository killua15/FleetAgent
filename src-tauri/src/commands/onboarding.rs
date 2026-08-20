//! What the user has actually done with Hangar yet.
//!
//! Hangar is 28 screens and 208 commands, and nobody discovers that on their
//! own. This answers "what is left to try, and why would I care" from the
//! database rather than from a script — so it stays true after the first day and
//! becomes a status panel instead of a tour that plays once and lies later.
//!
//! Two rules shape it:
//!
//! 1. **Facts here, words in the frontend.** A step is an id, whether it is
//!    done, and a number. The copy lives in `StartHere.tsx` with the rest of the
//!    UI's Spanish, because a checklist whose wording is split across two
//!    languages in two files is a checklist nobody edits.
//! 2. **One command, not thirteen.** These are trivial counts over one SQLite
//!    connection; thirteen IPC round-trips to paint a list is how a window ends
//!    up frozen while it "loads".

use crate::capabilities::Capability;
use crate::db::DbConnection;
use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

/// One thing worth doing, resolved.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingStep {
    /// Matches a key in the frontend's copy table. Renaming one is a change in
    /// both places, which is why they are short and boring.
    pub id: &'static str,
    pub done: bool,
    /// The real number behind the tick — "2 cuentas", "$4.10". A step marked
    /// done with nothing to show for it reads as decoration.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingProgress {
    /// The nine that tell the product's story, in order.
    pub basics: Vec<OnboardingStep>,
    /// Everything else worth finding, kept separate so the first list stays
    /// readable. Thirteen items in one column reads as homework.
    pub beyond: Vec<OnboardingStep>,
    pub basics_done: usize,
    pub basics_total: usize,
}

/// The raw state the steps are derived from.
///
/// Split from the derivation so the interesting half is a pure function over
/// plain numbers, testable without a database — the same cut `auth::access`
/// makes for the sign-in gate.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Counts {
    pub essentials_missing: usize,
    pub accounts: i64,
    /// The largest number of accounts sharing one provider. Two Claude accounts
    /// enable rotation; a Claude and a Codex do not, and counting `accounts`
    /// alone would tick this step for someone who cannot rotate at all.
    pub same_provider_max: i64,
    pub projects: i64,
    pub worktree_sessions: i64,
    pub rotating_projects: i64,
    pub rotation_events: i64,
    pub billed_turns: i64,
    pub spend_usd: f64,
    pub budgets: i64,
    pub capped_accounts: i64,
    pub queued_runs: i64,
    pub project_skills: i64,
    pub tracked_prs: i64,
    pub ready_graphs: i64,
}

fn step(id: &'static str, done: bool, detail: Option<String>) -> OnboardingStep {
    // A detail on an unfinished step would describe a thing that hasn't
    // happened, so it is dropped rather than rendered as "0".
    OnboardingStep { id, done, detail: if done { detail } else { None } }
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// State in, checklist out.
pub fn progress_from(c: &Counts) -> OnboardingProgress {
    let basics = vec![
        step("requirements", c.essentials_missing == 0, Some("todo instalado".into())),
        step("first_account", c.accounts >= 1, Some(plural(c.accounts, "cuenta", "cuentas"))),
        step("first_project", c.projects >= 1, Some(plural(c.projects, "proyecto", "proyectos"))),
        step("first_worktree", c.worktree_sessions >= 1, Some(plural(c.worktree_sessions, "worktree", "worktrees"))),
        step("second_account", c.same_provider_max >= 2, Some(format!("{} del mismo proveedor", c.same_provider_max))),
        step("rotation_on", c.rotating_projects >= 1, Some(plural(c.rotating_projects, "proyecto", "proyectos"))),
        step("has_rotated", c.rotation_events >= 1, Some(plural(c.rotation_events, "vez", "veces"))),
        step("spend_visible", c.billed_turns >= 1, Some(format!("${:.2} en {}", c.spend_usd, plural(c.billed_turns, "turno", "turnos")))),
        step("budget_set", c.budgets >= 1 || c.capped_accounts >= 1, Some(plural(c.budgets + c.capped_accounts, "límite", "límites"))),
    ];

    let beyond = vec![
        step("queue_used", c.queued_runs >= 1, Some(plural(c.queued_runs, "ejecución", "ejecuciones"))),
        step("skills_on", c.project_skills >= 1, Some(plural(c.project_skills, "skill activa", "skills activas"))),
        step("pr_inbox", c.tracked_prs >= 1, Some(plural(c.tracked_prs, "PR seguido", "PRs seguidos"))),
        step("graph_built", c.ready_graphs >= 1, Some(plural(c.ready_graphs, "grafo", "grafos"))),
    ];

    OnboardingProgress {
        basics_done: basics.iter().filter(|s| s.done).count(),
        basics_total: basics.len(),
        basics,
        beyond,
    }
}

/// `COUNT(*)`-shaped queries, defaulting to zero.
///
/// A missing table is treated as an empty one on purpose: this screen must
/// render on a database that predates any given migration rather than taking
/// the window down over a checklist.
fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0)
}

fn gather(conn: &Connection) -> Counts {
    Counts {
        // Only the essential capabilities gate this step. Telling someone their
        // setup is incomplete because they have no `glab` — when they don't use
        // GitLab — is how a checklist gets ignored.
        essentials_missing: Capability::ALL
            .iter()
            .filter(|cap| cap.spec().essential && cap.check_offline().is_err())
            .count(),
        accounts: count(conn, "SELECT COUNT(*) FROM accounts"),
        same_provider_max: count(conn, "SELECT COALESCE(MAX(n), 0) FROM (SELECT COUNT(*) AS n FROM accounts GROUP BY provider)"),
        projects: count(conn, "SELECT COUNT(*) FROM projects"),
        worktree_sessions: count(conn, "SELECT COUNT(*) FROM sessions WHERE worktree_path IS NOT NULL AND worktree_path <> ''"),
        rotating_projects: count(conn, "SELECT COUNT(*) FROM rotation_policies WHERE policy IN ('auto','round_robin')"),
        rotation_events: count(conn, "SELECT COUNT(*) FROM rotation_events"),
        billed_turns: count(conn, "SELECT COUNT(*) FROM turn_usage WHERE cost_usd IS NOT NULL"),
        spend_usd: conn
            .query_row("SELECT COALESCE(SUM(cost_usd), 0) FROM turn_usage", [], |r| r.get::<_, f64>(0))
            .unwrap_or(0.0),
        budgets: count(conn, "SELECT COUNT(*) FROM quota_budgets"),
        capped_accounts: count(conn, "SELECT COUNT(*) FROM accounts WHERE monthly_cap IS NOT NULL"),
        queued_runs: count(conn, "SELECT COUNT(*) FROM queued_runs"),
        project_skills: count(conn, "SELECT COUNT(*) FROM project_skills"),
        tracked_prs: count(conn, "SELECT COUNT(*) FROM pull_requests"),
        ready_graphs: count(conn, "SELECT COUNT(*) FROM graphs WHERE status = 'ready'"),
    }
}

#[tauri::command]
pub fn onboarding_progress(db: State<DbConnection>) -> Result<OnboardingProgress, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(progress_from(&gather(&conn)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(steps: &'a [OnboardingStep], id: &str) -> &'a OnboardingStep {
        steps.iter().find(|s| s.id == id).expect("step must exist")
    }

    /// A fresh install has done nothing, and must say so rather than flattering
    /// the user with a half-full list.
    #[test]
    fn an_empty_install_has_nothing_ticked() {
        let p = progress_from(&Counts::default());

        // `essentials_missing: 0` is the default, so requirements is the one
        // step a blank Counts legitimately passes.
        assert_eq!(p.basics_done, 1, "only the requirements step may pass on empty state: {:?}", p.basics);
        assert_eq!(p.basics_total, 9);
        assert!(!find(&p.basics, "first_account").done);
        assert!(p.beyond.iter().all(|s| !s.done));
    }

    /// The step that separates "I have accounts" from "I can rotate". Two
    /// accounts on *different* providers cannot cover for each other, so ticking
    /// this on a bare account count would promise a failover that never happens.
    #[test]
    fn two_accounts_only_count_when_they_share_a_provider() {
        let mixed = Counts { accounts: 2, same_provider_max: 1, ..Default::default() };
        assert!(find(&progress_from(&mixed).basics, "first_account").done);
        assert!(!find(&progress_from(&mixed).basics, "second_account").done, "one Claude + one Codex cannot rotate for each other");

        let pair = Counts { accounts: 2, same_provider_max: 2, ..Default::default() };
        assert!(find(&progress_from(&pair).basics, "second_account").done);
    }

    /// Either a per-project budget or a per-account monthly cap satisfies the
    /// spending-limit step: they are two ways to answer the same question, and
    /// demanding both would nag someone who already has a limit.
    #[test]
    fn a_budget_or_a_cap_both_count_as_a_limit() {
        for c in [
            Counts { budgets: 1, ..Default::default() },
            Counts { capped_accounts: 1, ..Default::default() },
        ] {
            assert!(find(&progress_from(&c).basics, "budget_set").done, "{c:?}");
        }
        assert!(!find(&progress_from(&Counts::default()).basics, "budget_set").done);
    }

    /// A detail describes something that happened. On an unfinished step there
    /// is nothing to describe, and printing "0 veces" next to an empty circle
    /// reads as a failure rather than as a step not reached yet.
    #[test]
    fn unfinished_steps_carry_no_detail() {
        let p = progress_from(&Counts::default());
        for s in p.basics.iter().chain(p.beyond.iter()).filter(|s| !s.done) {
            assert!(s.detail.is_none(), "{} is not done but carries a detail", s.id);
        }
    }

    /// Spend is the one detail with a shape worth pinning: two decimals and the
    /// turn count, because "$0" and "$0.004" mean different things to someone
    /// deciding whether tracking works.
    #[test]
    fn spend_reads_as_money_and_turns() {
        let c = Counts { billed_turns: 3, spend_usd: 4.0999, ..Default::default() };
        assert_eq!(find(&progress_from(&c).basics, "spend_visible").detail.as_deref(), Some("$4.10 en 3 turnos"));

        let one = Counts { billed_turns: 1, spend_usd: 0.5, ..Default::default() };
        assert_eq!(find(&progress_from(&one).basics, "spend_visible").detail.as_deref(), Some("$0.50 en 1 turno"));
    }
}
