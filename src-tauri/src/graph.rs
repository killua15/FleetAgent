//! Code knowledge graphs, built by `graphify` and served to agents over MCP.
//!
//! See `GRAPHIFY_TRD.md` for why this exists. The short version: six agents in
//! six worktrees each rebuild the same mental map of the repo from scratch, and
//! Hangar pays for it one turn at a time in `turn_usage`.
//!
//! Three rules from the TRD are enforced *here*, in code, because a promise
//! that lives only in a document is a promise that ships broken:
//!
//! 1. **`--code-only` by default** (§3). graphify's semantic pass sends docs,
//!    PDFs and images to a model; the AST pass never leaves the machine. The
//!    flag is the difference, and `build_args` is the only place that decides
//!    it. §3 is relaxable now — a scope can ask for docs to be indexed too via
//!    `IndexPolicy::WithDocs` — but only by being asked: the default, a NULL
//!    column and an unreadable value all resolve to code-only, and a test pins
//!    that. Code extraction stays local AST either way.
//! 2. **`--out`** (§3). The output lands in Hangar's app data, never in the
//!    user's checkout. Someone who uninstalls Hangar finds no residue.
//! 3. **One build in flight per scope** (§5.1). A rebuild racing six agents is
//!    CPU stolen from the work.
//!
//! What is deliberately *not* here: `graphify hook install` and `graphify
//! claude install`. Both write into the user's repo (a git hook, a `CLAUDE.md`
//! section). They are never invoked.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The binary graphify installs. Note the PyPI package is `graphifyy` (two
/// i's) — installing the name that matches the binary fails outright, which is
/// worth stating wherever we tell a user how to get it.
pub const BINARY: &str = "graphify";

/// The MCP server binary, installed alongside `graphify` by the same package.
pub const MCP_BINARY: &str = "graphify-mcp";

/// What a user has to run to get both. The `[sql]` extra is not optional for
/// us: without `tree_sitter_sql`, `.sql` files contribute nothing, and in this
/// repo that means `schema.sql` — the whole data model — is invisible to the
/// graph.
pub const INSTALL_HINT: &str = "uv tool install 'graphifyy[sql]'";

/// A graph belongs to a scope: one project, or a group of them (TRD §5.6).
/// Phase 1 only ever writes `Project`, but the column exists from the first
/// migration — adding it later means a second migration over a primary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScopeKind {
    Project,
    Group,
}

impl ScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ScopeKind::Project => "project",
            ScopeKind::Group => "group",
        }
    }

}

/// Where a scope's graph lives: `<app data>/graphs/<kind>-<id>/`.
///
/// graphify writes its own `graphify-out/` *inside* whatever `--out` names, so
/// this is the parent and [`graph_json`] is the file the MCP server is pointed
/// at.
pub fn scope_dir(app_data: &Path, kind: ScopeKind, scope_id: &str) -> PathBuf {
    app_data.join("graphs").join(format!("{}-{}", kind.as_str(), scope_id))
}

/// The graph file itself. `graphify-out` is graphify's own layout, not ours —
/// it is not configurable, only relocatable via `--out`.
pub fn graph_json(scope_dir: &Path) -> PathBuf {
    scope_dir.join("graphify-out").join("graph.json")
}

/// graphify's own interactive HTML. Hangar does **not** embed this: it loads
/// `vis-network` from `unpkg.com`, so it renders blank under the app's CSP and
/// needs the network besides (TRD §5.8). It is offered as "open in browser",
/// where those are the system browser's problem and not a broken panel.
pub fn graph_html(scope_dir: &Path) -> PathBuf {
    scope_dir.join("graphify-out").join("graph.html")
}

/// What a build is allowed to read, and therefore what leaves the machine.
///
/// `CodeOnly` is §3 as written: local AST extraction, no API key, nothing sent.
/// It is the default and the only value a new project or group is given.
///
/// `WithDocs` adds graphify's semantic pass over docs, papers and images, which
/// needs a model — so document *content* goes wherever that backend lives. It
/// is a deliberate, per-scope relaxation of §3 and never inherited, because the
/// difference between the two is the difference between "nothing leaves" and
/// "your notes leave", and that is not a default anyone should acquire by
/// accident. Code extraction is local AST in both cases; this only adds the
/// files `--code-only` skips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexPolicy {
    #[default]
    CodeOnly,
    WithDocs,
}

impl IndexPolicy {
    /// Round-trips through SQLite as text. An unrecognised value reads as
    /// `CodeOnly` rather than erroring: the safe direction for a column whose
    /// other value sends data somewhere.
    pub fn from_db(value: Option<&str>) -> Self {
        match value {
            Some("with_docs") => Self::WithDocs,
            _ => Self::CodeOnly,
        }
    }

    pub fn as_db(self) -> &'static str {
        match self {
            Self::CodeOnly => "code_only",
            Self::WithDocs => "with_docs",
        }
    }
}

/// The semantic backend used when docs are indexed and none was chosen.
///
/// `claude-cli` rather than an API-key backend: it drives the locally installed
/// `claude` binary on the user's existing Pro/Max subscription, so switching
/// docs on never requires a new credential — graphify's own words are "billed
/// to the plan, not pay-as-you-go API credit". It does send document text to
/// Anthropic. `ollama` is the answer for someone who wants docs indexed with
/// nothing leaving at all, which is why this is configurable and not hardcoded.
pub const DEFAULT_DOC_BACKEND: &str = "claude-cli";

