//! Codex-CLI style rendering for exec command cells.
//!
//! Visual layout (inspired by CodexPotter's `exec_cell/render.rs`):
//!
//! ```text
//! • Ran <command-preview>
//!   │ <command line 1>
//!   │ <command line 2>
//!   └ <output line 1>
//!     <output line 2>
//!     … +N lines
//!     <output line N-1>
//!     <output line N>
//! ```
//!
//! Long output is folded around an ellipsis marker so transcripts stay readable.
//! Our data model doesn't track exit codes or durations yet, so we omit the
//! trailing footer (`✓ • 2.3s`) that Codex renders; if those fields land in
//! `mli_types::ExecOutputCell` later, [`render_finalized`] is the single place
//! to plumb them through.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// How many head / tail lines to keep when folding output for the scrollback
/// render. Matches Codex's agent-call default (5+5 with ellipsis when the
/// total exceeds 2×).
pub const OUTPUT_KEEP_LINES: usize = 5;

/// Maximum number of command lines before the `│` header truncates with "…"
const COMMAND_KEEP_LINES: usize = 2;

const OUTPUT_PREFIX_FIRST: &str = "  └ ";
const OUTPUT_PREFIX_REST: &str = "    ";
const COMMAND_PREFIX: &str = "  │ ";

/// Render a command that has already started but whose output is still incoming.
/// Used for `ExecCommandCell` (no output yet).
pub fn render_running(command: &str) -> Vec<Line<'static>> {
    let mut lines = vec![header_line("Running", Color::Cyan, command)];
    for cmd_line in command_body_lines(command) {
        lines.push(cmd_line);
    }
    lines.push(Line::from(""));
    lines
}

/// Render a completed command with its output, collapsing long bodies around
/// an ellipsis. Used for `ExecOutputCell` with `streaming == false`.
pub fn render_finalized(command: &str, output: &str) -> Vec<Line<'static>> {
    let mut lines = vec![header_line("Ran", Color::Green, command)];
    lines.extend(command_body_lines(command));
    lines.extend(output_body_lines(output, OUTPUT_KEEP_LINES));
    lines.push(Line::from(""));
    lines
}

/// Render the header + command preamble for a streaming command whose output
/// is being pushed to scrollback line-by-line. This is only the **first**
/// block inserted before any output arrives.
pub fn render_streaming_prelude(command: &str) -> Vec<Line<'static>> {
    let mut lines = vec![header_line("Running", Color::Cyan, command)];
    lines.extend(command_body_lines(command));
    lines
}

/// Render a single output line with the "` └ `" or "`   `" prefix, depending
/// on whether it is the first output line for this exec cell.
pub fn prefix_output_line(content: &str, is_first: bool) -> Line<'static> {
    let prefix = if is_first {
        OUTPUT_PREFIX_FIRST
    } else {
        OUTPUT_PREFIX_REST
    };
    Line::from(vec![
        Span::styled(prefix.to_string(), Style::default().fg(Color::DarkGray)),
        Span::styled(
            content.to_string(),
            Style::default().add_modifier(Modifier::DIM),
        ),
    ])
}

/// Render an ellipsis marker (`    … +N lines`) to splice into the output
/// section after a head-only burst during folding.
pub fn ellipsis_line(omitted: usize) -> Line<'static> {
    let prefix = OUTPUT_PREFIX_REST;
    Line::from(vec![
        Span::styled(prefix.to_string(), Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("… +{omitted} line{}", if omitted == 1 { "" } else { "s" }),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ),
    ])
}

/// Close a streaming exec cell with a trailing blank row. Kept as its own
/// helper so future exit-code / duration footers can be slotted in here
/// without touching call-sites.
pub fn render_streaming_finalize_footer() -> Vec<Line<'static>> {
    vec![Line::from("")]
}

// -- internal helpers --------------------------------------------------------

