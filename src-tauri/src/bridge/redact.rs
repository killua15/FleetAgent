//! Turns raw PTY bytes into a bounded, best-effort-redacted plain-text tail
//! for terminals without a session (`kind='project'`) — see
//! `MOBILE_BRIDGE_TRD.md` §6/§12.4. This is the one exception to "no
//! scrollback ever", and it's an exception with real limits: it decides what
//! may cross the bridge from a PTY, the same role `allowlist` plays for
//! commands, and it's deliberately not a guarantee — a secret in a shape this
//! module doesn't recognize is not caught.
//!
//! **Backed by a real headless terminal emulator (`vt100`), not a hand-rolled
//! ANSI stripper.** An earlier version of this module tried to flatten the
//! byte stream into a plain-text log itself — decode, strip escapes, reflow,
//! dedup repeated lines, drop "thinking" chrome. It mostly worked, until a
//! real conversation showed the actual failure mode: Claude Code's own
//! renderer (Ink) redraws a whole paragraph by moving the cursor *up* and
//! reprinting it as it re-wraps to fit the growing text, and a flat
//! append-only log has no notion of "this repaints the last five lines" —
//! it just sees more bytes and appends more lines, so the same paragraph
//! shows up several times, scrambled. That's not a bug to patch with another
//! heuristic; it's the wrong tool. A terminal that redraws needs something
//! that tracks a cursor and a 2D grid — a terminal emulator — and `vt100` is
//! exactly that, already correct, already tested against real terminal
//! behavior, and column-width-aware the same way Ink is.
//!
//! **The tail is a full snapshot, not an appended delta.** Every time it's
//! due to emit, this reconstructs the *entire* current transcript (scrollback
//! history + the live screen) from the vt100 buffer and sends that whole
//! thing — bounded, redacted, filtered — rather than trying to compute "what
//! changed since last time" as text. That sidesteps the redraw problem
//! entirely: whatever Ink is doing to the screen right now, the vt100 buffer
//! already reflects the *correct final result* of it, and asking for that
//! fresh each time can never show a stale repaint. Bounded because reading
//! it is trivially cheap (an in-memory buffer, no I/O) even done every tick.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;

/// The bounded tail kept per terminal, already redacted — what
/// `get_terminal_tail` returns and what `terminal-tail` carries. Sized for a
/// phone screen crossing an encrypted relay meant for JSON command replies
/// (`MAX_PAYLOAD_BYTES` in `dispatch.rs` is 200 KB), not for the desktop's
/// own 256 KB local reconnect buffer (`SCROLLBACK_LIMIT_BYTES`,
/// `terminals.rs`) — that one stays exclusively local.
const MOBILE_TAIL_LIMIT_BYTES: usize = 32 * 1024;

/// Rows of vt100 scrollback to keep. Generous relative to
/// `MOBILE_TAIL_LIMIT_BYTES` (a screen-width row is rarely more than ~100
/// bytes, so this covers several times the byte budget) — the byte cap on
/// the reconstructed text is what actually bounds what gets sent; this just
/// has to not run out before that does.
const SCROLLBACK_ROWS: usize = 2000;

/// How often the tail is recomputed and, if it changed, marked for sending.
/// Live-testing against a real conversation showed the original 1s
/// (piggybacked on the terminal-states tick) reads as visibly out of sync
/// with the desktop's own instant display — this is fast enough to feel
/// live without reconstructing the transcript on every single byte.
const EMIT_DEBOUNCE: Duration = Duration::from_millis(250);

/// Known noise lines that aren't shaped like the "for Ns" status pattern but
/// are just as much clutter on a phone screen — grown by exact reports, not
/// guessed in advance.
const NOISE_LINES: &[&str] = &["Now using usage credits"];

/// Claude Code's own ephemeral "thinking" indicator — a whimsical randomized
/// verb and how long it took ("Cogitated for 2s", "Forming… (4s · ↓ 228
/// tokens)") — plus the tip line that rides along with it. Matched by shape
/// (a short line ending in "for Ns" or a duration in parentheses), not by
/// enumerating every verb: Claude Code's own list of them isn't part of this
/// codebase's contract and can change without notice. Best-effort, same
/// spirit as `redact` — a phrasing this doesn't recognize is left in.
fn is_ephemeral_status_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.chars().count() <= 60 {
        static STATUS_SHAPE: OnceLock<Regex> = OnceLock::new();
        let re = STATUS_SHAPE.get_or_init(|| Regex::new(r"(?i)\bfor \d+s\b|\(\d+s[^)]*\)").expect("static pattern must compile"));
        if re.is_match(trimmed) {
            return true;
        }
    }
    // The tip that sometimes hangs off the spinner, on its own indented
    // line ("  └ Tip: ..."), and whichever glyph Claude Code chose to lead
    // the status line with itself (spinner frame, moon phase, asterisk).
    let without_tree = trimmed.trim_start_matches(['└', '│', '├', '─', '∗', '☾', '·', '⚡', '✢', '✻', ' ']);
    without_tree.starts_with("Tip:") || NOISE_LINES.iter().any(|n| n.eq_ignore_ascii_case(without_tree))
}