/// The binary each doc backend shells out to, by the backend's own name.
///
/// graphify looks these up **itself**, by bare name on `$PATH`. That is the
/// whole reason this exists: Hangar already resolves binaries it runs directly
/// (`providers::resolve_binary`, which exists because a GUI app launched from
/// Finder inherits a `PATH` without `~/.local/bin`), but resolving a path does
/// nothing for a grandchild that searches for itself. Reported as
/// *"backend 'claude-cli' requires the `claude` CLI on $PATH"* with `claude`
/// sitting in `~/.local/bin` all along.
fn backend_binary(backend: &str) -> Option<&'static str> {
    match backend.trim() {
        "claude-cli" => Some("claude"),
        "ollama" => Some("ollama"),
        // An unknown backend is graphify's to complain about, and its message
        // is better than a guess made here.
        _ => None,
    }
}

/// The directory to put in front of graphify's `PATH` so it can find its
/// backend, or `None` when there is nothing useful to add.
///
/// `None` covers both "we don't know this backend" and "we could not find its
/// binary" — `resolve_binary` falls back to the bare name, which has no
/// directory, and inventing one would be worse than leaving `PATH` alone.
pub fn backend_bin_dir(backend: Option<&str>) -> Option<PathBuf> {
    let backend = backend.map(str::trim).filter(|b| !b.is_empty()).unwrap_or(DEFAULT_DOC_BACKEND);
    let resolved = crate::providers::resolve_binary(backend_binary(backend)?);
    PathBuf::from(resolved).parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf)
}

/// `dir` in front of an inherited `PATH`.
///
/// Prepended, never replacing: graphify needs its own interpreter and whatever
/// else its environment carries, so wiping `PATH` would trade one missing
/// binary for several.
///
/// Built with `join_paths` rather than a `:` of our own, because this repo also
/// ships a Windows installer and there the separator is `;`.
pub fn path_with(dir: &Path, inherited: Option<&str>) -> String {
    let rest = inherited.filter(|p| !p.is_empty()).unwrap_or_default();
    // Las entradas vacías se tiran, y no por estética: una entrada vacía en
    // `PATH` significa *el directorio actual* en Unix, así que dejarla haría
    // que graphify buscara binarios dentro del repo que está indexando.
    let inherited_dirs = std::env::split_paths(rest).filter(|p| !p.as_os_str().is_empty());
    let entries = std::iter::once(dir.to_path_buf()).chain(inherited_dirs);
    std::env::join_paths(entries)
        .map(|joined| joined.to_string_lossy().into_owned())
        // A directory with the separator in its name cannot be joined; leaving
        // `PATH` untouched beats handing the child a mangled one.
        .unwrap_or_else(|_| rest.to_string())
}

/// Extensions graphify's AST pass recognises — near enough, for a warning.
///
/// A deliberate and acknowledged copy of the tail of graphify's own
/// `detect.py:CODE_EXTENSIONS`. It drives **only** the pre-build warning below;
/// no build ever consults it, so drifting out of sync costs a warning that was
/// wrong and never a graph that was. That asymmetry is the reason it is allowed
/// to be a copy at all.
const CODE_EXTENSIONS: &[&str] = &[
    "py", "pyi", "ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "vue", "svelte", "astro", "go", "rs", "java", "kt", "kts", "scala", "swift", "m", "mm", "c", "h", "cc", "cpp", "hpp", "cs", "rb",
    "php", "lua", "zig", "dart", "ex", "exs", "jl", "sh", "bash", "ps1", "sql", "json", "tf", "f90", "v", "groovy",
];

/// Directories never worth walking: vendored or generated, and graphify's own
/// ignore rules drop them too. Without this, "does this repo have code?" walks
/// a `node_modules` to answer a question about the repo.
///
/// Every *hidden* directory is skipped as well, which is not tidiness — it was
/// found by running this against a real docs-only folder. Its single
/// code-extension file was `.claude/settings.local.json`, so counting it as
/// code made the whole warning fire never: the corpus looked non-empty and
/// graphify still produced zero nodes from it. Tool and editor config is not
/// source, and `.claude`, `.idea`, `.vscode`, `.venv` and `.git` are all the
/// same shape of not-source.
const SKIP_DIRS: &[&str] = &["node_modules", "target", "dist", "build", "vendor", "venv", "__pycache__", "Pods"];

/// What a checkout looks like to the AST pass, before spending a build on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Indexable {
    /// Files with a recognised code extension. Counting stops at the first one:
    /// every caller only asks "any?", and a repo with code is the case where a
    /// full walk would cost the most for the least.
    pub code_files: usize,
    /// Everything else, counted only when there was no code — which is exactly
    /// when the number is worth showing ("23 files, none of them code"). Zero
    /// otherwise, and meaningless there.
    pub other_files: usize,
}

/// Whether a build of this checkout could produce anything.
///
/// The cheap half of the answer: a repo with code exits on its first `.rs`,
/// and a repo without code is small enough to walk by definition of the
/// problem. Bounded anyway — a pathological tree must not hang the screen that
/// called this.
pub fn inspect_indexable(repo: &Path) -> Indexable {
    const MAX_ENTRIES: usize = 20_000;

    let mut other = 0usize;
    let mut seen = 0usize;
    let mut stack = vec![repo.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            if seen >= MAX_ENTRIES {
                // Out of budget with no code found. Reported as "no code" is
                // wrong; reported as code found is wrong too. One of them only
                // suppresses a warning, so it wins.
                return Indexable { code_files: 1, other_files: other };
            }
            seen += 1;
            let path = entry.path();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_ref()) {
                    stack.push(path);
                }
                continue;
            }
            match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase) {
                Some(ext) if CODE_EXTENSIONS.contains(&ext.as_str()) => {
                    return Indexable { code_files: 1, other_files: 0 };
                }
                _ => other += 1,
            }
        }
    }

    Indexable { code_files: 0, other_files: other }
}

