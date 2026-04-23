//! Startup banner for the inline TUI.
//!
//! Renders an ASCII-art "ML-INTERN" logo above the initial transcript, followed
//! by a dim metadata line (version + cwd + model). Inspired by CodexPotter's
//! startup banner, but uses our own lettering and color treatment.
//!
//! We ship two sizes:
//!
//! * [`ANSI_SHADOW`] — 6 rows, 68 columns. Used when the terminal is wide
//!   enough (≥ 72 cols). Generated with pyfiglet's `ansi_shadow` font.
//! * [`PAGGA`]       — 3 rows, 36 columns. Compact fallback for narrow
//!   terminals. Generated with pyfiglet's `pagga` font.
//!
//! Both fit comfortably in a standard 80×24 terminal; the renderer picks the
//! largest one that fits the supplied `width`.

use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

const ANSI_SHADOW: &[&str] = &[
    "███╗   ███╗██╗      ██╗███╗   ██╗████████╗███████╗██████╗ ███╗   ██╗",
    "████╗ ████║██║      ██║████╗  ██║╚══██╔══╝██╔════╝██╔══██╗████╗  ██║",
    "██╔████╔██║██║█████╗██║██╔██╗ ██║   ██║   █████╗  ██████╔╝██╔██╗ ██║",
    "██║╚██╔╝██║██║╚════╝██║██║╚██╗██║   ██║   ██╔══╝  ██╔══██╗██║╚██╗██║",
    "██║ ╚═╝ ██║███████╗ ██║██║ ╚████║   ██║   ███████╗██║  ██║██║ ╚████║",
    "╚═╝     ╚═╝╚══════╝ ╚═╝╚═╝  ╚═══╝   ╚═╝   ╚══════╝╚═╝  ╚═╝╚═╝  ╚═══╝",
];

const PAGGA: &[&str] = &[
    "░█▄█░█░░░░░░░▀█▀░█▀█░▀█▀░█▀▀░█▀▄░█▀█",
    "░█░█░█░░░▄▄▄░░█░░█░█░░█░░█▀▀░█▀▄░█░█",
    "░▀░▀░▀▀▀░░░░░▀▀▀░▀░▀░░▀░░▀▀▀░▀░▀░▀░▀",
];

/// Width threshold above which we render [`ANSI_SHADOW`]; otherwise
/// [`PAGGA`] is used (or the banner is skipped entirely if that is also too wide).
const FULL_BANNER_MIN_WIDTH: u16 = 72;

/// Build the banner as a list of ratatui `Line`s that can be fed to
/// `insert_history_lines`. Returns an empty vec if the terminal is too narrow
/// for even the compact banner.
pub fn build_startup_banner_lines(
    width: u16,
    version: &str,
    cwd: &Path,
    model_label: Option<&str>,
) -> Vec<Line<'static>> {
    let art = pick_art(width);
    let mut lines: Vec<Line<'static>> = Vec::new();

    for row in art {
        // Gradient: start dim-cyan on the left, brighten toward the right so the
        // logo reads as "moving into the cell".
        let spans = gradient_spans(row);
        lines.push(Line::from(spans));
    }
    if !art.is_empty() {
        lines.push(Line::from(""));
    }

    // Metadata line: version, cwd, optional model.
    let mut info_spans: Vec<Span<'static>> = Vec::new();
    info_spans.push(Span::styled(
        format!("v{version}"),
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
    ));
    info_spans.push(Span::styled("  ·  ", Style::default().fg(Color::DarkGray)));
    info_spans.push(Span::styled("cwd ", Style::default().fg(Color::DarkGray)));
    info_spans.push(Span::styled(
        format_cwd(cwd, width as usize),
        Style::default().fg(Color::Gray),
    ));
    if let Some(model) = model_label {
        info_spans.push(Span::styled("  ·  ", Style::default().fg(Color::DarkGray)));
        info_spans.push(Span::styled(
            model.to_string(),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::from(info_spans));
    lines.push(Line::from(""));

    lines
}

fn pick_art(width: u16) -> &'static [&'static str] {
    let full_w = ANSI_SHADOW.iter().map(|l| l.width()).max().unwrap_or(0) as u16;
    let compact_w = PAGGA.iter().map(|l| l.width()).max().unwrap_or(0) as u16;
    if width >= FULL_BANNER_MIN_WIDTH && width >= full_w {
        ANSI_SHADOW
    } else if width >= compact_w {
        PAGGA
    } else {
        &[]
    }
}

