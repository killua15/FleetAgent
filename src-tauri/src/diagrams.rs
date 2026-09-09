//! Diagrams an agent delivered for a project, and where they live.
//!
//! The `archify` skill turns a typed JSON spec into a self-contained HTML page.
//! Nothing in Hangar knew those pages existed: the skill's own instructions say
//! how to draw, not where to leave it, so the file landed wherever the agent
//! chose inside the worktree.
//!
//! The obvious way to notice does not work, and it is worth writing down so
//! nobody spends an afternoon on it. Hangar does record what a turn wrote —
//! `file_edits` — but `turn_runner::capture_file_edits` only recognises `Edit`,
//! `Write` and `MultiEdit`, and archify delivers through `Bash`
//! (`node bin/archify.mjs deliver … out.html`). So the one table that tracks
//! writes is blind to exactly this. An interactive terminal is worse: a PTY is
//! opaque bytes.
//!
//! So Hangar names the place instead of guessing it. `HANGAR_DIAGRAMS` reaches
//! the agent on the command line (`terminals.rs`) and the instruction note asks
//! for delivery there. The directory is Hangar's own app data, never the user's
//! checkout — the same rule the graph follows.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// Only the head of the file is read to describe it. A delivered diagram is
/// hundreds of kilobytes of inline SVG, and everything worth listing —
/// `<title>` and `<meta name="generator">` — sits in the first block.
const HEAD_BYTES: usize = 4096;

/// Where a project's delivered diagrams live: a sibling of `graphs/`, with the
/// same shape (`graph::scope_dir`). Not in the user's repo, so uninstalling
/// Hangar leaves no residue in a checkout.
pub fn dir(app_data: &Path, project_id: &str) -> PathBuf {
    app_data.join("diagrams").join(project_id)
}

/// One delivered page, as the settings list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagram {
    /// The file name, which is also what `open_diagram` takes back. Never a
    /// path: the command refuses anything with a separator in it.
    pub file_name: String,
    /// The page's own `<title>`, or the file name when it has none. A row with
    /// no name would be unclickable in practice.
    pub title: String,
    /// What wrote it, from `<meta name="generator">` — e.g. `archify 2.16.0`.
    /// `None` for an HTML file that does not say, which is listed anyway:
    /// hiding a file that is sitting in the folder is worse than naming it
    /// plainly.
    pub generator: Option<String>,
    pub bytes: u64,
    /// Modified time as an RFC 3339 string, or `None` when the filesystem
    /// will not say.
    pub modified: Option<String>,
}

/// `<title>` and `<meta name="generator">` out of a page's head.
///
/// Deliberately string scanning and not an HTML parser. The two things needed
/// are single well-formed tags emitted by one generator, and pulling in a
/// parser to read them would be a dependency bought for two `find` calls.
pub fn parse_header(head: &str) -> (Option<String>, Option<String>) {
    (extract_title(head), extract_generator(head))
}

fn extract_title(head: &str) -> Option<String> {
    let lower = head.to_lowercase();
    let open = lower.find("<title")?;
    // Skip any attributes on the tag itself before the text starts.
    let text_from = head[open..].find('>')? + open + 1;
    let close = lower[text_from..].find("</title>")? + text_from;
    let title = head[text_from..close].trim();
    (!title.is_empty()).then(|| title.to_string())
}

fn extract_generator(head: &str) -> Option<String> {
    let lower = head.to_lowercase();
    // Attribute order is not guaranteed, so find the tag and then its content
    // rather than matching one fixed spelling of the whole element.
    let mut from = 0;
    while let Some(at) = lower[from..].find("<meta") {
        let start = from + at;
        let end = head[start..].find('>').map(|e| start + e + 1).unwrap_or(head.len());
        let tag = &head[start..end];
        if tag.to_lowercase().contains("name=\"generator\"") || tag.to_lowercase().contains("name='generator'") {
            if let Some(value) = attribute(tag, "content") {
                return (!value.is_empty()).then_some(value);
            }
        }
        from = end;
    }
    None
}

/// One quoted attribute out of a tag, either quoting style.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let at = lower.find(&format!("{name}="))?;
    let rest = &tag[at + name.len() + 1..];
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let body = &rest[1..];
    let close = body.find(quote)?;
    Some(body[..close].trim().to_string())
}

/// Every `.html` file in the directory, newest first.
///
/// A missing directory is an empty list, not an error: most projects will never
/// have asked for a diagram, and that is not a failure to report.
pub fn list(dir: &Path) -> Vec<Diagram> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Diagram> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() || !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("html")) {
                return None;
            }
            let file_name = path.file_name()?.to_string_lossy().to_string();
            let (title, generator) = match read_head(&path) {
                Some(head) => parse_header(&head),
                None => (None, None),
            };
            let meta = entry.metadata().ok();
            Some(Diagram {
                title: title.unwrap_or_else(|| file_name.clone()),
                file_name,
                generator,
                bytes: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                modified: meta.and_then(|m| m.modified().ok()).map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()),
            })
        })
        .collect();
    // Newest first: the one just delivered is the one being looked for.
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.file_name.cmp(&b.file_name)));
    out
}