/// The argv for building one scope's graph.
///
/// Split out from the spawn so the flags that carry a promise can be asserted
/// in a unit test rather than trusted. See the module docs.
pub fn build_args(repo: &Path, out: &Path, policy: IndexPolicy, backend: Option<&str>) -> Vec<String> {
    let mut args = vec!["extract".to_string(), repo.to_string_lossy().into_owned()];

    match policy {
        // §3: no doc/PDF/image pass, so no API key is read and nothing leaves
        // the machine.
        IndexPolicy::CodeOnly => args.push("--code-only".to_string()),
        // Dropping `--code-only` is what enables the semantic pass; naming a
        // backend is what makes it run without prompting for a key mid-build.
        IndexPolicy::WithDocs => {
            args.push("--backend".to_string());
            args.push(backend.filter(|b| !b.trim().is_empty()).unwrap_or(DEFAULT_DOC_BACKEND).to_string());
        }
    }

    // §3: output into Hangar's app data, never the user's checkout.
    args.push("--out".to_string());
    args.push(out.to_string_lossy().into_owned());
    args
}

/// The `--mcp-config` payload that points Claude at a built graph, alone.
///
/// Additive by construction: Hangar passes `--mcp-config` and **never**
/// `--strict-mcp-config`, which would suppress the user's own MCP servers for
/// the privilege of adding ours (TRD §5.2). `crate::agent_mcp` builds the file
/// agents actually get, with the team wiki beside this one.
#[cfg(test)]
pub fn mcp_config_json(mcp_binary: &str, graph: &Path) -> String {
    let server = serde_json::json!({ "mcpServers": { "graphify": graphify_entry(mcp_binary, graph) } });
    // Pretty-printed because this file is a debugging aid as much as a config:
    // when an agent can't see the graph, this is the first thing to read.
    serde_json::to_string_pretty(&server).unwrap_or_else(|_| String::new())
}

/// graphify's own entry in `mcpServers`.
pub fn graphify_entry(mcp_binary: &str, graph: &Path) -> serde_json::Value {
    serde_json::json!({ "command": mcp_binary, "args": [graph.to_string_lossy()] })
}

/// Whether `graphify` is installed. Reuses the resolver that already handles
/// the app being launched from Finder with a minimal PATH, and that already
/// looks in `~/.local/bin` — exactly where `uv tool install` puts things.
pub fn detect() -> Option<String> {
    let resolved = crate::providers::resolve_binary(BINARY);
    // `resolve_binary` falls back to the bare name so a PATH-based spawn can
    // still work in dev; a bare name means nothing was found on disk.
    Path::new(&resolved).is_absolute().then_some(resolved)
}

/// Same, for the MCP server. Both come from one package, but a partial install
/// is possible enough (and confusing enough) to report separately.
pub fn detect_mcp() -> Option<String> {
    let resolved = crate::providers::resolve_binary(MCP_BINARY);
    Path::new(&resolved).is_absolute().then_some(resolved)
}

/// Scopes with a build running right now.
///
/// §5.1: one build in flight per scope, because a graph rebuilding while six
/// agents work is CPU stolen from the real work. Keyed by `kind-id` so a
/// project's build and its group's build are correctly distinct.
static BUILDING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Claims the build slot for a scope. `None` means someone else holds it.
///
/// The guard releases on drop, so an early `?` in the middle of a build cannot
/// leave a scope permanently unbuildable — which is the failure mode that makes
/// users restart the app.
pub fn claim_build(kind: ScopeKind, scope_id: &str) -> Option<BuildGuard> {
    let key = format!("{}-{}", kind.as_str(), scope_id);
    let mut guard = BUILDING.lock().ok()?;
    let set = guard.get_or_insert_with(HashSet::new);
    // `then`, not `then_some`: `then_some` evaluates its argument eagerly, so a
    // refused claim would build a `BuildGuard` and immediately drop it — and
    // `Drop` locks this same mutex, which we are still holding. `std::Mutex`
    // isn't reentrant, so that deadlocks every future build in the process.
    set.insert(key.clone()).then(|| BuildGuard { key })
}

pub struct BuildGuard {
    key: String,
}

impl Drop for BuildGuard {
    fn drop(&mut self) {
        if let Ok(mut guard) = BUILDING.lock() {
            if let Some(set) = guard.as_mut() {
                set.remove(&self.key);
            }
        }
    }
}

/// What a finished build reports back, parsed from graphify's own stdout.
///
/// The counts are on the line it prints last:
/// `[graphify extract] wrote …/graph.json: 2613 nodes, 7332 edges, 105 communities`
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BuildCounts {
    pub nodes: i64,
    pub edges: i64,
}

/// Pulls the node/edge counts out of a build's output.
///
/// Parsed rather than recomputed by reading the JSON back: the file is 3.6 MB
/// for this repo, and the number we want is already on stdout. Returns
/// `Default` when the line is absent — a graph that built but whose counts we
/// couldn't read is still a usable graph, so this must not fail the build.
pub fn parse_counts(stdout: &str) -> BuildCounts {
    let Some(tail) = stdout.lines().rev().find_map(|line| line.split("graph.json:").nth(1)) else {
        return BuildCounts::default();
    };
    let number_before = |unit: &str| -> i64 {
        tail.split(unit)
            .next()
            .and_then(|head| head.split_whitespace().last())
            .and_then(|n| n.trim_end_matches(',').parse().ok())
            .unwrap_or_default()
    };
    BuildCounts { nodes: number_before("nodes"), edges: number_before("edges") }
}

/// A node label that means nothing on its own. Two repos both having a `build`
/// or a `state` says only that both are software.
///
/// Measured, not guessed: of 53 raw label collisions between Hangar and
/// `hangar-mobile`, 31 were noise of exactly this shape (TRD §5.7). Without a
/// filter the bridge wires half of one graph to half of the other and the seam
/// map becomes unreadable.
const GENERIC_LABELS: &[&str] = &[
    "base", "board", "build", "command", "connection", "data", "datetime", "description", "error", "event", "failure", "hello",
    "id", "kind", "level", "main", "mode", "name", "path", "provider", "result", "state", "style", "task", "type", "value",
];

