#!/usr/bin/env bash
# Phase 0 spike for GRAPHIFY_TRD.md — reproduces the numbers in §5.7 and §6.
#
# Builds a code-only graph for two projects, merges them with graphify's own
# `merge-graphs`, and measures what actually connects the two repos:
#   - edges that cross from one repo to the other (measured: 0)
#   - nodes sharing a normalized label across repos — the bridge candidates
#
# Nothing is written inside either repo: `extract --out` keeps the output in
# OUT_DIR, which is what lets Hangar honour "we don't touch the user's repo".
#
# Usage: spike.sh <repo-a> <repo-b> [out-dir]
set -euo pipefail

REPO_A="${1:?usage: spike.sh <repo-a> <repo-b> [out-dir]}"
REPO_B="${2:?usage: spike.sh <repo-a> <repo-b> [out-dir]}"
OUT_DIR="${3:-$(mktemp -d)}"

# The PyPI package is `graphifyy` (two i's); the binary it installs is `graphify`.
# The [sql] extra is not optional here: without tree_sitter_sql, .sql files
# contribute nothing and schema.sql becomes invisible to the graph.
command -v graphify >/dev/null || {
  echo "graphify not found. Install with: uv tool install 'graphifyy[sql]'" >&2
  exit 1
}

mkdir -p "$OUT_DIR/a" "$OUT_DIR/b"
graphify extract "$REPO_A" --code-only --out "$OUT_DIR/a"
graphify extract "$REPO_B" --code-only --out "$OUT_DIR/b"
graphify merge-graphs \
  "$OUT_DIR/a/graphify-out/graph.json" \
  "$OUT_DIR/b/graphify-out/graph.json" \
  --out "$OUT_DIR/merged.json"

python3 - "$OUT_DIR/merged.json" <<'PY'
import collections, json, sys

merged = json.load(open(sys.argv[1]))
# merge-graphs prefixes every id with a repo tag and records it on the node,
# which is what makes provenance answerable at all.
owner = {n["id"]: n["repo"] for n in merged["nodes"]}
cross = [e for e in merged["links"] if owner[e["source"]] != owner[e["target"]]]

print()
print("nodes per repo:", dict(collections.Counter(owner.values())))
print(f"edges: {len(merged['links'])}, crossing repos: {len(cross)}")

# Community ids are NOT comparable across merged graphs: each source graph was
# clustered on its own, so `community 49` exists on both sides and means two
# different things. Any "mixed communities" metric computed on the raw field is
# an artefact. Bridges have to come from labels instead.
by_label = collections.defaultdict(lambda: collections.defaultdict(list))
for n in merged["nodes"]:
    by_label[n["norm_label"]][n["repo"]].append(n)
bridges = {k: v for k, v in by_label.items() if len(v) > 1}

print(f"bridge candidates (same normalized label, different repos): {len(bridges)}")
for label in sorted(bridges)[:15]:
    sides = [f"{repo}:{nodes[0]['source_file'] or '?'}" for repo, nodes in bridges[label].items()]
    print(f"  {label:22s} {'  <->  '.join(sides)}")
PY