/// A box-drawing character, or an ASCII stand-in for one — the parts Claude
/// Code uses to draw a panel border, a separator rule, or a progress bar.
fn is_rule_glyph(c: char) -> bool {
    matches!(c as u32, 0x2500..=0x257f) // Box Drawing
        || matches!(c as u32, 0x2580..=0x259f) // Block Elements
        || matches!(c, '-' | '_' | '=' | '.' | '·' | '•' | '*' | '~' | '^')
}

/// A line that's *only* a border/separator — no real text, just the
/// decoration around it (`─────`, `┌────┐`, a progress bar's fill). Real
/// content never reads as concrete on a phone screen next to a wall of
/// these, and there's no panel here for them to be the edge of.
fn is_decorative_rule_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.chars().count() >= 3 && trimmed.chars().all(is_rule_glyph)
}

struct Pattern {
    regex: Regex,
    replacement: &'static str,
}

fn p(pattern: &str, replacement: &'static str) -> Pattern {
    Pattern { regex: Regex::new(pattern).expect("static redaction pattern must compile"), replacement }
}

/// Known secret shapes, most-specific-first — once a match is replaced with
/// `[redacted]`, the literal prefix it had (`sk-ant-`, say) is gone, so a
/// broader pattern later in the list can't double-match the replacement.
/// Every pattern has an explicit upper bound on its quantifiers: that caps
/// the regex work per call regardless of what a program prints (the `regex`
/// crate is already linear-time, no backtracking, so there's no ReDoS
/// surface here either way).
fn patterns() -> &'static [Pattern] {
    static PATTERNS: OnceLock<Vec<Pattern>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            p(r"sk-ant-[A-Za-z0-9_-]{20,120}", "[redacted]"),
            p(r"github_pat_[A-Za-z0-9_]{20,120}", "[redacted]"),
            p(r"ghp_[A-Za-z0-9]{36}", "[redacted]"),
            p(r"AKIA[0-9A-Z]{16}", "[redacted]"),
            p(r"eyJ[A-Za-z0-9_-]{10,100}\.eyJ[A-Za-z0-9_-]{10,300}\.[A-Za-z0-9_-]{10,150}", "[redacted]"),
            p(r"sk-[A-Za-z0-9]{20,120}", "[redacted]"),
            // Keeps the key name and separator so redaction is legible, not just a black box.
            p(r"(?i)(api[_-]?key|token|secret|password)(\s*[:=]\s*)\S{1,200}", "$1$2[redacted]"),
        ]
    })
}

/// Best-effort, not a guarantee — a secret in a shape not listed above is
/// not caught. See `MOBILE_BRIDGE_TRD.md` §12.4.
pub fn redact(text: &str) -> String {
    patterns().iter().fold(text.to_string(), |acc, pat| pat.regex.replace_all(&acc, pat.replacement).into_owned())
}

fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Walks the vt100 buffer from the oldest scrollback row through the live
/// screen and rejoins it into logical lines (`row_wrapped` marks a row that
/// continues onto the next one — Ink hard-wraps a paragraph to the terminal
/// width, so without unwrapping that, one sentence would come out as several
/// disconnected lines). Real cursor-addressed redraws are why this has to be
/// re-walked from scratch each time rather than tracked incrementally: a row
/// that already went into a previous snapshot can still change before it
/// scrolls out of the live screen, and the vt100 buffer is the only thing
/// that knows whether it did.
fn transcript_lines(parser: &mut vt100::Parser) -> Vec<String> {
    let (rows, cols) = parser.screen().size();

    parser.screen_mut().set_scrollback(usize::MAX);
    let history = parser.screen().scrollback();

    let mut lines = Vec::new();
    let mut building = String::new();

    for offset in (1..=history).rev() {
        parser.screen_mut().set_scrollback(offset);
        let screen = parser.screen();
        let wrapped = screen.row_wrapped(0);
        building.push_str(&screen.rows(0, cols).next().unwrap_or_default());
        if !wrapped {
            lines.push(std::mem::take(&mut building));
        }
    }

    parser.screen_mut().set_scrollback(0);
    let screen = parser.screen();
    for row in 0..rows {
        let wrapped = screen.row_wrapped(row);
        building.push_str(&screen.rows(0, cols).nth(row as usize).unwrap_or_default());
        if !wrapped {
            lines.push(std::mem::take(&mut building));
        }
    }
    if !building.is_empty() {
        lines.push(building);
    }

    // The live screen has a fixed row count regardless of how much has
    // actually been printed — trailing rows below the last real content are
    // unused screen space, not meaningful (blank) lines of the transcript.
    // Only from the end: a real blank line in the middle (a paragraph break)
    // is content and stays.
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }

    lines
}

