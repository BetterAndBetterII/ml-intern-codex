//! Autocomplete popup for the composer.
//!
//! Two trigger modes, both anchored at the start of the composer buffer so the
//! existing `parse_leading_skill_token` / slash-command handling in `app.rs`
//! stays authoritative:
//!
//! * `$` + query characters → skills popup. Enter/Tab replaces the token with
//!   `$<skill-name> ` and leaves the cursor ready to keep typing.
//! * `/` + query characters → commands popup. Enter/Tab replaces the token
//!   with the full command name (e.g. `/skills`) and moves the cursor to the
//!   end. Commands like `/quit` auto-submit when confirmed.
//!
//! The popup re-evaluates after every composer change (key, paste, history
//! restore). Up/Down navigate, Esc closes without inserting.

use mli_types::SkillDescriptor;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

/// Maximum visible rows in the popup list. Matches typical IDE autocomplete.
pub const POPUP_MAX_ROWS: u16 = 6;

#[derive(Clone)]
pub struct Completion {
    pub kind: CompletionKind,
    /// Byte offset in the composer buffer where the trigger character (`$`
    /// or `/`) lives. Used to slice the token out on accept.
    pub anchor_byte: usize,
    /// Byte offset in the composer buffer of the *end* of the current token
    /// (usually equal to the cursor unless the cursor moved into the middle).
    pub token_end_byte: usize,
    /// The filter query (text after the trigger).
    pub query: String,
    /// Selected row in `filtered`.
    pub cursor: usize,
    /// Indices into the full item list that match `query`.
    pub filtered: Vec<usize>,
}

#[derive(Clone)]
pub enum CompletionKind {
    Skills(Vec<SkillDescriptor>),
    Commands(Vec<SlashCommand>),
}

#[derive(Clone, Copy, Debug)]
pub struct SlashCommand {
    pub name: &'static str,
    pub description: &'static str,
}

pub const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "/help",
        description: "show keybindings and commands",
    },
    SlashCommand {
        name: "/skills",
        description: "pick a skill to apply to the next prompt",
    },
    SlashCommand {
        name: "/threads",
        description: "resume a previous thread",
    },
    SlashCommand {
        name: "/artifacts",
        description: "browse generated artifacts",
    },
    SlashCommand {
        name: "/approval",
        description: "reopen the last approval request",
    },
    SlashCommand {
        name: "/clear",
        description: "clear the in-memory transcript view",
    },
    SlashCommand {
        name: "/quit",
        description: "exit ml-intern",
    },
];

pub enum AcceptOutcome {
    /// Replace the token [anchor..token_end] with `replacement`, position the
    /// cursor at `cursor_offset` after the replacement's start.
    Replace {
        replacement: String,
        cursor_offset: usize,
    },
    /// The command should be dispatched immediately (e.g. `/quit`, `/help`).
    Submit(String),
}

impl Completion {
    pub fn title(&self) -> &'static str {
        match self.kind {
            CompletionKind::Skills(_) => "skills",
            CompletionKind::Commands(_) => "commands",
        }
    }

    pub fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }

    /// Move the popup cursor with wrap-around.
    pub fn move_cursor(&mut self, delta: i32) {
        if self.filtered.is_empty() {
            return;
        }
        let len = self.filtered.len() as i32;
        let current = self.cursor as i32;
        let next = (current + delta).rem_euclid(len);
        self.cursor = next as usize;
    }

    /// Return an action describing what to do with the selected item.
    pub fn accept(&self) -> Option<AcceptOutcome> {
        let idx = *self.filtered.get(self.cursor)?;
        match &self.kind {
            CompletionKind::Skills(items) => {
                let skill = items.get(idx)?;
                let replacement = format!("${} ", skill.name);
                let cursor_offset = replacement.len();
                Some(AcceptOutcome::Replace {
                    replacement,
                    cursor_offset,
                })
            }
            CompletionKind::Commands(items) => {
                let cmd = items.get(idx)?;
                // Dispatch `/quit` and the like immediately. The full list of
                // "immediate" commands is whatever `handle_slash` handles
                // without extra arguments — all current slash commands qualify.
                Some(AcceptOutcome::Submit(cmd.name.to_string()))
            }
        }
    }
}

