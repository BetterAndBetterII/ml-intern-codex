//! Bottom pane: live composer + status indicator rendered inside the inline viewport.
//!
//! Rendered every frame on top of the scrollback-resident history. Holds three
//! elements:
//!
//! 1. Optional status banner (connection / streaming / approval) shown above
//!    the composer while the agent is thinking or waiting on input.
//! 2. The composer: a bordered multi-line textarea with a `›` prompt, live
//!    skill mention hint, placeholder text, and horizontal/vertical scrolling.
//!    The real terminal cursor is positioned via the returned
//!    [`BottomPaneLayout::cursor`] so focus blinking and screen readers work.
//! 3. A thin footer with key hints plus ephemeral toast / queued-prompt display.

use std::time::Instant;

use mli_types::{ApprovalKind, ApprovalPolicy, ConnectionState, PendingApproval, SandboxMode};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, WidgetRef, Wrap};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::completion::{self, Completion};

const MIN_HEIGHT: u16 = 5; // status + composer (3 rows including borders) + hint
const COMPOSER_MIN_INNER_ROWS: u16 = 1;
const COMPOSER_MAX_INNER_ROWS: u16 = 8;

pub struct BottomPaneProps<'a> {
    pub connection: ConnectionState,
    pub approval_policy: Option<ApprovalPolicy>,
    pub sandbox_mode: Option<SandboxMode>,
    pub selected_skill: Option<&'a str>,
    pub composer_buffer: &'a str,
    pub composer_cursor: usize,
    pub pending_approval: Option<&'a PendingApproval>,
    pub task_started_at: Option<Instant>,
    pub queued_prompts: usize,
    pub toast: Option<&'a str>,
    pub hint: Option<&'a str>,
    pub completion: Option<&'a Completion>,
}

/// Result of rendering the bottom pane. `cursor` is the absolute terminal
/// position where the caller should place the real cursor via
/// `frame.set_cursor_position`.
#[derive(Clone, Copy, Debug, Default)]
pub struct BottomPaneLayout {
    pub cursor: Option<Position>,
}

/// Preferred total height for the bottom pane given the current state & width.
pub fn desired_height(props: &BottomPaneProps, width: u16) -> u16 {
    let width = width.max(1);
    let status_rows = if show_status_row(props) { 1 } else { 0 };
    let approval_rows = if props.pending_approval.is_some() { 2 } else { 0 };
    let inner_cols = width.saturating_sub(4) as usize; // borders + left gutter
    let composer_inner = composer_inner_rows(props.composer_buffer, inner_cols);
    let composer_rows = composer_inner + 2; // borders top + bottom
    let hint_rows: u16 = 1;
    let popup_rows = props.completion.map(completion::desired_height).unwrap_or(0);
    let total = popup_rows + status_rows + approval_rows + composer_rows + hint_rows;
    total.max(MIN_HEIGHT)
}

pub fn render(area: Rect, buf: &mut Buffer, props: &BottomPaneProps) -> BottomPaneLayout {
    if area.height == 0 || area.width == 0 {
        return BottomPaneLayout::default();
    }

    let status_rows: u16 = if show_status_row(props) { 1 } else { 0 };
    let approval_rows: u16 = if props.pending_approval.is_some() { 2 } else { 0 };
    let hint_rows: u16 = 1;
    let popup_rows: u16 = props
        .completion
        .map(completion::desired_height)
        .unwrap_or(0);

    let inner_cols = area.width.saturating_sub(4) as usize;
    let composer_inner = composer_inner_rows(props.composer_buffer, inner_cols);
    let composer_total = (composer_inner + 2).max(3);
    let reserved = status_rows + approval_rows + hint_rows + popup_rows;
    let composer_rows = composer_total.min(area.height.saturating_sub(reserved).max(3));

    let mut constraints = Vec::new();
    if status_rows > 0 {
        constraints.push(Constraint::Length(status_rows));
    }
    if approval_rows > 0 {
        constraints.push(Constraint::Length(approval_rows));
    }
    if popup_rows > 0 {
        constraints.push(Constraint::Length(popup_rows));
    }
    constraints.push(Constraint::Length(composer_rows));
    constraints.push(Constraint::Length(hint_rows));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);
    let mut idx = 0usize;

    if status_rows > 0 {
        render_status(chunks[idx], buf, props);
        idx += 1;
    }
    if approval_rows > 0 {
        render_pending_approval(chunks[idx], buf, props);
        idx += 1;
    }
    if popup_rows > 0 {
        if let Some(popup) = props.completion {
            completion::render(chunks[idx], buf, popup);
        }
        idx += 1;
    }
    let composer_area = chunks[idx];
    idx += 1;
    let hint_area = chunks[idx];

    let cursor = render_composer(composer_area, buf, props);
    render_hint(hint_area, buf, props);
    BottomPaneLayout { cursor }
}

