//! Inline-viewport TUI main loop.
//!
//! Replaces the old full-screen ratatui viewport with CodexPotter-style inline
//! rendering: history cells scroll into the terminal scrollback via
//! [`insert_history_lines`](crate::insert_history::insert_history_lines) while
//! a compact bottom pane (composer + status + hints) lives in a small
//! reserved viewport pinned at the bottom of the screen.

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{
    self, Event as CEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use mli_protocol::ServerNotification;
use mli_types::{ConnectionState, HistoryCellModel};
use ratatui::backend::Backend;
use ratatui::layout::Rect;

use crate::app::{AppClient, TranscriptApp, selected_skill_label};
use crate::bottom_pane::{self, BottomPaneProps};
use crate::history_cell::render_cell;
use crate::insert_history::insert_history_lines;
use crate::markdown_stream::MarkdownStreamCollector;
use crate::overlay::{self, Overlay, OverlayOutcome};
use crate::render::line_utils::prefix_lines;
use crate::terminal_cleanup::clear_inline_viewport_for_exit;
use crate::tui_session::{self, Terminal};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

const TICK: Duration = Duration::from_millis(100);

pub fn run_inline_tui(app_server_bin: Option<PathBuf>) -> Result<()> {
    let client = AppClient::spawn(app_server_bin)?;
    let mut core = TranscriptApp::new(client);
    core.initialize_session()?;

    let mut terminal = tui_session::enter().context("failed to enter terminal")?;
    let mut app = InlineApp::new(&mut terminal, core)?;
    let result = app.run(&mut terminal);

    let _ = clear_inline_viewport_for_exit(&mut terminal);
    let _ = tui_session::restore();
    result
}

struct InlineApp {
    core: TranscriptApp,
    rendered_cells: usize,
    task_started_at: Option<Instant>,
    last_connection: ConnectionState,
    toast: Option<String>,
    should_quit: bool,
    selected_skill_label: Option<String>,
    overlay: Option<Overlay>,
    last_approval_id: Option<String>,
    streaming: Option<StreamingCtx>,
    prompt_history: Vec<String>,
    history_cursor: Option<usize>,
    history_draft: Option<String>,
}

struct StreamingCtx {
    collector: MarkdownStreamCollector,
    cell_index: usize,
    text_len_seen: usize,
    first_line_emitted: bool,
}

impl InlineApp {
    fn new(terminal: &mut Terminal, core: TranscriptApp) -> Result<Self> {
        let last_connection = core.state().connection;
        let mut me = Self {
            core,
            rendered_cells: 0,
            task_started_at: None,
            last_connection,
            toast: None,
            should_quit: false,
            selected_skill_label: None,
            overlay: None,
            last_approval_id: None,
            streaming: None,
            prompt_history: Vec::new(),
            history_cursor: None,
            history_draft: None,
        };
        me.refresh_selected_skill_label();
        // Must size the viewport before the first history flush so wrap_width is correct.
        me.resize_viewport(terminal)?;
        me.flush_new_history(terminal)?;
        Ok(me)
    }

    fn refresh_selected_skill_label(&mut self) {
        self.selected_skill_label = self.core.selected_skill().map(selected_skill_label);
    }

    fn run(&mut self, terminal: &mut Terminal) -> Result<()> {
        self.resize_viewport(terminal)?;
        let mut last_tick = Instant::now();

        while !self.should_quit {
            while let Some(notification) = self.core.poll_notification()? {
                self.apply_notification(notification)?;
            }
            self.flush_new_history(terminal)?;
            self.update_task_clock();
            self.auto_open_approval_overlay();

            self.resize_viewport(terminal)?;

            terminal.draw(|frame| {
                let area = frame.area();
                if let Some(overlay) = self.overlay.as_ref() {
                    overlay::render(area, frame.buffer_mut(), overlay);
                } else {
                    let props = self.bottom_pane_props();
                    let layout = bottom_pane::render(area, frame.buffer_mut(), &props);
                    if let Some(pos) = layout.cursor {
                        frame.set_cursor_position(pos);
                    }
                }
            })?;

            if event::poll(TICK)? {
                match event::read()? {
                    CEvent::Key(key) if key.kind != KeyEventKind::Release => {
                        self.handle_key(key, terminal)?;
                    }
                    CEvent::Paste(data) => self.handle_paste(&data),
                    CEvent::Resize(_, _) => {}
                    _ => {}
                }
            }

            if last_tick.elapsed() >= TICK {
                last_tick = Instant::now();
            }
        }
        Ok(())
    }

    /// Automatically open an approval overlay when a pending approval arrives
    /// (once per approval id). Closing a previously seen approval won't re-open
    /// the overlay until a new one comes in.
    fn auto_open_approval_overlay(&mut self) {
        let pending = self.core.state().approvals.pending.clone();
        match (pending, self.overlay.as_ref()) {
            (Some(approval), None) => {
                if self.last_approval_id.as_deref() != Some(approval.id.as_str()) {
                    self.last_approval_id = Some(approval.id.clone());
                    self.overlay = Some(Overlay::for_pending_approval(approval));
                }
            }
            (None, _) => {
                self.last_approval_id = None;
            }
            _ => {}
        }
    }

    fn bottom_pane_props(&self) -> BottomPaneProps<'_> {
        let state = self.core.state();
        BottomPaneProps {
            connection: state.connection,
            approval_policy: state.runtime.approval_policy,
            sandbox_mode: state.runtime.sandbox_mode,
            selected_skill: self.selected_skill_label.as_deref(),
            composer_buffer: &state.composer.buffer,
            composer_cursor: state.composer.cursor,
            pending_approval: state.approvals.pending.as_ref(),
            task_started_at: self.task_started_at,
            queued_prompts: 0,
            toast: self.toast.as_deref(),
            hint: None,
        }
    }

    fn update_task_clock(&mut self) {
        let state_conn = self.core.state().connection;
        if state_conn != self.last_connection {
            match state_conn {
                ConnectionState::Streaming | ConnectionState::WaitingApproval => {
                    if self.task_started_at.is_none() {
                        self.task_started_at = Some(Instant::now());
                    }
                }
                _ => {
                    self.task_started_at = None;
                }
            }
            self.last_connection = state_conn;
        }
    }

    fn apply_notification(&mut self, notification: ServerNotification) -> Result<()> {
        self.core.apply_notification(notification)
    }

    /// Look for new history cells appended to state.transcript.history since the
    /// last insertion and push them above the viewport. The last cell gets
    /// special handling if it is a still-streaming `AssistantMessage`: incoming
    /// deltas are buffered through a [`MarkdownStreamCollector`] and committed
    /// one logical line at a time so the transcript reflects the response as it
    /// arrives, rather than waiting for turn completion.
    fn flush_new_history(&mut self, terminal: &mut Terminal) -> Result<()> {
        let screen = terminal.backend().size()?;
        let width = terminal.viewport_area.width.max(screen.width);
        if width == 0 {
            return Ok(());
        }

        // Snapshot so we can drop the borrow before invoking `insert_history_lines`.
        let (total, snapshot_tail) = {
            let history = &self.core.state().transcript.history;
            let total = history.len();
            let tail = history.get(total.saturating_sub(1)).cloned();
            (total, tail)
        };

        // 1. Commit any cells STRICTLY before the last one (they are now final and
        //    won't change). Use a plain render.
        while self.rendered_cells + 1 < total {
            let cell = self
                .core
                .state()
                .transcript
                .history
                .get(self.rendered_cells)
                .cloned();
            if let Some(cell) = cell {
                // If we were streaming and a later cell arrived, flush the streaming
                // buffer and advance the counter past the streaming cell first.
                if let Some(mut ctx) = self.streaming.take() {
                    if ctx.cell_index == self.rendered_cells {
                        self.drain_streaming_ctx(&mut ctx, terminal, true)?;
                        self.rendered_cells += 1;
                        continue;
                    }
                }
                let lines = render_cell(&cell, Some(width));
                if !lines.is_empty() {
                    insert_history_lines(terminal, lines)?;
                }
                self.rendered_cells += 1;
            } else {
                break;
            }
        }

        // 2. Handle the tail cell.
        if self.rendered_cells >= total {
            return Ok(());
        }
        let Some(tail) = snapshot_tail else {
            return Ok(());
        };
        match tail {
            HistoryCellModel::AssistantMessage(ref cell) => {
                self.handle_streaming_tail(cell, terminal, width)?;
            }
            _ => {
                let lines = render_cell(&tail, Some(width));
                if !lines.is_empty() {
                    insert_history_lines(terminal, lines)?;
                }
                self.rendered_cells = total;
            }
        }
        Ok(())
    }

    fn handle_streaming_tail(
        &mut self,
        cell: &mli_types::AssistantMessageCell,
        terminal: &mut Terminal,
        width: u16,
    ) -> Result<()> {
        let tail_index = self.core.state().transcript.history.len() - 1;
        // Ensure we have a streaming ctx anchored to this cell.
        let needs_new_ctx = match &self.streaming {
            Some(ctx) => ctx.cell_index != tail_index,
            None => true,
        };
        if needs_new_ctx {
            if let Some(mut ctx) = self.streaming.take() {
                self.drain_streaming_ctx(&mut ctx, terminal, true)?;
            }
            let cwd = self
                .core
                .state()
                .runtime
                .cwd
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let collector_width = (width as usize).saturating_sub(2).max(8);
            self.streaming = Some(StreamingCtx {
                collector: MarkdownStreamCollector::new(Some(collector_width), &cwd),
                cell_index: tail_index,
                text_len_seen: 0,
                first_line_emitted: false,
            });
        }
        let ctx = self
            .streaming
            .as_mut()
            .expect("streaming ctx just initialized");
        // Feed any new text into the collector.
        if cell.text.len() > ctx.text_len_seen {
            let delta = &cell.text[ctx.text_len_seen..];
            ctx.collector.push_delta(delta);
            ctx.text_len_seen = cell.text.len();
        }
        let finalize = !cell.streaming;
        // Take ctx out momentarily so drain_streaming_ctx can borrow self mutably.
        let mut ctx = self.streaming.take().expect("ctx still some");
        self.drain_streaming_ctx(&mut ctx, terminal, finalize)?;
        if finalize {
            self.rendered_cells = tail_index + 1;
            self.streaming = None;
        } else {
            self.streaming = Some(ctx);
        }
        Ok(())
    }

    fn drain_streaming_ctx(
        &mut self,
        ctx: &mut StreamingCtx,
        terminal: &mut Terminal,
        finalize: bool,
    ) -> Result<()> {
        let mut lines = if finalize {
            ctx.collector.finalize_and_drain()
        } else {
            ctx.collector.commit_complete_lines()
        };
        if lines.is_empty() {
            return Ok(());
        }
        // Apply the assistant `• ` prefix to the first line of the very first
        // commit for this cell, and an indent to every following line.
        let initial_prefix = if !ctx.first_line_emitted {
            Span::styled(
                "• ",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("  ")
        };
        let subsequent_prefix = Span::raw("  ");
        lines = prefix_lines(lines, initial_prefix, subsequent_prefix);
        ctx.first_line_emitted = true;
        insert_history_lines(terminal, lines)?;
        Ok(())
    }

    fn resize_viewport(&mut self, terminal: &mut Terminal) -> Result<()> {
        let screen = terminal.backend().size()?;
        if screen.width == 0 || screen.height == 0 {
            return Ok(());
        }
        let height = if self.overlay.is_some() {
            // Full-height overlay: take as much vertical space as the terminal allows.
            screen.height.min(24).max(10)
        } else {
            let props = self.bottom_pane_props();
            let needed = bottom_pane::desired_height(&props, screen.width);
            needed.min(screen.height.saturating_sub(1).max(1))
        };
        let top = screen.height.saturating_sub(height);
        let new_area = Rect::new(0, top, screen.width, height);
        if terminal.viewport_area != new_area {
            terminal.set_viewport_area(new_area);
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent, terminal: &mut Terminal) -> Result<()> {
        self.toast = None;
        // Global quit / interrupt.
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            if matches!(self.core.state().connection, ConnectionState::Streaming) {
                let _ = self.core.request_interrupt();
                self.toast = Some("interrupt requested".into());
                return Ok(());
            }
            self.should_quit = true;
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('d'))
            && self.core.state().composer.buffer.is_empty()
            && self.overlay.is_none()
        {
            self.should_quit = true;
            return Ok(());
        }

        if self.overlay.is_some() {
            return self.handle_overlay_key(key, terminal);
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        match (key.code, ctrl, alt, shift) {
            (KeyCode::Enter, false, false, true) => self.insert_str("\n"),
            (KeyCode::Enter, false, false, false) => self.submit_composer(terminal)?,

            (KeyCode::Backspace, false, false, _) => {
                self.reset_history_nav();
                self.backspace();
            }
            (KeyCode::Backspace, true, _, _) | (KeyCode::Backspace, _, true, _) => {
                self.reset_history_nav();
                self.delete_word_backward();
            }
            (KeyCode::Delete, false, false, _) => {
                self.reset_history_nav();
                self.delete_forward();
            }
            (KeyCode::Delete, _, true, _) => {
                self.reset_history_nav();
                self.delete_word_forward();
            }

            (KeyCode::Left, false, false, _) => {
                self.reset_history_nav();
                self.move_left();
            }
            (KeyCode::Left, _, true, _) | (KeyCode::Left, true, _, _) => {
                self.reset_history_nav();
                self.move_word_left();
            }
            (KeyCode::Right, false, false, _) => {
                self.reset_history_nav();
                self.move_right();
            }
            (KeyCode::Right, _, true, _) | (KeyCode::Right, true, _, _) => {
                self.reset_history_nav();
                self.move_word_right();
            }

            (KeyCode::Up, false, false, _) => self.handle_up_arrow(),
            (KeyCode::Down, false, false, _) => self.handle_down_arrow(),

            (KeyCode::Home, false, false, _) | (KeyCode::Char('a'), true, _, _) => {
                self.reset_history_nav();
                self.move_line_home();
            }
            (KeyCode::End, false, false, _) | (KeyCode::Char('e'), true, _, _) => {
                self.reset_history_nav();
                self.move_line_end();
            }
            (KeyCode::Char('u'), true, _, _) => {
                self.reset_history_nav();
                self.clear_to_line_start();
            }
            (KeyCode::Char('k'), true, _, _) => {
                self.reset_history_nav();
                self.clear_to_line_end();
            }
            (KeyCode::Char('w'), true, _, _) => {
                self.reset_history_nav();
                self.delete_word_backward();
            }

            (KeyCode::Char(ch), false, false, _) => {
                self.reset_history_nav();
                self.insert_char(ch);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_paste(&mut self, data: &str) {
        if self.overlay.is_some() {
            // Overlays don't want multi-line paste; route each char.
            for ch in data.chars() {
                let synthetic = KeyEvent::new(KeyCode::Char(ch), KeyModifiers::empty());
                // Fire and forget; ignore the Result.
                let mut overlay = match self.overlay.as_mut() {
                    Some(o) => o,
                    None => return,
                };
                let _ = overlay::handle_key(&mut overlay, synthetic);
            }
            return;
        }
        self.reset_history_nav();
        self.insert_str(data);
    }

    fn insert_char(&mut self, ch: char) {
        let state = self.core.state_mut();
        let idx = state.composer.cursor.min(state.composer.buffer.len());
        state.composer.buffer.insert(idx, ch);
        state.composer.cursor = idx + ch.len_utf8();
    }

    fn insert_str(&mut self, s: &str) {
        let state = self.core.state_mut();
        let idx = state.composer.cursor.min(state.composer.buffer.len());
        state.composer.buffer.insert_str(idx, s);
        state.composer.cursor = idx + s.len();
    }

    fn backspace(&mut self) {
        let state = self.core.state_mut();
        if state.composer.cursor == 0 {
            return;
        }
        let prev_boundary = prev_char_boundary(&state.composer.buffer, state.composer.cursor);
        state.composer.buffer.drain(prev_boundary..state.composer.cursor);
        state.composer.cursor = prev_boundary;
    }

    fn delete_forward(&mut self) {
        let state = self.core.state_mut();
        if state.composer.cursor >= state.composer.buffer.len() {
            return;
        }
        let next_boundary = next_char_boundary(&state.composer.buffer, state.composer.cursor);
        state.composer.buffer.drain(state.composer.cursor..next_boundary);
    }

    fn delete_word_backward(&mut self) {
        let state = self.core.state_mut();
        let start = prev_word_boundary(&state.composer.buffer, state.composer.cursor);
        if start < state.composer.cursor {
            state.composer.buffer.drain(start..state.composer.cursor);
            state.composer.cursor = start;
        }
    }

    fn delete_word_forward(&mut self) {
        let state = self.core.state_mut();
        let end = next_word_boundary(&state.composer.buffer, state.composer.cursor);
        if end > state.composer.cursor {
            state.composer.buffer.drain(state.composer.cursor..end);
        }
    }

    fn move_left(&mut self) {
        let state = self.core.state_mut();
        state.composer.cursor = prev_char_boundary(&state.composer.buffer, state.composer.cursor);
    }

    fn move_right(&mut self) {
        let state = self.core.state_mut();
        state.composer.cursor = next_char_boundary(&state.composer.buffer, state.composer.cursor);
    }

    fn move_word_left(&mut self) {
        let state = self.core.state_mut();
        state.composer.cursor = prev_word_boundary(&state.composer.buffer, state.composer.cursor);
    }

    fn move_word_right(&mut self) {
        let state = self.core.state_mut();
        state.composer.cursor = next_word_boundary(&state.composer.buffer, state.composer.cursor);
    }

    fn move_line_home(&mut self) {
        let state = self.core.state_mut();
        let buf = &state.composer.buffer;
        let cursor = state.composer.cursor.min(buf.len());
        let line_start = buf[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
        state.composer.cursor = line_start;
    }

    fn move_line_end(&mut self) {
        let state = self.core.state_mut();
        let buf = &state.composer.buffer;
        let cursor = state.composer.cursor.min(buf.len());
        let line_end = buf[cursor..]
            .find('\n')
            .map(|i| cursor + i)
            .unwrap_or(buf.len());
        state.composer.cursor = line_end;
    }

    fn clear_to_line_start(&mut self) {
        let state = self.core.state_mut();
        let buf = &state.composer.buffer;
        let cursor = state.composer.cursor.min(buf.len());
        let line_start = buf[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
        if line_start < cursor {
            state.composer.buffer.drain(line_start..cursor);
            state.composer.cursor = line_start;
        }
    }

    fn clear_to_line_end(&mut self) {
        let state = self.core.state_mut();
        let buf = &state.composer.buffer;
        let cursor = state.composer.cursor.min(buf.len());
        let line_end = buf[cursor..]
            .find('\n')
            .map(|i| cursor + i)
            .unwrap_or(buf.len());
        if line_end > cursor {
            state.composer.buffer.drain(cursor..line_end);
        }
    }

    /// Up arrow: if cursor is on the first line of the buffer, move through prompt
    /// history; otherwise move the caret to the equivalent column on the prior line.
    fn handle_up_arrow(&mut self) {
        let (buffer, cursor) = {
            let state = self.core.state();
            (state.composer.buffer.clone(), state.composer.cursor)
        };
        if let Some(new_cursor) = prev_line_cursor(&buffer, cursor) {
            self.reset_history_nav();
            self.core.state_mut().composer.cursor = new_cursor;
            return;
        }
        self.history_prev();
    }

    fn handle_down_arrow(&mut self) {
        let (buffer, cursor) = {
            let state = self.core.state();
            (state.composer.buffer.clone(), state.composer.cursor)
        };
        if let Some(new_cursor) = next_line_cursor(&buffer, cursor) {
            self.reset_history_nav();
            self.core.state_mut().composer.cursor = new_cursor;
            return;
        }
        self.history_next();
    }

    fn history_prev(&mut self) {
        if self.prompt_history.is_empty() {
            return;
        }
        let idx = match self.history_cursor {
            Some(i) if i == 0 => 0,
            Some(i) => i - 1,
            None => {
                // Snapshot the current draft so Down-arrow can restore it.
                self.history_draft = Some(self.core.state().composer.buffer.clone());
                self.prompt_history.len() - 1
            }
        };
        self.set_composer_from_history(idx);
    }

    fn history_next(&mut self) {
        let Some(idx) = self.history_cursor else { return };
        if idx + 1 >= self.prompt_history.len() {
            // Past the newest entry: restore draft and exit history mode.
            let draft = self.history_draft.take().unwrap_or_default();
            let state = self.core.state_mut();
            state.composer.cursor = draft.len();
            state.composer.buffer = draft;
            self.history_cursor = None;
        } else {
            self.set_composer_from_history(idx + 1);
        }
    }

    fn set_composer_from_history(&mut self, idx: usize) {
        if let Some(entry) = self.prompt_history.get(idx).cloned() {
            self.history_cursor = Some(idx);
            let state = self.core.state_mut();
            state.composer.cursor = entry.len();
            state.composer.buffer = entry;
        }
    }

    fn reset_history_nav(&mut self) {
        self.history_cursor = None;
        self.history_draft = None;
    }

    fn push_history_entry(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        if self
            .prompt_history
            .last()
            .map(|last| last == trimmed)
            .unwrap_or(false)
        {
            return;
        }
        self.prompt_history.push(trimmed.to_string());
        const MAX: usize = 200;
        if self.prompt_history.len() > MAX {
            let drop = self.prompt_history.len() - MAX;
            self.prompt_history.drain(..drop);
        }
    }

    fn submit_composer(&mut self, terminal: &mut Terminal) -> Result<()> {
        self.reset_history_nav();
        let input = {
            let state = self.core.state_mut();
            let input = std::mem::take(&mut state.composer.buffer);
            state.composer.cursor = 0;
            input
        };
        let input = input.trim().to_owned();
        if input.is_empty() {
            return Ok(());
        }
        self.push_history_entry(&input);
        if input == "/quit" || input == "/exit" {
            self.should_quit = true;
            return Ok(());
        }
        if input.starts_with('/') {
            self.handle_slash(&input)?;
        } else {
            // Use `start_prompt` (non-blocking) rather than `send_prompt` (which drains
            // notifications synchronously). The main loop's tick-based polling then
            // feeds deltas into the streaming collector as they arrive.
            self.core.start_prompt(input)?;
            self.flush_new_history(terminal)?;
        }
        self.refresh_selected_skill_label();
        Ok(())
    }

    fn handle_slash(&mut self, command: &str) -> Result<()> {
        match command {
            "/help" => self.overlay = Some(Overlay::Help),
            "/clear" => {
                self.core.clear_transcript();
                self.rendered_cells = 0;
            }
            "/threads" => self.open_thread_picker()?,
            "/skills" => self.open_skill_picker()?,
            "/artifacts" => self.open_artifact_picker()?,
            "/approval" => {
                if let Some(approval) = self.core.state().approvals.pending.clone() {
                    self.overlay = Some(Overlay::for_pending_approval(approval));
                } else {
                    self.toast = Some("no pending approval".into());
                }
            }
            other => self
                .core
                .push_warning(&format!("Unknown command: {other}")),
        }
        Ok(())
    }

    fn open_skill_picker(&mut self) -> Result<()> {
        let skills = self.core.request_skills()?;
        if skills.is_empty() {
            self.toast = Some("no skills available".into());
            return Ok(());
        }
        self.overlay = Some(Overlay::SkillPicker {
            skills,
            query: String::new(),
            cursor: 0,
        });
        Ok(())
    }

    fn open_thread_picker(&mut self) -> Result<()> {
        self.core.refresh_threads()?;
        let threads = self.core.state().threads.clone();
        if threads.is_empty() {
            self.toast = Some("no threads yet".into());
            return Ok(());
        }
        self.overlay = Some(Overlay::ThreadPicker {
            threads,
            query: String::new(),
            cursor: 0,
        });
        Ok(())
    }

    fn open_artifact_picker(&mut self) -> Result<()> {
        let artifacts = self.core.request_artifacts()?;
        if artifacts.is_empty() {
            self.toast = Some("no artifacts yet".into());
            return Ok(());
        }
        self.overlay = Some(Overlay::ArtifactPicker {
            artifacts,
            query: String::new(),
            cursor: 0,
        });
        Ok(())
    }

    fn handle_overlay_key(&mut self, key: KeyEvent, terminal: &mut Terminal) -> Result<()> {
        let Some(overlay) = self.overlay.as_mut() else {
            return Ok(());
        };
        let outcome = overlay::handle_key(overlay, key);
        match outcome {
            OverlayOutcome::Keep => {}
            OverlayOutcome::Dismiss => {
                self.overlay = None;
            }
            OverlayOutcome::CancelApproval => {
                self.overlay = None;
                self.toast = Some("approval overlay dismissed (use /approval to reopen)".into());
            }
            OverlayOutcome::SubmitApproval(request) => {
                if let Some(approval) = self.core.state().approvals.pending.clone() {
                    let _ = self.core.submit_approval_response(&approval, request);
                }
                self.overlay = None;
                self.flush_new_history(terminal)?;
            }
            OverlayOutcome::SelectSkill(skill) => {
                self.core.set_selected_skill(Some(skill));
                self.overlay = None;
                self.refresh_selected_skill_label();
                self.toast = Some(
                    self.selected_skill_label
                        .clone()
                        .map(|label| format!("selected skill: {label}"))
                        .unwrap_or_else(|| "skill cleared".into()),
                );
            }
            OverlayOutcome::ResumeThread(thread_id) => {
                self.overlay = None;
                match self
                    .core
                    .resume_thread_into_view_no_follow(thread_id)
                {
                    Ok(_) => {
                        // Transcript was replaced; reset rendered counter and let
                        // flush push the full transcript into scrollback.
                        self.rendered_cells = 0;
                        self.flush_new_history(terminal)?;
                    }
                    Err(err) => {
                        self.toast = Some(format!("resume failed: {err}"));
                    }
                }
            }
            OverlayOutcome::OpenArtifact(id) => {
                self.overlay = None;
                self.toast = Some(format!("artifact viewer not yet implemented (id: {id:?})"));
            }
        }
        Ok(())
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

fn next_char_boundary(buffer: &str, cursor: usize) -> usize {
    let mut idx = cursor.min(buffer.len());
    if idx == buffer.len() {
        return buffer.len();
    }
    idx += 1;
    while idx < buffer.len() && !buffer.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

/// Return the byte index one "word" to the left of `cursor`, where a word is
/// a contiguous alphanumeric run. Leading whitespace/punctuation is skipped
/// before the word itself, matching readline's `backward-word` behavior.
fn prev_word_boundary(buffer: &str, cursor: usize) -> usize {
    let cursor = cursor.min(buffer.len());
    let bytes = buffer.as_bytes();
    let mut idx = cursor;
    // Skip trailing non-alphanumeric
    while idx > 0 {
        let prev = prev_char_boundary(buffer, idx);
        let ch = bytes[prev] as char;
        if is_word_char(ch) {
            break;
        }
        idx = prev;
    }
    // Walk back through the word
    while idx > 0 {
        let prev = prev_char_boundary(buffer, idx);
        let ch = bytes[prev] as char;
        if !is_word_char(ch) {
            break;
        }
        idx = prev;
    }
    idx
}

fn next_word_boundary(buffer: &str, cursor: usize) -> usize {
    let len = buffer.len();
    let bytes = buffer.as_bytes();
    let mut idx = cursor.min(len);
    while idx < len && !is_word_char(bytes[idx] as char) {
        idx = next_char_boundary(buffer, idx);
    }
    while idx < len && is_word_char(bytes[idx] as char) {
        idx = next_char_boundary(buffer, idx);
    }
    idx
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Move the cursor to the equivalent byte-column on the previous logical line.
/// Returns `None` if the cursor is already on the first line.
fn prev_line_cursor(buffer: &str, cursor: usize) -> Option<usize> {
    let cursor = cursor.min(buffer.len());
    let line_start = buffer[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
    if line_start == 0 {
        return None;
    }
    let col = cursor - line_start;
    let prev_line_end = line_start - 1; // position of the '\n'
    let prev_line_start = buffer[..prev_line_end].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let prev_line_len = prev_line_end - prev_line_start;
    let target = prev_line_start + col.min(prev_line_len);
    Some(snap_to_boundary(buffer, target))
}

fn next_line_cursor(buffer: &str, cursor: usize) -> Option<usize> {
    let cursor = cursor.min(buffer.len());
    let line_start = buffer[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = cursor - line_start;
    let Some(line_end_offset) = buffer[cursor..].find('\n') else {
        return None;
    };
    let next_line_start = cursor + line_end_offset + 1;
    let next_line_end = buffer[next_line_start..]
        .find('\n')
        .map(|i| next_line_start + i)
        .unwrap_or(buffer.len());
    let next_line_len = next_line_end - next_line_start;
    let target = next_line_start + col.min(next_line_len);
    Some(snap_to_boundary(buffer, target))
}

fn snap_to_boundary(buffer: &str, mut idx: usize) -> usize {
    idx = idx.min(buffer.len());
    while idx < buffer.len() && !buffer.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

// Ensure io is used (some release builds dead-code it via guards above).
#[allow(dead_code)]
fn _io_guard() -> io::Result<()> {
    Ok(())
}