/// The full pipeline: reconstruct → drop ephemeral status lines and pure
/// decoration → redact → bound to the mobile tail budget, keeping the *end*
/// (most recent content), trimmed to a clean line start where one is nearby.
fn redacted_transcript(parser: &mut vt100::Parser) -> String {
    let text = transcript_lines(parser)
        .into_iter()
        .map(|line| line.trim_end().to_string())
        .filter(|line| !is_ephemeral_status_line(line) && !is_decorative_rule_line(line))
        .collect::<Vec<_>>()
        .join("\n");

    let text = redact(&text);

    if text.len() <= MOBILE_TAIL_LIMIT_BYTES {
        return text;
    }
    let cut = floor_char_boundary(&text, text.len() - MOBILE_TAIL_LIMIT_BYTES);
    let cut = text[cut..].find('\n').map(|nl| cut + nl + 1).unwrap_or(cut);
    text[cut..].to_string()
}

/// Per-terminal state: raw PTY bytes in, a bounded redacted plain-text
/// snapshot out. One instance per open `kind='project'` terminal, fed from
/// its reader thread alongside the existing scrollback/title/sink handling.
pub struct PtyTail {
    parser: vt100::Parser,
    last_sent: String,
    pending: Option<String>,
    last_emit: Instant,
}

impl PtyTail {
    /// `rows`/`cols` must match the real PTY size (`open_terminal`'s
    /// `PtySize`) — vt100's cursor math is relative to that geometry, the
    /// same geometry Ink used to decide where to hard-wrap.
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows.max(1), cols.max(1), SCROLLBACK_ROWS),
            last_sent: String::new(),
            pending: None,
            // Backdated so the very first `due()` check doesn't have to wait
            // out a full debounce window before the first snapshot can send.
            last_emit: Instant::now() - EMIT_DEBOUNCE,
        }
    }

    /// Keeps the emulator's geometry in sync with the real PTY (`resize_terminal`).
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows.max(1), cols.max(1));
    }

    /// One `read()`'s worth of raw PTY bytes, handed straight to the
    /// emulator — decoding, escape sequences, and cursor movement are all
    /// its job now.
    pub fn ingest(&mut self, chunk: &[u8]) {
        self.parser.process(chunk);
    }

    /// Recomputes the transcript now and marks it pending if it actually
    /// changed since the last send — a full vt100 reconstruction is real
    /// (if cheap) work, so this runs on the periodic tick, not per chunk.
    fn recompute(&mut self) {
        let text = redacted_transcript(&mut self.parser);
        if text != self.last_sent {
            self.pending = Some(text);
        }
    }

    /// Whether it's time to check for and, if present, send an update.
    pub fn due(&mut self) -> bool {
        if self.last_emit.elapsed() < EMIT_DEBOUNCE {
            return false;
        }
        self.recompute();
        self.pending.is_some()
    }

    pub fn take_pending(&mut self) -> Option<String> {
        let next = self.pending.take()?;
        self.last_emit = Instant::now();
        self.last_sent = next.clone();
        Some(next)
    }

    /// The current redacted transcript, for a mobile screen that just
    /// opened — mirrors `get_session_view` for a session-backed terminal.
    /// Always fresh: there's no separate "committed" state to fall behind.
    pub fn snapshot(&mut self) -> String {
        redacted_transcript(&mut self.parser)
    }

    /// The PTY closed. Nothing to flush — `snapshot()`/`due()` always
    /// recompute straight from the emulator, which keeps whatever it last
    /// saw until `self` is dropped.
    pub fn finish(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tail(rows: u16, cols: u16) -> PtyTail {
        PtyTail::new(rows, cols)
    }

    fn ingest_str(tail: &mut PtyTail, s: &str) {
        tail.ingest(s.as_bytes());
    }

    #[test]
    fn known_secret_shapes_are_redacted() {
        assert_eq!(redact("key: sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789"), "key: [redacted]");
        assert_eq!(redact("sk-abcdefghijklmnopqrstuvwx"), "[redacted]");
        assert_eq!(redact("ghp_abcdefghijklmnopqrstuvwxyz0123456789"), "[redacted]");
        assert_eq!(redact("github_pat_11ABCDEFG0abcdefghijklmnopqrstuvwxyz"), "[redacted]");
        assert_eq!(redact("AKIAABCDEFGHIJKLMNOP"), "[redacted]");
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PYWnn8dGl4jI";
        assert_eq!(redact(jwt), "[redacted]");
        assert_eq!(redact("API_KEY=abc123def456"), "API_KEY=[redacted]");
        assert_eq!(redact("token: abc123def456"), "token: [redacted]");
    }

    #[test]
    fn redaction_leaves_ordinary_text_untouched() {
        let text = "$ git status\nOn branch feat/x\nnothing to commit\n$ _";
        assert_eq!(redact(text), text);
    }

    #[test]
    fn an_anthropic_key_is_not_double_matched_by_the_generic_sk_pattern() {
        let redacted = redact("sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789");
        assert_eq!(redacted, "[redacted]");
        assert_eq!(redacted.matches("[redacted]").count(), 1);
    }

    #[test]
    fn an_env_style_secret_keeps_its_key_name_and_separator() {
        assert_eq!(redact("SECRET=hunter2"), "SECRET=[redacted]");
        assert_eq!(redact("password: hunter2"), "password: [redacted]");
    }

    /// The whole reason this module exists in its current form: a redraw
    /// (cursor up, erase, reprint the same content) must settle into the
    /// buffer once, not accumulate as repeated lines.
    #[test]
    fn a_redrawn_line_appears_once_not_repeated() {
        let mut t = tail(24, 80);
        ingest_str(&mut t, "line one\r\n");
        // Move the cursor up one row, erase it, and reprint the same text —
        // exactly the shape of an Ink repaint-in-place.
        ingest_str(&mut t, "\x1b[1A\x1b[2Kline one (updated)\r\n");
        ingest_str(&mut t, "line two\r\n");
        let snapshot = t.snapshot();
        assert_eq!(snapshot.matches("line one").count(), 1, "the pre-redraw text must not survive: {snapshot:?}");
        assert!(snapshot.contains("line one (updated)"));
        assert!(snapshot.contains("line two"));
    }

    /// A paragraph that grows and re-wraps (more text arrives, Ink recomputes
    /// where the hard-wrap falls, redraws from higher up) must not leave the
    /// old wrap points behind as extra lines.
    #[test]
    fn a_repainted_reflowed_paragraph_leaves_no_stale_wrap() {
        let mut t = tail(24, 20);
        // First render: a short line that fits on one row.
        ingest_str(&mut t, "hello world\r\n");
        // Simulate Ink noticing more text arrived, moving up to redraw the
        // paragraph now that it wraps differently.
        ingest_str(&mut t, "\x1b[1A\x1b[Jhello world and more\r\n");
        let snapshot = t.snapshot();
        assert_eq!(snapshot.matches("hello world").count(), 1, "{snapshot:?}");
        assert!(snapshot.contains("hello world and more"));
    }

    #[test]
    fn a_secret_split_exactly_at_a_read_boundary_is_still_caught() {
        let key = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789";
        let (first, second) = key.split_at(key.len() / 2);

        let mut t = tail(24, 80);
        ingest_str(&mut t, &format!("before {first}"));
        ingest_str(&mut t, &format!("{second} after\r\n"));

        let snapshot = t.snapshot();
        assert!(!snapshot.contains(key), "the raw key must never appear whole: {snapshot}");
        assert!(snapshot.contains("[redacted]"), "the split key must still be redacted: {snapshot}");
    }

    #[test]
    fn a_secret_wrapped_in_ansi_color_codes_is_redacted() {
        let mut t = tail(24, 80);
        ingest_str(&mut t, "\x1b[31msk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789\x1b[0m\r\n");
        let snapshot = t.snapshot();
        assert!(!snapshot.contains("\x1b["), "escape codes must be stripped: {snapshot:?}");
        assert!(snapshot.contains("[redacted]"), "the wrapped key must still be redacted: {snapshot}");
    }

    #[test]
    fn an_escape_sequence_split_across_two_reads_is_still_stripped_cleanly() {
        let mut t = tail(24, 80);
        ingest_str(&mut t, "before\x1b[38;2;215;119;8");
        ingest_str(&mut t, "7msandbox.network\x1b[0m after\r\n");
        let snapshot = t.snapshot();
        assert!(!snapshot.contains('\u{1b}'));
        assert!(snapshot.contains("beforesandbox.network after"), "{snapshot:?}");
    }

    #[test]
    fn an_osc_title_sequence_contributes_no_visible_text() {
        let mut t = tail(24, 80);
        ingest_str(&mut t, "before\x1b]0;fixing the rounding\x07after\r\n");
        assert!(t.snapshot().contains("beforeafter"));
    }

    #[test]
    fn cursor_positioning_produces_real_spacing_not_a_guess() {
        // A real terminal renders "right" at whatever column the cursor
        // actually moved to — no heuristic single-space needed, vt100 just
        // gets this correct by construction.
        let mut t = tail(24, 80);
        ingest_str(&mut t, "left\x1b[10Gright\r\n");
        // `\x1b[10G` is 1-indexed: column 10 is index 9, and "left" ends at
        // index 3, so there are 5 blank cells (indices 4..=8) between them.
        assert_eq!(t.snapshot(), "left     right");
    }

    #[test]
    fn ephemeral_thinking_status_lines_are_dropped() {
        let mut t = tail(24, 80);
        ingest_str(&mut t, "hola\r\n");
        ingest_str(&mut t, "\x1b[2K∗ Cogitated for 2s\r\n");
        ingest_str(&mut t, "the real answer\r\n");
        assert_eq!(t.snapshot(), "hola\nthe real answer");
    }

    #[test]
    fn a_forming_indicator_and_its_tip_line_are_both_dropped() {
        let mut t = tail(24, 80);
        ingest_str(&mut t, "☾ Forming… (4s · ↓ 228 tokens)\r\n");
        ingest_str(&mut t, "  └ Tip: Use /statusline to set up a custom status line\r\n");
        ingest_str(&mut t, "the real answer\r\n");
        assert_eq!(t.snapshot(), "the real answer");
    }

    #[test]
    fn a_known_noise_line_is_dropped() {
        let mut t = tail(24, 80);
        ingest_str(&mut t, "real content\r\n");
        ingest_str(&mut t, "Now using usage credits\r\n");
        assert_eq!(t.snapshot(), "real content");
    }

    #[test]
    fn a_pure_box_drawing_separator_line_is_dropped() {
        let mut t = tail(24, 80);
        ingest_str(&mut t, "before\r\n");
        ingest_str(&mut t, "────────────────────\r\n");
        ingest_str(&mut t, "┌────────────────────┐\r\n");
        ingest_str(&mut t, "after\r\n");
        assert_eq!(t.snapshot(), "before\nafter");
    }

    #[test]
    fn a_markdown_bullet_with_a_dash_is_kept() {
        let mut t = tail(24, 80);
        ingest_str(&mut t, "- Explorar y entender el código existente\r\n");
        assert_eq!(t.snapshot(), "- Explorar y entender el código existente");
    }

    #[test]
    fn a_long_line_that_happens_to_contain_a_duration_is_kept() {
        let mut t = tail(24, 200);
        let sentence = "This is a much longer sentence that happens to say wait for 3s before retrying the request, real content.";
        ingest_str(&mut t, &format!("{sentence}\r\n"));
        assert_eq!(t.snapshot(), sentence);
    }

    #[test]
    fn the_snapshot_stays_under_its_byte_cap() {
        let mut t = tail(24, 80);
        for i in 0..2000 {
            ingest_str(&mut t, &format!("line number {i} of filler content to grow the transcript\r\n"));
        }
        assert!(t.snapshot().len() <= MOBILE_TAIL_LIMIT_BYTES);
    }

    #[test]
    fn resizing_does_not_panic_and_keeps_working() {
        let mut t = tail(24, 80);
        ingest_str(&mut t, "before resize\r\n");
        t.resize(30, 100);
        ingest_str(&mut t, "after resize\r\n");
        let snapshot = t.snapshot();
        assert!(snapshot.contains("before resize"));
        assert!(snapshot.contains("after resize"));
    }

    #[test]
    fn due_is_false_before_the_debounce_and_true_once_it_elapses_with_new_content() {
        let mut t = tail(24, 80);
        // The constructor backdates `last_emit`, so the very first check can
        // fire immediately if there's content.
        ingest_str(&mut t, "hola\r\n");
        assert!(t.due(), "first content should be due immediately");
        let sent = t.take_pending();
        assert!(sent.is_some());

        // Nothing changed — should not be due again right away even past a
        // moment, since there's no new content to report.
        assert!(!t.due() || t.take_pending().is_none());
    }
}
