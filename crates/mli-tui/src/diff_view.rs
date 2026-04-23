//! Minimal unified-diff renderer for patch summaries.
//!
//! When an agent commits a patch, our `PatchSummaryCell` holds a free-form
//! string. If that string looks like a unified diff (hunks prefixed with `@@`
//! and lines with `+`/`-`/` `), we render it with colored backgrounds and a
//! small hunk separator; otherwise the caller should fall back to plain
//! markdown rendering.
//!
//! We intentionally do *not* mirror CodexPotter's full syntax-highlighting
//! pipeline (syntect). The goal here is to port the folding + color semantics
//! so transcripts read well; full language-aware highlighting can come later.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Heuristically decide whether `text` looks like a unified diff (has at least
/// one hunk header) so callers can pick the renderer.
pub fn looks_like_unified_diff(text: &str) -> bool {
    text.lines().any(|l| l.starts_with("@@"))
}

/// Render a unified-diff string as colored `Line`s ready for
/// `insert_history_lines`. Includes a header `◆ patch` banner plus a plus/minus
/// summary line when the counts are available.
pub fn render_unified_diff(diff: &str) -> Vec<Line<'static>> {
    let (added, removed) = count_changes(diff);
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(diff_header(added, removed));

    for raw in diff.lines() {
        let line = if let Some(rest) = raw.strip_prefix("diff --git ") {
            Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    rest.to_string(),
                    Style::default()
                        .fg(Color::LightBlue)
                        .add_modifier(Modifier::BOLD),
                ),
            ])
        } else if raw.starts_with("+++ ") || raw.starts_with("--- ") {
            Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(raw.to_string(), Style::default().fg(Color::DarkGray)),
            ])
        } else if let Some(hunk) = raw.strip_prefix("@@") {
            // Hunk header: `@@ -1,3 +1,4 @@ context…`
            Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    "@@".to_string(),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(hunk.to_string(), Style::default().fg(Color::Magenta)),
            ])
        } else if let Some(rest) = raw.strip_prefix('+') {
            if rest.starts_with('+') {
                // Probably `+++` header we already handled above; skip fallthrough.
                Line::from(Span::styled(
                    raw.to_string(),
                    Style::default().fg(Color::DarkGray),
                ))
            } else {
                diff_line('+', rest, Color::LightGreen, added_bg())
            }
        } else if let Some(rest) = raw.strip_prefix('-') {
            if rest.starts_with('-') {
                Line::from(Span::styled(
                    raw.to_string(),
                    Style::default().fg(Color::DarkGray),
                ))
            } else {
                diff_line('-', rest, Color::LightRed, removed_bg())
            }
        } else if let Some(rest) = raw.strip_prefix(' ') {
            diff_line(' ', rest, Color::Gray, None)
        } else if raw.is_empty() {
            Line::from("")
        } else {
            // Anything else (binary marker, "no newline", etc.) passes through dim.
            Line::from(Span::styled(
                raw.to_string(),
                Style::default().fg(Color::DarkGray),
            ))
        };
        lines.push(line);
    }
    lines.push(Line::from(""));
    lines
}

fn diff_header(added: usize, removed: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "◆ ",
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "patch".to_string(),
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(format!("+{added}"), Style::default().fg(Color::LightGreen)),
        Span::raw(" "),
        Span::styled(format!("-{removed}"), Style::default().fg(Color::LightRed)),
    ])
}

fn diff_line(marker: char, content: &str, fg: Color, bg: Option<Color>) -> Line<'static> {
    let marker_style = {
        let mut s = Style::default().fg(fg).add_modifier(Modifier::BOLD);
        if let Some(bg) = bg {
            s = s.bg(bg);
        }
        s
    };
    let body_style = {
        let mut s = Style::default().fg(fg);
        if let Some(bg) = bg {
            s = s.bg(bg);
        }
        s
    };
    Line::from(vec![
        Span::styled(format!("  {marker} "), marker_style),
        Span::styled(content.to_string(), body_style),
    ])
}

fn count_changes(diff: &str) -> (usize, usize) {
    let mut added = 0usize;
    let mut removed = 0usize;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

/// Background color for `+` lines. Returns `None` on truecolor-deficient
/// terminals where coloring without a bg looks better than a blocky rectangle;
/// since we don't currently have that signal, we unconditionally return a
/// Some(...) subtle shade. ANSI-16 terminals will round to a close match.
fn added_bg() -> Option<Color> {
    Some(Color::Rgb(33, 58, 43))
}

fn removed_bg() -> Option<Color> {
    Some(Color::Rgb(74, 34, 29))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_unified_diff() {
        let diff = "diff --git a/foo b/foo\n--- a/foo\n+++ b/foo\n@@ -1,2 +1,2 @@\n a\n-b\n+c\n";
        assert!(looks_like_unified_diff(diff));
    }

    #[test]
    fn rejects_plain_text() {
        assert!(!looks_like_unified_diff("just a sentence"));
    }

    #[test]
    fn render_counts_plus_minus() {
        let diff = "@@ -1,2 +1,2 @@\n-one\n+two\n";
        let lines = render_unified_diff(diff);
        let header: String = lines[0].spans.iter().map(|s| s.content.clone()).collect();
        assert!(header.contains("+1"));
        assert!(header.contains("-1"));
    }

    #[test]
    fn render_styles_added_and_removed_lines() {
        let diff = "@@ -1 +1 @@\n-foo\n+bar\n";
        let lines = render_unified_diff(diff);
        // Find the '+' and '-' lines.
        let plus = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("bar")));
        let minus = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("foo")));
        assert!(plus.is_some());
        assert!(minus.is_some());
    }
}