/// The first `HEAD_BYTES` of a file, as lossy UTF-8.
///
/// Read as bytes and converted lossily on purpose: a truncated read can land
/// mid-character, and that must not make the file unlistable.
fn read_head(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut buffer = vec![0u8; HEAD_BYTES];
    let read = file.read(&mut buffer).ok()?;
    buffer.truncate(read);
    Some(String::from_utf8_lossy(&buffer).to_string())
}

/// Whether a name from the UI is safe to join onto the diagrams directory.
///
/// The only parameter in this feature that comes from the frontend and ends in
/// a system call, so it is checked rather than trusted: no separators, no
/// traversal, no absolute paths, and it has to be an HTML file.
pub fn is_safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && name != "."
        && name != ".."
        && !name.contains("..")
        && !Path::new(name).is_absolute()
        && Path::new(name).extension().is_some_and(|e| e.eq_ignore_ascii_case("html"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real head of a delivered artifact, copied from
    /// `docs/architecture/skills-to-agent.html`. A hand-written fixture would
    /// have agreed with whatever the parser did.
    const REAL_HEAD: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="generator" content="archify 2.16.0">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>How a skill reaches an agent Diagram</title>
<link href="https://fonts.googleapis.com/css2?family=JetBrains+Mono" rel="stylesheet">
"#;

    #[test]
    fn the_head_of_a_real_artifact_gives_a_title_and_a_generator() {
        let (title, generator) = parse_header(REAL_HEAD);
        assert_eq!(title.as_deref(), Some("How a skill reaches an agent Diagram"));
        assert_eq!(generator.as_deref(), Some("archify 2.16.0"));
    }

    #[test]
    fn an_html_file_that_says_nothing_still_parses() {
        // Listed by file name rather than hidden — see `Diagram::generator`.
        let (title, generator) = parse_header("<html><body>hola</body></html>");
        assert_eq!(title, None);
        assert_eq!(generator, None);
    }

    #[test]
    fn a_title_survives_attributes_and_single_quotes() {
        let (title, generator) = parse_header("<title lang=\"en\">Con atributos</title><meta name='generator' content='otra 1.0'>");
        assert_eq!(title.as_deref(), Some("Con atributos"));
        assert_eq!(generator.as_deref(), Some("otra 1.0"));
    }

    /// A different `<meta>` before the generator must not stop the scan, and a
    /// `content` that isn't the generator's must not be picked up.
    #[test]
    fn the_generator_is_found_past_other_meta_tags() {
        let head = "<meta charset=\"UTF-8\"><meta name=\"description\" content=\"no soy el generador\"><meta name=\"generator\" content=\"archify 2.17.0\">";
        assert_eq!(parse_header(head).1.as_deref(), Some("archify 2.17.0"));
    }

    #[test]
    fn a_name_that_could_escape_the_directory_is_refused() {
        for bad in [
            "../secreto.html",
            "../../etc/passwd.html",
            "sub/dir.html",
            "sub\\dir.html",
            "/absoluto.html",
            "",
            "..",
            "sin-extension",
            "script.mjs",
        ] {
            assert!(!is_safe_file_name(bad), "debería rechazar {bad:?}");
        }
        for good in ["diagrama.html", "skills-to-agent.HTML", "con espacios.html", "acentuado-á.html"] {
            assert!(is_safe_file_name(good), "debería aceptar {good:?}");
        }
    }

    #[test]
    fn a_directory_that_was_never_created_is_an_empty_list() {
        // The common case: nobody asked this project for a diagram.
        assert!(list(&std::env::temp_dir().join(format!("hangar-no-existe-{}", uuid::Uuid::new_v4()))).is_empty());
    }

    #[test]
    fn only_html_is_listed_and_the_newest_comes_first() {
        let root = std::env::temp_dir().join(format!("hangar-diagrams-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("viejo.html"), REAL_HEAD).unwrap();
        // The spec next to it is not a diagram, and neither is a stray note.
        std::fs::write(root.join("spec.json"), "{}").unwrap();
        std::fs::write(root.join("notas.md"), "# no").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(15));
        std::fs::write(root.join("nuevo.html"), "<title>El nuevo</title>").unwrap();

        let found = list(&root);
        assert_eq!(found.len(), 2, "solo los .html: {found:?}");
        assert_eq!(found[0].file_name, "nuevo.html", "el más reciente primero: {found:?}");
        assert_eq!(found[0].title, "El nuevo");
        assert_eq!(found[1].title, "How a skill reaches an agent Diagram");
        assert_eq!(found[1].generator.as_deref(), Some("archify 2.16.0"));
        assert!(found[1].bytes > 0, "el tamaño se lee del disco");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_directory_is_hangar_own_data_never_the_checkout() {
        let d = dir(Path::new("/tmp/app-data"), "p1");
        assert_eq!(d, Path::new("/tmp/app-data/diagrams/p1"));
    }
}