/// Decide whether a popup should be open, based on the composer buffer and cursor.
///
/// `skill_cache` is a borrow into the parent app's lazily-fetched skill list.
/// Pass `None` on the first invocation to let the caller lazy-populate and re-run.
pub fn evaluate(
    buffer: &str,
    cursor: usize,
    skill_cache: Option<&[SkillDescriptor]>,
    previous: Option<&Completion>,
) -> Option<Completion> {
    let cursor = cursor.min(buffer.len());
    let (trigger_byte, kind_kind) = detect_trigger(buffer, cursor)?;

    // Find end of token (first whitespace after trigger, or cursor if user hasn't typed a space).
    let token_end_byte = buffer[trigger_byte..]
        .find(|c: char| c.is_whitespace())
        .map(|off| trigger_byte + off)
        .unwrap_or(buffer.len());
    if cursor < trigger_byte || cursor > token_end_byte {
        return None;
    }
    let query = buffer[trigger_byte + 1..cursor].to_string();

    match kind_kind {
        TriggerKind::Skill => {
            let items = skill_cache?.to_vec();
            let filtered = filter_skills(&items, &query);
            // Preserve cursor selection if the filter set overlaps previous state.
            let cursor = previous
                .and_then(|prev| {
                    if matches!(prev.kind, CompletionKind::Skills(_)) {
                        Some(prev.cursor.min(filtered.len().saturating_sub(1)))
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            Some(Completion {
                kind: CompletionKind::Skills(items),
                anchor_byte: trigger_byte,
                token_end_byte,
                query,
                cursor,
                filtered,
            })
        }
        TriggerKind::Command => {
            let items = SLASH_COMMANDS.to_vec();
            let filtered = filter_commands(&items, &query);
            let cursor = previous
                .and_then(|prev| {
                    if matches!(prev.kind, CompletionKind::Commands(_)) {
                        Some(prev.cursor.min(filtered.len().saturating_sub(1)))
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            Some(Completion {
                kind: CompletionKind::Commands(items),
                anchor_byte: trigger_byte,
                token_end_byte,
                query,
                cursor,
                filtered,
            })
        }
    }
}

enum TriggerKind {
    Skill,
    Command,
}

/// The trigger character is valid only at byte 0 of the buffer (first token),
/// matching the "leading skill token" / slash-command semantics elsewhere.
fn detect_trigger(buffer: &str, cursor: usize) -> Option<(usize, TriggerKind)> {
    if cursor == 0 {
        return None;
    }
    let first = buffer.as_bytes().first()?;
    match first {
        b'$' => Some((0, TriggerKind::Skill)),
        b'/' => Some((0, TriggerKind::Command)),
        _ => None,
    }
}

fn filter_skills(skills: &[SkillDescriptor], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..skills.len()).collect();
    }
    let q = query.to_ascii_lowercase();
    let mut hits: Vec<(usize, bool)> = Vec::new();
    for (idx, s) in skills.iter().enumerate() {
        let name = s.name.to_ascii_lowercase();
        if name.starts_with(&q) {
            hits.push((idx, true));
        } else if name.contains(&q) || s.description.to_ascii_lowercase().contains(&q) {
            hits.push((idx, false));
        }
    }
    // Prefix matches first.
    hits.sort_by(|a, b| b.1.cmp(&a.1));
    hits.into_iter().map(|(i, _)| i).collect()
}

fn filter_commands(cmds: &[SlashCommand], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..cmds.len()).collect();
    }
    let q = query.to_ascii_lowercase();
    let mut hits: Vec<(usize, bool)> = Vec::new();
    for (idx, cmd) in cmds.iter().enumerate() {
        // `query` does not include the leading '/'; the cmd names do.
        let name = cmd.name.trim_start_matches('/').to_ascii_lowercase();
        if name.starts_with(&q) {
            hits.push((idx, true));
        } else if name.contains(&q) || cmd.description.to_ascii_lowercase().contains(&q) {
            hits.push((idx, false));
        }
    }
    hits.sort_by(|a, b| b.1.cmp(&a.1));
    hits.into_iter().map(|(i, _)| i).collect()
}

/// Height the popup wants, given its filtered set. Returns 0 when there is
/// nothing to show.
pub fn desired_height(popup: &Completion) -> u16 {
    if popup.filtered.is_empty() {
        return 0;
    }
    let rows = popup.filtered.len().min(POPUP_MAX_ROWS as usize) as u16;
    // +2 for block borders.
    rows + 2
}

pub fn render(area: Rect, buf: &mut Buffer, popup: &Completion) {
    if popup.filtered.is_empty() || area.height == 0 || area.width == 0 {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Line::from(vec![
            Span::styled(
                format!(" {} ", popup.title()),
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("({} match{})", popup.filtered.len(), if popup.filtered.len() == 1 { "" } else { "es" }),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height == 0 {
        return;
    }

    let capacity = inner.height as usize;
    let (start, end) = visible_window(popup.filtered.len(), capacity, popup.cursor);

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(end - start);
    for (visible_idx, idx) in popup.filtered[start..end].iter().enumerate() {
        let absolute = start + visible_idx;
        let (name_span, desc_span) = match &popup.kind {
            CompletionKind::Skills(items) => {
                let Some(s) = items.get(*idx) else { continue };
                (
                    Span::styled(
                        format!("${}", s.name),
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        s.description.clone(),
                        Style::default().fg(Color::DarkGray),
                    ),
                )
            }
            CompletionKind::Commands(items) => {
                let Some(cmd) = items.get(*idx) else { continue };
                (
                    Span::styled(
                        cmd.name.to_string(),
                        Style::default()
                            .fg(Color::LightCyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        cmd.description.to_string(),
                        Style::default().fg(Color::DarkGray),
                    ),
                )
            }
        };

        let row_style = if absolute == popup.cursor {
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let marker = Span::styled(
            if absolute == popup.cursor { "› " } else { "  " }.to_string(),
            row_style,
        );
        let name_span = Span::styled(name_span.content, name_span.style.patch(row_style));
        let desc_span = Span::styled(desc_span.content, desc_span.style.patch(row_style));
        lines.push(Line::from(vec![
            marker,
            name_span,
            Span::styled("  ".to_string(), row_style),
            desc_span,
        ]));
    }
    Paragraph::new(lines).render(inner, buf);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn skill(name: &str, description: &str) -> SkillDescriptor {
        SkillDescriptor {
            name: name.to_string(),
            description: description.to_string(),
            short_description: None,
            path: PathBuf::from("/tmp/test"),
            scope: mli_types::SkillScope::Repo,
            enabled: true,
        }
    }

    #[test]
    fn command_popup_opens_on_slash() {
        let popup = evaluate("/", 1, None, None).expect("popup");
        assert!(matches!(popup.kind, CompletionKind::Commands(_)));
        assert_eq!(popup.query, "");
        assert_eq!(popup.filtered.len(), SLASH_COMMANDS.len());
    }

    #[test]
    fn command_popup_filters_by_prefix() {
        let popup = evaluate("/sk", 3, None, None).expect("popup");
        let names: Vec<&str> = popup
            .filtered
            .iter()
            .map(|i| SLASH_COMMANDS[*i].name)
            .collect();
        assert_eq!(names, vec!["/skills"]);
    }

    #[test]
    fn command_popup_closes_after_space() {
        let popup = evaluate("/skills ", 8, None, None);
        assert!(popup.is_none());
    }

    #[test]
    fn skill_popup_opens_on_dollar_when_cache_present() {
        let skills = vec![skill("rewrite", "refactor code"), skill("test", "run tests")];
        let popup = evaluate("$", 1, Some(&skills), None).expect("popup");
        assert!(matches!(popup.kind, CompletionKind::Skills(_)));
        assert_eq!(popup.filtered.len(), 2);
    }

    #[test]
    fn skill_popup_prefix_wins_over_substring() {
        let skills = vec![
            skill("archive", "something with t in desc"),
            skill("test", "run tests"),
        ];
        let popup = evaluate("$te", 3, Some(&skills), None).expect("popup");
        // Expect "test" (prefix) to come before "archive" (substring).
        assert_eq!(popup.filtered[0], 1);
    }

    #[test]
    fn completion_closes_outside_first_token() {
        assert!(evaluate("hello /help", 11, None, None).is_none());
    }

    #[test]
    fn cursor_before_trigger_is_noop() {
        // Cursor at byte 0 should not open the popup.
        assert!(evaluate("/help", 0, None, None).is_none());
    }

    #[test]
    fn skill_accept_replaces_token() {
        let skills = vec![skill("rewrite", "refactor code")];
        let popup = evaluate("$rew", 4, Some(&skills), None).expect("popup");
        let outcome = popup.accept().expect("accept");
        match outcome {
            AcceptOutcome::Replace {
                replacement,
                cursor_offset,
            } => {
                assert_eq!(replacement, "$rewrite ");
                assert_eq!(cursor_offset, replacement.len());
            }
            AcceptOutcome::Submit(_) => panic!("skills should not Submit"),
        }
    }

    #[test]
    fn command_accept_submits() {
        let popup = evaluate("/hel", 4, None, None).expect("popup");
        match popup.accept().expect("accept") {
            AcceptOutcome::Submit(cmd) => assert_eq!(cmd, "/help"),
            AcceptOutcome::Replace { .. } => panic!("commands should Submit"),
        }
    }
}
