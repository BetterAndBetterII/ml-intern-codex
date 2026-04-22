use std::collections::BTreeMap;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEvent as CrosstermKeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use mli_protocol::{ApprovalAnswer, ApprovalDecision, ApprovalRespondParams, ServerNotification};
use mli_types::{
    AppState, ApprovalKind, ArtifactFilePayload, ArtifactManifest, ConnectionState,
    HistoryCellModel, PendingApproval, SkillDescriptor, ThreadListItem,
};
use ratatui::{
    Frame, Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::app::{
    AppClient, ApprovalQuestion, ApprovalQuestionPayload, ApprovalResolutionOutcome, TranscriptApp,
    describe_skill, filter_artifacts, filter_skills, filter_threads, preferred_artifact_file_index,
};
use crate::renderer::render_history_cell;

const APP_TITLE: &str = "ml-intern-codex";
const TICK_INTERVAL: Duration = Duration::from_millis(250);
const IDLE_SLEEP: Duration = Duration::from_millis(16);
const FALLBACK_TERMINAL_WIDTH: u16 = 80;
const FALLBACK_TERMINAL_HEIGHT: u16 = 24;

pub fn run_fullscreen_tui(app_server_bin: Option<PathBuf>) -> Result<()> {
    let client = AppClient::spawn(app_server_bin)?;
    let mut core = TranscriptApp::new(client);
    core.initialize_session()?;
    let mut app = FullscreenApp::new(core)?;
    app.run()
}

struct FullscreenApp {
    core: TranscriptApp,
    terminal: TerminalSession,
    ui: FullscreenUiState,
    should_quit: bool,
    last_tick: Instant,
    pending_redraw: Option<RedrawRequest>,
}

impl FullscreenApp {
    fn new(core: TranscriptApp) -> Result<Self> {
        let terminal = TerminalSession::enter()?;
        let mut app = Self {
            core,
            terminal,
            ui: FullscreenUiState::default(),
            should_quit: false,
            last_tick: Instant::now(),
            pending_redraw: None,
        };
        app.ui.size = app.terminal.size().unwrap_or_default();
        app.sync_overlays_with_state()?;
        app.refresh_status_toast();
        app.request_redraw(RedrawRequest::Initial);
        Ok(app)
    }

    fn run(&mut self) -> Result<()> {
        while !self.should_quit {
            if let Some(notification) = self.core.poll_notification()? {
                self.handle_server_notification(notification)?;
                continue;
            }
            if let Some(reason) = self.pending_redraw.take() {
                self.render(reason)?;
                continue;
            }
            if self.last_tick.elapsed() >= TICK_INTERVAL {
                self.handle_tick();
                continue;
            }
            if let Some(event) = self.poll_terminal_event()? {
                self.handle_terminal_event(event)?;
                continue;
            }
        }
        Ok(())
    }

    fn handle_server_notification(&mut self, notification: ServerNotification) -> Result<()> {
        let follow_tail = self.ui.transcript_scroll == 0;
        self.core.apply_notification(notification)?;
        if follow_tail {
            self.ui.transcript_scroll = 0;
        }
        self.sync_overlays_with_state()?;
        self.refresh_status_toast();
        self.request_redraw(RedrawRequest::DataChanged);
        Ok(())
    }

    fn handle_tick(&mut self) {
        self.last_tick = Instant::now();
        if matches!(self.core.state().connection, ConnectionState::Streaming) {
            self.request_redraw(RedrawRequest::Tick);
        }
    }

    fn poll_terminal_event(&mut self) -> Result<Option<Event>> {
        if event::poll(IDLE_SLEEP).context("failed to poll terminal events")? {
            return event::read()
                .map(Some)
                .context("failed to read terminal event");
        }
        Ok(None)
    }

    fn handle_terminal_event(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Key(key) => {
                if let Some(key) = map_key_event(key) {
                    self.handle_key_event(key)?;
                }
            }
            Event::Resize(width, height) => {
                self.ui.size = TerminalSize::new(width, height);
                self.request_redraw(RedrawRequest::Resized);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> Result<()> {
        if !self.ui.overlay_stack.is_empty()
            && matches!(self.core.state().connection, ConnectionState::Streaming)
            && matches!(key, KeyEvent::CtrlC | KeyEvent::Esc)
        {
            self.ui.overlay_stack.pop();
            self.core.request_interrupt()?;
            self.sync_overlays_with_state()?;
            self.refresh_status_toast();
            self.request_redraw(RedrawRequest::KeyPress);
            return Ok(());
        }

        if let Some(overlay) = self.ui.overlay_stack.pop() {
            if let Some(overlay) = self.handle_overlay_key(overlay, key)? {
                self.ui.overlay_stack.push(overlay);
            }
            self.sync_overlays_with_state()?;
            self.refresh_status_toast();
            self.request_redraw(RedrawRequest::KeyPress);
            return Ok(());
        }

        match key {
            KeyEvent::CtrlC | KeyEvent::Esc
                if matches!(self.core.state().connection, ConnectionState::Streaming) =>
            {
                self.core.request_interrupt()?;
            }
            KeyEvent::CtrlC => {
                self.should_quit = true;
            }
            KeyEvent::CtrlL => {
                self.push_help_overlay();
            }
            KeyEvent::Up => {
                self.ui.transcript_scroll = self.ui.transcript_scroll.saturating_add(1);
            }
            KeyEvent::Down => {
                self.ui.transcript_scroll = self.ui.transcript_scroll.saturating_sub(1);
            }
            KeyEvent::PageUp => {
                self.ui.transcript_scroll = self.ui.transcript_scroll.saturating_add(10);
            }
            KeyEvent::PageDown => {
                self.ui.transcript_scroll = self.ui.transcript_scroll.saturating_sub(10);
            }
            KeyEvent::Home => {
                self.core.state_mut().composer.cursor = 0;
            }
            KeyEvent::End => {
                self.core.state_mut().composer.cursor = self.core.state().composer.buffer.len();
            }
            KeyEvent::Left => {
                move_composer_cursor_left(self.core.state_mut());
            }
            KeyEvent::Right => {
                move_composer_cursor_right(self.core.state_mut());
            }
            KeyEvent::Backspace => {
                delete_composer_backward(self.core.state_mut());
            }
            KeyEvent::Delete => {
                delete_composer_forward(self.core.state_mut());
            }
            KeyEvent::Tab => {
                self.push_help_overlay();
            }
            KeyEvent::Enter => {
                self.submit_composer()?;
            }
            KeyEvent::Char('$')
                if self.core.state().composer.buffer.is_empty()
                    && self.ready_only_action_allowed("opening the skill picker") =>
            {
                self.open_skill_picker()?;
            }
            KeyEvent::Char(ch) if !ch.is_control() => {
                insert_composer_char(self.core.state_mut(), ch);
            }
            _ => {}
        }
        self.sync_overlays_with_state()?;
        self.refresh_status_toast();
        self.request_redraw(RedrawRequest::KeyPress);
        Ok(())
    }

    fn handle_overlay_key(
        &mut self,
        mut overlay: OverlayState,
        key: KeyEvent,
    ) -> Result<Option<OverlayState>> {
        let keep_open = match &mut overlay {
            OverlayState::Help => !matches!(key, KeyEvent::Esc | KeyEvent::Enter | KeyEvent::CtrlC),
            OverlayState::Skills(state) => self.handle_skill_picker_key(state, key)?,
            OverlayState::Threads(state) => self.handle_thread_picker_key(state, key)?,
            OverlayState::Artifacts(state) => self.handle_artifact_picker_key(state, key)?,
            OverlayState::ArtifactViewer(state) => self.handle_artifact_viewer_key(state, key),
            OverlayState::Approval(state) => self.handle_approval_key(state, key)?,
        };
        Ok(keep_open.then_some(overlay))
    }

    fn submit_composer(&mut self) -> Result<()> {
        let input = self.core.state().composer.buffer.trim().to_owned();
        if input.is_empty() {
            return Ok(());
        }
        match input.as_str() {
            "/quit" | "/exit" => {
                self.should_quit = true;
            }
            "/help" => self.push_help_overlay(),
            "/clear" => {
                if self.ready_only_action_allowed("clearing the transcript") {
                    self.core.clear_transcript();
                    self.core.push_status("Transcript cleared.");
                }
            }
            "/threads" => {
                if self.ready_only_action_allowed("running /threads") {
                    self.open_thread_picker()?;
                }
            }
            "/skills" => {
                if self.ready_only_action_allowed("running /skills") {
                    self.open_skill_picker()?;
                }
            }
            "/artifacts" => {
                if self.ready_only_action_allowed("running /artifacts") {
                    self.open_artifact_picker()?;
                }
            }
            "/approval" => self.open_pending_approval_overlay()?,
            command if command.starts_with('/') => {
                self.core
                    .push_warning(&format!("Unknown command: {command}"));
            }
            prompt => {
                if !matches!(self.core.state().connection, ConnectionState::Ready) {
                    self.core
                        .push_warning("Wait for the current turn to finish or interrupt it first.");
                } else {
                    let before_skill = self.core.selected_skill().cloned();
                    let before_turn = self.core.active_turn_id();
                    self.core.start_prompt(prompt.to_owned())?;
                    let started_turn = self.core.active_turn_id() != before_turn
                        && matches!(self.core.state().connection, ConnectionState::Streaming);
                    let skill_changed = self.core.selected_skill() != before_skill.as_ref();
                    if started_turn || skill_changed {
                        self.clear_composer();
                        self.ui.transcript_scroll = 0;
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_skill_picker_key(
        &mut self,
        state: &mut SkillPickerState,
        key: KeyEvent,
    ) -> Result<bool> {
        match key {
            KeyEvent::Esc | KeyEvent::CtrlC => return Ok(false),
            KeyEvent::Up => {
                state.selected = state.selected.saturating_sub(1);
            }
            KeyEvent::Down => {
                let len = filtered_skills(state).len();
                if len > 0 {
                    state.selected = usize::min(state.selected + 1, len - 1);
                }
            }
            KeyEvent::PageUp => {
                state.selected = state.selected.saturating_sub(10);
            }
            KeyEvent::PageDown => {
                let len = filtered_skills(state).len();
                if len > 0 {
                    state.selected = usize::min(state.selected + 10, len - 1);
                }
            }
            KeyEvent::Backspace => {
                state.filter.pop();
                let len = filtered_skills(state).len();
                clamp_picker_selection(state, len);
            }
            KeyEvent::Enter => {
                let filtered = filtered_skills(state);
                if let Some(skill) = filtered.get(state.selected).cloned() {
                    self.core.set_selected_skill(Some(skill.clone()));
                    self.core
                        .push_status(&format!("Selected skill: {}", describe_skill(&skill)));
                    return Ok(false);
                }
            }
            KeyEvent::Char(ch) if !ch.is_control() => {
                state.filter.push(ch);
                let len = filtered_skills(state).len();
                clamp_picker_selection(state, len);
            }
            _ => {}
        }
        Ok(true)
    }

    fn handle_thread_picker_key(
        &mut self,
        state: &mut ThreadPickerState,
        key: KeyEvent,
    ) -> Result<bool> {
        match key {
            KeyEvent::Esc | KeyEvent::CtrlC => return Ok(false),
            KeyEvent::Up => {
                state.selected = state.selected.saturating_sub(1);
            }
            KeyEvent::Down => {
                let len = filtered_threads_for_overlay(state).len();
                if len > 0 {
                    state.selected = usize::min(state.selected + 1, len - 1);
                }
            }
            KeyEvent::PageUp => {
                state.selected = state.selected.saturating_sub(10);
            }
            KeyEvent::PageDown => {
                let len = filtered_threads_for_overlay(state).len();
                if len > 0 {
                    state.selected = usize::min(state.selected + 10, len - 1);
                }
            }
            KeyEvent::Backspace => {
                state.filter.pop();
                let len = filtered_threads_for_overlay(state).len();
                clamp_picker_selection(state, len);
            }
            KeyEvent::Enter => {
                let filtered = filtered_threads_for_overlay(state);
                if let Some(item) = filtered.get(state.selected) {
                    let thread_id = item.thread.id;
                    self.core.resume_thread_into_view_no_follow(thread_id)?;
                    self.ui.transcript_scroll = 0;
                    return Ok(false);
                }
            }
            KeyEvent::Char(ch) if !ch.is_control() => {
                state.filter.push(ch);
                let len = filtered_threads_for_overlay(state).len();
                clamp_picker_selection(state, len);
            }
            _ => {}
        }
        Ok(true)
    }

    fn handle_artifact_picker_key(
        &mut self,
        state: &mut ArtifactPickerState,
        key: KeyEvent,
    ) -> Result<bool> {
        match key {
            KeyEvent::Esc | KeyEvent::CtrlC => return Ok(false),
            KeyEvent::Up => {
                state.selected = state.selected.saturating_sub(1);
            }
            KeyEvent::Down => {
                let len = filtered_artifacts_for_overlay(state).len();
                if len > 0 {
                    state.selected = usize::min(state.selected + 1, len - 1);
                }
            }
            KeyEvent::PageUp => {
                state.selected = state.selected.saturating_sub(10);
            }
            KeyEvent::PageDown => {
                let len = filtered_artifacts_for_overlay(state).len();
                if len > 0 {
                    state.selected = usize::min(state.selected + 10, len - 1);
                }
            }
            KeyEvent::Backspace => {
                state.filter.pop();
                let len = filtered_artifacts_for_overlay(state).len();
                clamp_picker_selection(state, len);
            }
            KeyEvent::Enter => {
                let filtered = filtered_artifacts_for_overlay(state);
                if let Some(artifact) = filtered.get(state.selected) {
                    let payload = self.core.read_artifact(artifact.id)?;
                    if payload.files.is_empty() {
                        self.core
                            .push_warning("Selected artifact has no readable files.");
                        return Ok(true);
                    }
                    self.ui
                        .overlay_stack
                        .push(OverlayState::Artifacts(state.clone()));
                    self.ui.overlay_stack.push(OverlayState::ArtifactViewer(
                        ArtifactViewerState::new(payload),
                    ));
                    return Ok(false);
                }
            }
            KeyEvent::Char(ch) if !ch.is_control() => {
                state.filter.push(ch);
                let len = filtered_artifacts_for_overlay(state).len();
                clamp_picker_selection(state, len);
            }
            _ => {}
        }
        Ok(true)
    }

    fn handle_artifact_viewer_key(
        &mut self,
        state: &mut ArtifactViewerState,
        key: KeyEvent,
    ) -> bool {
        match key {
            KeyEvent::Esc | KeyEvent::CtrlC => return false,
            KeyEvent::Left => {
                state.selected_file = state.selected_file.saturating_sub(1);
                state.scroll = 0;
            }
            KeyEvent::Right if state.selected_file + 1 < state.payload.files.len() => {
                state.selected_file += 1;
                state.scroll = 0;
            }
            KeyEvent::Right => {}
            KeyEvent::Up => {
                state.scroll = state.scroll.saturating_sub(1);
            }
            KeyEvent::Down => {
                state.scroll = state.scroll.saturating_add(1);
            }
            KeyEvent::PageUp => {
                state.scroll = state.scroll.saturating_sub(10);
            }
            KeyEvent::PageDown => {
                state.scroll = state.scroll.saturating_add(10);
            }
            _ => {}
        }
        true
    }

    fn handle_approval_key(
        &mut self,
        state: &mut ApprovalOverlayState,
        key: KeyEvent,
    ) -> Result<bool> {
        match state {
            ApprovalOverlayState::Decision { approval, choice } => match key {
                KeyEvent::Esc | KeyEvent::CtrlC => {
                    self.submit_binary_approval(approval, ApprovalChoice::Reject)?;
                    return Ok(self.core.state().approvals.pending.is_some());
                }
                KeyEvent::Left | KeyEvent::Right | KeyEvent::Tab => {
                    *choice = choice.toggle();
                }
                KeyEvent::Char('a') | KeyEvent::Char('A') => {
                    *choice = ApprovalChoice::Approve;
                }
                KeyEvent::Char('r') | KeyEvent::Char('R') => {
                    *choice = ApprovalChoice::Reject;
                }
                KeyEvent::Enter => {
                    self.submit_binary_approval(approval, *choice)?;
                    return Ok(self.core.state().approvals.pending.is_some());
                }
                _ => {}
            },
            ApprovalOverlayState::Questionnaire {
                approval,
                questions,
                current,
                option_selected,
                focus,
                input,
                answers,
            } => {
                let Some(question) = questions.get(*current) else {
                    self.submit_questionnaire(approval, answers.clone())?;
                    return Ok(self.core.state().approvals.pending.is_some());
                };
                match key {
                    KeyEvent::Esc | KeyEvent::CtrlC => {
                        self.reject_approval(approval)?;
                        return Ok(self.core.state().approvals.pending.is_some());
                    }
                    KeyEvent::Up
                        if question.options.is_some()
                            && matches!(focus, QuestionFocus::Options) =>
                    {
                        *option_selected = option_selected.saturating_sub(1);
                    }
                    KeyEvent::Down
                        if question.options.is_some()
                            && matches!(focus, QuestionFocus::Options) =>
                    {
                        let len = question.options.as_ref().map_or(0, Vec::len);
                        if len > 0 {
                            *option_selected = usize::min(*option_selected + 1, len - 1);
                        }
                    }
                    KeyEvent::PageUp
                        if question.options.is_some()
                            && matches!(focus, QuestionFocus::Options) =>
                    {
                        *option_selected = option_selected.saturating_sub(5);
                    }
                    KeyEvent::PageDown
                        if question.options.is_some()
                            && matches!(focus, QuestionFocus::Options) =>
                    {
                        let len = question.options.as_ref().map_or(0, Vec::len);
                        if len > 0 {
                            *option_selected = usize::min(*option_selected + 5, len - 1);
                        }
                    }
                    KeyEvent::Tab if question.is_other && question.options.is_some() => {
                        *focus = focus.toggle();
                    }
                    KeyEvent::Backspace
                        if matches!(focus, QuestionFocus::TextInput)
                            || question.options.is_none() =>
                    {
                        input.pop();
                    }
                    KeyEvent::Char(ch)
                        if !ch.is_control()
                            && (matches!(focus, QuestionFocus::TextInput)
                                || question.options.is_none()) =>
                    {
                        input.push(ch);
                    }
                    KeyEvent::Char(ch)
                        if question.options.is_some()
                            && matches!(focus, QuestionFocus::Options) =>
                    {
                        if let Some(index) = ch.to_digit(10).and_then(|value| value.checked_sub(1))
                        {
                            let len = question.options.as_ref().map_or(0, Vec::len);
                            if (index as usize) < len {
                                *option_selected = index as usize;
                            }
                        }
                    }
                    KeyEvent::Enter => {
                        let Some(answer) =
                            current_question_answer(question, *focus, *option_selected, input)
                        else {
                            self.core.push_warning(
                                "Provide an answer or press Esc to cancel the request.",
                            );
                            return Ok(true);
                        };
                        answers.insert(question.id.clone(), answer);
                        input.clear();
                        *option_selected = 0;
                        *focus = default_question_focus(question);
                        *current += 1;
                        if *current >= questions.len() {
                            self.submit_questionnaire(approval, answers.clone())?;
                            return Ok(self.core.state().approvals.pending.is_some());
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(true)
    }

    fn open_skill_picker(&mut self) -> Result<()> {
        let skills = self.core.request_skills()?;
        if skills.is_empty() {
            self.core.push_warning("No skills available.");
            return Ok(());
        }
        self.ui
            .overlay_stack
            .push(OverlayState::Skills(SkillPickerState {
                filter: String::new(),
                selected: 0,
                skills,
            }));
        Ok(())
    }

    fn open_thread_picker(&mut self) -> Result<()> {
        self.core.refresh_threads()?;
        let threads = self.core.state().threads.clone();
        if threads.is_empty() {
            self.core.push_warning("No threads found.");
            return Ok(());
        }
        self.ui
            .overlay_stack
            .push(OverlayState::Threads(ThreadPickerState {
                filter: String::new(),
                selected: 0,
                threads,
            }));
        Ok(())
    }

    fn open_artifact_picker(&mut self) -> Result<()> {
        let artifacts = self.core.request_artifacts()?;
        if artifacts.is_empty() {
            self.core.push_warning("No artifacts found.");
            return Ok(());
        }
        self.ui
            .overlay_stack
            .push(OverlayState::Artifacts(ArtifactPickerState {
                filter: String::new(),
                selected: 0,
                artifacts,
            }));
        Ok(())
    }

    fn open_pending_approval_overlay(&mut self) -> Result<()> {
        let Some(approval) = self.core.state().approvals.pending.clone() else {
            self.core.push_warning("No pending approval.");
            return Ok(());
        };
        self.upsert_approval_overlay(&approval)
    }

    fn push_help_overlay(&mut self) {
        self.ui.overlay_stack.push(OverlayState::Help);
    }

    fn ready_only_action_allowed(&mut self, action: &str) -> bool {
        let warning = ready_only_action_warning(
            &self.core.state().connection,
            self.core.state().approvals.pending.is_some(),
            action,
        );
        if let Some(warning) = warning {
            self.core.push_warning(&warning);
            return false;
        }
        true
    }

    fn submit_binary_approval(
        &mut self,
        approval: &PendingApproval,
        choice: ApprovalChoice,
    ) -> Result<()> {
        let decision = match choice {
            ApprovalChoice::Approve => ApprovalDecision::Approve,
            ApprovalChoice::Reject => ApprovalDecision::Reject,
        };
        let outcome = self.core.submit_approval_response(
            approval,
            ApprovalRespondParams {
                approval_id: approval.id.clone(),
                decision,
                answers: None,
            },
        )?;
        if matches!(outcome, ApprovalResolutionOutcome::Resolved) {
            self.remove_approval_overlays();
        }
        Ok(())
    }

    fn reject_approval(&mut self, approval: &PendingApproval) -> Result<()> {
        let outcome = self.core.submit_approval_response(
            approval,
            ApprovalRespondParams {
                approval_id: approval.id.clone(),
                decision: ApprovalDecision::Reject,
                answers: None,
            },
        )?;
        if matches!(outcome, ApprovalResolutionOutcome::Resolved) {
            self.remove_approval_overlays();
        }
        Ok(())
    }

    fn submit_questionnaire(
        &mut self,
        approval: &PendingApproval,
        answers: BTreeMap<String, String>,
    ) -> Result<()> {
        let structured_answers = answers
            .into_iter()
            .map(|(question_id, answer)| {
                (
                    question_id,
                    ApprovalAnswer {
                        answers: vec![answer],
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let outcome = self.core.submit_approval_response(
            approval,
            ApprovalRespondParams {
                approval_id: approval.id.clone(),
                decision: ApprovalDecision::Approve,
                answers: Some(structured_answers),
            },
        )?;
        if matches!(outcome, ApprovalResolutionOutcome::Resolved) {
            self.remove_approval_overlays();
        }
        Ok(())
    }

    fn sync_overlays_with_state(&mut self) -> Result<()> {
        if let Some(approval) = self.core.state().approvals.pending.clone() {
            self.upsert_approval_overlay(&approval)?;
        } else {
            self.remove_approval_overlays();
        }
        Ok(())
    }

    fn upsert_approval_overlay(&mut self, approval: &PendingApproval) -> Result<()> {
        let current_id = self
            .ui
            .overlay_stack
            .iter()
            .rev()
            .find_map(|overlay| match overlay {
                OverlayState::Approval(ApprovalOverlayState::Decision { approval, .. }) => {
                    Some(approval.id.as_str())
                }
                OverlayState::Approval(ApprovalOverlayState::Questionnaire {
                    approval, ..
                }) => Some(approval.id.as_str()),
                _ => None,
            });
        if current_id == Some(approval.id.as_str()) {
            return Ok(());
        }
        self.ui
            .overlay_stack
            .push(OverlayState::Approval(build_approval_overlay(approval)?));
        Ok(())
    }

    fn remove_approval_overlays(&mut self) {
        self.ui
            .overlay_stack
            .retain(|overlay| !matches!(overlay, OverlayState::Approval(_)));
    }

    fn refresh_status_toast(&mut self) {
        self.ui.status_toast = latest_status_toast(self.core.state());
    }

    fn clear_composer(&mut self) {
        self.core.state_mut().composer.buffer.clear();
        self.core.state_mut().composer.cursor = 0;
    }

    fn request_redraw(&mut self, reason: RedrawRequest) {
        self.pending_redraw = Some(reason);
    }

    fn render(&mut self, _reason: RedrawRequest) -> Result<()> {
        self.ui.size = self.terminal.size().unwrap_or(self.ui.size);
        let state = self.core.state().clone();
        let selected_skill = self.core.selected_skill().cloned();
        let ui = self.ui.clone();
        self.terminal
            .draw(move |frame| render_screen(frame, &state, selected_skill.as_ref(), &ui))
    }
}

#[derive(Clone, Copy, Debug)]
enum RedrawRequest {
    Initial,
    DataChanged,
    KeyPress,
    Tick,
    Resized,
}

#[derive(Clone, Debug, Default)]
struct FullscreenUiState {
    overlay_stack: Vec<OverlayState>,
    transcript_scroll: usize,
    status_toast: Option<StatusToast>,
    size: TerminalSize,
}

#[derive(Clone, Debug)]
enum OverlayState {
    Help,
    Skills(SkillPickerState),
    Threads(ThreadPickerState),
    Artifacts(ArtifactPickerState),
    ArtifactViewer(ArtifactViewerState),
    Approval(ApprovalOverlayState),
}

struct PickerOverlaySpec<'a> {
    title: &'a str,
    filter: &'a str,
    count: usize,
    footer_hint: &'a str,
    items: &'a [String],
    selected: usize,
    detail: &'a [String],
    color: Color,
}

#[derive(Clone, Debug)]
struct SkillPickerState {
    filter: String,
    selected: usize,
    skills: Vec<SkillDescriptor>,
}

#[derive(Clone, Debug)]
struct ThreadPickerState {
    filter: String,
    selected: usize,
    threads: Vec<ThreadListItem>,
}

#[derive(Clone, Debug)]
struct ArtifactPickerState {
    filter: String,
    selected: usize,
    artifacts: Vec<ArtifactManifest>,
}

#[derive(Clone, Debug)]
struct ArtifactViewerState {
    payload: mli_protocol::ArtifactReadResult,
    selected_file: usize,
    scroll: usize,
}

impl ArtifactViewerState {
    fn new(payload: mli_protocol::ArtifactReadResult) -> Self {
        let selected_file = preferred_artifact_file_index(&payload);
        Self {
            payload,
            selected_file,
            scroll: 0,
        }
    }
}

#[derive(Clone, Debug)]
enum ApprovalOverlayState {
    Decision {
        approval: PendingApproval,
        choice: ApprovalChoice,
    },
    Questionnaire {
        approval: PendingApproval,
        questions: Vec<ApprovalQuestion>,
        current: usize,
        option_selected: usize,
        focus: QuestionFocus,
        input: String,
        answers: BTreeMap<String, String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApprovalChoice {
    Approve,
    Reject,
}

impl ApprovalChoice {
    fn toggle(self) -> Self {
        match self {
            Self::Approve => Self::Reject,
            Self::Reject => Self::Approve,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QuestionFocus {
    Options,
    TextInput,
}

impl QuestionFocus {
    fn toggle(self) -> Self {
        match self {
            Self::Options => Self::TextInput,
            Self::TextInput => Self::Options,
        }
    }
}

fn build_approval_overlay(approval: &PendingApproval) -> Result<ApprovalOverlayState> {
    if approval.kind != ApprovalKind::RequestUserInput {
        return Ok(ApprovalOverlayState::Decision {
            approval: approval.clone(),
            choice: ApprovalChoice::Approve,
        });
    }
    let payload: ApprovalQuestionPayload = serde_json::from_value(approval.raw_payload.clone())
        .context("failed to decode request_user_input payload")?;
    let focus = payload
        .questions
        .first()
        .map_or(QuestionFocus::TextInput, default_question_focus);
    Ok(ApprovalOverlayState::Questionnaire {
        approval: approval.clone(),
        questions: payload.questions,
        current: 0,
        option_selected: 0,
        focus,
        input: String::new(),
        answers: BTreeMap::new(),
    })
}

fn default_question_focus(question: &ApprovalQuestion) -> QuestionFocus {
    if question.options.is_some() {
        QuestionFocus::Options
    } else {
        QuestionFocus::TextInput
    }
}

fn current_question_answer(
    question: &ApprovalQuestion,
    focus: QuestionFocus,
    option_selected: usize,
    input: &str,
) -> Option<String> {
    if let Some(options) = &question.options {
        if matches!(focus, QuestionFocus::Options) {
            return options
                .get(option_selected)
                .map(|option| option.label.clone());
        }
        if question.is_other {
            let trimmed = input.trim();
            return (!trimmed.is_empty()).then(|| trimmed.to_owned());
        }
        return None;
    }
    let trimmed = input.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn ready_only_action_warning(
    connection: &ConnectionState,
    has_pending_approval: bool,
    action: &str,
) -> Option<String> {
    if matches!(connection, ConnectionState::Ready) {
        return None;
    }
    if matches!(connection, ConnectionState::WaitingApproval) && has_pending_approval {
        return Some("Resolve the pending approval first with /approval.".to_owned());
    }
    if matches!(connection, ConnectionState::Streaming) {
        return Some(format!("Interrupt the active turn before {action}."));
    }
    Some(format!(
        "Wait for the current turn to finish before {action}."
    ))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StatusToast {
    level: ToastLevel,
    text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToastLevel {
    Info,
    Warning,
    Error,
}

fn latest_status_toast(state: &AppState) -> Option<StatusToast> {
    state
        .transcript
        .history
        .iter()
        .rev()
        .find_map(|cell| match cell {
            HistoryCellModel::Status(cell) => Some(StatusToast {
                level: ToastLevel::Info,
                text: cell.message.clone(),
            }),
            HistoryCellModel::Warning(cell) => Some(StatusToast {
                level: ToastLevel::Warning,
                text: cell.message.clone(),
            }),
            HistoryCellModel::Error(cell) => Some(StatusToast {
                level: ToastLevel::Error,
                text: cell.message.clone(),
            }),
            _ => None,
        })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct TerminalSize {
    width: usize,
    height: usize,
}

impl TerminalSize {
    fn new(width: u16, height: u16) -> Self {
        Self {
            width: if width == 0 {
                FALLBACK_TERMINAL_WIDTH as usize
            } else {
                width as usize
            },
            height: if height == 0 {
                FALLBACK_TERMINAL_HEIGHT as usize
            } else {
                height as usize
            },
        }
    }

    fn as_rect(self) -> Rect {
        Rect::new(0, 0, self.width as u16, self.height as u16)
    }
}

fn render_screen(
    frame: &mut Frame,
    state: &AppState,
    selected_skill: Option<&SkillDescriptor>,
    ui: &FullscreenUiState,
) {
    let area = frame.area();
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(5),
            Constraint::Length(5),
        ])
        .split(area);

    render_header(frame, sections[0], state, selected_skill);
    render_transcript(frame, sections[1], state, ui.transcript_scroll);
    render_composer(frame, sections[2], state, selected_skill);
    if let Some(overlay) = ui.overlay_stack.last() {
        render_overlay(frame, area, overlay);
    }
    render_status_toast(frame, area, ui.status_toast.as_ref());
}

fn render_header(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    selected_skill: Option<&SkillDescriptor>,
) {
    let thread = state
        .active_thread_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "none".to_owned());
    let skill = selected_skill
        .map(|skill| skill.name.clone())
        .unwrap_or_else(|| "none".to_owned());
    let lines = vec![
        format!(
            "{APP_TITLE} | conn: {:?} | thread: {thread} | skill: {skill}",
            state.connection
        ),
        format!(
            "cwd: {}",
            state
                .runtime
                .cwd
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "unknown".to_owned())
        ),
        format!(
            "codex: {} | approval: {} | sandbox: {} | overlays: {}",
            state.runtime.codex_version.as_deref().unwrap_or("unknown"),
            state
                .runtime
                .approval_policy
                .map(|value| format!("{value:?}"))
                .unwrap_or_else(|| "unknown".to_owned()),
            state
                .runtime
                .sandbox_mode
                .map(|value| format!("{value:?}"))
                .unwrap_or_else(|| "unknown".to_owned()),
            if state.approvals.pending.is_some() {
                "approval pending"
            } else {
                "none"
            }
        ),
    ];
    render_panel(frame, area, "Session", &lines, Color::Cyan);
}

fn render_transcript(frame: &mut Frame, area: Rect, state: &AppState, scroll: usize) {
    let inner = panel_inner(area);
    let visible_lines =
        visible_transcript_lines(state, inner.width as usize, inner.height as usize, scroll);
    render_panel(frame, area, "Transcript", &visible_lines, Color::Blue);
}

fn render_composer(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    selected_skill: Option<&SkillDescriptor>,
) {
    let hint = match state.connection {
        ConnectionState::Streaming => "Esc/Ctrl+C interrupt, /approval retries pending decisions",
        ConnectionState::WaitingApproval => "Resolve approval first; Enter edits only",
        _ => "Enter submit, $ skill picker, /threads /skills /artifacts /help /quit",
    };
    let selected = selected_skill
        .map(|skill| format!("selected skill: {}", describe_skill(skill)))
        .unwrap_or_else(|| "selected skill: none".to_owned());
    let composer = composer_with_cursor(&state.composer.buffer, state.composer.cursor);
    let lines = vec![hint.to_owned(), selected, composer];
    render_panel(frame, area, "Composer", &lines, Color::Green);
}

fn render_status_toast(frame: &mut Frame, area: Rect, toast: Option<&StatusToast>) {
    let Some(toast) = toast else {
        return;
    };
    if area.width <= 4 || area.height <= 4 {
        return;
    }
    let max_width = usize::min(area.width.saturating_sub(4) as usize, 40).max(12);
    let lines = wrap_text(&toast.text, max_width.saturating_sub(2));
    let content_width = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0)
        .min(max_width);
    let width = (content_width + 2).min(area.width.saturating_sub(2) as usize) as u16;
    let height = (lines.len() + 2).min(area.height.saturating_sub(2) as usize) as u16;
    if width == 0 || height == 0 {
        return;
    }
    let rect = Rect::new(
        area.width.saturating_sub(width + 1),
        area.height.saturating_sub(height + 1),
        width,
        height,
    );
    let (title, color) = match toast.level {
        ToastLevel::Info => ("Status", Color::Cyan),
        ToastLevel::Warning => ("Warning", Color::Yellow),
        ToastLevel::Error => ("Error", Color::Red),
    };
    render_popup(frame, rect, title, &lines, color);
}

fn render_overlay(frame: &mut Frame, area: Rect, overlay: &OverlayState) {
    match overlay {
        OverlayState::Help => render_help_overlay(frame, area),
        OverlayState::Skills(state) => render_skill_picker(frame, area, state),
        OverlayState::Threads(state) => render_thread_picker(frame, area, state),
        OverlayState::Artifacts(state) => render_artifact_picker(frame, area, state),
        OverlayState::ArtifactViewer(state) => render_artifact_viewer(frame, area, state),
        OverlayState::Approval(state) => render_approval_overlay(frame, area, state),
    }
}

fn render_help_overlay(frame: &mut Frame, area: Rect) {
    let rect = centered_rect(area, 72, 12);
    let lines = vec![
        "Enter submits the composer when the app is ready.".to_owned(),
        "$ opens the skill picker when the composer is empty.".to_owned(),
        "/threads, /skills, /artifacts, /approval, /clear, /quit stay available.".to_owned(),
        "Up/Down/PageUp/PageDown scroll the transcript when no overlay is open.".to_owned(),
        "Live turns keep Esc/Ctrl+C reserved for interrupt, even over overlays.".to_owned(),
        "When idle, Esc/Ctrl+C close the active overlay.".to_owned(),
        "Tab opens this help overlay.".to_owned(),
    ];
    render_popup(frame, rect, "Help", &lines, Color::Magenta);
}

fn render_skill_picker(frame: &mut Frame, area: Rect, state: &SkillPickerState) {
    let filtered = filtered_skills(state);
    let items = filtered.iter().map(describe_skill).collect::<Vec<_>>();
    let detail = filtered
        .get(state.selected)
        .map(|skill| {
            vec![
                skill.description.clone(),
                format!("path: {}", skill.path.display()),
            ]
        })
        .unwrap_or_else(|| vec!["No matching skills.".to_owned()]);
    let spec = PickerOverlaySpec {
        title: "Skills",
        filter: &state.filter,
        count: filtered.len(),
        footer_hint: "Esc close, Enter select",
        items: &items,
        selected: state.selected,
        detail: &detail,
        color: Color::Yellow,
    };
    render_picker_overlay(frame, area, &spec);
}

fn render_thread_picker(frame: &mut Frame, area: Rect, state: &ThreadPickerState) {
    let filtered = filtered_threads_for_overlay(state);
    let items = filtered
        .iter()
        .map(|item| {
            format!(
                "{} ({:?})",
                item.thread
                    .title
                    .clone()
                    .unwrap_or_else(|| item.thread.id.to_string()),
                item.thread.status
            )
        })
        .collect::<Vec<_>>();
    let detail = filtered
        .get(state.selected)
        .map(|item| {
            vec![
                format!("cwd: {}", item.thread.cwd.display()),
                format!("transcript: {}", item.thread.transcript_path.display()),
                format!("updated: {}", item.thread.updated_at),
            ]
        })
        .unwrap_or_else(|| vec!["No matching threads.".to_owned()]);
    let spec = PickerOverlaySpec {
        title: "Threads",
        filter: &state.filter,
        count: filtered.len(),
        footer_hint: "Esc close, Enter resume",
        items: &items,
        selected: state.selected,
        detail: &detail,
        color: Color::Cyan,
    };
    render_picker_overlay(frame, area, &spec);
}

fn render_artifact_picker(frame: &mut Frame, area: Rect, state: &ArtifactPickerState) {
    let filtered = filtered_artifacts_for_overlay(state);
    let items = filtered
        .iter()
        .map(|artifact| format!("{} ({})", artifact.title, artifact.primary_path.display()))
        .collect::<Vec<_>>();
    let detail = filtered
        .get(state.selected)
        .map(|artifact| {
            vec![
                artifact.summary.clone(),
                format!("kind: {:?}", artifact.kind),
                format!("tags: {}", artifact.tags.join(", ")),
            ]
        })
        .unwrap_or_else(|| vec!["No matching artifacts.".to_owned()]);
    let spec = PickerOverlaySpec {
        title: "Artifacts",
        filter: &state.filter,
        count: filtered.len(),
        footer_hint: "Esc close, Enter open viewer",
        items: &items,
        selected: state.selected,
        detail: &detail,
        color: Color::Green,
    };
    render_picker_overlay(frame, area, &spec);
}

fn render_picker_overlay(frame: &mut Frame, area: Rect, spec: &PickerOverlaySpec<'_>) {
    let rect = centered_rect(area, 88, 16);
    let inner = popup_inner(frame, rect, spec.title, spec.color);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(inner);
    let header = vec![
        format!(
            "Filter: {}",
            if spec.filter.is_empty() {
                "(all)"
            } else {
                spec.filter
            }
        ),
        format!("Matches: {} | {}", spec.count, spec.footer_hint),
    ];
    render_lines(frame, sections[0], &header);
    render_picker_items(frame, sections[1], spec.items, spec.selected);
    render_picker_detail(frame, sections[2], spec.detail);
}

fn render_picker_items(frame: &mut Frame, area: Rect, items: &[String], selected: usize) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let visible = usize::min(area.height as usize, items.len());
    let start = selected
        .saturating_sub(visible.saturating_sub(1))
        .min(items.len().saturating_sub(visible));
    let lines = items
        .iter()
        .skip(start)
        .take(visible)
        .enumerate()
        .map(|(offset, item)| {
            let row = start + offset;
            let marker = if row == selected { '>' } else { ' ' };
            format!("{marker} {item}")
        })
        .collect::<Vec<_>>();
    render_lines(frame, area, &lines);
}

fn render_picker_detail(frame: &mut Frame, area: Rect, lines: &[String]) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let wrapped = wrap_lines(lines, area.width as usize);
    let visible = wrapped
        .into_iter()
        .take(area.height as usize)
        .collect::<Vec<_>>();
    render_lines(frame, area, &visible);
}

fn render_artifact_viewer(frame: &mut Frame, area: Rect, state: &ArtifactViewerState) {
    let rect = centered_rect(area, 92, 20);
    let inner = popup_inner(frame, rect, &state.payload.manifest.title, Color::LightCyan);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if state.payload.files.is_empty() {
        let lines = vec![
            "No readable files | Esc close".to_owned(),
            String::new(),
            "Selected artifact has no readable files.".to_owned(),
        ];
        render_lines(frame, inner, &lines);
        return;
    }
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(inner);
    let header_lines = vec![
        format!(
            "File {}/{} | Left/Right switch | Esc close",
            state.selected_file + 1,
            state.payload.files.len()
        ),
        state
            .payload
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let marker = if index == state.selected_file {
                    '*'
                } else {
                    ' '
                };
                format!("{marker}{}", file.path.display())
            })
            .collect::<Vec<_>>()
            .join(" | "),
    ];
    render_lines(frame, sections[0], &header_lines);
    let file = state
        .payload
        .files
        .get(state.selected_file)
        .or_else(|| state.payload.files.first());
    let content = file
        .map(|file| render_artifact_file_content_wrapped(file, sections[1].width as usize))
        .unwrap_or_else(|| vec!["Selected artifact has no readable files.".to_owned()]);
    let start = state
        .scroll
        .min(content.len().saturating_sub(sections[1].height as usize));
    let visible = content
        .into_iter()
        .skip(start)
        .take(sections[1].height as usize)
        .collect::<Vec<_>>();
    render_lines(frame, sections[1], &visible);
}

fn render_approval_overlay(frame: &mut Frame, area: Rect, state: &ApprovalOverlayState) {
    let desired_height = match state {
        ApprovalOverlayState::Decision { .. } => 12,
        ApprovalOverlayState::Questionnaire { .. } => 14,
    };
    let rect = centered_rect(area, 84, desired_height);
    let inner = popup_inner(frame, rect, "Approval", Color::Red);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let lines = approval_overlay_lines(state, inner.width as usize);
    let visible = lines
        .into_iter()
        .take(inner.height as usize)
        .collect::<Vec<_>>();
    render_lines(frame, inner, &visible);
}

fn approval_overlay_lines(state: &ApprovalOverlayState, width: usize) -> Vec<String> {
    match state {
        ApprovalOverlayState::Decision { approval, choice } => {
            let approve_label = if *choice == ApprovalChoice::Approve {
                "[ Approve ]"
            } else {
                "  Approve  "
            };
            let reject_label = if *choice == ApprovalChoice::Reject {
                "[ Reject ]"
            } else {
                "  Reject  "
            };
            let mut lines = vec![approval.title.clone()];
            lines.extend(wrap_text(&approval.description, width));
            lines.push(String::new());
            lines.push(format!("{approve_label}   {reject_label}"));
            lines.push("Left/Right/Tab switch, Enter submit, Esc reject".to_owned());
            lines
        }
        ApprovalOverlayState::Questionnaire {
            approval,
            questions,
            current,
            option_selected,
            focus,
            input,
            answers,
        } => {
            let Some(question) = questions.get(*current) else {
                return vec!["Submitting answers...".to_owned()];
            };
            let mut lines = vec![
                format!("{} ({}/{})", approval.title, current + 1, questions.len()),
                question.header.clone(),
            ];
            lines.extend(wrap_text(&question.question, width));
            lines.push(String::new());
            if let Some(options) = &question.options {
                for (index, option) in options.iter().enumerate() {
                    let marker =
                        if matches!(focus, QuestionFocus::Options) && index == *option_selected {
                            '>'
                        } else {
                            ' '
                        };
                    lines.push(format!(
                        "{marker} {}. {} - {}",
                        index + 1,
                        option.label,
                        option.description
                    ));
                }
            }
            if question.is_other || question.options.is_none() {
                let rendered_input = if question.is_secret {
                    masked_text_with_cursor(input, input.len())
                } else {
                    composer_with_cursor(input, input.len())
                };
                lines.push(format!(
                    "Input{}: {}",
                    if matches!(focus, QuestionFocus::TextInput) {
                        "*"
                    } else {
                        ""
                    },
                    rendered_input
                ));
            }
            if question.is_secret {
                lines.push("Secret input is masked locally.".to_owned());
            }
            lines.push(format!(
                "answers saved: {} | Enter confirm, Tab switch focus, Esc cancel",
                answers.len()
            ));
            lines
        }
    }
}

fn render_panel(frame: &mut Frame, area: Rect, title: &str, lines: &[String], color: Color) {
    let block = framed_block(title, color);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    render_lines(frame, inner, lines);
}

fn render_popup(frame: &mut Frame, area: Rect, title: &str, lines: &[String], color: Color) {
    let inner = popup_inner(frame, area, title, color);
    render_lines(frame, inner, lines);
}

fn popup_inner(frame: &mut Frame, area: Rect, title: &str, color: Color) -> Rect {
    frame.render_widget(Clear, area);
    let block = framed_block(title, color);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

fn render_lines(frame: &mut Frame, area: Rect, lines: &[String]) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let text = Text::from(lines.join("\n"));
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), area);
}

fn framed_block<'a>(title: &'a str, color: Color) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .title(Line::from(title).style(Style::default().fg(color).add_modifier(Modifier::BOLD)))
        .border_style(Style::default().fg(color))
}

fn panel_inner(area: Rect) -> Rect {
    let block = Block::default().borders(Borders::ALL);
    block.inner(area)
}

fn visible_transcript_lines(
    state: &AppState,
    width: usize,
    height: usize,
    scroll: usize,
) -> Vec<String> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let lines = transcript_lines(state, width);
    let total = lines.len();
    let visible = height;
    let max_scroll = total.saturating_sub(visible);
    let scroll = scroll.min(max_scroll);
    let end = total.saturating_sub(scroll);
    let start = end.saturating_sub(visible);
    lines[start..end.min(total)].to_vec()
}

fn render_artifact_file_content_wrapped(file: &ArtifactFilePayload, width: usize) -> Vec<String> {
    wrap_lines(&render_artifact_file_content(file), width)
}

fn wrap_lines(lines: &[String], width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut wrapped = Vec::new();
    for line in lines {
        let segments = wrap_text(line, width);
        if segments.is_empty() {
            wrapped.push(String::new());
        } else {
            wrapped.extend(segments);
        }
    }
    wrapped
}

fn centered_rect(area: Rect, width: usize, height: usize) -> Rect {
    let max_width = area.width.saturating_sub(2).max(1);
    let max_height = area.height.saturating_sub(2).max(1);
    let min_width = max_width.min(20);
    let min_height = max_height.min(6);
    let width = (width as u16).min(max_width).max(min_width);
    let height = (height as u16).min(max_height).max(min_height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn transcript_lines(state: &AppState, width: usize) -> Vec<String> {
    if state.transcript.history.is_empty() {
        return vec!["(empty)".to_owned()];
    }
    let mut lines = Vec::new();
    for cell in &state.transcript.history {
        let wrapped = wrap_text(&render_history_cell(cell), width);
        if wrapped.is_empty() {
            lines.push(String::new());
            continue;
        }
        for (index, line) in wrapped.into_iter().enumerate() {
            if index == 0 {
                lines.push(line);
            } else {
                lines.push(format!("  {line}"));
            }
        }
    }
    lines
}

fn render_artifact_file_content(file: &ArtifactFilePayload) -> Vec<String> {
    if let Some(read_error) = &file.read_error {
        return wrap_text(read_error, 120);
    }
    if let Some(text) = &file.text {
        if file.media_type.contains("json")
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(text)
            && let Ok(pretty) = serde_json::to_string_pretty(&value)
        {
            return pretty.lines().map(str::to_owned).collect();
        }
        return text.lines().map(str::to_owned).collect();
    }
    vec!["<binary payload omitted>".to_owned()]
}

fn composer_with_cursor(buffer: &str, cursor: usize) -> String {
    let mut rendered = String::new();
    let cursor = normalized_cursor(buffer, cursor);
    let (head, tail) = buffer.split_at(cursor);
    rendered.push_str(head);
    rendered.push('|');
    rendered.push_str(tail);
    rendered
}

fn masked_text_with_cursor(buffer: &str, cursor: usize) -> String {
    let cursor = normalized_cursor(buffer, cursor);
    let cursor_chars = buffer[..cursor].chars().count();
    let total_chars = buffer.chars().count();
    let mut rendered = String::with_capacity(total_chars.saturating_add(1));
    for _ in 0..cursor_chars {
        rendered.push('*');
    }
    rendered.push('|');
    for _ in cursor_chars..total_chars {
        rendered.push('*');
    }
    rendered
}

fn normalized_cursor(buffer: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(buffer.len());
    while cursor > 0 && !buffer.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

fn previous_char_boundary(buffer: &str, cursor: usize) -> usize {
    let cursor = normalized_cursor(buffer, cursor);
    if cursor == 0 {
        return 0;
    }
    let mut previous = cursor - 1;
    while previous > 0 && !buffer.is_char_boundary(previous) {
        previous -= 1;
    }
    previous
}

fn next_char_boundary(buffer: &str, cursor: usize) -> usize {
    let cursor = normalized_cursor(buffer, cursor);
    if cursor >= buffer.len() {
        return buffer.len();
    }
    let mut next = cursor + 1;
    while next < buffer.len() && !buffer.is_char_boundary(next) {
        next += 1;
    }
    next
}

fn move_composer_cursor_left(state: &mut AppState) {
    state.composer.cursor = previous_char_boundary(&state.composer.buffer, state.composer.cursor);
}

fn move_composer_cursor_right(state: &mut AppState) {
    state.composer.cursor = next_char_boundary(&state.composer.buffer, state.composer.cursor);
}

fn insert_composer_char(state: &mut AppState, ch: char) {
    let cursor = normalized_cursor(&state.composer.buffer, state.composer.cursor);
    state.composer.buffer.insert(cursor, ch);
    state.composer.cursor = cursor + ch.len_utf8();
}

fn delete_composer_backward(state: &mut AppState) {
    let cursor = normalized_cursor(&state.composer.buffer, state.composer.cursor);
    if cursor == 0 {
        return;
    }
    let remove_at = previous_char_boundary(&state.composer.buffer, cursor);
    state.composer.buffer.replace_range(remove_at..cursor, "");
    state.composer.cursor = remove_at;
}

fn delete_composer_forward(state: &mut AppState) {
    let cursor = normalized_cursor(&state.composer.buffer, state.composer.cursor);
    if cursor >= state.composer.buffer.len() {
        return;
    }
    let remove_end = next_char_boundary(&state.composer.buffer, cursor);
    state.composer.buffer.replace_range(cursor..remove_end, "");
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    for raw_line in text.lines() {
        if raw_line.is_empty() {
            lines.push(String::new());
            continue;
        }
        let chars = raw_line.chars().collect::<Vec<_>>();
        let mut start = 0;
        while start < chars.len() {
            let max_end = usize::min(start + width, chars.len());
            let mut end = max_end;
            if max_end < chars.len()
                && let Some(space) = chars[start..max_end]
                    .iter()
                    .rposition(|ch| ch.is_whitespace())
                    .filter(|space| *space > 0)
            {
                end = start + space;
            }
            if end == start {
                end = max_end;
            }
            let segment = chars[start..end].iter().collect::<String>();
            lines.push(segment.trim_end().to_owned());
            start = end;
            while start < chars.len() && chars[start].is_whitespace() {
                start += 1;
            }
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn filtered_skills(state: &SkillPickerState) -> Vec<SkillDescriptor> {
    filter_skills(
        &state.skills,
        (!state.filter.trim().is_empty()).then_some(state.filter.as_str()),
    )
    .into_iter()
    .cloned()
    .collect()
}

fn filtered_threads_for_overlay(state: &ThreadPickerState) -> Vec<ThreadListItem> {
    filter_threads(
        &state.threads,
        (!state.filter.trim().is_empty()).then_some(state.filter.as_str()),
    )
    .into_iter()
    .cloned()
    .collect()
}

fn filtered_artifacts_for_overlay(state: &ArtifactPickerState) -> Vec<ArtifactManifest> {
    filter_artifacts(
        &state.artifacts,
        (!state.filter.trim().is_empty()).then_some(state.filter.as_str()),
    )
    .into_iter()
    .cloned()
    .collect()
}

fn clamp_picker_selection<T>(state: &mut T, len: usize)
where
    T: PickerSelection,
{
    state.set_selected(if len == 0 {
        0
    } else {
        usize::min(state.selected(), len - 1)
    });
}

trait PickerSelection {
    fn selected(&self) -> usize;
    fn set_selected(&mut self, value: usize);
}

impl PickerSelection for SkillPickerState {
    fn selected(&self) -> usize {
        self.selected
    }

    fn set_selected(&mut self, value: usize) {
        self.selected = value;
    }
}

impl PickerSelection for ThreadPickerState {
    fn selected(&self) -> usize {
        self.selected
    }

    fn set_selected(&mut self, value: usize) {
        self.selected = value;
    }
}

impl PickerSelection for ArtifactPickerState {
    fn selected(&self) -> usize {
        self.selected
    }

    fn set_selected(&mut self, value: usize) {
        self.selected = value;
    }
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("failed to enable crossterm raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, Hide).context("failed to enter alternate screen")?;
        let backend = CrosstermBackend::new(stdout);
        let size = current_terminal_size();
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Fixed(size.as_rect()),
            },
        )
        .context("failed to create ratatui terminal")?;
        terminal
            .resize(size.as_rect())
            .context("failed to set initial ratatui viewport size")?;
        terminal
            .clear()
            .context("failed to clear ratatui terminal")?;
        Ok(Self { terminal })
    }

    fn size(&mut self) -> Result<TerminalSize> {
        let size = current_terminal_size();
        self.terminal
            .resize(size.as_rect())
            .context("failed to resize ratatui viewport")?;
        Ok(size)
    }

    fn draw<F>(&mut self, render: F) -> Result<()>
    where
        F: FnOnce(&mut Frame),
    {
        self.terminal
            .draw(render)
            .context("failed to draw fullscreen frame")?;
        Ok(())
    }
}

fn current_terminal_size() -> TerminalSize {
    match crossterm::terminal::size() {
        Ok((width, height)) => TerminalSize::new(width, height),
        Err(_) => TerminalSize::new(FALLBACK_TERMINAL_WIDTH, FALLBACK_TERMINAL_HEIGHT),
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), Show, LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KeyEvent {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Tab,
    CtrlC,
    CtrlL,
}

fn map_key_event(event: CrosstermKeyEvent) -> Option<KeyEvent> {
    if !matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    let plain_char = event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT;
    match event.code {
        KeyCode::Enter => Some(KeyEvent::Enter),
        KeyCode::Esc => Some(KeyEvent::Esc),
        KeyCode::Backspace => Some(KeyEvent::Backspace),
        KeyCode::Delete => Some(KeyEvent::Delete),
        KeyCode::Left => Some(KeyEvent::Left),
        KeyCode::Right => Some(KeyEvent::Right),
        KeyCode::Up => Some(KeyEvent::Up),
        KeyCode::Down => Some(KeyEvent::Down),
        KeyCode::PageUp => Some(KeyEvent::PageUp),
        KeyCode::PageDown => Some(KeyEvent::PageDown),
        KeyCode::Home => Some(KeyEvent::Home),
        KeyCode::End => Some(KeyEvent::End),
        KeyCode::Tab | KeyCode::BackTab => Some(KeyEvent::Tab),
        KeyCode::Char(ch)
            if event.modifiers.contains(KeyModifiers::CONTROL) && ch.eq_ignore_ascii_case(&'c') =>
        {
            Some(KeyEvent::CtrlC)
        }
        KeyCode::Char(ch)
            if event.modifiers.contains(KeyModifiers::CONTROL) && ch.eq_ignore_ascii_case(&'l') =>
        {
            Some(KeyEvent::CtrlL)
        }
        KeyCode::Char(ch) if plain_char && !ch.is_control() => Some(KeyEvent::Char(ch)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::{backend::TestBackend, buffer::Buffer};

    use super::*;
    use mli_types::{
        ApprovalCell, ApprovalPolicy, ArtifactEventCell, ArtifactFilePayload, ArtifactId,
        ArtifactKind, ArtifactManifest, ArtifactPreview, AssistantMessageCell, ComposerState,
        LocalThreadId, LocalTurnId, RuntimeBannerState, SandboxMode, TranscriptState,
        UserMessageCell, utc_now,
    };

    fn sample_state() -> AppState {
        AppState {
            connection: ConnectionState::Ready,
            runtime: RuntimeBannerState {
                cwd: Some(PathBuf::from("/tmp/project")),
                codex_version: Some("0.120.0".to_owned()),
                approval_policy: Some(ApprovalPolicy::OnRequest),
                sandbox_mode: Some(SandboxMode::WorkspaceWrite),
            },
            composer: ComposerState {
                buffer: "inspect dataset".to_owned(),
                cursor: "inspect dataset".len(),
                skill_query: None,
            },
            transcript: TranscriptState {
                history: vec![
                    HistoryCellModel::UserMessage(UserMessageCell {
                        text: "hello".to_owned(),
                    }),
                    HistoryCellModel::AssistantMessage(AssistantMessageCell {
                        text: "world".to_owned(),
                        streaming: false,
                    }),
                ],
            },
            ..AppState::default()
        }
    }

    fn render_screen_snapshot(
        state: &AppState,
        selected_skill: Option<&SkillDescriptor>,
        ui: &FullscreenUiState,
    ) -> String {
        let backend = TestBackend::new(ui.size.width as u16, ui.size.height as u16);
        let mut terminal =
            Terminal::new(backend).unwrap_or_else(|error| panic!("create test terminal: {error}"));
        terminal
            .draw(|frame| render_screen(frame, state, selected_skill, ui))
            .unwrap_or_else(|error| panic!("draw test frame: {error}"));
        buffer_to_string(terminal.backend().buffer())
    }

    fn buffer_to_string(buffer: &Buffer) -> String {
        let mut lines = Vec::with_capacity(buffer.area.height as usize);
        for y in 0..buffer.area.height {
            let mut line = String::new();
            for x in 0..buffer.area.width {
                line.push_str(buffer[(x, y)].symbol());
            }
            lines.push(line.trim_end().to_owned());
        }
        lines.join("\n")
    }

    #[test]
    fn wrap_text_preserves_paragraphs() {
        let wrapped = wrap_text("alpha beta gamma", 8);
        assert_eq!(
            wrapped,
            vec!["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()]
        );
    }

    #[test]
    fn composer_with_cursor_marks_insertion_point() {
        assert_eq!(composer_with_cursor("hello", 2), "he|llo");
        assert_eq!(composer_with_cursor("", 0), "|");
    }

    #[test]
    fn masked_text_with_cursor_hides_secret_contents() {
        assert_eq!(masked_text_with_cursor("secret", 3), "***|***");
        assert_eq!(masked_text_with_cursor("你好", "你".len()), "*|*");
    }

    #[test]
    fn ready_only_action_warning_allows_ready_state() {
        assert_eq!(
            ready_only_action_warning(&ConnectionState::Ready, false, "running /threads"),
            None
        );
    }

    #[test]
    fn ready_only_action_warning_requires_interrupt_while_streaming() {
        assert_eq!(
            ready_only_action_warning(&ConnectionState::Streaming, false, "running /threads"),
            Some("Interrupt the active turn before running /threads.".to_owned())
        );
    }

    #[test]
    fn ready_only_action_warning_prefers_pending_approval_hint() {
        assert_eq!(
            ready_only_action_warning(
                &ConnectionState::WaitingApproval,
                true,
                "opening the skill picker"
            ),
            Some("Resolve the pending approval first with /approval.".to_owned())
        );
    }

    #[test]
    fn map_key_event_preserves_navigation_and_ctrl_shortcuts() {
        let cases = [
            (
                CrosstermKeyEvent::new(KeyCode::Char('你'), KeyModifiers::NONE),
                Some(KeyEvent::Char('你')),
            ),
            (
                CrosstermKeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT),
                Some(KeyEvent::Char('C')),
            ),
            (
                CrosstermKeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                Some(KeyEvent::CtrlC),
            ),
            (
                CrosstermKeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
                Some(KeyEvent::CtrlL),
            ),
            (
                CrosstermKeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
                Some(KeyEvent::Delete),
            ),
            (
                CrosstermKeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
                Some(KeyEvent::PageDown),
            ),
            (
                CrosstermKeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
                Some(KeyEvent::Home),
            ),
        ];

        for (event, expected) in cases {
            assert_eq!(map_key_event(event), expected);
        }
    }

    #[test]
    fn composer_editing_respects_utf8_boundaries() {
        let mut state = sample_state();
        state.composer.buffer = "你a好".to_owned();
        state.composer.cursor = state.composer.buffer.len();

        move_composer_cursor_left(&mut state);
        assert_eq!(
            composer_with_cursor(&state.composer.buffer, state.composer.cursor),
            "你a|好"
        );

        move_composer_cursor_left(&mut state);
        insert_composer_char(&mut state, '🙂');
        assert_eq!(state.composer.buffer, "你🙂a好");
        assert_eq!(
            composer_with_cursor(&state.composer.buffer, state.composer.cursor),
            "你🙂|a好"
        );

        delete_composer_backward(&mut state);
        assert_eq!(state.composer.buffer, "你a好");
        assert_eq!(
            composer_with_cursor(&state.composer.buffer, state.composer.cursor),
            "你|a好"
        );

        delete_composer_forward(&mut state);
        assert_eq!(state.composer.buffer, "你好");
        assert_eq!(
            composer_with_cursor(&state.composer.buffer, state.composer.cursor),
            "你|好"
        );
    }

    #[test]
    fn render_screen_includes_header_transcript_and_composer() {
        let ui = FullscreenUiState {
            size: TerminalSize {
                width: 80,
                height: 24,
            },
            ..FullscreenUiState::default()
        };

        let rendered = render_screen_snapshot(&sample_state(), None, &ui);

        assert!(rendered.contains("Session"));
        assert!(rendered.contains("Transcript"));
        assert!(rendered.contains("Composer"));
        assert!(rendered.contains("you> hello"));
        assert!(rendered.contains("assistant> world"));
        assert!(rendered.contains("inspect dataset|"));
    }

    #[test]
    fn build_approval_overlay_decodes_request_user_input_questions() {
        let approval = PendingApproval {
            id: "approval-1".to_owned(),
            kind: ApprovalKind::RequestUserInput,
            title: "Need more detail".to_owned(),
            description: "Collect context".to_owned(),
            raw_payload: serde_json::json!({
                "questions": [
                    {
                        "id": "priority",
                        "header": "Priority",
                        "question": "Choose one",
                        "options": [
                            {"label": "low", "description": "Later"},
                            {"label": "high", "description": "Now"}
                        ]
                    }
                ]
            }),
        };

        let overlay = build_approval_overlay(&approval)
            .unwrap_or_else(|error| panic!("build approval overlay: {error}"));

        match overlay {
            ApprovalOverlayState::Questionnaire { questions, .. } => {
                assert_eq!(questions.len(), 1);
                assert_eq!(questions[0].header, "Priority");
            }
            other => panic!("expected questionnaire overlay, got {other:?}"),
        }
    }

    #[test]
    fn latest_status_toast_prefers_most_recent_warning_or_error() {
        let mut state = sample_state();
        let thread_id = LocalThreadId::new();
        let turn_id = LocalTurnId::new();
        state
            .transcript
            .history
            .push(HistoryCellModel::ArtifactCreated(ArtifactEventCell {
                manifest: ArtifactManifest {
                    id: ArtifactId::new(),
                    version: 1,
                    local_thread_id: thread_id,
                    local_turn_id: turn_id,
                    kind: ArtifactKind::DatasetAudit,
                    title: "artifact".to_owned(),
                    created_at: utc_now(),
                    updated_at: utc_now(),
                    summary: "summary".to_owned(),
                    tags: Vec::new(),
                    primary_path: PathBuf::from("report.md"),
                    extra_paths: Vec::new(),
                    metadata: serde_json::json!({}),
                },
                preview: ArtifactPreview::Generic {
                    headline: "headline".to_owned(),
                },
            }));
        state
            .transcript
            .history
            .push(HistoryCellModel::ApprovalRequest(ApprovalCell {
                approval: PendingApproval {
                    id: "approval-2".to_owned(),
                    kind: ApprovalKind::CommandExecution,
                    title: "Approve".to_owned(),
                    description: "run tests".to_owned(),
                    raw_payload: serde_json::json!({}),
                },
            }));
        state
            .transcript
            .history
            .push(HistoryCellModel::Warning(mli_types::WarningCell {
                message: "Heads up".to_owned(),
            }));

        let toast = latest_status_toast(&state).unwrap_or_else(|| panic!("missing toast"));
        assert_eq!(toast.level, ToastLevel::Warning);
        assert_eq!(toast.text, "Heads up");
    }

    #[test]
    fn render_screen_handles_empty_artifact_viewer_payload() {
        let mut ui = FullscreenUiState {
            size: TerminalSize {
                width: 100,
                height: 28,
            },
            ..FullscreenUiState::default()
        };
        ui.overlay_stack
            .push(OverlayState::ArtifactViewer(ArtifactViewerState::new(
                mli_protocol::ArtifactReadResult {
                    manifest: ArtifactManifest {
                        id: ArtifactId::new(),
                        version: 1,
                        local_thread_id: LocalThreadId::new(),
                        local_turn_id: LocalTurnId::new(),
                        kind: ArtifactKind::DatasetAudit,
                        title: "empty artifact".to_owned(),
                        created_at: utc_now(),
                        updated_at: utc_now(),
                        summary: "summary".to_owned(),
                        tags: Vec::new(),
                        primary_path: PathBuf::from("report.md"),
                        extra_paths: Vec::new(),
                        metadata: serde_json::json!({}),
                    },
                    files: Vec::<ArtifactFilePayload>::new(),
                },
            )));

        let rendered = render_screen_snapshot(&sample_state(), None, &ui);

        assert!(rendered.contains("No readable files | Esc close"));
        assert!(rendered.contains("Selected artifact has no readable files."));
        assert!(!rendered.contains("File 1/0"));
    }

    #[test]
    fn render_screen_masks_secret_request_user_input_answers() {
        let mut ui = FullscreenUiState {
            size: TerminalSize {
                width: 100,
                height: 28,
            },
            ..FullscreenUiState::default()
        };
        ui.overlay_stack.push(OverlayState::Approval(
            ApprovalOverlayState::Questionnaire {
                approval: PendingApproval {
                    id: "approval-secret".to_owned(),
                    kind: ApprovalKind::RequestUserInput,
                    title: "Need secret".to_owned(),
                    description: "Collect a token".to_owned(),
                    raw_payload: serde_json::json!({}),
                },
                questions: vec![ApprovalQuestion {
                    id: "token".to_owned(),
                    header: "Token".to_owned(),
                    question: "Enter a secret token".to_owned(),
                    is_other: true,
                    is_secret: true,
                    options: None,
                }],
                current: 0,
                option_selected: 0,
                focus: QuestionFocus::TextInput,
                input: "topsecret".to_owned(),
                answers: BTreeMap::new(),
            },
        ));

        let rendered = render_screen_snapshot(&sample_state(), None, &ui);

        assert!(rendered.contains("Secret input is masked locally."));
        assert!(rendered.contains("Input*: *********|"));
        assert!(!rendered.contains("topsecret"));
    }

    #[test]
    fn render_screen_keeps_status_toast_visible_over_overlay() {
        let mut ui = FullscreenUiState {
            size: TerminalSize {
                width: 40,
                height: 16,
            },
            status_toast: Some(StatusToast {
                level: ToastLevel::Error,
                text: "send failed".to_owned(),
            }),
            ..FullscreenUiState::default()
        };
        ui.overlay_stack
            .push(OverlayState::Approval(ApprovalOverlayState::Decision {
                approval: PendingApproval {
                    id: "approval-1".to_owned(),
                    kind: ApprovalKind::CommandExecution,
                    title: "Approve".to_owned(),
                    description: "run tests".to_owned(),
                    raw_payload: serde_json::json!({}),
                },
                choice: ApprovalChoice::Approve,
            }));

        let rendered = render_screen_snapshot(&sample_state(), None, &ui);

        assert!(rendered.contains("Approval"));
        assert!(rendered.contains("send failed"));
    }

    #[test]
    fn render_screen_clamps_transcript_scroll_to_available_history() {
        let ui = FullscreenUiState {
            size: TerminalSize {
                width: 80,
                height: 24,
            },
            transcript_scroll: 99,
            ..FullscreenUiState::default()
        };

        let rendered = render_screen_snapshot(&sample_state(), None, &ui);

        assert!(rendered.contains("you> hello"));
        assert!(rendered.contains("assistant> world"));
        assert!(!rendered.contains(
            "+-Transcript---------------------------+\n|                                      |"
        ));
    }
}