/// Apply a left-to-right color gradient (dark cyan → light cyan) so the banner
/// has a subtle "shine" instead of a flat block of color.
fn gradient_spans(row: &str) -> Vec<Span<'static>> {
    let total_cols = row.width() as f32;
    if total_cols == 0.0 {
        return vec![Span::raw(row.to_string())];
    }
    // Break the row into roughly equal-width chunks, each with a progressively
    // brighter color. Three bands is enough for a clear gradient without
    // fragmenting every character into its own span.
    let bands: [Color; 3] = [Color::Cyan, Color::LightCyan, Color::White];
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut col = 0f32;
    let mut buf = String::new();
    let mut current_band = 0usize;

    for ch in row.chars() {
        let ch_w = ratatui::text::Span::raw(ch.to_string()).width() as f32;
        let band = ((col / total_cols) * bands.len() as f32) as usize;
        let band = band.min(bands.len() - 1);
        if band != current_band && !buf.is_empty() {
            out.push(Span::styled(
                std::mem::take(&mut buf),
                Style::default()
                    .fg(bands[current_band])
                    .add_modifier(Modifier::BOLD),
            ));
            current_band = band;
        }
        if buf.is_empty() {
            current_band = band;
        }
        buf.push(ch);
        col += ch_w;
    }
    if !buf.is_empty() {
        out.push(Span::styled(
            buf,
            Style::default()
                .fg(bands[current_band])
                .add_modifier(Modifier::BOLD),
        ));
    }
    out
}

/// Shorten `cwd` for banner display. Replaces the user's home directory with
/// `~` and truncates from the left (keeping the tail) when the path would
/// otherwise overflow.
fn format_cwd(cwd: &Path, term_width: usize) -> String {
    let home = dirs::home_dir();
    let raw = cwd.display().to_string();
    let shortened = match home {
        Some(ref home) => {
            let home_s = home.display().to_string();
            if raw == home_s {
                "~".to_string()
            } else if let Some(rest) = raw.strip_prefix(&(home_s + "/")) {
                format!("~/{rest}")
            } else {
                raw
            }
        }
        None => raw,
    };
    let max = term_width.saturating_sub(32).max(20);
    if shortened.width() <= max {
        return shortened;
    }
    // Truncate from the head, keeping the final component(s).
    let mut chars: Vec<char> = shortened.chars().collect();
    while chars.len() > max.saturating_sub(1) {
        chars.remove(0);
    }
    let mut out = String::from("…");
    out.extend(chars);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn full_banner_used_for_wide_terminal() {
        let lines = build_startup_banner_lines(120, "0.1.0", &PathBuf::from("/tmp"), None);
        // 6 art rows + blank + info + blank = 9
        assert_eq!(lines.len(), 9);
    }

    #[test]
    fn compact_banner_used_for_narrow_terminal() {
        let lines = build_startup_banner_lines(40, "0.1.0", &PathBuf::from("/tmp"), None);
        // 3 art rows + blank + info + blank = 6
        assert_eq!(lines.len(), 6);
    }

    #[test]
    fn tiny_terminal_yields_only_metadata() {
        let lines = build_startup_banner_lines(20, "0.1.0", &PathBuf::from("/tmp"), None);
        // No art fits; just the info line + trailing blank = 2
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn model_label_rendered_when_present() {
        let lines =
            build_startup_banner_lines(120, "0.1.0", &PathBuf::from("/tmp"), Some("codex 0.120.0"));
        let info_line = &lines[lines.len() - 2];
        let text: String = info_line.spans.iter().map(|s| s.content.clone()).collect();
        assert!(text.contains("codex 0.120.0"));
    }
}
