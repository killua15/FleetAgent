//! Checking a brain before it's sealed, without AI: that every `check:` is
//! a command that can run, that none falls in a trap we already paid for,
//! that its rules don't contradict how the Factory works, and that this
//! machine has what its «Entorno» section asks for. What fails here would
//! otherwise be found by a builder that can't fix it, round after round.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use super::brain::Rule;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub rule_id: String,
    /// `error` blocks the seal; `warn` doesn't.
    pub severity: &'static str,
    /// `sintaxis`, `herramienta`, `trampa`, `factory` or `entorno`.
    pub kind: &'static str,
    pub message: String,
    pub fix: String,
}

fn finding(rule: &Rule, severity: &'static str, kind: &'static str, message: impl Into<String>, fix: impl Into<String>) -> Finding {
    Finding { rule_id: rule.id.clone(), severity, kind, message: message.into(), fix: fix.into() }
}

/// The base branch the line cuts every order from.
pub const BASE: &str = "dev";

/// What the brain's branch rule asks for: the ERE of a
/// `git branch --show-current | grep -E '…'` check, if it has one.
pub fn branch_pattern(rules: &[Rule]) -> Option<regex::Regex> {
    let re = regex::Regex::new(r#"git branch --show-current\s*\|\s*grep\s+-[a-zA-Z]*E[a-zA-Z]*\s+(?:'([^']+)'|"([^"]+)")"#).expect("static regex");
    rules.iter().filter_map(|r| r.check.as_deref()).find_map(|c| {
        let caps = re.captures(c)?;
        let pattern = caps.get(1).or(caps.get(2))?.as_str();
        regex::Regex::new(pattern).ok()
    })
}

/// Everything that doesn't need the machine: syntax, tools, traps, and the
/// Factory's own conventions. `sample_key` is an order key of this project
/// (`POK-1`), to try the branch rule on the names the line would make.
pub fn lint_static(rules: &[Rule], sample_key: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    // A diff or log range against `main` (`main...HEAD`): the order's branch
    // comes from `dev`, so it would count everything `dev` has. Asking
    // whether `main` is in `dev` (`merge-base --is-ancestor main dev`) is fine.
    let compares_main = regex::Regex::new(r"(?:\b(?:origin/)?main\.\.\.?|\.\.\.?(?:origin/)?main\b)").expect("static regex");
    for r in rules {
        let Some(cmd) = r.check.as_deref() else { continue };
        // Syntax, as the gate's own `sh -c` would read it.
        match std::process::Command::new("sh").args(["-n", "-c", cmd]).output() {
            Ok(o) if !o.status.success() => {
                let why = String::from_utf8_lossy(&o.stderr).trim().to_string();
                out.push(finding(r, "error", "sintaxis", format!("`{cmd}` no es un comando de shell válido: {why}"), "Corrige la sintaxis del check."));
                continue;
            }
            _ => {}
        }
        if let Some(program) = super::env::first_program(cmd).filter(|p| !p.contains('/') && !p.starts_with('$')) {
            if !super::env::has_executable(&program) {
                out.push(finding(r, "error", "herramienta", format!("`{program}` no está instalado en esta máquina"), format!("Instala {program} o cambia el check.")));
            }
        }
        if let Some(n) = check_ignore_paths(cmd) {
            out.push(finding(
                r,
                "error",
                "trampa",
                format!("`git check-ignore -q` con {n} rutas siempre falla: «--quiet is only valid with a single pathname»"),
                "Comprueba cada ruta por separado: `for p in node_modules dist; do git check-ignore -q \"$p\" || exit 1; done`.",
            ));
        }
        if compares_main.is_match(cmd) {
            out.push(finding(r, "error", "factory", format!("compara con `main`, pero la Factory corta cada orden desde `{BASE}`: nunca pasará"), format!("Compara con `{BASE}` (`{BASE}...HEAD`).")));
        }
        if quiet_grep_after_a_talker(cmd) {
            out.push(finding(
                r,
                "warn",
                "trampa",
                "`| grep -q` corta el pipe en cuanto encuentra algo: el comando de la izquierda puede morir con EPIPE y hacer fallar el check",
                "Usa `| grep … > /dev/null`, que lee hasta el final.",
            ));
        }
    }
    if let Some(re) = branch_pattern(rules) {
        let candidates = super::line::branch_candidates(sample_key, "titulo de ejemplo");
        if !candidates.iter().any(|b| re.is_match(b)) {
            let rule = rules.iter().find(|r| r.check.as_deref().is_some_and(|c| c.contains("git branch --show-current"))).expect("the pattern came from a rule");
            out.push(finding(
                rule,
                "error",
                "factory",
                format!("la regla de ramas no admite las que crea la Factory ({})", candidates.join(" o ")),
                format!("Que la regex acepte `feat/{sample_key}-<slug>`."),
            ));
        }
    }
    if !rules.iter().any(|r| r.section == "ent") {
        out.push(Finding {
            rule_id: String::new(),
            severity: "warn",
            kind: "entorno",
            message: "sin sección Entorno: la Factory no puede comprobar que esta máquina tiene lo que piden los checks (navegadores, runtimes, servicios)".into(),
            fix: "Añade «## Entorno» con un check que ejecute cada cosa, p. ej. que cada navegador de Playwright arranque.".into(),
        });
    }
    out
}

/// `pnpm e2e --list | grep -q x`: a grep that quits at the first match,
/// fed by a program that keeps writing — which then dies with EPIPE. A
/// one-line producer (`git branch --show-current`) is done before grep quits.
fn quiet_grep_after_a_talker(cmd: &str) -> bool {
    const TALKERS: [&str; 13] = ["pnpm", "npm", "npx", "yarn", "bun", "node", "playwright", "vitest", "jest", "pytest", "cargo", "go", "python"];
    let quiet = regex::Regex::new(r"^\s*grep\s+(?:-[a-zA-Z]*q[a-zA-Z]*|--quiet)\b").expect("static regex");
    let parts: Vec<&str> = cmd.split('|').collect();
    parts.windows(2).any(|w| {
        // `a || b` splits into an empty piece: not a pipe.
        if w[0].is_empty() || w[1].is_empty() || !quiet.is_match(w[1]) {
            return false;
        }
        let left = w[0].rsplit(['&', ';']).next().unwrap_or(w[0]);
        left.split_whitespace().next().is_some_and(|p| TALKERS.contains(&p))
    })
}

/// `git check-ignore -q a b` → `Some(2)`: more than one path with `-q`.
fn check_ignore_paths(cmd: &str) -> Option<usize> {
    let start = cmd.find("check-ignore")?;
    let rest = &cmd[start + "check-ignore".len()..];
    let end = rest.find(['|', ';', '&']).unwrap_or(rest.len());
    let words: Vec<&str> = rest[..end].split_whitespace().collect();
    let quiet = words.iter().any(|w| *w == "--quiet" || (w.starts_with('-') && !w.starts_with("--") && w.contains('q')));
    let paths = words.iter().filter(|w| !w.starts_with('-')).count();
    (quiet && paths > 1).then_some(paths)
}

/// The «Entorno» checks, run on this machine in the project's folder.
pub fn lint_env(rules: &[Rule], project: &str, dir: &Path) -> Vec<Finding> {
    rules
        .iter()
        .filter(|r| r.section == "ent")
        .filter_map(|r| {
            let cmd = r.check.as_deref()?;
            match super::env::shell(project, "lint", cmd, dir, Duration::from_secs(180)) {
                Ok(o) if o.status.success() => None,
                Ok(o) => {
                    let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
                    Some(finding(r, "error", "entorno", format!("esta máquina no cumple {}: `{cmd}`\n{}", r.id, super::line::failure_excerpt(&text)), "Instálalo o arréglalo en la máquina antes de sellar, o cambia la regla."))
                }
                Err(e) => Some(finding(r, "error", "entorno", format!("no se pudo ejecutar `{cmd}`: {e}"), "Revisa el comando.")),
            }
        })
        .collect()
}

pub fn has_errors(findings: &[Finding]) -> bool {
    findings.iter().any(|f| f.severity == "error")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, section: &str, check: &str) -> Rule {
        Rule { id: id.into(), section: section.into(), level: "must".into(), text: "x".into(), check: Some(check.into()) }
    }

    fn kinds(f: &[Finding], id: &str) -> Vec<(&'static str, &'static str)> {
        f.iter().filter(|x| x.rule_id == id).map(|x| (x.severity, x.kind)).collect()
    }

    /// The checks of POK's brain that cost rounds, each caught before sealing.
    #[test]
    fn the_checks_that_cost_pok_rounds_are_caught_before_sealing() {
        let rules = vec![
            rule("GIT-02", "git", "git check-ignore -q node_modules dist coverage .env"),
            rule("CAL-02", "cal", r"pnpm e2e --list | grep -q '\[firefox\]' && pnpm e2e"),
            rule("RSG-09", "rsg", r#"test "$(git diff --numstat main...HEAD -- . | awk '{s+=$1+$2} END{print s+0}')" -le 400"#),
            rule("GIT-04", "git", "git branch --show-current | grep -qE '^(main|(feat|fix|chore)/[a-z0-9-]+)$'"),
            rule("COD-01", "cod", "echo 'sin cerrar"),
            rule("OK-01", "cod", "git status --porcelain"),
            rule("GIT-06", "git", "git merge-base --is-ancestor main dev"),
        ];
        let f = lint_static(&rules, "POK-1");
        assert_eq!(kinds(&f, "GIT-02"), vec![("error", "trampa")]);
        assert_eq!(kinds(&f, "CAL-02"), vec![("warn", "trampa")]);
        assert_eq!(kinds(&f, "RSG-09"), vec![("error", "factory")]);
        assert!(kinds(&f, "GIT-04").is_empty(), "lowercase keys fit: the line can make `feat/pok-1-…`");
        assert_eq!(kinds(&f, "COD-01"), vec![("error", "sintaxis")]);
        assert!(kinds(&f, "OK-01").is_empty());
        assert!(kinds(&f, "GIT-06").is_empty(), "main inside dev is no comparison with main");
        assert!(f.iter().any(|x| x.rule_id.is_empty() && x.kind == "entorno"), "no Entorno section: a warning");

        let no_feat = vec![rule("GIT-04", "git", "git branch --show-current | grep -qE '^(main|task/[0-9]+)$'")];
        assert_eq!(kinds(&lint_static(&no_feat, "POK-1"), "GIT-04"), vec![("error", "factory")]);
        let missing = vec![rule("TEC-01", "tec", "definitely-not-a-tool-xyz --version")];
        assert_eq!(kinds(&lint_static(&missing, "POK-1"), "TEC-01"), vec![("error", "herramienta")]);
    }

    #[test]
    fn the_entorno_checks_run_on_this_machine() {
        let rules = vec![rule("ENT-01", "ent", "true"), rule("ENT-02", "ent", "echo 'browserType.launch: Failed to launch the browser process' >&2; exit 1")];
        let f = lint_env(&rules, "p-lint-tests", &std::env::temp_dir());
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].rule_id.as_str(), f[0].kind), ("ENT-02", "entorno"));
        assert!(f[0].message.contains("Failed to launch"));
    }
}