/// Below this, a name is too short to carry meaning across a repo boundary.
const MIN_BRIDGE_LABEL: usize = 6;

/// A label appearing in more than this many files *within its own repo* is
/// vocabulary, not a type. `Envelope` lives in one file on each side; `Config`
/// lives in twenty.
const MAX_FILES_PER_REPO: usize = 3;

/// One end of a bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeEnd {
    pub repo: String,
    pub node_id: String,
    pub source_file: String,
}

/// A name that exists on both sides of the seam — the same type, declared
/// twice, once per language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bridge {
    /// The label as written, e.g. `PermissionRequest`.
    pub label: String,
    pub ends: Vec<BridgeEnd>,
}

/// A string field that graphify may write as `null` rather than omit.
///
/// `#[serde(default)]` covers a *missing* key, not an explicit `null`, and a
/// real graph has plenty of the latter: 306 of 4 480 nodes in the measured pair
/// carry `"source_file": null`. Without this the whole file fails to parse and
/// building any group dies with "is not a graphify graph" — which is exactly
/// what the end-to-end test caught, and what every unit test missed because
/// they all used hand-written fixtures.
fn null_as_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

/// A node as `merge-graphs` leaves it. Only the fields the bridge needs —
/// graphify's own schema is much wider and we do not depend on the rest.
#[derive(Debug, Clone, Deserialize)]
pub struct MergedNode {
    pub id: String,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub label: String,
    /// Set by `merge-graphs`, which prefixes ids and tags every node with the
    /// graph it came from. Absent in a single-repo graph.
    #[serde(default, deserialize_with = "null_as_empty")]
    pub repo: String,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub norm_label: String,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub source_file: String,
}

/// Finds the bridges in a merged graph.
///
/// **This is the one piece of graph logic Hangar owns** (TRD §5.7), and it is
/// deliberately a join, not an extractor: `merge-graphs` produces a union with
/// *zero* edges crossing between repos — measured, 0 of 10 194 — because
/// between two services the link is HTTP, a queue or a shared schema, and none
/// of those is an AST edge. What does cross is the name. `norm_label` is
/// already computed by graphify; this pairs nodes on it.
///
/// Deterministic, no model, no quota (§3 stays intact).
pub fn find_bridges(nodes: &[MergedNode]) -> Vec<Bridge> {
    use std::collections::BTreeMap;

    // How many distinct files each label occupies inside its own repo, which
    // is what separates a shared type from shared vocabulary.
    let mut spread: BTreeMap<(&str, &str), HashSet<&str>> = BTreeMap::new();
    for n in nodes {
        spread.entry((n.norm_label.as_str(), n.repo.as_str())).or_default().insert(n.source_file.as_str());
    }

    let mut by_label: BTreeMap<&str, BTreeMap<&str, &MergedNode>> = BTreeMap::new();
    for n in nodes {
        if n.norm_label.is_empty() || n.repo.is_empty() || n.source_file.is_empty() {
            continue;
        }
        // Keep the first node seen per (label, repo): a bridge names a seam,
        // not every occurrence of it.
        by_label.entry(n.norm_label.as_str()).or_default().entry(n.repo.as_str()).or_insert(n);
    }

    by_label
        .into_iter()
        .filter(|(label, ends)| {
            ends.len() > 1
                && label.len() >= MIN_BRIDGE_LABEL
                && !GENERIC_LABELS.contains(label)
                && ends.keys().all(|repo| spread.get(&(*label, *repo)).map_or(0, HashSet::len) <= MAX_FILES_PER_REPO)
        })
        .map(|(_, ends)| Bridge {
            label: ends.values().next().map(|n| n.label.clone()).unwrap_or_default(),
            ends: ends
                .into_iter()
                .map(|(repo, n)| BridgeEnd {
                    repo: repo.to_string(),
                    node_id: n.id.clone(),
                    source_file: n.source_file.clone(),
                })
                .collect(),
        })
        .collect()
}

/// Reads the nodes out of a merged `graph.json`.
///
/// The file is millions of bytes (5.9 MB for two medium repos), so this is the
/// one place that pays to parse it — callers get the bridges, not the graph.
pub fn read_merged_nodes(path: &Path) -> Result<Vec<MergedNode>, String> {
    #[derive(Deserialize)]
    struct Graph {
        #[serde(default)]
        nodes: Vec<MergedNode>,
    }
    let raw = std::fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let graph: Graph = serde_json::from_str(&raw).map_err(|e| format!("{} is not a graphify graph: {e}", path.display()))?;
    Ok(graph.nodes)
}

