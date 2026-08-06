# Hangar

**Mission control for your coding agents.**

A local-first desktop app that runs real agent CLI terminals in parallel — each in its own git
worktree, each under the account it's supposed to use. When an account runs out of quota, Hangar
carries on with another one, keeping the conversation. Nothing leaves the machine: state is SQLite,
credentials are in the OS keychain.

Claude Code is the provider wired end to end today. Codex and Gemini are recognized but not yet
supported (the UI marks them as such).

## What it does

- **A grid of real terminals** — one PTY each (up to six), spawned through your login shell so they
  behave exactly like typing `claude` in Terminal.app. Minimized terminals keep running, and open
  terminals come back with their conversation after a relaunch.
- **Isolated worktrees** — every subsession works in `.hangar/worktrees/<slug>` on its own branch,
  so parallel agents never fight over the same files.
- **Multi-account, no cross-contamination** — each account gets its own isolated CLI config dir;
  work and personal accounts are separate scopes and are never mixed silently.
- **Capacity** — real 5h/weekly usage read from the CLI, monthly spend caps, and opt-in per-project
  rotation that fails over to another allowed account of the same provider when one is exhausted.
- **Continuity** — a conversation can move between accounts: the transcript is placed where the
  destination CLI looks and resumed there.
- **Governance** — four session modes (`plan` read-only, ask-edits, auto-edit, yolo) plus a
  permission bridge that surfaces each tool call for approval, even in headless turns.
- **Git and PRs** — diffs, branches, per-turn checkpoints, PR creation via `gh`, a review inbox, and
  sending PR review comments straight back into a session.
- **Hierarchies and shared memory** — parent agents that spawn children (three levels, three views)
  and memory groups shared across projects.
- **Per-project agent context** — the language an agent answers in and the one it writes code in, set
  globally and overridable per project, plus a library of Agent Skills you pick per project. All of it
  reaches the CLI as spawn flags, so nothing is written into your repository. See
  [AGENT_CONTEXT.md](AGENT_CONTEXT.md).

## Development

Requires Node 22+, a stable Rust toolchain, and the `claude` CLI on PATH for anything that actually
runs an agent.

```sh
npm install
npm run tauri dev      # run the app
npm run build          # typecheck + build the frontend
cd src-tauri && cargo test
```

Tests marked `#[ignore]` spawn the real `claude` CLI — run them with `cargo test -- --ignored`.

## Layout

| Path | What's in it |
| --- | --- |
| `src/` | React + TypeScript frontend (screens, components, design tokens) |
| `src-tauri/src/` | Rust backend: commands, PTY terminals, turn runner, rotation, git/PR, SQLite |
| `src-tauri/src/db/schema.sql` | The schema, applied idempotently on startup |
| `src-tauri/tests/fixtures/` | Real captured CLI output used by the parser tests — recorded data, not to be edited |

## Design documents

| Document | What it covers |
| --- | --- |
| [AGENT_CONTEXT.md](AGENT_CONTEXT.md) | How language preferences and skills reach the CLI: where they're configured, where they land on disk, which flag carries them, and what was verified by actually running each CLI |
| [TERMINAL_CONTINUITY.md](TERMINAL_CONTINUITY.md) | How a conversation survives a relaunch and an account switch |
| [MOBILE_BRIDGE_TRD.md](MOBILE_BRIDGE_TRD.md) | The phone companion: protocol, encryption, pairing ([build docs](docs/mobile/README.md)) |
| [GRAPHIFY_TRD.md](GRAPHIFY_TRD.md) | Draft, nothing implemented — a code knowledge graph shared by parallel agents |
| [ROADMAP.md](ROADMAP.md) | Where this is going |

## License

**GNU AGPL-3.0** — see [LICENSE](LICENSE). Copyright © 2026 David Lopez.

Free to use, study, change and share. The one obligation that matters: if you distribute a modified
version, or **run one as a network service**, you have to offer its source under the same terms.

The choice is deliberate. Hangar's edge is that no other agent orchestrator manages accounts —
isolation per account, rotation on exhaustion, the conversation surviving the switch. Every
comparable tool is MIT or Apache-2.0, which would let any of them fold that work into a closed or
permissive product without giving anything back. The AGPL doesn't stop anyone from *using* Hangar; it
stops that.

Two consequences worth knowing up front:

- **A hosted Hangar would have to publish its source** (AGPL §13). A separate team dashboard that
  merely talks to the same database is not a derivative work and isn't covered — but hosting the app
  itself is.
- **Only the copyright holder can relicense.** Contributions are welcome; if this ever needs
  dual-licensing, that conversation has to happen with contributors rather than around them.

## Note on the rename

This project was called **Fleet** until July 2026. The rename is complete, with two deliberate
exceptions kept for compatibility: worktrees created before it still live in `.fleet/` and are still
honored, and PR review branches named `fleet/pr-<n>-review` are still recognized. App data is moved
automatically from the old identifier on first launch (`src-tauri/src/app_data.rs`).
