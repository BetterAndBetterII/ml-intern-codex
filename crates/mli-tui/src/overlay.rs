//! Modal overlays for the inline TUI: help text, skill/thread/artifact pickers,
//! and approval prompts. When an overlay is active the viewport expands to cover
//! the terminal and the overlay is drawn in place of the composer.

use std::collections::BTreeMap;

use mli_protocol::{ApprovalAnswer, ApprovalDecision, ApprovalRespondParams};
use mli_types::{ApprovalKind, ArtifactManifest, PendingApproval, SkillDescriptor, ThreadListItem};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

use crate::app::{
    ApprovalQuestion, ApprovalQuestionPayload, describe_skill, filter_artifacts, filter_skills,
    filter_threads,
};
use crate::completion::SLASH_COMMANDS;

pub enum Overlay {
    Help,
    ApprovalYesNo {
        approval: PendingApproval,
        selected_yes: bool,
    },
    ApprovalQuestionnaire {
        approval: PendingApproval,
        questions: Vec<ApprovalQuestion>,
        current_index: usize,
        answers: BTreeMap<String, ApprovalAnswer>,
        input_buffer: String,
        input_cursor: usize,
        option_cursor: usize,
    },
    SkillPicker {
        skills: Vec<SkillDescriptor>,
        query: String,
        cursor: usize,
    },
    ThreadPicker {
        threads: Vec<ThreadListItem>,
        query: String,
        cursor: usize,
    },
    ArtifactPicker {
        artifacts: Vec<ArtifactManifest>,
        query: String,
        cursor: usize,
    },
}

pub enum OverlayOutcome {
    Keep,
    Dismiss,
    SubmitApproval(ApprovalRespondParams),
    CancelApproval,
    SelectSkill(SkillDescriptor),
    ResumeThread(mli_types::LocalThreadId),
    OpenArtifact(mli_types::ArtifactId),
}

impl Overlay {
    pub fn title(&self) -> &'static str {
        match self {
            Overlay::Help => "help",
            Overlay::ApprovalYesNo { .. } => "approval requested",
            Overlay::ApprovalQuestionnaire { .. } => "approval requested",
            Overlay::SkillPicker { .. } => "skills",
            Overlay::ThreadPicker { .. } => "threads",
            Overlay::ArtifactPicker { .. } => "artifacts",
        }
    }

    pub fn for_pending_approval(approval: PendingApproval) -> Self {
        match approval.kind {
            ApprovalKind::RequestUserInput => {
                let payload =
                    serde_json::from_value::<ApprovalQuestionPayload>(approval.raw_payload.clone())
                        .ok();
                let questions = payload.map(|p| p.questions).unwrap_or_default();
                if questions.is_empty() {
                    Overlay::ApprovalYesNo {
                        approval,
                        selected_yes: false,
                    }
                } else {
                    Overlay::ApprovalQuestionnaire {
                        approval,
                        questions,
                        current_index: 0,
                        answers: BTreeMap::new(),
                        input_buffer: String::new(),
                        input_cursor: 0,
                        option_cursor: 0,
                    }
                }
            }
            _ => Overlay::ApprovalYesNo {
                approval,
                selected_yes: true,
            },
        }
    }
}

// -- Rendering --------------------------------------------------------------