/// Writes the bridges into the merged graph as real edges, and reports how
/// many were added.
///
/// Without this the bridges are a drawing: the graph an agent queries over MCP
/// still has **zero** edges crossing between repos, which is the exact problem
/// §5.7 exists to solve. The seam map would show a connection the agent cannot
/// traverse.
///
/// Every added edge is tagged `_origin: "hangar-bridge"` and
/// `confidence: "INFERRED"`, matching graphify's own audit vocabulary. That
/// matters: graphify's whole pitch is that every edge says where it came from,
/// and edges we invented must not masquerade as ones it extracted from an AST.
///
/// The only place Hangar writes into a graphify artifact.
pub fn inject_bridges(graph_path: &Path, bridges: &[Bridge]) -> Result<usize, String> {
    if bridges.is_empty() {
        return Ok(0);
    }
    let raw = std::fs::read_to_string(graph_path).map_err(|e| format!("could not read {}: {e}", graph_path.display()))?;
    let mut graph: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("{} is not a graphify graph: {e}", graph_path.display()))?;

    let Some(links) = graph.get_mut("links").and_then(|l| l.as_array_mut()) else {
        return Err(format!("{} has no links array", graph_path.display()));
    };

    let mut added = 0;
    for bridge in bridges {
        // One edge per pair of ends, so a type present in three repos joins all
        // three rather than chaining through whichever happened to be first.
        for (i, from) in bridge.ends.iter().enumerate() {
            for to in bridge.ends.iter().skip(i + 1) {
                links.push(serde_json::json!({
                    "source": from.node_id,
                    "target": to.node_id,
                    "relation": "shared-type",
                    "confidence": "INFERRED",
                    "confidence_score": 0.5,
                    "_origin": "hangar-bridge",
                    "weight": 1.0,
                    "source_file": from.source_file,
                }));
                added += 1;
            }
        }
    }

    let out = serde_json::to_string(&graph).map_err(|e| e.to_string())?;
    std::fs::write(graph_path, out).map_err(|e| format!("could not write {}: {e}", graph_path.display()))?;
    Ok(added)
}

/// The compact architecture brief an agent gets before it starts (Phase 5).
///
/// **Deliberately not graphify's own `GRAPH_REPORT.md`**, which the TRD named.
/// That file is 25 KB for this repo — roughly 6 000 tokens on *every* turn's
/// system prompt. Handing that to an agent to save it context is a trade that
/// loses, and it would poison the Phase 3 comparison it is supposed to improve.
///
/// What survives the cut is what an agent cannot cheaply grep for: the
/// architectural hubs (a file's degree in the graph is not something twenty
/// greps reveal) and, for a group, the seam. A few hundred tokens, paid once
/// per turn, against the twenty-grep orientation it replaces.
pub fn architecture_brief(graph_path: &Path, bridges: &[Bridge], top_hubs: usize) -> Result<String, String> {
    use std::collections::BTreeMap;

    #[derive(Deserialize)]
    struct Graph {
        #[serde(default)]
        nodes: Vec<MergedNode>,
        #[serde(default)]
        links: Vec<Link>,
    }
    #[derive(Deserialize)]
    struct Link {
        #[serde(default, deserialize_with = "null_as_empty")]
        source: String,
        #[serde(default, deserialize_with = "null_as_empty")]
        target: String,
    }

    let raw = std::fs::read_to_string(graph_path).map_err(|e| format!("could not read {}: {e}", graph_path.display()))?;
    let graph: Graph = serde_json::from_str(&raw).map_err(|e| format!("{} is not a graphify graph: {e}", graph_path.display()))?;

    // Degree per *file*, not per node: "src-tauri/src/models.rs is central" is
    // actionable, "node g-fleet::pullrequestinfo is central" is not.
    let file_of: BTreeMap<&str, &str> = graph.nodes.iter().map(|n| (n.id.as_str(), n.source_file.as_str())).collect();
    let mut degree: BTreeMap<&str, usize> = BTreeMap::new();
    for link in &graph.links {
        for end in [link.source.as_str(), link.target.as_str()] {
            if let Some(file) = file_of.get(end).filter(|f| !f.is_empty()) {
                *degree.entry(file).or_default() += 1;
            }
        }
    }

    let mut hubs: Vec<(&str, usize)> = degree.into_iter().collect();
    hubs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    hubs.truncate(top_hubs);

    let mut out = String::from("# Architecture, from this repo's code graph\n\n");
    out.push_str(&format!(
        "Extracted from the AST of {} symbols and {} relations. Ask the `graphify` MCP \
         tools (`query_graph`, `shortest_path`, `god_nodes`) before grepping to orient yourself.\n\n",
        graph.nodes.len(),
        graph.links.len()
    ));

    if !hubs.is_empty() {
        out.push_str("## Most connected files\n\nEditing these reaches further than it looks:\n\n");
        for (file, deg) in &hubs {
            out.push_str(&format!("- `{file}` — {deg} relations\n"));
        }
        out.push('\n');
    }

    if !bridges.is_empty() {
        out.push_str(&format!(
            "## The seam between repos\n\n{} type names are declared on both sides. \
             There is no compiler checking these agree — changing one side silently breaks the other:\n\n",
            bridges.len()
        ));
        for bridge in bridges {
            let files: Vec<&str> = bridge.ends.iter().map(|e| e.source_file.as_str()).collect();
            out.push_str(&format!("- **{}** — {}\n", bridge.label, files.join(" ↔ ")));
        }
    }
    Ok(out)
}

/// Where a scope's brief lives. Small, so reading it at every spawn is free —
/// unlike the multi-megabyte graph it was distilled from.
pub fn brief_path(scope_dir: &Path) -> PathBuf {
    scope_dir.join("brief.md")
}

/// How many edges a built graph has. Reported to the UI, where a hardcoded
/// zero next to a finished build reads as a broken index.
pub fn count_edges(graph_path: &Path) -> Result<i64, String> {
    #[derive(Deserialize)]
    struct Links {
        #[serde(default)]
        links: Vec<serde_json::Value>,
    }
    let raw = std::fs::read_to_string(graph_path).map_err(|e| e.to_string())?;
    let parsed: Links = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    Ok(parsed.links.len() as i64)
}

/// The argv for re-clustering a graph after the bridges went in.
///
/// **`--no-label` is not optional.** Community naming calls a model, and §3
/// says nothing leaves the machine. Without it, building a group would quietly
/// start spending quota and sending code structure to an API.
///
/// This is also what produces `graph.html`: `merge-graphs` writes only JSON, so
/// without a clustering pass a group would never have a visualization to open.
pub fn recluster_args(scope_dir: &Path) -> Vec<String> {
    vec![
        "cluster-only".to_string(),
        scope_dir.to_string_lossy().into_owned(),
        "--no-label".to_string(),
    ]
}

