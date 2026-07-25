//! Moving a Claude Code conversation between accounts.
//!
//! Verified against the real CLI (v2.1.219): `claude --resume <uuid>` reads and
//! continues a transcript the destination config dir never created — a
//! conversation is not tied to an account, only to where its file lives (see
//! `TERMINAL_CONTINUITY.md` §2 and the Phase 0 spike). So switching a terminal
//! to another Claude account *keeping the conversation* is a file copy: put the
//! transcript where the destination CLI looks, then resume it.
//!
//! Only claude→claude. Codex/Gemini transcripts use different schemas; that
//! path is a context handoff (a rendered summary as a first prompt), a
//! different mechanism.

use serde_json::Value;
use std::path::{Path, PathBuf};

/// One turn of the conversation, flattened. `role` is `"user"` or
/// `"assistant"`; `text` is the human-readable message (thinking and tool
/// plumbing stripped).
#[derive(Debug, Clone, PartialEq)]
pub struct NeutralTurn {
    pub role: String,
    pub text: String,
}

/// A Claude Code conversation reduced to what carries across providers: the
/// message turns, the actions taken as prose, the files touched, and the
/// working context. This is the "understanding, not replay" contract — tool
/// calls become descriptions because no two CLIs share a tool schema (see the
/// module docs and `TERMINAL_CONTINUITY.md` §2 Caso B).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NeutralSession {
    pub turns: Vec<NeutralTurn>,
    /// Meaningful actions, in order: `"edited src/a.rs"`, "ran `npm test`".
    pub actions: Vec<String>,
    /// Distinct files written/edited, for the destination agent's working set.
    pub modified_files: Vec<String>,
    pub cwd: Option<String>,
    pub branch: Option<String>,
}

/// Pulls the plain text out of a message's `content` (a string, or an array of
/// `{type:"text", text:...}` items). Thinking, tool_use and tool_result items
/// are skipped — they're handled separately or dropped.
fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter(|it| it.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|it| it.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Whether a `user` line is really a tool result being fed back (not a human
/// message) — those carry `tool_result` content items and shouldn't count as a
/// conversation turn.
fn is_tool_result(content: &Value) -> bool {
    content
        .as_array()
        .is_some_and(|items| items.iter().any(|it| it.get("type").and_then(|t| t.as_str()) == Some("tool_result")))
}

/// One assistant `tool_use` item → an action line, plus a modified-file path
/// when it's an edit. Mirrors `turn_runner`'s tool formatting but for the
/// summary, and only for the tools worth surfacing (edits and shell commands);
/// reads and internal tools are noise in a handoff.
fn describe_tool_use(item: &Value, actions: &mut Vec<String>, modified: &mut Vec<String>) {
    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let input = item.get("input").cloned().unwrap_or(Value::Null);
    match name {
        "Bash" => {
            if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
                actions.push(format!("ran `{}`", cmd.trim()));
            }
        }
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            if let Some(path) = input.get("file_path").and_then(|v| v.as_str()) {
                actions.push(format!("edited {path}"));
                if !modified.iter().any(|p| p == path) {
                    modified.push(path.to_string());
                }
            }
        }
        _ => {}
    }
}

/// Parses an on-disk Claude Code transcript (`projects/<slug>/<uuid>.jsonl`)
/// into a `NeutralSession`. The on-disk format is one JSON object per line with
/// a `type` (`user`/`assistant`/…) and the same `message.content` shape the
/// live stream uses, plus `cwd`/`gitBranch` metadata.
pub fn parse_claude_transcript(config_dir: &str, session_uuid: &str) -> Result<NeutralSession, String> {
    let path = find_transcript(config_dir, session_uuid).ok_or_else(|| format!("no transcript for session {session_uuid} in {config_dir}"))?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("reading transcript failed: {e}"))?;
    Ok(parse_transcript_lines(&text))
}

