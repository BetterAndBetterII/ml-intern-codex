//! Plain-text fallback for syntax highlighting.
//!
//! CodexPotter's upstream module wraps `syntect`/`two_face` for ~250-language
//! highlighting. We ported the surface-level API but keep the implementation
//! intentionally minimal: every highlight request returns an unstyled
//! `Vec<Line>`. This preserves call-site ergonomics (markdown rendering, bash
//! rendering, etc.) while avoiding the heavyweight dependency tree.
//!
//! Swap this module out if/when richer highlighting is worth the deps.

use ratatui::text::Line;
use ratatui::text::Span;
use std::path::Path;
use std::path::PathBuf;

const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;
const MAX_HIGHLIGHT_LINES: usize = 10_000;

pub fn exceeds_highlight_limits(total_bytes: usize, total_lines: usize) -> bool {
    total_bytes > MAX_HIGHLIGHT_BYTES || total_lines > MAX_HIGHLIGHT_LINES
}

pub fn highlight_code_to_lines(code: &str, _lang: &str) -> Vec<Line<'static>> {
    let mut result: Vec<Line<'static>> = code.lines().map(|l| Line::from(l.to_string())).collect();
    if result.is_empty() {
        result.push(Line::from(String::new()));
    }
    result
}

pub fn highlight_bash_to_lines(script: &str) -> Vec<Line<'static>> {
    highlight_code_to_lines(script, "bash")
}

pub fn highlight_code_to_styled_spans(_code: &str, _lang: &str) -> Option<Vec<Vec<Span<'static>>>> {
    None
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DiffScopeBackgroundRgbs {
    pub inserted: Option<(u8, u8, u8)>,
    pub deleted: Option<(u8, u8, u8)>,
}

pub fn diff_scope_background_rgbs() -> DiffScopeBackgroundRgbs {
    DiffScopeBackgroundRgbs::default()
}

pub fn set_theme_override(_name: Option<String>, _codex_home: Option<PathBuf>) -> Option<String> {
    None
}

pub fn adaptive_default_theme_name() -> &'static str {
    "catppuccin-mocha"
}

pub fn configured_theme_name() -> String {
    adaptive_default_theme_name().to_string()
}

pub struct ThemeEntry {
    pub name: String,
    pub is_custom: bool,
}

pub fn list_available_themes(_codex_home: Option<&Path>) -> Vec<ThemeEntry> {
    Vec::new()
}