fn show_status_row(props: &BottomPaneProps) -> bool {
    !matches!(props.connection, ConnectionState::Ready) || props.queued_prompts > 0
}

fn render_status(area: Rect, buf: &mut Buffer, props: &BottomPaneProps) {
    let (label, color) = match props.connection {
        ConnectionState::Booting | ConnectionState::Connecting | ConnectionState::Initializing => {
            ("connecting", Color::Yellow)
        }
        ConnectionState::Ready => ("ready", Color::Green),
        ConnectionState::Streaming => ("working", Color::Cyan),
        ConnectionState::WaitingApproval => ("waiting for approval", Color::LightYellow),
        ConnectionState::Disconnected => ("disconnected", Color::Red),
        ConnectionState::Reconnecting => ("reconnecting", Color::Yellow),
    };
    let mut spans = vec![
        Span::styled("● ", Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::styled(
            label.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(started) = props.task_started_at {
        let elapsed = started.elapsed().as_secs();
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("{elapsed}s"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if props.queued_prompts > 0 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("{} queued", props.queued_prompts),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if let Some(policy) = props.approval_policy {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("approval: {}", policy_label(policy)),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if let Some(mode) = props.sandbox_mode {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("sandbox: {}", sandbox_label(mode)),
            Style::default().fg(Color::DarkGray),
        ));
    }
    Paragraph::new(Line::from(spans)).render(area, buf);
}

fn render_pending_approval(area: Rect, buf: &mut Buffer, props: &BottomPaneProps) {
    let Some(approval) = props.pending_approval else {
        return;
    };
    let kind = match approval.kind {
        ApprovalKind::CommandExecution => "run command",
        ApprovalKind::FileChange => "apply patch",
        ApprovalKind::PermissionRequest => "grant permission",
        ApprovalKind::RequestUserInput => "answer prompt",
    };
    let header = Line::from(vec![
        Span::styled(
            "? approval required — ",
            Style::default()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(kind.to_string(), Style::default().fg(Color::LightYellow)),
    ]);
    let title = Line::from(vec![
        Span::raw("  "),
        Span::styled(
            approval.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]);
    Paragraph::new(vec![header, title])
        .wrap(Wrap { trim: false })
        .render(area, buf);
}

/// Render the composer inside a titled/bordered block and return the terminal
/// cursor position that should blink when input is focused.
fn render_composer(area: Rect, buf: &mut Buffer, props: &BottomPaneProps) -> Option<Position> {
    let border_color = match props.connection {
        ConnectionState::Ready => Color::DarkGray,
        ConnectionState::Streaming => Color::Cyan,
        ConnectionState::WaitingApproval => Color::LightYellow,
        _ => Color::DarkGray,
    };

    let mut title_spans = vec![Span::styled(
        " compose ",
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(skill) = props.selected_skill {
        title_spans.push(Span::raw(" "));
        title_spans.push(Span::styled(
            format!("[{skill}]"),
            Style::default().fg(Color::Magenta),
        ));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(title_spans));
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.width == 0 || inner.height == 0 {
        return None;
    }

    // Left gutter for the prompt marker, rendered on every inner row.
    let gutter_width: u16 = 2;
    let text_area = Rect {
        x: inner.x.saturating_add(gutter_width),
        y: inner.y,
        width: inner.width.saturating_sub(gutter_width),
        height: inner.height,
    };
    if text_area.width == 0 || text_area.height == 0 {
        return None;
    }

    // Gutter marker on every visible line (dim for all but the first).
    for row in 0..inner.height {
        let y = inner.y + row;
        let marker = if row == 0 { "› " } else { "  " };
        let style = if row == 0 {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let mut buf_x = inner.x;
        for ch in marker.chars() {
            if buf_x >= inner.x + gutter_width {
                break;
            }
            let cell = &mut buf[(buf_x, y)];
            cell.set_symbol(&ch.to_string());
            cell.set_style(style);
            buf_x += 1;
        }
    }

    let buffer = props.composer_buffer;
    let is_empty = buffer.is_empty();

    if is_empty {
        // Placeholder text.
        let placeholder = match props.connection {
            ConnectionState::WaitingApproval => "approve or reject above before sending",
            ConnectionState::Streaming => "type to queue; Ctrl+C to interrupt",
            _ => "type a prompt, `$` for skills, `/` for commands, Enter to send",
        };
        Paragraph::new(Line::from(Span::styled(
            placeholder,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )))
        .render(text_area, buf);
        return Some(Position {
            x: text_area.x,
            y: text_area.y,
        });
    }

    // Build a virtual rendering of the buffer where each logical line is broken
    // into physical rows (hard wrapping by display width). We also remember the
    // (line_idx, col_bytes) mapping for the cursor.
    let (row_contents, cursor_rc) = layout_buffer(
        buffer,
        props.composer_cursor,
        text_area.width as usize,
        text_area.height as usize,
    );

    let (cursor_row, cursor_col) = cursor_rc;
    // Vertical scroll: keep the cursor row on-screen.
    let first_visible = if cursor_row >= text_area.height as usize {
        cursor_row + 1 - text_area.height as usize
    } else {
        0
    };
    let last_visible = (first_visible + text_area.height as usize).min(row_contents.len());

    for (row_offset, idx) in (first_visible..last_visible).enumerate() {
        let text = &row_contents[idx];
        let y = text_area.y + row_offset as u16;
        let mut x = text_area.x;
        for ch in text.chars() {
            if x >= text_area.x + text_area.width {
                break;
            }
            let width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
            if width == 0 {
                continue;
            }
            if x + width > text_area.x + text_area.width {
                break;
            }
            let cell = &mut buf[(x, y)];
            cell.set_symbol(&ch.to_string());
            x += width;
        }
    }

    if cursor_row >= first_visible && cursor_row < last_visible {
        let y = text_area.y + (cursor_row - first_visible) as u16;
        let x = text_area.x + (cursor_col as u16).min(text_area.width.saturating_sub(1));
        Some(Position { x, y })
    } else {
        // Cursor off-screen (shouldn't happen with the scroll logic, but be safe).
        None
    }
}

/// Produce the physical rows that represent `buffer` when hard-wrapped at
/// `width`, and compute `(row, col)` for the byte cursor.
fn layout_buffer(
    buffer: &str,
    byte_cursor: usize,
    width: usize,
    _max_rows: usize,
) -> (Vec<String>, (usize, usize)) {
    let width = width.max(1);
    let byte_cursor = byte_cursor.min(buffer.len());
    let mut rows: Vec<String> = Vec::new();
    let mut cursor_row = 0usize;
    let mut cursor_col = 0usize;

    let logical_lines: Vec<&str> = buffer.split('\n').collect();
    let mut byte_offset = 0usize;
    for (line_idx, line) in logical_lines.iter().enumerate() {
        let line_bytes_end = byte_offset + line.len();
        // Break this logical line into physical rows by display width.
        let mut current = String::new();
        let mut current_width = 0usize;
        let mut row_for_this_logical_line: Vec<String> = Vec::new();
        let mut segment_start_byte = byte_offset;
        let mut cursor_found_in_line = false;

        for (g_byte_offset, g) in line.grapheme_indices(true) {
            let g_width = g.width();
            // Check if cursor falls at the *start* of this grapheme.
            let abs_byte = byte_offset + g_byte_offset;
            if !cursor_found_in_line && byte_cursor == abs_byte {
                cursor_row = rows.len() + row_for_this_logical_line.len();
                cursor_col =
                    display_width_of(&current[..]).min(width);
                cursor_found_in_line = true;
            }
            if current_width + g_width > width && !current.is_empty() {
                row_for_this_logical_line.push(std::mem::take(&mut current));
                current_width = 0;
                segment_start_byte = abs_byte;
            }
            current.push_str(g);
            current_width += g_width;
            let _ = segment_start_byte;
        }
        // Cursor at end-of-line?
        if !cursor_found_in_line && byte_cursor == line_bytes_end {
            cursor_row = rows.len() + row_for_this_logical_line.len();
            cursor_col = display_width_of(&current[..]).min(width);
        }
        let _ = cursor_found_in_line;
        row_for_this_logical_line.push(std::mem::take(&mut current));
        if row_for_this_logical_line.is_empty() {
            row_for_this_logical_line.push(String::new());
        }
        rows.extend(row_for_this_logical_line);

        byte_offset = line_bytes_end + 1; // +1 for the '\n' (unless last)
        let _ = line_idx;
    }

    (rows, (cursor_row, cursor_col))
}

fn display_width_of(s: &str) -> usize {
    s.width()
}

fn render_hint(area: Rect, buf: &mut Buffer, props: &BottomPaneProps) {
    let text = props
        .toast
        .map(|t| t.to_string())
        .or_else(|| props.hint.map(|h| h.to_string()))
        .unwrap_or_else(|| match props.connection {
            ConnectionState::WaitingApproval => {
                "enter to confirm  ·  esc to deny  ·  /quit to exit".to_string()
            }
            ConnectionState::Streaming => "ctrl+c interrupts  ·  /quit to exit".to_string(),
            _ => {
                "enter  send  ·  shift+enter  newline  ·  ctrl+u  clear  ·  ↑ history  ·  /help"
                    .to_string()
            }
        });
    let style = Style::default().fg(Color::DarkGray);
    Paragraph::new(Line::from(Span::styled(text, style))).render(area, buf);
}

fn composer_inner_rows(buffer: &str, width: usize) -> u16 {
    if buffer.is_empty() {
        return COMPOSER_MIN_INNER_ROWS.max(1);
    }
    let width = width.max(1);
    let mut rows = 0usize;
    for line in buffer.split('\n') {
        let w = line.width().max(1);
        rows += w.div_ceil(width);
    }
    (rows.max(1) as u16)
        .max(COMPOSER_MIN_INNER_ROWS)
        .min(COMPOSER_MAX_INNER_ROWS)
}

fn policy_label(policy: ApprovalPolicy) -> &'static str {
    match policy {
        ApprovalPolicy::Never => "never",
        ApprovalPolicy::OnFailure => "on-failure",
        ApprovalPolicy::OnRequest => "on-request",
        ApprovalPolicy::Untrusted => "untrusted",
    }
}

fn sandbox_label(mode: SandboxMode) -> &'static str {
    match mode {
        SandboxMode::ReadOnly => "read-only",
        SandboxMode::WorkspaceWrite => "workspace-write",
        SandboxMode::DangerFullAccess => "danger-full-access",
    }
}

/// Wrapper that implements [`WidgetRef`] so the pane can be used with ratatui's
/// frame rendering without taking ownership of the props.
pub struct BottomPaneWidget<'a>(pub &'a BottomPaneProps<'a>);

impl<'a> WidgetRef for BottomPaneWidget<'a> {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        render(area, buf, self.0);
    }
}

// Silence unused-import warning until the grapheme-based width helper lands.
#[allow(dead_code)]
fn _unused_width(s: &str) -> usize {
    s.graphemes(true).map(|g| g.width()).sum()
}