/// The argv for merging member graphs into one group graph.
///
/// graphify's own command (§5.6), not a composed root and not a merge we
/// wrote: it prefixes ids per repo and tags each node with its origin, which
/// is exactly what [`find_bridges`] needs to tell the sides apart.
pub fn merge_args(member_graphs: &[PathBuf], out: &Path) -> Vec<String> {
    let mut args = vec!["merge-graphs".to_string()];
    args.extend(member_graphs.iter().map(|p| p.to_string_lossy().into_owned()));
    args.push("--out".to_string());
    args.push(out.to_string_lossy().into_owned());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, repo: &str, label: &str, file: &str) -> MergedNode {
        MergedNode {
            id: id.to_string(),
            label: label.to_string(),
            repo: repo.to_string(),
            norm_label: label.to_lowercase(),
            source_file: file.to_string(),
        }
    }

    /// The flags that carry §3's promises. If these drift, the app starts
    /// sending source to a model or writing into the user's checkout — neither
    /// of which is visible in a diff of the calling code.
    ///
    /// Docs may now be indexed, but only on an explicit `WithDocs`. The cases
    /// below pin the *default* rather than the feature: a new scope, a NULL
    /// column and an unreadable value must all come out code-only, because that
    /// is the difference between "nothing leaves" and "your notes leave".
    /// Cada backend busca su binario por nombre, y ese mapeo es lo único que
    /// Hangar puede aportar: un backend que no conoce se lo deja a graphify,
    /// cuyo mensaje es mejor que una suposición nuestra.
    #[test]
    fn every_known_backend_names_the_binary_it_shells_out_to() {
        assert_eq!(backend_binary("claude-cli"), Some("claude"));
        assert_eq!(backend_binary("ollama"), Some("ollama"));
        assert_eq!(backend_binary(" claude-cli "), Some("claude"), "el valor viene de la base y puede traer espacios");
        assert_eq!(backend_binary("un-backend-que-no-existe"), None);
        assert_eq!(backend_binary(""), None);
    }

    /// Anteponer, nunca reemplazar: graphify necesita su propio intérprete y lo
    /// que traiga su entorno, así que borrar el `PATH` cambiaría un binario
    /// que falta por varios.
    #[test]
    fn the_backend_directory_goes_in_front_without_losing_the_rest() {
        let sep = if cfg!(windows) { ';' } else { ':' };
        let inherited = format!("/usr/bin{sep}/bin");
        let built = path_with(Path::new("/Users/me/.local/bin"), Some(&inherited));

        let entries: Vec<String> = std::env::split_paths(&built).map(|p| p.display().to_string()).collect();
        assert_eq!(entries[0], "/Users/me/.local/bin", "el directorio del backend va primero: {built}");
        assert!(entries.contains(&"/usr/bin".to_string()), "lo heredado sigue ahí: {built}");
        assert!(entries.contains(&"/bin".to_string()), "entero, no solo el primero: {built}");
    }

    /// Un `PATH` vacío o ausente no debe producir uno con un separador suelto.
    /// No es cosmético: una entrada vacía significa «el directorio actual», así
    /// que graphify buscaría binarios dentro del repo que está indexando.
    #[test]
    fn an_empty_inherited_path_is_just_the_directory() {
        for inherited in [None, Some("")] {
            let built = path_with(Path::new("/opt/bin"), inherited);
            assert_eq!(built, "/opt/bin", "got: {built:?} para {inherited:?}");
        }
    }

    /// La comprobación que de verdad importa, y que solo se puede hacer en una
    /// máquina con Claude Code instalado: que el backend por defecto se
    /// resuelva **sin** el `PATH` del shell, que es la situación exacta de la
    /// app empaquetada y el origen del fallo reportado.
    ///
    /// `#[ignore]` porque depende de la máquina, como los demás que hablan con
    /// CLIs reales. Ejecutado aquí con `PATH=/usr/bin:/bin`, donde `claude` no
    /// aparece, y aun así resuelto por `install_locations`.
    #[test]
    #[ignore = "needs Claude Code installed; run with PATH=/usr/bin:/bin to reproduce the packaged app"]
    fn the_default_backend_resolves_without_a_shell_path() {
        let dir = backend_bin_dir(None).expect("claude tiene que encontrarse aunque no esté en PATH");
        assert!(dir.join("claude").exists() || dir.join("claude.cmd").exists(), "no hay claude en {}", dir.display());
    }

    /// `resolve_binary` cae al nombre pelado cuando no encuentra nada, y de ahí
    /// no sale ningún directorio. Inventarse uno sería peor que dejar el `PATH`
    /// en paz.
    #[test]
    fn a_backend_whose_binary_is_nowhere_adds_no_directory() {
        assert_eq!(backend_bin_dir(Some("un-backend-que-no-existe")), None);
    }
    #[test]
    fn a_build_is_code_only_unless_docs_were_explicitly_asked_for() {
        let repo = Path::new("/repos/thing");
        let out_dir = Path::new("/appdata/graphs/project-1");

        for policy in [IndexPolicy::default(), IndexPolicy::from_db(None), IndexPolicy::from_db(Some("")), IndexPolicy::from_db(Some("WITH_DOCS")), IndexPolicy::from_db(Some("nonsense"))] {
            let args = build_args(repo, out_dir, policy, Some("claude-cli"));
            assert!(args.contains(&"--code-only".to_string()), "the semantic pass reaches a model: it must stay off unless asked. policy {policy:?} got: {args:?}");
            assert!(!args.contains(&"--backend".to_string()), "no backend may be named without --with-docs. policy {policy:?} got: {args:?}");
        }

        // And the opt-in actually opts in, or the setting is decoration.
        let with_docs = build_args(repo, out_dir, IndexPolicy::WithDocs, None);
        assert!(!with_docs.contains(&"--code-only".to_string()), "--code-only would suppress the very pass this policy asks for: {with_docs:?}");
        let backend = with_docs.iter().position(|a| a == "--backend").expect("docs need a backend named, or graphify prompts mid-build");
        assert_eq!(with_docs[backend + 1], DEFAULT_DOC_BACKEND, "an unset backend must fall back to the subscription-based one, not to an API key");

        // Whatever the policy, output never lands in the checkout.
        for args in [build_args(repo, out_dir, IndexPolicy::CodeOnly, None), with_docs] {
            let out = args.iter().position(|a| a == "--out").expect("--out must be passed");
            assert_eq!(args[out + 1], "/appdata/graphs/project-1", "output must land in the app data, not the repo");
            assert!(!args.iter().any(|a| a.contains("/repos/thing/graphify-out")), "nothing may be written inside the user's checkout");
        }
    }

    /// The case this function exists for, and the one that nearly slipped
    /// through: a documentation folder whose only code-extension file is agent
    /// config in a hidden directory. graphify counts that as "1 code file" and
    /// then produces zero nodes from it, so a warning that trusted the count
    /// would never fire on the exact folder that breaks the build.
    #[test]
    fn a_docs_folder_reads_as_empty_even_with_config_in_a_hidden_dir() {
        let dir = std::env::temp_dir().join(format!("hangar-indexable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::write(dir.join("use-cases.md"), "# docs").unwrap();
        std::fs::write(dir.join("notes.md"), "more").unwrap();
        std::fs::write(dir.join("preview.html"), "<p>").unwrap();
        std::fs::write(dir.join(".claude/settings.local.json"), "{}").unwrap();

        let found = inspect_indexable(&dir);
        assert_eq!(found.code_files, 0, "config in a dot-directory is not source");
        assert_eq!(found.other_files, 3, "the docs are what is actually there");

        // One real source file is enough to stop warning, and stops the walk.
        std::fs::write(dir.join("main.rs"), "fn main() {}").unwrap();
        assert_eq!(inspect_indexable(&dir).code_files, 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A blank backend is what a settings field yields when someone clears it,
    /// and it must not reach graphify as an empty `--backend ''`.
    #[test]
    fn a_blank_backend_falls_back_rather_than_passing_nothing() {
        for blank in [Some(""), Some("   "), None] {
            let args = build_args(Path::new("/r"), Path::new("/o"), IndexPolicy::WithDocs, blank);
            let backend = args.iter().position(|a| a == "--backend").expect("--backend must be present");
            assert_eq!(args[backend + 1], DEFAULT_DOC_BACKEND, "blank backend {blank:?} must fall back");
        }
    }

    /// Additive or nothing (§5.2). `--strict-mcp-config` would switch off the
    /// user's own servers, and this config is what the flag points at.
    #[test]
    fn the_mcp_config_names_the_graph_and_nothing_else() {
        let json = mcp_config_json("/bin/graphify-mcp", Path::new("/appdata/graphs/project-1/graphify-out/graph.json"));
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["mcpServers"]["graphify"]["command"], "/bin/graphify-mcp");
        assert_eq!(parsed["mcpServers"]["graphify"]["args"][0], "/appdata/graphs/project-1/graphify-out/graph.json");
        assert_eq!(parsed["mcpServers"].as_object().unwrap().len(), 1, "Hangar registers one server; the rest are the user's");
    }

    /// Real output from the spike, verbatim. The parser exists so a build can
    /// report 2 613 nodes without reading a 3.6 MB file back.
    #[test]
    fn counts_come_off_graphifys_own_last_line() {
        let stdout = "[graphify extract] AST extraction on 182 code files...\n\
                      [graphify extract] wrote /tmp/g/graphify-out/graph.json: 2613 nodes, 7332 edges, 105 communities\n\
                      [graphify extract] wrote /tmp/g/graphify-out/.graphify_analysis.json\n";

        assert_eq!(parse_counts(stdout), BuildCounts { nodes: 2613, edges: 7332 });
    }

    /// A build whose output we can't parse still produced a graph. Failing here
    /// would throw away a perfectly good index over a cosmetic number.
    #[test]
    fn unreadable_output_is_zero_counts_not_an_error() {
        assert_eq!(parse_counts("something went sideways"), BuildCounts::default());
    }

    /// The case the whole feature exists for, with the real pair from the
    /// spike: one type declared on each side of the wire, in two languages
    /// that share no compiler. `merge-graphs` gives zero edges between them —
    /// this is what puts the seam back.
    #[test]
    fn a_type_declared_in_both_repos_is_a_bridge() {
        let nodes = vec![
            node("g-fleet::envelope", "g-fleet", "Envelope", "src-tauri/src/bridge/protocol.rs"),
            node("g-mobile::envelope", "g-mobile", "Envelope", "lib/bridge/envelope.dart"),
        ];

        let bridges = find_bridges(&nodes);

        assert_eq!(bridges.len(), 1);
        assert_eq!(bridges[0].label, "Envelope");
        let files: Vec<&str> = bridges[0].ends.iter().map(|e| e.source_file.as_str()).collect();
        assert!(files.contains(&"src-tauri/src/bridge/protocol.rs"), "got: {files:?}");
        assert!(files.contains(&"lib/bridge/envelope.dart"), "got: {files:?}");
    }

    /// A name living only on one side is not a seam, however often it appears.
    #[test]
    fn a_label_in_one_repo_only_is_not_a_bridge() {
        let nodes = vec![
            node("g-fleet::rotation", "g-fleet", "RotationPolicy", "src-tauri/src/rotation.rs"),
            node("g-fleet::rotation2", "g-fleet", "RotationPolicy", "src/types/settings.ts"),
        ];

        assert!(find_bridges(&nodes).is_empty(), "one repo cannot bridge to itself");
    }

    /// The filter that decides whether the seam map is readable. Of 53 raw
    /// collisions in the spike, 31 were noise like these — wiring them up
    /// connects half of one graph to half of the other and says nothing.
    #[test]
    fn generic_and_short_names_are_not_bridges() {
        let nodes = vec![
            node("a::build", "a", "build", "package.json"),
            node("b::build", "b", "build", "lib/main.dart"),
            node("a::state", "a", "state", "src/store.ts"),
            node("b::state", "b", "state", "lib/state.dart"),
            // Short enough to be an accident even though it isn't in the list.
            node("a::env", "a", "env", "src/env.ts"),
            node("b::env", "b", "env", "lib/env.dart"),
        ];

        assert!(find_bridges(&nodes).is_empty(), "got: {:?}", find_bridges(&nodes));
    }

    /// A name spread across many files inside its own repo is vocabulary, not
    /// a type: `Envelope` lives in one file per side, `Settings` in twenty.
    #[test]
    fn a_label_spread_across_its_own_repo_is_vocabulary_not_a_type() {
        let mut nodes = vec![node("b::settings", "b", "SettingsPanel", "lib/settings.dart")];
        for i in 0..5 {
            nodes.push(node(&format!("a::settings{i}"), "a", "SettingsPanel", &format!("src/screens/s{i}.tsx")));
        }

        assert!(find_bridges(&nodes).is_empty(), "five files in one repo is not a shared type");
    }

    /// A node with no file can't be pointed at, and a bridge whose whole job is
    /// to say "this file and that file" would be useless without one.
    #[test]
    fn nodes_without_a_source_file_are_skipped() {
        let nodes = vec![
            node("a::permissionrequest", "a", "PermissionRequest", ""),
            node("b::permissionrequest", "b", "PermissionRequest", "lib/domain/entities/permission_request.dart"),
        ];

        assert!(find_bridges(&nodes).is_empty(), "a bridge needs a file at both ends to be worth drawing");
    }

    /// graphify's own merge command, not one we wrote. Its id prefixing is
    /// what makes `find_bridges` able to tell the sides apart at all.
    #[test]
    fn the_merge_uses_graphifys_own_command() {
        let args = merge_args(
            &[PathBuf::from("/a/graphify-out/graph.json"), PathBuf::from("/b/graphify-out/graph.json")],
            Path::new("/appdata/graphs/group-g1/graph.json"),
        );

        assert_eq!(args[0], "merge-graphs");
        assert!(args.contains(&"/a/graphify-out/graph.json".to_string()));
        assert!(args.contains(&"/b/graphify-out/graph.json".to_string()));
        let out = args.iter().position(|a| a == "--out").expect("--out must be passed");
        assert_eq!(args[out + 1], "/appdata/graphs/group-g1/graph.json");
    }

    /// The bridges have to reach the graph the MCP serves, not just the
    /// screen. And they have to be labelled as ours: graphify's whole audit
    /// story is that every edge says where it came from.
    #[test]
    fn bridges_become_real_edges_marked_as_ours() {
        let dir = std::env::temp_dir().join(format!("hangar-bridge-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("graph.json");
        std::fs::write(&path, r#"{"nodes":[],"links":[{"source":"a::x","target":"a::y","relation":"contains"}]}"#).unwrap();

        let bridges = vec![Bridge {
            label: "Envelope".to_string(),
            ends: vec![
                BridgeEnd { repo: "a".into(), node_id: "a::envelope".into(), source_file: "protocol.rs".into() },
                BridgeEnd { repo: "b".into(), node_id: "b::envelope".into(), source_file: "envelope.dart".into() },
            ],
        }];

        assert_eq!(inject_bridges(&path, &bridges).unwrap(), 1);

        let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let links = written["links"].as_array().unwrap();
        assert_eq!(links.len(), 2, "the original edge must survive");
        let ours = links.iter().find(|l| l["_origin"] == "hangar-bridge").expect("the bridge must be written");
        assert_eq!(ours["source"], "a::envelope");
        assert_eq!(ours["target"], "b::envelope");
        assert_eq!(ours["confidence"], "INFERRED", "we inferred this; it must not pass as extracted from an AST");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Nothing to add means nothing touched — a group with no bridges must not
    /// have its graph rewritten for no reason.
    #[test]
    fn no_bridges_leaves_the_graph_alone() {
        assert_eq!(inject_bridges(Path::new("/definitely/not/here.json"), &[]).unwrap(), 0);
    }

    /// Community naming calls a model. §3 says nothing leaves the machine, so
    /// this flag is the difference between a local rebuild and quota being
    /// spent behind the user's back.
    #[test]
    fn reclustering_never_calls_a_model() {
        let args = recluster_args(Path::new("/appdata/graphs/group-g1"));

        assert_eq!(args[0], "cluster-only");
        assert!(args.contains(&"--no-label".to_string()), "community labelling reaches an API: it must stay off. got: {args:?}");
    }

    /// §5.1. The second caller has to be turned away, or six worktrees rebuild
    /// the same graph six times in parallel.
    #[test]
    fn only_one_build_per_scope_runs_at_a_time() {
        let first = claim_build(ScopeKind::Project, "p-solo");
        assert!(first.is_some());
        assert!(claim_build(ScopeKind::Project, "p-solo").is_none(), "a second build on the same scope must be refused");
        // A different scope is unaffected — the lock is per scope, not global.
        assert!(claim_build(ScopeKind::Group, "p-solo").is_some(), "a group's build must not be blocked by a project's");

        drop(first);
        assert!(claim_build(ScopeKind::Project, "p-solo").is_some(), "the slot must free up, or one crash makes a scope unbuildable forever");
    }
}