fn header_line(verb: &str, verb_color: Color, command: &str) -> Line<'static> {
    let preview = command_preview(command, 72);
    Line::from(vec![
        Span::styled(
            "• ",
            Style::default().fg(verb_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{verb} "),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(preview, Style::default().fg(Color::Cyan)),
    ])
}

/// Produce the `  │ …` command continuation lines, collapsing after
/// [`COMMAND_KEEP_LINES`] rows so long heredocs don't dominate the cell.
fn command_body_lines(command: &str) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let raw_lines: Vec<&str> = command.split('\n').collect();
    if raw_lines.len() <= 1 {
        // Single-line command is already in the header preview.
        return out;
    }
    for (idx, raw) in raw_lines.iter().enumerate() {
        if idx >= COMMAND_KEEP_LINES {
            let omitted = raw_lines.len() - idx;
            out.push(Line::from(vec![
                Span::styled(
                    COMMAND_PREFIX.to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!(
                        "… +{omitted} more command line{}",
                        if omitted == 1 { "" } else { "s" }
                    ),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]));
            break;
        }
        out.push(Line::from(vec![
            Span::styled(
                COMMAND_PREFIX.to_string(),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(raw.to_string(), Style::default().fg(Color::Cyan)),
        ]));
    }
    out
}

/// Break `output` into head + optional ellipsis + tail, emitting prefixed
/// lines suitable for `insert_history_lines`.
fn output_body_lines(output: &str, keep: usize) -> Vec<Line<'static>> {
    let trimmed = output.trim_end_matches('\n');
    if trimmed.is_empty() {
        return Vec::new();
    }
    let all: Vec<&str> = trimmed.split('\n').collect();
    let total = all.len();
    let mut out: Vec<Line<'static>> = Vec::new();

    if total <= keep * 2 {
        for (i, line) in all.iter().enumerate() {
            out.push(prefix_output_line(line, i == 0));
        }
        return out;
    }

    for (i, line) in all.iter().take(keep).enumerate() {
        out.push(prefix_output_line(line, i == 0));
    }
    out.push(ellipsis_line(total - keep * 2));
    for line in all.iter().skip(total - keep) {
        out.push(prefix_output_line(line, false));
    }
    out
}

/// Truncate a single-line command for the header row, appending `…` when cut.
fn command_preview(command: &str, max_display: usize) -> String {
    let first_line = command.lines().next().unwrap_or(command).trim_end();
    if first_line.width() <= max_display {
        if command.contains('\n') {
            format!("{first_line} …")
        } else {
            first_line.to_string()
        }
    } else {
        let mut acc = String::new();
        let mut width = 0usize;
        for ch in first_line.chars() {
            let ch_w = Span::raw(ch.to_string()).width();
            if width + ch_w + 1 > max_display {
                break;
            }
            acc.push(ch);
            width += ch_w;
        }
        format!("{acc}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.clone()).collect()
    }

    #[test]
    fn short_output_is_not_folded() {
        let out = render_finalized("echo hello", "hello\n");
        let texts: Vec<String> = out.iter().map(plain).collect();
        // Header + output line + blank
        assert!(texts.iter().any(|t| t.contains("hello")));
        assert!(texts.iter().all(|t| !t.contains("…")));
    }

    #[test]
    fn long_output_is_folded_head_and_tail() {
        let mut body = String::new();
        for i in 0..50 {
            body.push_str(&format!("line{i}\n"));
        }
        let out = render_finalized("seq 50", &body);
        let texts: Vec<String> = out.iter().map(plain).collect();
        // Head lines present
        assert!(texts.iter().any(|t| t.contains("line0")));
        assert!(texts.iter().any(|t| t.contains("line4")));
        // Ellipsis with omitted count
        assert!(texts.iter().any(|t| t.contains("… +40 lines")));
        // Tail lines present
        assert!(texts.iter().any(|t| t.contains("line49")));
    }

    #[test]
    fn multiline_command_shows_continuation() {
        let out = render_finalized("set -e\npython -u run.py\necho done", "ok\n");
        let texts: Vec<String> = out.iter().map(plain).collect();
        assert!(texts.iter().any(|t| t.contains("│ set -e")));
        assert!(texts.iter().any(|t| t.contains("│ python -u run.py")));
        assert!(texts.iter().any(|t| t.contains("… +1 more command line")));
    }

    #[test]
    fn command_preview_truncates_width() {
        let long = "a".repeat(200);
        let out = render_running(&long);
        let header = plain(&out[0]);
        assert!(header.contains("…"));
    }

    #[test]
    fn empty_output_produces_no_body() {
        let out = render_finalized("true", "");
        let has_tee = out.iter().any(|l| plain(l).contains("└"));
        assert!(!has_tee, "empty output should not emit a body prefix");
    }

    #[test]
    fn prefix_output_line_uses_tee_for_first_only() {
        let a = prefix_output_line("hello", true);
        let b = prefix_output_line("world", false);
        assert!(plain(&a).starts_with("  └ "));
        assert!(plain(&b).starts_with("    "));
        assert!(!plain(&b).contains("└"));
    }

    #[test]
    fn ellipsis_formats_singular_and_plural() {
        assert_eq!(plain(&ellipsis_line(1)), "    … +1 line");
        assert_eq!(plain(&ellipsis_line(42)), "    … +42 lines");
    }
}
