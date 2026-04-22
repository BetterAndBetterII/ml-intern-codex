//! Render [`HistoryCellModel`] variants into styled ratatui `Line`s for the
//! inline-viewport transcript.
//!
//! Each variant has a visual convention borrowed from CodexPotter / Codex CLI:
//! user messages get a `› ` prefix, assistant messages get a `• ` prefix for
//! the first line and a two-space indent for continuations, exec cells render
//! as `$ command`, errors as `⚠ …` / `✖ …`, and so on.
//!
//! The rendered `Vec<Line<'static>>` is fed to
//! [`insert_history_lines`](crate::insert_history::insert_history_lines), which
//! writes them above the live bottom pane using ANSI scroll regions.

use mli_types::{ApprovalKind, HistoryCellModel, PendingApproval};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::diff_view;
use crate::exec_render;
use crate::markdown_render::render_markdown_text_with_width;
use crate::render::line_utils::prefix_lines;

/// Width assumed when no terminal width is available. Chosen to keep wrapping
/// reasonable while producing nonzero output in headless test contexts.
const FALLBACK_WIDTH: usize = 80;

/// Column width reserved for the live prefix (`› `, `• `, etc.) when wrapping.
const LIVE_PREFIX_COLS: usize = 2;

/// Render one model into its final styled lines for the scrollback.
pub fn render_cell(cell: &HistoryCellModel, width: Option<u16>) -> Vec<Line<'static>> {
    let width = width.map(usize::from).unwrap_or(FALLBACK_WIDTH).max(8);
    match cell {
        HistoryCellModel::UserMessage(c) => render_user_message(&c.text, width),
        HistoryCellModel::AssistantMessage(c) => render_assistant_message(&c.text, width),
        HistoryCellModel::ExecCommand(c) => exec_render::render_running(&c.command),
        HistoryCellModel::ExecOutput(c) => exec_render::render_finalized(&c.command, &c.output),
        HistoryCellModel::PatchSummary(c) => render_patch_summary(&c.summary, width),
        HistoryCellModel::PlanUpdate(c) => render_plan_update(&c.summary, width),
        HistoryCellModel::ApprovalRequest(c) => render_approval(&c.approval, width),
        HistoryCellModel::ArtifactCreated(c) => {
            render_artifact(&c.manifest, "artifact created", width)
        }
        HistoryCellModel::ArtifactUpdated(c) => {
            render_artifact(&c.manifest, "artifact updated", width)
        }
        HistoryCellModel::Warning(c) => render_warning(&c.message, width),
        HistoryCellModel::Error(c) => render_error(&c.message, width),
        HistoryCellModel::Status(c) => render_status(&c.message, width),
    }
}

fn render_user_message(text: &str, width: usize) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(LIVE_PREFIX_COLS).max(8);
    let rendered = render_markdown_text_with_width(text, Some(content_width));
    let lines: Vec<Line<'static>> = rendered.lines;

    let prefix = Span::styled("› ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    let indent = Span::raw("  ");
    let mut out = prefix_lines(lines, prefix, indent);
    if out.is_empty() {
        out.push(Line::from(vec![Span::styled(
            "› ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )]));
    }
    add_trailing_blank(out)
}

fn render_assistant_message(text: &str, width: usize) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(LIVE_PREFIX_COLS).max(8);
    let rendered = render_markdown_text_with_width(text, Some(content_width));
    let lines: Vec<Line<'static>> = rendered.lines;

    let bullet = Span::styled("• ", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD));
    let indent = Span::raw("  ");
    let mut out = prefix_lines(lines, bullet, indent);
    if out.is_empty() {
        out.push(Line::from(Span::styled(
            "• ",
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        )));
    }
    add_trailing_blank(out)
}

fn render_patch_summary(summary: &str, width: usize) -> Vec<Line<'static>> {
    if diff_view::looks_like_unified_diff(summary) {
        let mut lines = diff_view::render_unified_diff(summary);
        add_trailing_blank_lines(&mut lines);
        lines
    } else {
        render_prefixed_markdown(summary, width, "✎ ", Color::Yellow)
    }
}

fn add_trailing_blank_lines(lines: &mut Vec<Line<'static>>) {
    match lines.last() {
        Some(last) if last.spans.iter().all(|s| s.content.is_empty()) => {}
        _ => lines.push(Line::from("")),
    }
}

fn render_plan_update(summary: &str, width: usize) -> Vec<Line<'static>> {
    render_prefixed_markdown(summary, width, "◇ ", Color::LightBlue)
}

fn render_approval(approval: &PendingApproval, width: usize) -> Vec<Line<'static>> {
    let kind = match approval.kind {
        ApprovalKind::CommandExecution => "command",
        ApprovalKind::FileChange => "file change",
        ApprovalKind::PermissionRequest => "permission",
        ApprovalKind::RequestUserInput => "input",
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("? ", Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("approval requested ({kind})"),
            Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD),
        ),
    ])];
    if !approval.title.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(approval.title.clone(), Style::default().add_modifier(Modifier::BOLD)),
        ]));
    }
    let content_width = width.saturating_sub(2).max(8);
    let rendered = render_markdown_text_with_width(&approval.description, Some(content_width));
    lines.extend(prefix_lines(rendered.lines, Span::raw("  "), Span::raw("  ")));
    add_trailing_blank(lines)
}

fn render_artifact(
    manifest: &mli_types::ArtifactManifest,
    verb: &str,
    width: usize,
) -> Vec<Line<'static>> {
    let _ = width;
    let title = if manifest.title.is_empty() {
        manifest.id.to_string()
    } else {
        manifest.title.clone()
    };
    vec![
        Line::from(vec![
            Span::styled("◆ ", Style::default().fg(Color::LightMagenta).add_modifier(Modifier::BOLD)),
            Span::styled(verb.to_string(), Style::default().fg(Color::LightMagenta)),
            Span::raw("  "),
            Span::styled(title, Style::default().add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
    ]
}

fn render_warning(message: &str, width: usize) -> Vec<Line<'static>> {
    render_prefixed_markdown(message, width, "⚠ ", Color::Yellow)
}

fn render_error(message: &str, width: usize) -> Vec<Line<'static>> {
    render_prefixed_markdown(message, width, "✖ ", Color::Red)
}

fn render_status(message: &str, width: usize) -> Vec<Line<'static>> {
    render_prefixed_markdown(message, width, "· ", Color::DarkGray)
}

fn render_prefixed_markdown(
    text: &str,
    width: usize,
    prefix: &str,
    color: Color,
) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(LIVE_PREFIX_COLS).max(8);
    let rendered = render_markdown_text_with_width(text, Some(content_width));
    let lines = rendered.lines;
    let prefix_span = Span::styled(prefix.to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD));
    let indent = Span::raw("  ");
    let mut out = prefix_lines(lines, prefix_span, indent);
    if out.is_empty() {
        out.push(Line::from(vec![Span::styled(
            prefix.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )]));
    }
    add_trailing_blank(out)
}

fn add_trailing_blank(mut lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    match lines.last() {
        Some(last) if last.spans.iter().all(|s| s.content.is_empty()) => lines,
        _ => {
            lines.push(Line::from(""));
            lines
        }
    }
}

/// Unused today but kept because stylize traits need an import path.
#[allow(dead_code)]
fn _style_imports() {
    let _ = Style::default().bold();
}