pub fn render(area: Rect, buf: &mut Buffer, overlay: &Overlay) {
    Clear.render(area, buf);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(vec![Span::styled(
            format!(" {} ", overlay.title()),
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )]))
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    block.render(area, buf);
    match overlay {
        Overlay::Help => render_help(inner, buf),
        Overlay::ApprovalYesNo {
            approval,
            selected_yes,
        } => render_approval_yesno(inner, buf, approval, *selected_yes),
        Overlay::ApprovalQuestionnaire {
            approval,
            questions,
            current_index,
            input_buffer,
            input_cursor,
            option_cursor,
            ..
        } => render_approval_questionnaire(
            inner,
            buf,
            approval,
            questions,
            *current_index,
            input_buffer,
            *input_cursor,
            *option_cursor,
        ),
        Overlay::SkillPicker {
            skills,
            query,
            cursor,
        } => {
            let filtered = filter_skills(skills, Some(query.as_str()));
            let rows: Vec<Line> = filtered
                .iter()
                .map(|skill| {
                    let label = describe_skill(skill);
                    Line::from(label)
                })
                .collect();
            render_picker(inner, buf, query, &rows, *cursor);
        }
        Overlay::ThreadPicker {
            threads,
            query,
            cursor,
        } => {
            let filtered = filter_threads(threads, Some(query.as_str()));
            let rows: Vec<Line> = filtered
                .iter()
                .map(|item| {
                    let title = item.thread.title.as_deref().unwrap_or("(untitled)");
                    let id = item.thread.id.to_string();
                    let status = format!("{:?}", item.thread.status);
                    let marker = if item.selected { "◎ " } else { "  " };
                    Line::from(vec![
                        Span::raw(marker.to_string()),
                        Span::styled(
                            title.to_string(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("  "),
                        Span::styled(status, Style::default().fg(Color::DarkGray)),
                        Span::raw("  "),
                        Span::styled(id, Style::default().fg(Color::DarkGray)),
                    ])
                })
                .collect();
            render_picker(inner, buf, query, &rows, *cursor);
        }
        Overlay::ArtifactPicker {
            artifacts,
            query,
            cursor,
        } => {
            let filtered = filter_artifacts(artifacts, Some(query.as_str()));
            let rows: Vec<Line> = filtered
                .iter()
                .map(|art| {
                    let title = if art.title.is_empty() {
                        art.id.to_string()
                    } else {
                        art.title.clone()
                    };
                    Line::from(vec![
                        Span::styled(title, Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw("  "),
                        Span::styled(
                            format!("{:?}", art.kind),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ])
                })
                .collect();
            render_picker(inner, buf, query, &rows, *cursor);
        }
    }
}

fn render_help(area: Rect, buf: &mut Buffer) {
    let mut lines = vec![Line::from("Commands")];
    for command in SLASH_COMMANDS {
        lines.push(Line::from(format!(
            "  {:<11} {}",
            command.name, command.description
        )));
    }
    lines.extend([
        Line::from(""),
        Line::from("Keys"),
        Line::from("  Enter          submit prompt"),
        Line::from("  Shift+Enter    newline in composer"),
        Line::from("  Ctrl+C         interrupt / exit"),
        Line::from("  Esc            close overlay"),
        Line::from(""),
        Line::from("Tips"),
        Line::from("  type `$skill-name …` to use a skill just for this turn"),
    ]);
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .render(area, buf);
}

fn render_approval_yesno(
    area: Rect,
    buf: &mut Buffer,
    approval: &PendingApproval,
    selected_yes: bool,
) {
    let kind = match approval.kind {
        ApprovalKind::CommandExecution => "run this command",
        ApprovalKind::FileChange => "apply this patch",
        ApprovalKind::PermissionRequest => "grant this permission",
        ApprovalKind::RequestUserInput => "continue with this input",
    };
    let mut lines = vec![
        Line::from(vec![Span::styled(
            format!("Approve: {kind}?"),
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
    ];
    if !approval.title.is_empty() {
        lines.push(Line::from(Span::styled(
            approval.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
    }
    if !approval.description.is_empty() {
        for l in approval.description.lines() {
            lines.push(Line::from(l.to_string()));
        }
    }
    lines.push(Line::from(""));
    let yes_style = if selected_yes {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green)
    };
    let no_style = if !selected_yes {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Red)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Red)
    };
    lines.push(Line::from(vec![
        Span::styled(" approve ", yes_style),
        Span::raw("   "),
        Span::styled(" reject ", no_style),
    ]));
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .render(area, buf);
}

#[allow(clippy::too_many_arguments)]
fn render_approval_questionnaire(
    area: Rect,
    buf: &mut Buffer,
    approval: &PendingApproval,
    questions: &[ApprovalQuestion],
    current_index: usize,
    input_buffer: &str,
    _input_cursor: usize,
    option_cursor: usize,
) {
    let progress = format!("({}/{})", current_index + 1, questions.len());
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                approval.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(progress, Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(""),
    ];
    if let Some(q) = questions.get(current_index) {
        lines.push(Line::from(vec![Span::styled(
            q.header.clone(),
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(q.question.clone()));
        lines.push(Line::from(""));
        if let Some(options) = &q.options {
            for (idx, opt) in options.iter().enumerate() {
                let style = if idx == option_cursor {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!(" {} ", if idx == option_cursor { "›" } else { " " }),
                        style,
                    ),
                    Span::raw(" "),
                    Span::styled(
                        opt.label.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        opt.description.clone(),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }
            if q.is_other {
                lines.push(Line::from(""));
                lines.push(Line::from("or type free-form text below and press Enter"));
            }
        } else {
            lines.push(Line::from(Span::styled(
                "type a response and press Enter",
                Style::default().fg(Color::DarkGray),
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                "› ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(input_buffer.to_string()),
            Span::styled(
                "│",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .render(area, buf);
}

fn render_picker(area: Rect, buf: &mut Buffer, query: &str, rows: &[Line<'_>], cursor: usize) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(area);
    let query_line = Line::from(vec![
        Span::styled("filter › ", Style::default().fg(Color::DarkGray)),
        Span::raw(query.to_string()),
        Span::styled(
            "│",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    Paragraph::new(vec![query_line, Line::from("")]).render(chunks[0], buf);

    let list_area = chunks[1];
    if rows.is_empty() {
        Paragraph::new(Line::from(Span::styled(
            "no matches",
            Style::default().fg(Color::DarkGray),
        )))
        .render(list_area, buf);
        return;
    }
    let capacity = list_area.height as usize;
    let (start, end) = visible_window(rows.len(), capacity, cursor);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(end - start);
    for (idx, row) in rows.iter().enumerate().skip(start).take(end - start) {
        let style = if idx == cursor {
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let marker = if idx == cursor { "› " } else { "  " };
        let mut spans = vec![Span::styled(marker.to_string(), style)];
        for span in &row.spans {
            let content: String = span.content.as_ref().to_string();
            spans.push(Span::styled(content, span.style.patch(style)));
        }
        lines.push(Line::from(spans));
    }
    Paragraph::new(lines).render(list_area, buf);
}

fn visible_window(total: usize, capacity: usize, cursor: usize) -> (usize, usize) {
    if capacity == 0 || total == 0 {
        return (0, 0);
    }
    let capacity = capacity.min(total);
    let half = capacity / 2;
    let start = cursor.saturating_sub(half);
    let start = start.min(total.saturating_sub(capacity));
    let end = (start + capacity).min(total);
    (start, end)
}

// -- Key handling -----------------------------------------------------------

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn handle_key(overlay: &mut Overlay, key: KeyEvent) -> OverlayOutcome {
    if matches!(key.code, KeyCode::Esc) {
        return match overlay {
            Overlay::ApprovalYesNo { .. } | Overlay::ApprovalQuestionnaire { .. } => {
                OverlayOutcome::CancelApproval
            }
            _ => OverlayOutcome::Dismiss,
        };
    }
    match overlay {
        Overlay::Help => {
            if matches!(key.code, KeyCode::Enter | KeyCode::Char('q' | 'Q')) {
                OverlayOutcome::Dismiss
            } else {
                OverlayOutcome::Keep
            }
        }
        Overlay::ApprovalYesNo {
            approval,
            selected_yes,
        } => match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                *selected_yes = !*selected_yes;
                OverlayOutcome::Keep
            }
            KeyCode::Char('y' | 'Y') => {
                *selected_yes = true;
                submit_yes_no(approval, true)
            }
            KeyCode::Char('n' | 'N') => {
                *selected_yes = false;
                submit_yes_no(approval, false)
            }
            KeyCode::Enter => submit_yes_no(approval, *selected_yes),
            _ => OverlayOutcome::Keep,
        },
        Overlay::ApprovalQuestionnaire {
            approval,
            questions,
            current_index,
            answers,
            input_buffer,
            input_cursor,
            option_cursor,
        } => handle_questionnaire_key(
            key,
            approval,
            questions,
            current_index,
            answers,
            input_buffer,
            input_cursor,
            option_cursor,
        ),
        Overlay::SkillPicker {
            skills,
            query,
            cursor,
        } => {
            let query_snapshot = query.clone();
            let filtered: Vec<SkillDescriptor> =
                filter_skills(skills, Some(query_snapshot.as_str()))
                    .into_iter()
                    .cloned()
                    .collect();
            let filtered_count = filtered.len();
            handle_picker_key(key, query, cursor, filtered_count, |idx| {
                filtered.get(idx).cloned().map(OverlayOutcome::SelectSkill)
            })
        }
        Overlay::ThreadPicker {
            threads,
            query,
            cursor,
        } => {
            let query_snapshot = query.clone();
            let filtered: Vec<mli_types::LocalThreadId> =
                filter_threads(threads, Some(query_snapshot.as_str()))
                    .into_iter()
                    .map(|item| item.thread.id)
                    .collect();
            let filtered_count = filtered.len();
            handle_picker_key(key, query, cursor, filtered_count, |idx| {
                filtered.get(idx).copied().map(OverlayOutcome::ResumeThread)
            })
        }
        Overlay::ArtifactPicker {
            artifacts,
            query,
            cursor,
        } => {
            let query_snapshot = query.clone();
            let filtered: Vec<mli_types::ArtifactId> =
                filter_artifacts(artifacts, Some(query_snapshot.as_str()))
                    .into_iter()
                    .map(|art| art.id)
                    .collect();
            let filtered_count = filtered.len();
            handle_picker_key(key, query, cursor, filtered_count, |idx| {
                filtered.get(idx).copied().map(OverlayOutcome::OpenArtifact)
            })
        }
    }
}

fn submit_yes_no(approval: &PendingApproval, yes: bool) -> OverlayOutcome {
    let decision = if yes {
        ApprovalDecision::Approve
    } else {
        ApprovalDecision::Reject
    };
    OverlayOutcome::SubmitApproval(ApprovalRespondParams {
        approval_id: approval.id.clone(),
        decision,
        answers: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn handle_questionnaire_key(
    key: KeyEvent,
    approval: &PendingApproval,
    questions: &[ApprovalQuestion],
    current_index: &mut usize,
    answers: &mut BTreeMap<String, ApprovalAnswer>,
    input_buffer: &mut String,
    input_cursor: &mut usize,
    option_cursor: &mut usize,
) -> OverlayOutcome {
    let Some(question) = questions.get(*current_index) else {
        return OverlayOutcome::Keep;
    };
    let has_options = question.options.is_some();
    let option_count = question.options.as_ref().map(|o| o.len()).unwrap_or(0);
    match key.code {
        KeyCode::Up if has_options => {
            if *option_cursor > 0 {
                *option_cursor -= 1;
            }
            OverlayOutcome::Keep
        }
        KeyCode::Down if has_options => {
            if *option_cursor + 1 < option_count {
                *option_cursor += 1;
            }
            OverlayOutcome::Keep
        }
        KeyCode::Enter => {
            let chosen_text = if !input_buffer.trim().is_empty() {
                input_buffer.trim().to_string()
            } else if let Some(options) = &question.options {
                if let Some(opt) = options.get(*option_cursor) {
                    opt.label.clone()
                } else {
                    return OverlayOutcome::Keep;
                }
            } else {
                return OverlayOutcome::Keep;
            };
            answers.insert(
                question.id.clone(),
                ApprovalAnswer {
                    answers: vec![chosen_text],
                },
            );
            input_buffer.clear();
            *input_cursor = 0;
            *option_cursor = 0;
            if *current_index + 1 < questions.len() {
                *current_index += 1;
                OverlayOutcome::Keep
            } else {
                let answers_map = std::mem::take(answers);
                OverlayOutcome::SubmitApproval(ApprovalRespondParams {
                    approval_id: approval.id.clone(),
                    decision: ApprovalDecision::Approve,
                    answers: Some(answers_map),
                })
            }
        }
        KeyCode::Backspace => {
            if !input_buffer.is_empty() {
                let prev = prev_char_boundary(input_buffer, *input_cursor);
                input_buffer.drain(prev..*input_cursor);
                *input_cursor = prev;
            }
            OverlayOutcome::Keep
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let idx = (*input_cursor).min(input_buffer.len());
            input_buffer.insert(idx, ch);
            *input_cursor = idx + ch.len_utf8();
            OverlayOutcome::Keep
        }
        _ => OverlayOutcome::Keep,
    }
}

fn handle_picker_key(
    key: KeyEvent,
    query: &mut String,
    cursor: &mut usize,
    filtered_count: usize,
    on_confirm: impl FnOnce(usize) -> Option<OverlayOutcome>,
) -> OverlayOutcome {
    match key.code {
        KeyCode::Up => {
            if *cursor > 0 {
                *cursor -= 1;
            }
            OverlayOutcome::Keep
        }
        KeyCode::Down => {
            if *cursor + 1 < filtered_count {
                *cursor += 1;
            }
            OverlayOutcome::Keep
        }
        KeyCode::Home => {
            *cursor = 0;
            OverlayOutcome::Keep
        }
        KeyCode::End => {
            if filtered_count > 0 {
                *cursor = filtered_count - 1;
            }
            OverlayOutcome::Keep
        }
        KeyCode::Enter => on_confirm(*cursor).unwrap_or(OverlayOutcome::Keep),
        KeyCode::Backspace => {
            if !query.is_empty() {
                let mut end = query.len();
                loop {
                    end -= 1;
                    if query.is_char_boundary(end) {
                        break;
                    }
                }
                query.truncate(end);
                *cursor = 0;
            }
            OverlayOutcome::Keep
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            query.push(ch);
            *cursor = 0;
            OverlayOutcome::Keep
        }
        _ => OverlayOutcome::Keep,
    }
}

fn prev_char_boundary(buffer: &str, cursor: usize) -> usize {
    let mut idx = cursor.min(buffer.len());
    if idx == 0 {
        return 0;
    }
    idx -= 1;
    while idx > 0 && !buffer.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}