/// The pure core of `parse_claude_transcript`, split out so it's testable
/// against a hand-built transcript without touching disk.
fn parse_transcript_lines(text: &str) -> NeutralSession {
    let mut session = NeutralSession::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue };

        // Working context: keep the latest non-empty value seen.
        if let Some(cwd) = value.get("cwd").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            session.cwd = Some(cwd.to_string());
        }
        if let Some(branch) = value.get("gitBranch").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            session.branch = Some(branch.to_string());
        }

        let msg_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let content = value.pointer("/message/content");
        match msg_type {
            "user" => {
                if let Some(content) = content {
                    if is_tool_result(content) {
                        continue; // tool output, not a human turn
                    }
                    let text = content_text(content);
                    if !text.trim().is_empty() {
                        session.turns.push(NeutralTurn { role: "user".to_string(), text: text.trim().to_string() });
                    }
                }
            }
            "assistant" => {
                if let Some(content) = content {
                    let text = content_text(content);
                    if !text.trim().is_empty() {
                        session.turns.push(NeutralTurn { role: "assistant".to_string(), text: text.trim().to_string() });
                    }
                    if let Some(items) = content.as_array() {
                        for item in items {
                            if item.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                                describe_tool_use(item, &mut session.actions, &mut session.modified_files);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    session
}

/// Renders a `NeutralSession` as the markdown handoff a destination agent
/// reads to pick up the work — provider-agnostic, so it feeds claude, codex or
/// gemini identically.
///
/// `budget_chars` caps the verbatim conversation: the most recent turns are
/// kept up to the budget and older ones dropped with a note, because a raw dump
/// (transcripts run to megabytes) wouldn't fit a session and isn't the point —
/// the files-touched and actions sections carry the concrete state regardless.
pub fn render_handoff(session: &NeutralSession, budget_chars: usize) -> String {
    let mut out = String::from("# Handoff from a previous Claude Code session\n\n");
    out.push_str("You are picking up a conversation that ran under a different account. Continue the work below.\n\n");

    if let Some(cwd) = &session.cwd {
        out.push_str(&format!("- Working directory: `{cwd}`\n"));
    }
    if let Some(branch) = &session.branch {
        out.push_str(&format!("- Git branch: `{branch}`\n"));
    }

    if !session.modified_files.is_empty() {
        out.push_str("\n## Files touched\n");
        for f in &session.modified_files {
            out.push_str(&format!("- `{f}`\n"));
        }
    }

    if !session.actions.is_empty() {
        out.push_str("\n## What was done\n");
        for a in &session.actions {
            out.push_str(&format!("- {a}\n"));
        }
    }

    out.push_str("\n## Conversation\n");
    // Keep the most recent turns that fit the budget; drop older ones.
    let mut kept: Vec<&NeutralTurn> = Vec::new();
    let mut used = 0usize;
    for turn in session.turns.iter().rev() {
        let cost = turn.text.len() + turn.role.len() + 8;
        if used + cost > budget_chars && !kept.is_empty() {
            break;
        }
        used += cost;
        kept.push(turn);
    }
    let dropped = session.turns.len() - kept.len();
    if dropped > 0 {
        out.push_str(&format!("_({dropped} earlier turn(s) omitted to fit — the sections above summarize them.)_\n\n"));
    }
    for turn in kept.iter().rev() {
        out.push_str(&format!("**{}:** {}\n\n", turn.role, turn.text));
    }
    out
}

/// Finds a session's transcript under a config dir **by uuid**, not by
/// computing the slug.
///
/// The slug is the cwd with `/` turned into `-`, which is lossy and
/// irreversible — `/Users/me/Proyects/Fleet-Agent` and a folder literally
/// named `Fleet-Agent` both flatten the same way. But the uuid is unique, so a
/// scan of `projects/*/<uuid>.jsonl` finds the one file and, with it, the real
/// slug directory to reuse for the destination.
pub(crate) fn find_transcript(config_dir: &str, session_uuid: &str) -> Option<PathBuf> {
    let projects = Path::new(config_dir).join("projects");
    for slug_dir in std::fs::read_dir(&projects).ok()?.flatten() {
        let candidate = slug_dir.path().join(format!("{session_uuid}.jsonl"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Whether a resumable transcript exists for this session in this config dir —
/// what restore checks to decide `--resume` (the conversation is there) vs a
/// fresh `--session-id` start (nothing to resume).
pub(crate) fn transcript_exists(config_dir: &str, session_uuid: &str) -> bool {
    find_transcript(config_dir, session_uuid).is_some()
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Copies a Claude Code conversation from one account's config dir to another,
/// preserving the session uuid so `claude --resume <uuid>` finds it in the
/// destination.
///
/// **Copy, never move**: if the resume fails, the original is untouched — the
/// same discipline `reopen_with_account_inner` uses (open the new before
/// closing the old). The destination path reuses the *source's* slug verbatim,
/// because both terminals share the same cwd (the worktree), so it's exactly
/// where the destination CLI will look.
///
/// Returns `Err` when there is no transcript to copy — a terminal opened but
/// never actually conversed in has nothing to carry, and the caller falls back
/// to a fresh session. `file-history/<uuid>` and `session-env/<uuid>` (present
/// only when the session made edits) are carried best-effort; the conversation
/// itself resumes without them.
pub fn transfer_claude_session(from_config_dir: &str, to_config_dir: &str, session_uuid: &str) -> Result<(), String> {
    let src = find_transcript(from_config_dir, session_uuid).ok_or_else(|| format!("no transcript for session {session_uuid} in {from_config_dir}"))?;
    let slug = src.parent().and_then(|p| p.file_name()).ok_or("malformed transcript path")?;

    let dst_dir = Path::new(to_config_dir).join("projects").join(slug);
    std::fs::create_dir_all(&dst_dir).map_err(|e| e.to_string())?;
    std::fs::copy(&src, dst_dir.join(format!("{session_uuid}.jsonl"))).map_err(|e| format!("copying transcript failed: {e}"))?;

    for sub in ["file-history", "session-env"] {
        let artifact = Path::new(from_config_dir).join(sub).join(session_uuid);
        if artifact.is_dir() {
            let _ = copy_dir_all(&artifact, &Path::new(to_config_dir).join(sub).join(session_uuid));
        }
    }
    log::info!("handoff: copied session {session_uuid} from {from_config_dir} to {to_config_dir}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn plant_transcript(config_dir: &Path, slug: &str, uuid: &str, body: &str) {
        let dir = config_dir.join("projects").join(slug);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{uuid}.jsonl")), body).unwrap();
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fleet-handoff-{label}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn transfer_copies_the_transcript_preserving_the_uuid_and_leaves_the_source() {
        let from = temp_dir("from");
        let to = temp_dir("to");
        let slug = "-Users-me-Proyects-App";
        let uuid = "11111111-2222-3333-4444-555555555555";
        plant_transcript(&from, slug, uuid, "line one\nline two\n");

        transfer_claude_session(from.to_str().unwrap(), to.to_str().unwrap(), uuid).unwrap();

        let dst_file = to.join("projects").join(slug).join(format!("{uuid}.jsonl"));
        assert!(dst_file.is_file(), "the transcript must land at the same slug/uuid in the destination");
        assert_eq!(fs::read_to_string(&dst_file).unwrap(), "line one\nline two\n");
        assert!(from.join("projects").join(slug).join(format!("{uuid}.jsonl")).is_file(), "copy, not move — the source must survive");

        fs::remove_dir_all(&from).ok();
        fs::remove_dir_all(&to).ok();
    }

    /// The slug is found by scanning for the uuid, so a folder name containing
    /// a hyphen (which the slug can't distinguish from a path separator) is
    /// handled correctly — the source's own directory name is reused.
    #[test]
    fn transfer_reuses_the_source_slug_even_with_hyphens_in_the_folder_name() {
        let from = temp_dir("from");
        let to = temp_dir("to");
        let slug = "-Users-me-Proyects-Fleet-Agent"; // "Fleet-Agent" has a real hyphen
        let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        plant_transcript(&from, slug, uuid, "convo\n");

        transfer_claude_session(from.to_str().unwrap(), to.to_str().unwrap(), uuid).unwrap();
        assert!(to.join("projects").join(slug).join(format!("{uuid}.jsonl")).is_file());

        fs::remove_dir_all(&from).ok();
        fs::remove_dir_all(&to).ok();
    }

    #[test]
    fn transfer_carries_file_history_when_the_session_made_edits() {
        let from = temp_dir("from");
        let to = temp_dir("to");
        let slug = "-x";
        let uuid = "77777777-7777-7777-7777-777777777777";
        plant_transcript(&from, slug, uuid, "convo\n");
        let hist = from.join("file-history").join(uuid);
        fs::create_dir_all(&hist).unwrap();
        fs::write(hist.join("edit-1.json"), "{}").unwrap();

        transfer_claude_session(from.to_str().unwrap(), to.to_str().unwrap(), uuid).unwrap();
        assert!(to.join("file-history").join(uuid).join("edit-1.json").is_file(), "edit undo-state must be carried when present");

        fs::remove_dir_all(&from).ok();
        fs::remove_dir_all(&to).ok();
    }

    /// A terminal that never conversed has no transcript — the transfer says so
    /// rather than silently succeeding, so reopen falls back to a fresh session.
    #[test]
    fn transfer_errors_when_there_is_no_transcript_to_carry() {
        let from = temp_dir("from");
        let to = temp_dir("to");
        let result = transfer_claude_session(from.to_str().unwrap(), to.to_str().unwrap(), "no-such-uuid");
        assert!(result.is_err());

        fs::remove_dir_all(&from).ok();
        fs::remove_dir_all(&to).ok();
    }

    // -- neutral parse + render (the cross-provider handoff core) -------------

    /// A hand-built transcript in the real on-disk shape: user text, assistant
    /// text + tool_use (Bash + Edit), a tool_result fed back as a user line,
    /// and cwd/gitBranch metadata.
    fn sample_transcript() -> String {
        [
            r#"{"type":"user","cwd":"/repo/app","gitBranch":"feat/x","message":{"role":"user","content":[{"type":"text","text":"add a health check"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"secret plan"},{"type":"text","text":"Adding it now."},{"type":"tool_use","name":"Write","input":{"file_path":"src/health.rs"}},{"type":"tool_use","name":"Bash","input":{"command":"cargo test"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done — tests pass."}]}}"#,
        ]
        .join("\n")
    }

    #[test]
    fn parse_extracts_turns_actions_files_and_context() {
        let s = parse_transcript_lines(&sample_transcript());

        assert_eq!(s.cwd.as_deref(), Some("/repo/app"));
        assert_eq!(s.branch.as_deref(), Some("feat/x"));
        // Human/assistant turns only — the tool_result user line is not a turn,
        // and thinking is stripped from the assistant turn.
        assert_eq!(s.turns.len(), 3);
        assert_eq!(s.turns[0], NeutralTurn { role: "user".into(), text: "add a health check".into() });
        assert_eq!(s.turns[1], NeutralTurn { role: "assistant".into(), text: "Adding it now.".into() });
        assert!(!s.turns[1].text.contains("secret plan"), "thinking must never leak into a turn");
        assert_eq!(s.turns[2].text, "Done — tests pass.");

        assert_eq!(s.actions, vec!["edited src/health.rs".to_string(), "ran `cargo test`".to_string()]);
        assert_eq!(s.modified_files, vec!["src/health.rs".to_string()]);
    }

    #[test]
    fn render_includes_context_files_and_the_conversation() {
        let s = parse_transcript_lines(&sample_transcript());
        let md = render_handoff(&s, 10_000);
        assert!(md.contains("Working directory: `/repo/app`"));
        assert!(md.contains("Git branch: `feat/x`"));
        assert!(md.contains("- `src/health.rs`"));
        assert!(md.contains("ran `cargo test`"));
        assert!(md.contains("**user:** add a health check"));
        assert!(md.contains("**assistant:** Done — tests pass."));
    }

    /// A raw transcript can be megabytes; the render must keep the most recent
    /// turns within budget and note the drop, not dump everything.
    #[test]
    fn render_keeps_recent_turns_within_budget_and_notes_the_rest() {
        let mut s = NeutralSession::default();
        for i in 0..50 {
            s.turns.push(NeutralTurn { role: "user".into(), text: format!("message number {i} with some padding text") });
        }
        let md = render_handoff(&s, 200);
        assert!(md.contains("earlier turn(s) omitted"), "must say it truncated");
        assert!(md.contains("message number 49"), "the most recent turn must survive");
        assert!(!md.contains("message number 0"), "the oldest turn must be dropped under a tight budget");
    }
}
