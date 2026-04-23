use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use mli_config::AppConfig;
use mli_protocol::{
    ApprovalAnswer, ApprovalDecision, ApprovalRespondParams, ArtifactListParams,
    ArtifactReadParams, ConfigReadResult, ConfigWriteParams, ConfigWriteResult, JsonRpcMessage,
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RequestId, ServerNotification,
    SkillsListParams, ThreadReadParams, ThreadResumeParams, ThreadStartParams,
    TurnInterruptParams, TurnStartParams, UserInput,
};
use mli_runtime::default_initialize_params;
use mli_types::{
    AppState, ApprovalCell, ApprovalKind, ArtifactEventCell, ArtifactFilePayload, ArtifactManifest,
    ArtifactPreview, AssistantMessageCell, ApprovalPolicy, ComposerState, ConnectionState,
    ErrorCell, HistoryCellModel, SandboxMode, SkillDescriptor, StatusCell, ThreadListItem,
    UserMessageCell, WarningCell,
};

use crate::renderer::render_app;

#[derive(Debug)]
enum ClientMessage {
    Response(JsonRpcResponse),
    Error(mli_protocol::JsonRpcError),
    Notification(Box<ServerNotification>),
    Ignored,
}

enum SkillTokenMatch {
    Missing,
    Unique(SkillDescriptor),
    Ambiguous(Vec<SkillDescriptor>),
}

pub(crate) enum ApprovalResolutionOutcome {
    Resolved,
    Deferred,
    NoPending,
}

enum PickerFilter {
    All,
    Query(String),
    Cancel,
}

pub(crate) struct AppClient {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<ClientMessage>,
    buffered_notifications: VecDeque<ServerNotification>,
    stderr_log: Arc<Mutex<String>>,
    next_request_id: i64,
}

impl AppClient {
    pub fn spawn(app_server_bin: Option<PathBuf>) -> Result<Self> {
        let app_server_bin = match app_server_bin {
            Some(path) => path,
            None => {
                let current_exe =
                    std::env::current_exe().context("failed to resolve current executable")?;
                current_exe.with_file_name("ml-intern-app-server")
            }
        };
        let mut child = Command::new(&app_server_bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn {}", app_server_bin.display()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("missing app-server stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("missing app-server stderr"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("missing app-server stdin"))?;
        let (tx, rx) = mpsc::channel();
        let stderr_log = Arc::new(Mutex::new(String::new()));
        spawn_reader_thread(stdout, tx);
        spawn_stderr_reader(stderr, Arc::clone(&stderr_log));
        Ok(Self {
            child,
            stdin,
            rx,
            buffered_notifications: VecDeque::new(),
            stderr_log,
            next_request_id: 0,
        })
    }

    pub fn initialize(&mut self) -> Result<()> {
        let params = default_initialize_params();
        let _: mli_protocol::InitializeResult = self.request("initialize", &params)?;
        self.notify::<()>("initialized", None)
    }

    pub fn request<P, R>(&mut self, method: &str, params: &P) -> Result<R>
    where
        P: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        self.next_request_id += 1;
        let request_id = RequestId::Integer(self.next_request_id);
        let message = JsonRpcMessage::Request(JsonRpcRequest {
            id: request_id.clone(),
            method: method.to_owned(),
            params: Some(
                serde_json::to_value(params).context("failed to serialize client params")?,
            ),
        });
        self.write_message(&message)?;
        loop {
            let message = match self.rx.recv() {
                Ok(message) => message,
                Err(_) => return Err(self.app_server_closed_error()),
            };
            match message {
                ClientMessage::Response(response) if response.id == request_id => {
                    return serde_json::from_value(response.result)
                        .context("failed to decode app-server response");
                }
                ClientMessage::Error(error) if error.id == request_id => {
                    return Err(anyhow!(
                        "app-server error {}: {}",
                        error.error.code,
                        error.error.message
                    ));
                }
                ClientMessage::Notification(notification) => {
                    self.buffered_notifications.push_back(*notification)
                }
                ClientMessage::Response(_) | ClientMessage::Error(_) | ClientMessage::Ignored => {
                    continue;
                }
            }
        }
    }

    pub fn notify<P: serde::Serialize>(&mut self, method: &str, params: Option<&P>) -> Result<()> {
        let message = JsonRpcMessage::Notification(JsonRpcNotification {
            method: method.to_owned(),
            params: match params {
                Some(params) => Some(
                    serde_json::to_value(params)
                        .context("failed to serialize client notification")?,
                ),
                None => None,
            },
        });
        self.write_message(&message)
    }

    pub fn recv_notification(&mut self) -> Result<Option<ServerNotification>> {
        if let Some(notification) = self.buffered_notifications.pop_front() {
            return Ok(Some(notification));
        }
        match self.rx.try_recv() {
            Ok(ClientMessage::Notification(notification)) => Ok(Some(*notification)),
            Ok(ClientMessage::Response(_))
            | Ok(ClientMessage::Error(_))
            | Ok(ClientMessage::Ignored) => Ok(None),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Ok(None),
        }
    }

    fn write_message(&mut self, message: &JsonRpcMessage) -> Result<()> {
        let line = serde_json::to_string(message).context("failed to encode client JSONL")?;
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|_| self.stdin.write_all(b"\n"))
            .and_then(|_| self.stdin.flush())
            .context("failed to write to app-server")
    }

    fn app_server_closed_error(&mut self) -> anyhow::Error {
        let mut message = "app-server closed unexpectedly".to_owned();
        match self.child.try_wait() {
            Ok(Some(status)) => {
                message.push_str(&format!(" (status {status})"));
            }
            Ok(None) => {}
            Err(error) => {
                message.push_str(&format!(" (failed to inspect exit status: {error})"));
            }
        }
        let stderr = self.stderr_snapshot();
        if !stderr.is_empty() {
            message.push_str(&format!(": {stderr}"));
        }
        anyhow!(message)
    }

    fn stderr_snapshot(&self) -> String {
        self.stderr_log
            .lock()
            .map(|buffer| buffer.trim().to_owned())
            .unwrap_or_default()
    }
}

impl Drop for AppClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

pub struct TranscriptApp {
    client: AppClient,
    state: AppState,
    selected_skill: Option<SkillDescriptor>,
    active_turn_id: Option<mli_types::LocalTurnId>,
    interactive_rendering: bool,
}

impl TranscriptApp {
    pub(crate) fn new(client: AppClient) -> Self {
        Self {
            client,
            state: AppState {
                connection: ConnectionState::Connecting,
                composer: ComposerState::default(),
                ..AppState::default()
            },
            selected_skill: None,
            active_turn_id: None,
            interactive_rendering: false,
        }
    }

    pub fn run(&mut self) -> Result<()> {
        self.initialize_session()?;
        let stdin = io::stdin();
        loop {
            self.render_interactive_frame()?;
            print!("> ");
            io::stdout().flush().context("failed to flush prompt")?;
            let mut input = String::new();
            stdin
                .read_line(&mut input)
                .context("failed to read input")?;
            let input = input.trim().to_owned();
            if input.is_empty() {
                continue;
            }
            if input == "$" {
                self.pick_skill()?;
                continue;
            }
            if input == "/quit" || input == "/exit" {
                break;
            }
            if input.starts_with('/') {
                self.handle_command(&input)?;
            } else {
                self.send_prompt(input)?;
            }
        }
        Ok(())
    }

    pub(crate) fn initialize_session(&mut self) -> Result<()> {
        self.client.initialize()?;
        let runtime_info: mli_protocol::RuntimeInfoResult = self
            .client
            .request("runtime/info", &serde_json::json!({}))?;
        self.state.runtime.cwd = Some(runtime_info.cwd.clone());
        self.state.runtime.codex_version = Some(runtime_info.codex_version.clone());
        self.state.runtime.approval_policy = Some(runtime_info.approval_policy);
        self.state.runtime.sandbox_mode = Some(runtime_info.sandbox_mode);
        self.state.connection = ConnectionState::Ready;
        self.interactive_rendering = true;
        self.refresh_threads()?;
        self.push_status("Ready. Enter a prompt or use /help.");
        Ok(())
    }

    fn handle_command(&mut self, command: &str) -> Result<()> {
        match command {
            "/help" => {
                self.push_status("/threads 列线程并恢复；输入 `$` 或 `/skills` 选技能；/artifacts 浏览 artifacts；/approval 重试 pending approval；/yolo 切换 YOLO；/mode safe|readonly|yolo；/clear 清屏；/quit 退出");
            }
            "/clear" => self.state.transcript.history.clear(),
            "/threads" => self.pick_thread()?,
            "/skills" => self.pick_skill()?,
            "/artifacts" => self.browse_artifacts()?,
            "/approval" => self.retry_pending_approval()?,
            "/yolo" => self.toggle_yolo_mode()?,
            other if other.starts_with("/mode") => self.set_mode_command(other)?,
            other => self.push_warning(&format!("Unknown command: {other}")),
        }
        Ok(())
    }

    pub(crate) fn send_prompt(&mut self, text: String) -> Result<()> {
        let previous_turn_id = self.active_turn_id;
        let previous_connection = self.state.connection;
        self.start_prompt(text)?;
        let started_streaming = self.active_turn_id != previous_turn_id
            || (!matches!(previous_connection, ConnectionState::Streaming)
                && matches!(self.state.connection, ConnectionState::Streaming));
        if started_streaming {
            self.drain_notifications_until_ready()
        } else {
            Ok(())
        }
    }

    pub(crate) fn start_prompt(&mut self, text: String) -> Result<()> {
        if self.state.connection == ConnectionState::WaitingApproval
            && self.state.approvals.pending.is_some()
        {
            self.push_warning("Resolve the pending approval first with /approval.");
            return Ok(());
        }
        let Some((inline_skill, prompt_text)) = self.resolve_inline_skill(&text)? else {
            return Ok(());
        };
        if prompt_text.trim().is_empty()
            && let Some(skill) = inline_skill
        {
            self.selected_skill = Some(skill.clone());
            self.push_status(&format!("Selected skill: {}", selected_skill_label(&skill)));
            return Ok(());
        }
        self.state
            .transcript
            .history
            .push(HistoryCellModel::UserMessage(UserMessageCell {
                text: text.clone(),
            }));
        if self.state.active_thread_id.is_none() {
            let cwd = std::env::current_dir().context("failed to resolve cwd")?;
            let result: mli_protocol::ThreadStartResult = self.client.request(
                "thread/start",
                &ThreadStartParams {
                    cwd,
                    title: Some(text.chars().take(60).collect()),
                    model: None,
                    approval_policy: None,
                    sandbox_mode: None,
                },
            )?;
            self.set_active_thread(result.thread.id);
        }
        let thread_id = self
            .state
            .active_thread_id
            .ok_or_else(|| anyhow!("missing active thread"))?;
        let mut input_items = Vec::new();
        let active_skill = inline_skill.or_else(|| self.selected_skill.clone());
        if let Some(skill) = &active_skill {
            input_items.push(UserInput::Skill {
                name: skill.name.clone(),
                path: skill.path.clone(),
            });
            self.selected_skill = Some(skill.clone());
        }
        input_items.push(UserInput::Text { text: prompt_text });
        self.state.connection = ConnectionState::Streaming;
        let turn: mli_protocol::TurnStartResult = self.client.request(
            "turn/start",
            &TurnStartParams {
                thread_id,
                input: input_items,
            },
        )?;
        self.active_turn_id = Some(turn.turn.id);
        Ok(())
    }

    fn resolve_inline_skill(
        &mut self,
        raw_text: &str,
    ) -> Result<Option<(Option<SkillDescriptor>, String)>> {
        let Some((token, remainder)) = parse_leading_skill_token(raw_text) else {
            return Ok(Some((None, raw_text.to_owned())));
        };
        if let Some(skill) = self
            .selected_skill
            .as_ref()
            .filter(|skill| skill.name == token)
            .cloned()
        {
            return Ok(Some((Some(skill), remainder)));
        }
        let result: mli_protocol::SkillsListResult = self.client.request(
            "skills/list",
            &SkillsListParams {
                cwd: None,
                force_reload: Some(false),
            },
        )?;
        match match_skill_token(&token, &result.skills) {
            SkillTokenMatch::Missing => Ok(Some((None, raw_text.to_owned()))),
            SkillTokenMatch::Unique(skill) => Ok(Some((Some(skill), remainder))),
            SkillTokenMatch::Ambiguous(skills) => {
                self.push_warning(&format!(
                    "Multiple skills named `{token}`; use `$` or /skills to pick one: {}",
                    summarize_skill_paths(&skills)
                ));
                Ok(None)
            }
        }
    }

    fn drain_notifications_until_ready(&mut self) -> Result<()> {
        self.drain_notifications_until_ready_with_interrupts(true)
    }

    fn drain_notifications_until_ready_with_interrupts(
        &mut self,
        enable_interrupts: bool,
    ) -> Result<()> {
        let mut interrupt_watcher = if enable_interrupts && stdio_supports_interrupt_watcher() {
            match InterruptWatcher::new() {
                Ok(watcher) => Some(watcher),
                Err(error) => {
                    self.push_warning(&format!("Interrupt hotkeys unavailable: {error}"));
                    None
                }
            }
        } else {
            None
        };
        loop {
            if let Some(notification) = self.client.recv_notification()? {
                let completed = matches!(notification, ServerNotification::TurnCompleted { .. });
                self.apply_notification(notification)?;
                self.render_interactive_frame()?;
                if self.state.connection == ConnectionState::WaitingApproval {
                    if let Some(watcher) = interrupt_watcher.as_mut() {
                        watcher.suspend()?;
                    }
                    let outcome = self.resolve_pending_approval()?;
                    if let Some(watcher) = interrupt_watcher.as_mut() {
                        watcher.resume()?;
                    }
                    if matches!(outcome, ApprovalResolutionOutcome::Deferred) {
                        break;
                    }
                }
                if completed {
                    self.state.connection = ConnectionState::Ready;
                    while let Some(notification) = self.client.recv_notification()? {
                        self.apply_notification(notification)?;
                    }
                    break;
                }
                continue;
            }
            if interrupt_watcher
                .as_ref()
                .is_some_and(InterruptWatcher::poll_interrupt)
            {
                self.request_interrupt()?;
                self.render_interactive_frame()?;
            }
            if self.state.connection == ConnectionState::WaitingApproval {
                if let Some(watcher) = interrupt_watcher.as_mut() {
                    watcher.suspend()?;
                }
                let outcome = self.resolve_pending_approval()?;
                if let Some(watcher) = interrupt_watcher.as_mut() {
                    watcher.resume()?;
                }
                if matches!(outcome, ApprovalResolutionOutcome::Deferred) {
                    break;
                }
            }
            if self.state.connection == ConnectionState::Disconnected {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        finalize_streaming_cells(&mut self.state);
        self.render_interactive_frame()?;
        Ok(())
    }

    fn continue_resumed_live_thread_if_needed(&mut self) -> Result<()> {
        self.continue_resumed_live_thread_if_needed_with_interrupts(true)
    }

    fn continue_resumed_live_thread_if_needed_with_interrupts(
        &mut self,
        enable_interrupts: bool,
    ) -> Result<()> {
        if !matches!(
            self.state.connection,
            ConnectionState::Streaming | ConnectionState::WaitingApproval
        ) {
            return Ok(());
        }
        if self.state.connection == ConnectionState::Streaming {
            mark_latest_streaming_cells_live(&mut self.state);
        }
        self.drain_notifications_until_ready_with_interrupts(enable_interrupts)
    }

    fn retry_pending_approval(&mut self) -> Result<()> {
        match self.resolve_pending_approval()? {
            ApprovalResolutionOutcome::Resolved => {
                if self.state.connection == ConnectionState::Streaming {
                    self.drain_notifications_until_ready()
                } else {
                    Ok(())
                }
            }
            ApprovalResolutionOutcome::Deferred | ApprovalResolutionOutcome::NoPending => Ok(()),
        }
    }

    fn resolve_pending_approval(&mut self) -> Result<ApprovalResolutionOutcome> {
        let Some(approval) = self.state.approvals.pending.clone() else {
            self.state.connection = ConnectionState::Streaming;
            return Ok(ApprovalResolutionOutcome::NoPending);
        };
        println!("\nApproval requested: {}", approval.title);
        if !approval.description.is_empty() {
            println!("{}", approval.description);
        }
        let (decision, answers) = collect_approval_response(&approval)?;
        let request = ApprovalRespondParams {
            approval_id: approval.id.clone(),
            decision,
            answers,
        };
        self.submit_approval_response(&approval, request)
    }

    pub(crate) fn submit_approval_response(
        &mut self,
        approval: &mli_types::PendingApproval,
        request: ApprovalRespondParams,
    ) -> Result<ApprovalResolutionOutcome> {
        match self
            .client
            .request::<_, mli_protocol::ApprovalRespondResult>("approval/respond", &request)
        {
            Ok(_) => {
                self.state.approvals.pending = None;
                self.state.connection = ConnectionState::Streaming;
                self.push_status(&format!(
                    "Approval {}: {}",
                    approval.id,
                    describe_approval_decision(&request.decision)
                ));
                Ok(ApprovalResolutionOutcome::Resolved)
            }
            Err(error) => {
                self.state.approvals.pending = Some(approval.clone());
                self.state.connection = ConnectionState::WaitingApproval;
                self.push_error(&format!(
                    "Failed to send approval {}: {error}. Use /approval to retry.",
                    approval.id
                ));
                Ok(ApprovalResolutionOutcome::Deferred)
            }
        }
    }

    pub(crate) fn request_interrupt(&mut self) -> Result<()> {
        let Some(thread_id) = self.state.active_thread_id else {
            self.push_warning("No active thread to interrupt.");
            return Ok(());
        };
        let Some(turn_id) = self.active_turn_id else {
            self.push_warning("No active turn to interrupt.");
            return Ok(());
        };
        match self.client.request::<_, serde_json::Value>(
            "turn/interrupt",
            &TurnInterruptParams { thread_id, turn_id },
        ) {
            Ok(_) => self.push_status("Interrupt requested."),
            Err(error) => self.push_error(&format!(
                "Failed to send interrupt for turn {turn_id}: {error}"
            )),
        }
        Ok(())
    }

    pub(crate) fn refresh_threads(&mut self) -> Result<()> {
        let result: mli_protocol::ThreadListResult =
            self.client.request("thread/list", &serde_json::json!({}))?;
        let active_thread_id = self.state.active_thread_id;
        self.state.threads = result
            .threads
            .into_iter()
            .map(|thread| ThreadListItem {
                selected: active_thread_id == Some(thread.id),
                thread,
            })
            .collect();
        Ok(())
    }

    fn pick_thread(&mut self) -> Result<()> {
        self.refresh_threads()?;
        if self.state.threads.is_empty() {
            self.push_warning("No threads found.");
            return Ok(());
        }
        let query = match prompt_picker_filter("threads")? {
            PickerFilter::All => None,
            PickerFilter::Query(query) => Some(query),
            PickerFilter::Cancel => return Ok(()),
        };
        let filtered_threads = filter_threads(&self.state.threads, query.as_deref());
        if filtered_threads.is_empty() {
            self.push_warning(&format!(
                "No threads matched `{}`.",
                query.as_deref().unwrap_or_default()
            ));
            return Ok(());
        }
        println!("Threads:");
        for (index, item) in filtered_threads.iter().enumerate() {
            println!(
                "  {}. {} ({:?})",
                index + 1,
                item.thread
                    .title
                    .clone()
                    .unwrap_or_else(|| item.thread.id.to_string()),
                item.thread.status
            );
        }
        print!("Select thread number (blank to cancel): ");
        io::stdout()
            .flush()
            .context("failed to flush thread picker")?;
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .context("failed to read thread selection")?;
        let line = line.trim();
        if line.is_empty() {
            return Ok(());
        }
        let index: usize = line.parse().context("invalid thread selection")?;
        let thread_id = filtered_threads
            .get(index.saturating_sub(1))
            .map(|item| item.thread.id)
            .ok_or_else(|| anyhow!("thread selection out of range"))?;
        self.resume_thread_into_view(thread_id)
    }

    fn restore_transcript_from_file(&mut self, path: &Path) -> Result<()> {
        reset_restored_transcript_state(&mut self.state);
        if !path.exists() {
            return Ok(());
        }
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(error) => {
                self.push_warning(&format!(
                    "Failed to read transcript {}: {error}",
                    path.display()
                ));
                return Ok(());
            }
        };
        replay_transcript_from_raw(&mut self.state, path, &raw);
        finalize_restored_transcript(&mut self.state);
        Ok(())
    }

    pub(crate) fn resume_thread_into_view(
        &mut self,
        thread_id: mli_types::LocalThreadId,
    ) -> Result<()> {
        self.resume_thread_into_view_no_follow(thread_id)?;
        self.continue_resumed_live_thread_if_needed()
    }

    pub(crate) fn resume_thread_into_view_no_follow(
        &mut self,
        thread_id: mli_types::LocalThreadId,
    ) -> Result<()> {
        self.set_active_thread(thread_id);
        match self.client.request::<_, mli_protocol::ThreadResumeResult>(
            "thread/resume",
            &ThreadResumeParams { thread_id },
        ) {
            Ok(result) => {
                self.set_active_thread(result.thread.id);
                let details = self.read_thread_details(result.thread.id)?;
                self.restore_thread_details(&details)?;
                self.active_turn_id = restored_active_turn_id(&details.turns);
                self.sync_connection_from_thread_status(&details.thread.status);
                Ok(())
            }
            Err(error) => {
                let details = self.read_thread_details(thread_id)?;
                self.restore_thread_details(&details)?;
                self.active_turn_id = None;
                self.state.approvals.pending = None;
                self.state.connection = ConnectionState::Ready;
                self.push_warning(&format!(
                    "Resume failed: {error}. Loaded local transcript only."
                ));
                Ok(())
            }
        }
    }

    pub(crate) fn read_thread_details(
        &mut self,
        thread_id: mli_types::LocalThreadId,
    ) -> Result<mli_protocol::ThreadReadResult> {
        self.client
            .request("thread/read", &ThreadReadParams { thread_id })
    }

    pub(crate) fn restore_thread_details(
        &mut self,
        details: &mli_protocol::ThreadReadResult,
    ) -> Result<()> {
        self.restore_transcript_from_file(&details.thread.transcript_path)?;
        if self.state.transcript.history.is_empty() {
            for turn in &details.turns {
                self.push_status(&format!("Restored turn {} ({:?})", turn.id, turn.status));
            }
        }
        Ok(())
    }

    pub(crate) fn sync_connection_from_thread_status(&mut self, status: &mli_types::ThreadStatus) {
        self.state.connection = match status {
            mli_types::ThreadStatus::Running => ConnectionState::Streaming,
            mli_types::ThreadStatus::WaitingApproval if self.state.approvals.pending.is_some() => {
                ConnectionState::WaitingApproval
            }
            mli_types::ThreadStatus::NotLoaded
            | mli_types::ThreadStatus::Idle
            | mli_types::ThreadStatus::Interrupted
            | mli_types::ThreadStatus::Error
            | mli_types::ThreadStatus::Starting
            | mli_types::ThreadStatus::WaitingApproval => ConnectionState::Ready,
        };
    }

    fn pick_skill(&mut self) -> Result<()> {
        let result: mli_protocol::SkillsListResult = self.client.request(
            "skills/list",
            &SkillsListParams {
                cwd: None,
                force_reload: Some(false),
            },
        )?;
        if result.skills.is_empty() {
            self.push_warning("No skills available.");
            return Ok(());
        }
        let query = match prompt_picker_filter("skills")? {
            PickerFilter::All => None,
            PickerFilter::Query(query) => Some(query),
            PickerFilter::Cancel => return Ok(()),
        };
        let filtered_skills = filter_skills(&result.skills, query.as_deref());
        if filtered_skills.is_empty() {
            self.push_warning(&format!(
                "No skills matched `{}`.",
                query.as_deref().unwrap_or_default()
            ));
            return Ok(());
        }
        println!("Skills:");
        for (index, skill) in filtered_skills.iter().enumerate() {
            println!("  {}. {}", index + 1, describe_skill(skill));
            println!("     {}", skill.description);
        }
        print!("Select skill number (blank to clear/cancel): ");
        io::stdout()
            .flush()
            .context("failed to flush skill picker")?;
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .context("failed to read skill selection")?;
        let line = line.trim();
        if line.is_empty() {
            self.selected_skill = None;
            self.push_status("Cleared selected skill.");
            return Ok(());
        }
        let index: usize = line.parse().context("invalid skill selection")?;
        let skill = filtered_skills
            .get(index.saturating_sub(1))
            .cloned()
            .cloned()
            .ok_or_else(|| anyhow!("skill selection out of range"))?;
        self.push_status(&format!("Selected skill: {}", selected_skill_label(&skill)));
        self.selected_skill = Some(skill);
        Ok(())
    }

    fn browse_artifacts(&mut self) -> Result<()> {
        let result: mli_protocol::ArtifactListResult = self.client.request(
            "artifact/list",
            &ArtifactListParams {
                thread_id: self.state.active_thread_id,
                kind: None,
                limit: Some(20),
            },
        )?;
        if result.artifacts.is_empty() {
            self.push_warning("No artifacts found.");
            return Ok(());
        }
        let query = match prompt_picker_filter("artifacts")? {
            PickerFilter::All => None,
            PickerFilter::Query(query) => Some(query),
            PickerFilter::Cancel => return Ok(()),
        };
        let filtered_artifacts = filter_artifacts(&result.artifacts, query.as_deref());
        if filtered_artifacts.is_empty() {
            self.push_warning(&format!(
                "No artifacts matched `{}`.",
                query.as_deref().unwrap_or_default()
            ));
            return Ok(());
        }
        println!("Artifacts:");
        for (index, artifact) in filtered_artifacts.iter().enumerate() {
            println!(
                "  {}. {} ({})",
                index + 1,
                artifact.title,
                artifact.primary_path.display()
            );
        }
        print!("Select artifact number to view (blank to cancel): ");
        io::stdout()
            .flush()
            .context("failed to flush artifact picker")?;
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .context("failed to read artifact selection")?;
        let line = line.trim();
        if line.is_empty() {
            return Ok(());
        }
        let index: usize = line.parse().context("invalid artifact selection")?;
        let artifact = filtered_artifacts
            .get(index.saturating_sub(1))
            .ok_or_else(|| anyhow!("artifact selection out of range"))?;
        let payload: mli_protocol::ArtifactReadResult = self.client.request(
            "artifact/read",
            &ArtifactReadParams {
                artifact_id: artifact.id,
            },
        )?;
        self.view_artifact(payload)
    }

    fn view_artifact(&mut self, payload: mli_protocol::ArtifactReadResult) -> Result<()> {
        if payload.files.is_empty() {
            self.push_warning("Selected artifact has no readable files.");
            return Ok(());
        }
        let mut selected_file = preferred_artifact_file_index(&payload);
        loop {
            let file = payload
                .files
                .get(selected_file)
                .ok_or_else(|| anyhow!("artifact file selection out of range"))?;
            println!("\n=== {} ===", payload.manifest.title);
            println!(
                "File {}/{}: {} ({})",
                selected_file + 1,
                payload.files.len(),
                file.path.display(),
                file.media_type
            );
            println!("----------------------------------------");
            print_artifact_file(file);
            if payload.files.len() == 1 {
                println!("=== end artifact ===\n");
                break;
            }
            println!("\nFiles:");
            for (index, candidate) in payload.files.iter().enumerate() {
                let marker = if index == selected_file { "*" } else { " " };
                println!(
                    "  {} {}. {} ({}){}",
                    marker,
                    index + 1,
                    candidate.path.display(),
                    candidate.media_type,
                    candidate
                        .read_error
                        .as_ref()
                        .map(|_| " [read error]")
                        .unwrap_or("")
                );
            }
            print!("Select another file number (blank/q to close): ");
            io::stdout()
                .flush()
                .context("failed to flush artifact viewer")?;
            let mut line = String::new();
            io::stdin()
                .read_line(&mut line)
                .context("failed to read artifact viewer selection")?;
            let line = line.trim();
            if line.is_empty() || line.eq_ignore_ascii_case("q") {
                println!("=== end artifact ===\n");
                break;
            }
            let index: usize = line.parse().context("invalid artifact file selection")?;
            selected_file = index
                .checked_sub(1)
                .filter(|index| *index < payload.files.len())
                .ok_or_else(|| anyhow!("artifact file selection out of range"))?;
        }
        Ok(())
    }

    pub(crate) fn apply_notification(&mut self, notification: ServerNotification) -> Result<()> {
        match notification {
            ServerNotification::ThreadStarted { params } => {
                self.set_active_thread(params.thread.id);
                self.refresh_threads()?;
            }
            ServerNotification::ThreadStatusChanged { params } => {
                self.set_active_thread(params.thread.id);
                self.refresh_threads()?;
                if !matches!(
                    params.thread.status,
                    mli_types::ThreadStatus::WaitingApproval
                ) {
                    self.state.approvals.pending = None;
                    if self.state.connection == ConnectionState::WaitingApproval {
                        self.state.connection = match params.thread.status {
                            mli_types::ThreadStatus::Running => ConnectionState::Streaming,
                            _ => ConnectionState::Ready,
                        };
                    }
                }
            }
            ServerNotification::TurnStarted { params } => {
                self.active_turn_id = Some(params.turn.id);
                self.state.connection = ConnectionState::Streaming;
            }
            ServerNotification::TurnCompleted { params: _ } => {
                self.active_turn_id = None;
                self.state.approvals.pending = None;
                self.state.connection = ConnectionState::Ready;
                finalize_streaming_cells(&mut self.state);
            }
            ServerNotification::PlanUpdated { params } => {
                self.state
                    .transcript
                    .history
                    .push(HistoryCellModel::PlanUpdate(mli_types::PlanUpdateCell {
                        summary: params.summary,
                    }));
            }
            ServerNotification::ItemStarted { params } => {
                let _ = push_projected_item_cells(
                    &mut self.state,
                    &params.item,
                    ProjectedItemPhase::Started,
                );
            }
            ServerNotification::ItemCompleted { params } => {
                let _ = push_projected_item_cells(
                    &mut self.state,
                    &params.item,
                    ProjectedItemPhase::Completed,
                );
            }
            ServerNotification::AgentMessageDelta { params } => {
                match self.state.transcript.history.last_mut() {
                    Some(HistoryCellModel::AssistantMessage(cell)) if cell.streaming => {
                        cell.text.push_str(&params.delta);
                    }
                    _ => self
                        .state
                        .transcript
                        .history
                        .push(HistoryCellModel::AssistantMessage(AssistantMessageCell {
                            text: params.delta,
                            streaming: true,
                        })),
                }
            }
            ServerNotification::CommandExecutionOutputDelta { params } => {
                append_command_output_delta(&mut self.state, &params.item_id, &params.delta);
            }
            ServerNotification::ArtifactCreated { params } => {
                self.state.artifacts.manifests.push(params.manifest.clone());
                push_artifact_event_cell(&mut self.state, params.manifest, params.preview, false);
            }
            ServerNotification::ArtifactUpdated { params } => {
                self.state
                    .artifacts
                    .manifests
                    .retain(|artifact| artifact.id != params.manifest.id);
                self.state.artifacts.manifests.push(params.manifest.clone());
                push_artifact_event_cell(&mut self.state, params.manifest, params.preview, true);
            }
            ServerNotification::RuntimeStatusChanged { params } => {
                let _ = params;
            }
            ServerNotification::SkillsChanged { params } => {
                self.push_status(&format!("Skills changed ({} total)", params.skills.len()));
            }
            ServerNotification::ApprovalRequested { params } => {
                self.state.approvals.pending = Some(params.approval.clone());
                self.state
                    .transcript
                    .history
                    .push(HistoryCellModel::ApprovalRequest(ApprovalCell {
                        approval: params.approval,
                    }));
                self.state.connection = ConnectionState::WaitingApproval;
            }
            ServerNotification::Warning { params } => {
                self.state
                    .transcript
                    .history
                    .push(HistoryCellModel::Warning(WarningCell {
                        message: params.message,
                    }));
            }
            ServerNotification::Error { params } => {
                self.active_turn_id = None;
                self.state.approvals.pending = None;
                self.state
                    .transcript
                    .history
                    .push(HistoryCellModel::Error(ErrorCell {
                        message: params.message,
                    }));
                self.state.connection = ConnectionState::Ready;
            }
        }
        if self.state.connection == ConnectionState::Ready
            && let Some(cell) = latest_assistant_message_mut(&mut self.state)
        {
            cell.streaming = false;
        }
        Ok(())
    }

    pub(crate) fn set_active_thread(&mut self, thread_id: mli_types::LocalThreadId) {
        self.state.active_thread_id = Some(thread_id);
        for item in &mut self.state.threads {
            item.selected = item.thread.id == thread_id;
        }
    }

    pub(crate) fn push_status(&mut self, message: &str) {
        push_status_cell(&mut self.state, message);
    }

    pub(crate) fn push_warning(&mut self, message: &str) {
        push_warning_cell(&mut self.state, message);
    }

    pub(crate) fn push_error(&mut self, message: &str) {
        push_error_cell(&mut self.state, message);
    }

    pub(crate) fn state(&self) -> &AppState {
        &self.state
    }

    pub(crate) fn state_mut(&mut self) -> &mut AppState {
        &mut self.state
    }

    pub(crate) fn selected_skill(&self) -> Option<&SkillDescriptor> {
        self.selected_skill.as_ref()
    }

    pub(crate) fn set_selected_skill(&mut self, skill: Option<SkillDescriptor>) {
        self.selected_skill = skill;
    }

    pub(crate) fn clear_transcript(&mut self) {
        self.state.transcript.history.clear();
    }

    #[allow(dead_code)]
    pub(crate) fn active_turn_id(&self) -> Option<mli_types::LocalTurnId> {
        self.active_turn_id
    }

    pub(crate) fn poll_notification(&mut self) -> Result<Option<ServerNotification>> {
        self.client.recv_notification()
    }

    pub(crate) fn request_skills(&mut self) -> Result<Vec<SkillDescriptor>> {
        let result: mli_protocol::SkillsListResult = self.client.request(
            "skills/list",
            &SkillsListParams {
                cwd: None,
                force_reload: Some(false),
            },
        )?;
        Ok(result.skills)
    }

    pub(crate) fn request_artifacts(&mut self) -> Result<Vec<ArtifactManifest>> {
        let result: mli_protocol::ArtifactListResult = self.client.request(
            "artifact/list",
            &ArtifactListParams {
                thread_id: self.state.active_thread_id,
                kind: None,
                limit: Some(20),
            },
        )?;
        Ok(result.artifacts)
    }

    fn request_config(&mut self) -> Result<AppConfig> {
        let result: ConfigReadResult = self
            .client
            .request("config/read", &serde_json::json!({}))?;
        serde_json::from_value(result.config).context("failed to decode app config")
    }

    fn write_config(&mut self, config: &AppConfig) -> Result<AppConfig> {
        let result: ConfigWriteResult = self.client.request(
            "config/write",
            &ConfigWriteParams {
                config: serde_json::to_value(config)
                    .context("failed to encode config for write")?,
            },
        )?;
        serde_json::from_value(result.config).context("failed to decode written app config")
    }

    fn apply_mode_to_banner(&mut self, approval_policy: ApprovalPolicy, sandbox_mode: SandboxMode) {
        self.state.runtime.approval_policy = Some(approval_policy);
        self.state.runtime.sandbox_mode = Some(sandbox_mode);
    }

    fn set_mode(&mut self, approval_policy: ApprovalPolicy, sandbox_mode: SandboxMode) -> Result<()> {
        let mut config = self.request_config()?;
        config.codex.approval_policy = approval_policy;
        config.codex.sandbox_mode = sandbox_mode;
        let persisted = self.write_config(&config)?;
        self.apply_mode_to_banner(
            persisted.codex.approval_policy,
            persisted.codex.sandbox_mode,
        );
        let mode_label = runtime_mode_label(approval_policy, sandbox_mode);
        self.push_status(&format!(
            "Default mode set to {mode_label} for new threads ({}/{})",
            approval_policy_label(approval_policy),
            sandbox_mode_label(sandbox_mode)
        ));
        Ok(())
    }

    pub(crate) fn toggle_yolo_mode(&mut self) -> Result<()> {
        let current_policy = self
            .state
            .runtime
            .approval_policy
            .unwrap_or(ApprovalPolicy::OnRequest);
        let current_sandbox = self
            .state
            .runtime
            .sandbox_mode
            .unwrap_or(SandboxMode::WorkspaceWrite);
        if is_yolo_mode(current_policy, current_sandbox) {
            self.set_mode(ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite)
        } else {
            self.set_mode(ApprovalPolicy::Never, SandboxMode::DangerFullAccess)
        }
    }

    pub(crate) fn set_mode_command(&mut self, command: &str) -> Result<()> {
        let mut parts = command.split_whitespace();
        let _ = parts.next();
        let Some(mode) = parts.next() else {
            self.push_warning("Usage: /mode safe | /mode readonly | /mode yolo");
            return Ok(());
        };
        if parts.next().is_some() {
            self.push_warning("Usage: /mode safe | /mode readonly | /mode yolo");
            return Ok(());
        }
        match mode {
            "safe" => self.set_mode(ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite),
            "readonly" | "read-only" => {
                self.set_mode(ApprovalPolicy::OnRequest, SandboxMode::ReadOnly)
            }
            "yolo" => self.set_mode(ApprovalPolicy::Never, SandboxMode::DangerFullAccess),
            other => {
                self.push_warning(&format!(
                    "Unknown mode `{other}`. Use: safe, readonly, yolo"
                ));
                Ok(())
            }
        }
    }

    #[allow(dead_code)] // used by forthcoming artifact viewer overlay
    pub(crate) fn read_artifact(
        &mut self,
        artifact_id: mli_types::ArtifactId,
    ) -> Result<mli_protocol::ArtifactReadResult> {
        self.client
            .request("artifact/read", &ArtifactReadParams { artifact_id })
    }

    fn rendered_frame(&self) -> Option<String> {
        self.interactive_rendering.then(|| {
            format!(
                "\x1b[2J\x1b[H{}",
                render_app(
                    &self.state,
                    self.selected_skill
                        .as_ref()
                        .map(|skill| skill.name.as_str())
                )
            )
        })
    }

    fn render_interactive_frame(&self) -> Result<()> {
        let Some(frame) = self.rendered_frame() else {
            return Ok(());
        };
        print!("{frame}");
        io::stdout()
            .flush()
            .context("failed to flush interactive transcript")
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalQuestionPayload {
    pub(crate) questions: Vec<ApprovalQuestion>,
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalQuestion {
    pub(crate) id: String,
    pub(crate) header: String,
    pub(crate) question: String,
    #[serde(default)]
    pub(crate) is_other: bool,
    #[serde(default)]
    pub(crate) is_secret: bool,
    #[serde(default)]
    pub(crate) options: Option<Vec<ApprovalQuestionOption>>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub(crate) struct ApprovalQuestionOption {
    pub(crate) label: String,
    pub(crate) description: String,
}

#[derive(Clone, Copy)]
enum ProjectedItemPhase {
    Started,
    Completed,
}

#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ProjectableThreadItem {
    CommandExecution {
        id: String,
        command: String,
        #[serde(default, rename = "aggregatedOutput")]
        aggregated_output: Option<String>,
        #[serde(default)]
        status: Option<String>,
    },
    FileChange {
        changes: Vec<ProjectedFileChange>,
        #[serde(default)]
        status: Option<String>,
    },
    Plan {
        text: String,
    },
}

#[derive(serde::Deserialize)]
struct ProjectedFileChange {
    path: PathBuf,
    kind: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoredPlanUpdatePayload {
    #[serde(default)]
    explanation: Option<String>,
    plan: Vec<RestoredPlanStep>,
}

#[derive(serde::Deserialize)]
struct RestoredPlanStep {
    step: String,
    status: RestoredPlanStepStatus,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum RestoredPlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

fn collect_approval_response(
    approval: &mli_types::PendingApproval,
) -> Result<(ApprovalDecision, Option<BTreeMap<String, ApprovalAnswer>>)> {
    match approval.kind {
        ApprovalKind::RequestUserInput => collect_request_user_input_response(approval),
        _ => {
            let approved = prompt_yes_no("Approve? [y/N]: ")?;
            let decision = if approved {
                ApprovalDecision::Approve
            } else {
                ApprovalDecision::Reject
            };
            Ok((decision, None))
        }
    }
}

fn collect_request_user_input_response(
    approval: &mli_types::PendingApproval,
) -> Result<(ApprovalDecision, Option<BTreeMap<String, ApprovalAnswer>>)> {
    collect_request_user_input_response_with(approval, prompt_line)
}

fn collect_request_user_input_response_with<F>(
    approval: &mli_types::PendingApproval,
    mut prompt: F,
) -> Result<(ApprovalDecision, Option<BTreeMap<String, ApprovalAnswer>>)>
where
    F: FnMut(&str) -> Result<String>,
{
    let payload: ApprovalQuestionPayload = serde_json::from_value(approval.raw_payload.clone())
        .context("failed to decode request_user_input payload")?;
    if payload.questions.is_empty() {
        return Ok((ApprovalDecision::Reject, None));
    }
    let mut answers = BTreeMap::new();
    for question in payload.questions {
        let answer = prompt_for_question_with(&question, &mut prompt)?;
        let Some(answer) = answer else {
            return Ok((ApprovalDecision::Reject, None));
        };
        answers.insert(
            question.id,
            ApprovalAnswer {
                answers: vec![answer],
            },
        );
    }
    Ok((ApprovalDecision::Approve, Some(answers)))
}

fn prompt_for_question_with<F>(
    question: &ApprovalQuestion,
    prompt: &mut F,
) -> Result<Option<String>>
where
    F: FnMut(&str) -> Result<String>,
{
    println!("\n{}: {}", question.header, question.question);
    if question.is_secret {
        println!("(secret input; terminal echo is still enabled in this minimal client)");
    }
    if let Some(options) = &question.options {
        for (index, option) in options.iter().enumerate() {
            println!("  {}. {} - {}", index + 1, option.label, option.description);
        }
        if question.is_other {
            println!("  or enter free-form text");
        }
    }
    loop {
        let raw = prompt("Answer (blank to cancel): ")?;
        if raw.is_empty() {
            return Ok(None);
        }
        if let Some(options) = &question.options {
            if let Ok(index) = raw.parse::<usize>() {
                if let Some(option) = options.get(index.saturating_sub(1)) {
                    return Ok(Some(option.label.clone()));
                }
                println!("Selection out of range.");
                continue;
            }
            if !question.is_other {
                println!("Enter one of the listed option numbers.");
                continue;
            }
        }
        return Ok(Some(raw));
    }
}

fn prompt_yes_no(prompt: &str) -> Result<bool> {
    loop {
        let raw = prompt_line(prompt)?;
        match raw.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "" | "n" | "no" => return Ok(false),
            _ => println!("Please enter y or n."),
        }
    }
}

fn prompt_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush().context("failed to flush prompt")?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("failed to read approval input")?;
    Ok(line.trim().to_owned())
}

fn describe_approval_decision(decision: &ApprovalDecision) -> &'static str {
    match decision {
        ApprovalDecision::Approve => "approved",
        ApprovalDecision::Reject => "rejected",
    }
}

struct InterruptWatcher {
    tty_mode: TtyModeGuard,
    read_enabled: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    rx: Receiver<InterruptKey>,
    reader_thread: Option<thread::JoinHandle<()>>,
}

impl InterruptWatcher {
    fn new() -> Result<Self> {
        let tty_mode = TtyModeGuard::new()?;
        let mut tty = open_terminal_input()?;
        let read_enabled = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let stop_reader = Arc::clone(&stop);
        let read_enabled_reader = Arc::clone(&read_enabled);
        let reader_thread = thread::spawn(move || {
            let mut buffer = [0_u8; 1];
            while !stop_reader.load(Ordering::Relaxed) {
                if !read_enabled_reader.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(25));
                    continue;
                }
                match tty.read(&mut buffer) {
                    Ok(0) => continue,
                    Ok(_) => match buffer[0] {
                        27 => {
                            let _ = tx.send(InterruptKey::Escape);
                        }
                        3 => {
                            let _ = tx.send(InterruptKey::CtrlC);
                        }
                        _ => {}
                    },
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            tty_mode,
            read_enabled,
            stop,
            rx,
            reader_thread: Some(reader_thread),
        })
    }

    fn poll_interrupt(&self) -> bool {
        matches!(
            self.rx.try_recv(),
            Ok(InterruptKey::Escape) | Ok(InterruptKey::CtrlC)
        )
    }

    fn suspend(&mut self) -> Result<()> {
        self.read_enabled.store(false, Ordering::Relaxed);
        self.tty_mode.suspend()
    }

    fn resume(&mut self) -> Result<()> {
        self.tty_mode.resume()?;
        self.read_enabled.store(true, Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for InterruptWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(reader_thread) = self.reader_thread.take() {
            let _ = reader_thread.join();
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum InterruptKey {
    Escape,
    CtrlC,
}

struct TtyModeGuard {
    original_state: String,
    raw_mode_enabled: bool,
}

impl TtyModeGuard {
    fn new() -> Result<Self> {
        let original_state = capture_tty_state()?;
        set_tty_raw_mode()?;
        Ok(Self {
            original_state,
            raw_mode_enabled: true,
        })
    }

    fn suspend(&mut self) -> Result<()> {
        if self.raw_mode_enabled {
            restore_tty_state(&self.original_state)?;
            self.raw_mode_enabled = false;
        }
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        if !self.raw_mode_enabled {
            set_tty_raw_mode()?;
            self.raw_mode_enabled = true;
        }
        Ok(())
    }
}

impl Drop for TtyModeGuard {
    fn drop(&mut self) {
        if self.raw_mode_enabled {
            let _ = restore_tty_state(&self.original_state);
        }
    }
}

fn capture_tty_state() -> Result<String> {
    let output = Command::new("stty")
        .arg("-g")
        .stdin(tty_stdio()?)
        .output()
        .context("failed to capture terminal state with stty -g")?;
    if !output.status.success() {
        return Err(anyhow!("stty -g exited with status {}", output.status));
    }
    String::from_utf8(output.stdout)
        .map(|state| state.trim().to_owned())
        .context("stty -g output is not utf-8")
}

fn set_tty_raw_mode() -> Result<()> {
    run_stty(["raw", "-echo", "min", "0", "time", "1"])
}

fn restore_tty_state(state: &str) -> Result<()> {
    run_stty([state])
}

fn run_stty<const N: usize>(args: [&str; N]) -> Result<()> {
    let status = Command::new("stty")
        .args(args)
        .stdin(tty_stdio()?)
        .status()
        .context("failed to run stty")?;
    if !status.success() {
        return Err(anyhow!("stty exited with status {}", status));
    }
    Ok(())
}

fn tty_stdio() -> Result<Stdio> {
    Ok(Stdio::from(open_terminal_input()?))
}

fn stdio_supports_interrupt_watcher() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn open_terminal_input() -> Result<fs::File> {
    match fs::File::open("/dev/tty") {
        Ok(file) => Ok(file),
        Err(tty_error) => fs::File::open("/dev/stdin").with_context(|| {
            format!("failed to open /dev/tty ({tty_error}) or fall back to /dev/stdin")
        }),
    }
}

fn spawn_reader_thread(stdout: impl std::io::Read + Send + 'static, tx: Sender<ClientMessage>) {
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else {
                let _ = tx.send(ClientMessage::Ignored);
                break;
            };
            if line.trim().is_empty() {
                continue;
            }
            let message = match serde_json::from_str::<JsonRpcMessage>(&line) {
                Ok(message) => message,
                Err(_) => {
                    let _ = tx.send(ClientMessage::Ignored);
                    continue;
                }
            };
            let client_message = match message {
                JsonRpcMessage::Response(response) => ClientMessage::Response(response),
                JsonRpcMessage::Error(error) => ClientMessage::Error(error),
                JsonRpcMessage::Notification(notification) => {
                    match decode_notification(notification) {
                        Ok(Some(notification)) => {
                            ClientMessage::Notification(Box::new(notification))
                        }
                        Ok(None) => ClientMessage::Ignored,
                        Err(_) => ClientMessage::Ignored,
                    }
                }
                JsonRpcMessage::Request(_) => ClientMessage::Ignored,
            };
            let _ = tx.send(client_message);
        }
    });
}

fn spawn_stderr_reader(stderr: impl Read + Send + 'static, sink: Arc<Mutex<String>>) {
    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            let Ok(line) = line else {
                break;
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(mut buffer) = sink.lock() {
                if !buffer.is_empty() {
                    buffer.push('\n');
                }
                buffer.push_str(trimmed);
            }
        }
    });
}

fn decode_notification(notification: JsonRpcNotification) -> Result<Option<ServerNotification>> {
    let method = notification.method.clone();
    let params = notification.params.unwrap_or(serde_json::Value::Null);
    match method.as_str() {
        "thread/started"
        | "thread/statusChanged"
        | "turn/started"
        | "turn/completed"
        | "turn/plan/updated"
        | "item/started"
        | "item/completed"
        | "item/agentMessage/delta"
        | "item/commandExecution/outputDelta"
        | "runtime/statusChanged"
        | "skills/changed"
        | "artifact/created"
        | "artifact/updated"
        | "approval/requested"
        | "warning"
        | "error" => {
            let value = serde_json::json!({ "method": method, "params": params });
            Ok(Some(
                serde_json::from_value(value).context("failed to decode server notification")?,
            ))
        }
        _ => Ok(None),
    }
}

pub fn run_line_mode_tui(app_server_bin: Option<PathBuf>) -> Result<()> {
    let client = AppClient::spawn(app_server_bin)?;
    let mut app = TranscriptApp::new(client);
    app.run()
}

pub fn run_default_tui(app_server_bin: Option<PathBuf>) -> Result<()> {
    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        return crate::inline_tui::run_inline_tui(app_server_bin);
    }
    run_line_mode_tui(app_server_bin)
}

fn compact_json(value: &serde_json::Value) -> String {
    let raw = value.to_string();
    if raw.len() > 120 {
        format!("{}...", &raw[..120])
    } else {
        raw
    }
}

fn reset_restored_transcript_state(state: &mut AppState) {
    state.transcript.history.clear();
    state.artifacts.manifests.clear();
    state.approvals.pending = None;
}

fn replay_transcript_from_raw(state: &mut AppState, path: &Path, raw: &str) {
    for (line_index, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event = match serde_json::from_str::<mli_types::TranscriptEvent>(line) {
            Ok(event) => event,
            Err(error) => {
                push_warning_cell(
                    state,
                    &format!(
                        "Skipped corrupt transcript line {} in {}: {error}",
                        line_index + 1,
                        path.display()
                    ),
                );
                continue;
            }
        };
        match event.source {
            mli_types::TranscriptEventSource::User => {
                let summary = event
                    .payload
                    .get("input")
                    .cloned()
                    .and_then(|value| serde_json::from_value::<Vec<UserInput>>(value).ok())
                    .map(|inputs| UserInput::summary(&inputs))
                    .unwrap_or_else(|| "<user input>".to_owned());
                state
                    .transcript
                    .history
                    .push(HistoryCellModel::UserMessage(UserMessageCell {
                        text: summary,
                    }));
            }
            mli_types::TranscriptEventSource::UpstreamCodex => {
                restore_upstream_event(state, &event.payload)
            }
            mli_types::TranscriptEventSource::Wrapper => {
                restore_wrapper_event(state, &event.payload)
            }
            mli_types::TranscriptEventSource::ArtifactSystem => {
                restore_artifact_event(state, &event.payload)
            }
        }
    }
}

fn finalize_restored_transcript(state: &mut AppState) {
    finalize_streaming_cells(state);
}

fn restore_upstream_event(state: &mut AppState, payload: &serde_json::Value) {
    if payload.get("event").and_then(|value| value.as_str()) == Some("plan_updated")
        && let Ok(plan) = serde_json::from_value::<RestoredPlanUpdatePayload>(payload.clone())
    {
        let summary = summarize_restored_plan_update(plan.explanation.as_deref(), &plan.plan);
        if !summary.is_empty() {
            state.transcript.history.push(HistoryCellModel::PlanUpdate(
                mli_types::PlanUpdateCell { summary },
            ));
            return;
        }
    }

    if payload.get("event").and_then(|value| value.as_str())
        == Some("command_execution_output_delta")
        && let (Some(item_id), Some(delta)) = (
            payload.get("item_id").and_then(|value| value.as_str()),
            payload.get("delta").and_then(|value| value.as_str()),
        )
    {
        append_command_output_delta(state, item_id, delta);
        return;
    }

    if let Some(delta) = payload.get("delta").and_then(|value| value.as_str()) {
        match state.transcript.history.last_mut() {
            Some(HistoryCellModel::AssistantMessage(cell)) => cell.text.push_str(delta),
            _ => state
                .transcript
                .history
                .push(HistoryCellModel::AssistantMessage(AssistantMessageCell {
                    text: delta.to_owned(),
                    streaming: false,
                })),
        }
        return;
    }

    if let Some(message) = payload
        .get("error")
        .and_then(|value| value.get("message"))
        .and_then(|value| value.as_str())
    {
        state
            .transcript
            .history
            .push(HistoryCellModel::Error(ErrorCell {
                message: message.to_owned(),
            }));
        return;
    }

    if let Some(status) = payload
        .get("turn")
        .and_then(|value| value.get("status"))
        .and_then(|value| value.as_str())
    {
        push_status_cell(state, &format!("Turn completed with status {status}"));
        return;
    }

    if let Some(item) = payload.get("item") {
        let phase = payload
            .get("event")
            .and_then(|value| value.as_str())
            .map(projected_item_phase_from_event)
            .unwrap_or_else(|| infer_projected_item_phase(item));
        if push_projected_item_cells(state, item, phase) || is_ignorable_transcript_item(item) {
            return;
        }
        return;
    }

    push_status_cell(
        state,
        &format!("restored upstream event: {}", compact_json(payload)),
    );
}

fn restore_wrapper_event(state: &mut AppState, payload: &serde_json::Value) {
    if let Some(approval) = payload.get("approval").cloned() {
        let value = serde_json::json!({
            "approval": approval,
        });
        if let Ok(params) =
            serde_json::from_value::<mli_protocol::ApprovalRequestNotification>(value)
        {
            state.approvals.pending = Some(params.approval.clone());
            state
                .transcript
                .history
                .push(HistoryCellModel::ApprovalRequest(ApprovalCell {
                    approval: params.approval,
                }));
            return;
        }
    }

    if payload.get("event").and_then(|value| value.as_str()) == Some("interrupt_requested") {
        push_status_cell(state, "Interrupt requested.");
        return;
    }

    if let Some(approval_id) = payload.get("approval_id").and_then(|value| value.as_str()) {
        if let Some(decision) = payload.get("decision").and_then(|value| value.as_str()) {
            if state
                .approvals
                .pending
                .as_ref()
                .is_some_and(|approval| approval.id == approval_id)
            {
                state.approvals.pending = None;
            }
            push_status_cell(
                state,
                &format!("restored approval {approval_id}: {decision}"),
            );
            return;
        }
        if payload
            .get("resolved")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            if state
                .approvals
                .pending
                .as_ref()
                .is_some_and(|approval| approval.id == approval_id)
            {
                state.approvals.pending = None;
            }
            push_status_cell(state, &format!("restored approval {approval_id} resolved"));
            return;
        }
    }

    if let Some(status) = payload.get("status").and_then(|value| value.as_str())
        && payload.get("transcript_path").is_some()
        && payload.get("artifact_root").is_some()
    {
        push_status_cell(state, &format!("Thread status: {status}"));
        return;
    }

    push_status_cell(
        state,
        &format!("restored wrapper event: {}", compact_json(payload)),
    );
}

fn restore_artifact_event(state: &mut AppState, payload: &serde_json::Value) {
    if let Ok(warning) =
        serde_json::from_value::<mli_protocol::WarningNotification>(payload.clone())
    {
        state
            .transcript
            .history
            .push(HistoryCellModel::Warning(WarningCell {
                message: warning.message,
            }));
        return;
    }

    if let Ok(notification) =
        serde_json::from_value::<mli_protocol::ArtifactCreatedNotification>(payload.clone())
    {
        let existed = record_restored_artifact(state, notification.manifest.clone());
        push_artifact_event_cell(state, notification.manifest, notification.preview, existed);
        return;
    }

    push_status_cell(
        state,
        &format!("restored artifact event: {}", compact_json(payload)),
    );
}

fn record_restored_artifact(state: &mut AppState, manifest: ArtifactManifest) -> bool {
    let existed = state
        .artifacts
        .manifests
        .iter()
        .any(|artifact| artifact.id == manifest.id);
    state
        .artifacts
        .manifests
        .retain(|artifact| artifact.id != manifest.id);
    state.artifacts.manifests.push(manifest);
    existed
}

fn projected_item_phase_from_event(event: &str) -> ProjectedItemPhase {
    match event {
        "item_started" => ProjectedItemPhase::Started,
        _ => ProjectedItemPhase::Completed,
    }
}

fn infer_projected_item_phase(item: &serde_json::Value) -> ProjectedItemPhase {
    if item
        .get("status")
        .and_then(|value| value.as_str())
        .is_some_and(|status| status.eq_ignore_ascii_case("inProgress"))
    {
        ProjectedItemPhase::Started
    } else {
        ProjectedItemPhase::Completed
    }
}

fn append_command_output_delta(state: &mut AppState, item_id: &str, delta: &str) {
    if delta.is_empty() {
        return;
    }
    if let Some(index) = find_exec_output_cell_index(state, item_id) {
        if let HistoryCellModel::ExecOutput(cell) = &mut state.transcript.history[index] {
            cell.output.push_str(delta);
            cell.streaming = true;
        }
        return;
    }
    state
        .transcript
        .history
        .push(HistoryCellModel::ExecOutput(mli_types::ExecOutputCell {
            item_id: item_id.to_owned(),
            command: latest_exec_command(state, item_id).unwrap_or_else(|| "<command>".to_owned()),
            output: delta.to_owned(),
            streaming: true,
        }));
}

fn complete_command_execution_cell(
    state: &mut AppState,
    item_id: String,
    command: String,
    aggregated_output: Option<String>,
    status: Option<String>,
) {
    if let Some(index) = find_exec_output_cell_index(state, &item_id) {
        if let HistoryCellModel::ExecOutput(cell) = &mut state.transcript.history[index] {
            cell.command = command;
            cell.output = merge_command_output(
                &cell.output,
                aggregated_output.as_deref(),
                status.as_deref(),
            );
            cell.streaming = false;
        }
        return;
    }
    state
        .transcript
        .history
        .push(HistoryCellModel::ExecOutput(mli_types::ExecOutputCell {
            item_id,
            command,
            output: summarize_command_output(aggregated_output, status.as_deref()),
            streaming: false,
        }));
}

fn merge_command_output(
    streamed_output: &str,
    aggregated_output: Option<&str>,
    status: Option<&str>,
) -> String {
    if streamed_output.is_empty() {
        return summarize_command_output(aggregated_output.map(|output| output.to_owned()), status);
    }
    if let Some(aggregated_output) = aggregated_output.filter(|output| !output.is_empty()) {
        if aggregated_output == streamed_output || aggregated_output.starts_with(streamed_output) {
            return aggregated_output.to_owned();
        }
        if streamed_output.starts_with(aggregated_output) {
            return streamed_output.to_owned();
        }
    }
    streamed_output.to_owned()
}

fn find_exec_output_cell_index(state: &AppState, item_id: &str) -> Option<usize> {
    state.transcript.history.iter().rposition(
        |cell| matches!(cell, HistoryCellModel::ExecOutput(cell) if cell.item_id == item_id),
    )
}

fn latest_exec_command(state: &AppState, item_id: &str) -> Option<String> {
    state
        .transcript
        .history
        .iter()
        .rev()
        .find_map(|cell| match cell {
            HistoryCellModel::ExecCommand(cell) if cell.item_id == item_id => {
                Some(cell.command.clone())
            }
            HistoryCellModel::ExecOutput(cell) if cell.item_id == item_id => {
                Some(cell.command.clone())
            }
            _ => None,
        })
}

fn push_projected_item_cells(
    state: &mut AppState,
    item: &serde_json::Value,
    phase: ProjectedItemPhase,
) -> bool {
    let Ok(item) = serde_json::from_value::<ProjectableThreadItem>(item.clone()) else {
        return false;
    };
    match (phase, item) {
        (
            ProjectedItemPhase::Started,
            ProjectableThreadItem::CommandExecution { id, command, .. },
        ) => {
            state.transcript.history.push(HistoryCellModel::ExecCommand(
                mli_types::ExecCommandCell {
                    item_id: id,
                    command,
                },
            ));
            true
        }
        (
            ProjectedItemPhase::Completed,
            ProjectableThreadItem::CommandExecution {
                id,
                command,
                aggregated_output,
                status,
            },
        ) => {
            complete_command_execution_cell(state, id, command, aggregated_output, status);
            true
        }
        (ProjectedItemPhase::Completed, ProjectableThreadItem::FileChange { changes, status }) => {
            state
                .transcript
                .history
                .push(HistoryCellModel::PatchSummary(
                    mli_types::PatchSummaryCell {
                        summary: summarize_file_change(&changes, status.as_deref()),
                    },
                ));
            true
        }
        (ProjectedItemPhase::Started, ProjectableThreadItem::FileChange { changes, status }) => {
            state
                .transcript
                .history
                .push(HistoryCellModel::PatchSummary(
                    mli_types::PatchSummaryCell {
                        summary: summarize_file_change(&changes, status.as_deref()),
                    },
                ));
            true
        }
        (ProjectedItemPhase::Completed, ProjectableThreadItem::Plan { text }) => {
            let summary = text.trim();
            if summary.is_empty() {
                return false;
            }
            state.transcript.history.push(HistoryCellModel::PlanUpdate(
                mli_types::PlanUpdateCell {
                    summary: summary.to_owned(),
                },
            ));
            true
        }
        _ => false,
    }
}

fn is_ignorable_transcript_item(item: &serde_json::Value) -> bool {
    matches!(
        item.get("type").and_then(|value| value.as_str()),
        Some("userMessage" | "agentMessage")
    )
}

fn summarize_command_output(aggregated_output: Option<String>, status: Option<&str>) -> String {
    match aggregated_output {
        Some(output) if !output.trim().is_empty() => output,
        _ => match status {
            Some(status) => format!("<no output> ({status})"),
            None => "<no output>".to_owned(),
        },
    }
}

fn summarize_file_change(changes: &[ProjectedFileChange], status: Option<&str>) -> String {
    let mut rendered_paths = changes
        .iter()
        .take(3)
        .map(|change| format!("{} ({})", change.path.display(), change.kind))
        .collect::<Vec<_>>();
    if changes.len() > rendered_paths.len() {
        rendered_paths.push(format!("+{} more", changes.len() - rendered_paths.len()));
    }
    let details = if rendered_paths.is_empty() {
        "no files".to_owned()
    } else {
        rendered_paths.join(", ")
    };
    match status {
        Some(status) => format!("Patch {status}: {details}"),
        None => format!("Patch: {details}"),
    }
}

fn summarize_restored_plan_update(explanation: Option<&str>, plan: &[RestoredPlanStep]) -> String {
    let mut parts = Vec::new();
    if let Some(explanation) = explanation
        && !explanation.trim().is_empty()
    {
        parts.push(explanation.trim().to_owned());
    }
    if !plan.is_empty() {
        parts.push(
            plan.iter()
                .map(|entry| {
                    format!(
                        "[{}] {}",
                        restored_plan_status_label(&entry.status),
                        entry.step.trim()
                    )
                })
                .collect::<Vec<_>>()
                .join("; "),
        );
    }
    parts.join(" | ")
}

fn restored_plan_status_label(status: &RestoredPlanStepStatus) -> &'static str {
    match status {
        RestoredPlanStepStatus::Pending => "pending",
        RestoredPlanStepStatus::InProgress => "in_progress",
        RestoredPlanStepStatus::Completed => "completed",
    }
}

fn push_artifact_event_cell(
    state: &mut AppState,
    manifest: ArtifactManifest,
    preview: ArtifactPreview,
    updated: bool,
) {
    let cell = ArtifactEventCell { manifest, preview };
    state.transcript.history.push(if updated {
        HistoryCellModel::ArtifactUpdated(cell)
    } else {
        HistoryCellModel::ArtifactCreated(cell)
    });
}

fn push_status_cell(state: &mut AppState, message: &str) {
    state
        .transcript
        .history
        .push(HistoryCellModel::Status(StatusCell {
            message: message.to_owned(),
        }));
}

fn push_warning_cell(state: &mut AppState, message: &str) {
    state
        .transcript
        .history
        .push(HistoryCellModel::Warning(WarningCell {
            message: message.to_owned(),
        }));
}

fn push_error_cell(state: &mut AppState, message: &str) {
    state
        .transcript
        .history
        .push(HistoryCellModel::Error(ErrorCell {
            message: message.to_owned(),
        }));
}

fn latest_assistant_message_mut(state: &mut AppState) -> Option<&mut AssistantMessageCell> {
    state
        .transcript
        .history
        .iter_mut()
        .rev()
        .find_map(|cell| match cell {
            HistoryCellModel::AssistantMessage(cell) => Some(cell),
            _ => None,
        })
}

fn latest_exec_output_cell_mut(state: &mut AppState) -> Option<&mut mli_types::ExecOutputCell> {
    state
        .transcript
        .history
        .iter_mut()
        .rev()
        .find_map(|cell| match cell {
            HistoryCellModel::ExecOutput(cell) => Some(cell),
            _ => None,
        })
}

fn mark_latest_streaming_cells_live(state: &mut AppState) {
    if let Some(cell) = latest_assistant_message_mut(state) {
        cell.streaming = true;
    }
    if let Some(cell) = latest_exec_output_cell_mut(state) {
        cell.streaming = true;
    }
}

fn finalize_streaming_cells(state: &mut AppState) {
    if let Some(cell) = latest_assistant_message_mut(state) {
        cell.streaming = false;
    }
    for cell in &mut state.transcript.history {
        if let HistoryCellModel::ExecOutput(cell) = cell {
            cell.streaming = false;
        }
    }
}

pub(crate) fn parse_leading_skill_token(raw_text: &str) -> Option<(String, String)> {
    let trimmed = raw_text.trim_start();
    let rest = trimmed.strip_prefix('$')?;
    let token = rest.split_whitespace().next()?.trim();
    if token.is_empty() {
        return None;
    }
    Some((
        token.to_owned(),
        rest[token.len()..].trim_start().to_owned(),
    ))
}

fn match_skill_token(token: &str, skills: &[SkillDescriptor]) -> SkillTokenMatch {
    let matches = skills
        .iter()
        .filter(|skill| skill.name == token)
        .cloned()
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => SkillTokenMatch::Missing,
        [skill] => SkillTokenMatch::Unique(skill.clone()),
        _ => SkillTokenMatch::Ambiguous(matches),
    }
}

pub(crate) fn describe_skill(skill: &SkillDescriptor) -> String {
    format!(
        "{} [{}] ({})",
        skill.name,
        skill_scope_label(skill.scope),
        skill.path.display()
    )
}

pub(crate) fn selected_skill_label(skill: &SkillDescriptor) -> String {
    format!("{} [{}]", skill.name, skill_scope_label(skill.scope))
}

pub(crate) fn summarize_skill_paths(skills: &[SkillDescriptor]) -> String {
    skills
        .iter()
        .map(|skill| {
            format!(
                "{}:{}",
                skill_scope_label(skill.scope),
                skill.path.display()
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn skill_scope_label(scope: mli_types::SkillScope) -> &'static str {
    match scope {
        mli_types::SkillScope::Bundled => "bundled",
        mli_types::SkillScope::User => "user",
        mli_types::SkillScope::Repo => "repo",
        mli_types::SkillScope::Generated => "generated",
    }
}

fn prompt_picker_filter(label: &str) -> Result<PickerFilter> {
    print!("Filter {label} (blank for all, q to cancel): ");
    io::stdout()
        .flush()
        .with_context(|| format!("failed to flush {label} filter prompt"))?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .with_context(|| format!("failed to read {label} filter"))?;
    let line = line.trim();
    if line.is_empty() {
        return Ok(PickerFilter::All);
    }
    if line.eq_ignore_ascii_case("q") {
        return Ok(PickerFilter::Cancel);
    }
    Ok(PickerFilter::Query(line.to_owned()))
}

pub(crate) fn filter_threads<'a>(
    threads: &'a [ThreadListItem],
    query: Option<&str>,
) -> Vec<&'a ThreadListItem> {
    let Some(query) = normalize_picker_query(query) else {
        return threads.iter().collect();
    };
    threads
        .iter()
        .filter(|item| {
            let title = item.thread.title.as_deref().unwrap_or("");
            let id = item.thread.id.to_string();
            let status = format!("{:?}", item.thread.status);
            picker_fields_match(&query, [title, id.as_str(), status.as_str()])
        })
        .collect()
}

pub(crate) fn filter_skills<'a>(
    skills: &'a [SkillDescriptor],
    query: Option<&str>,
) -> Vec<&'a SkillDescriptor> {
    let Some(query) = normalize_picker_query(query) else {
        return skills.iter().collect();
    };
    skills
        .iter()
        .filter(|skill| {
            let path = skill.path.to_string_lossy().into_owned();
            picker_fields_match(
                &query,
                [
                    skill.name.as_str(),
                    skill.description.as_str(),
                    skill_scope_label(skill.scope),
                    path.as_str(),
                ],
            )
        })
        .collect()
}

pub(crate) fn filter_artifacts<'a>(
    artifacts: &'a [ArtifactManifest],
    query: Option<&str>,
) -> Vec<&'a ArtifactManifest> {
    let Some(query) = normalize_picker_query(query) else {
        return artifacts.iter().collect();
    };
    artifacts
        .iter()
        .filter(|artifact| {
            let kind = format!("{:?}", artifact.kind);
            let path = artifact.primary_path.to_string_lossy();
            picker_fields_match(
                &query,
                std::iter::once(artifact.title.as_str())
                    .chain(std::iter::once(artifact.summary.as_str()))
                    .chain(std::iter::once(kind.as_str()))
                    .chain(std::iter::once(path.as_ref()))
                    .chain(artifact.tags.iter().map(|tag| tag.as_str())),
            )
        })
        .collect()
}

fn normalize_picker_query(query: Option<&str>) -> Option<String> {
    query
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(|query| query.to_ascii_lowercase())
}

fn picker_fields_match<'a>(query: &str, fields: impl IntoIterator<Item = &'a str>) -> bool {
    fields
        .into_iter()
        .any(|field| field.to_ascii_lowercase().contains(query))
}

pub(crate) fn preferred_artifact_file_index(bundle: &mli_protocol::ArtifactReadResult) -> usize {
    bundle
        .files
        .iter()
        .position(|file| file.path == bundle.manifest.primary_path)
        .unwrap_or(0)
}

fn print_artifact_file(file: &ArtifactFilePayload) {
    if let Some(read_error) = &file.read_error {
        println!("{read_error}");
    } else if let Some(text) = &file.text {
        println!("{text}");
    } else {
        println!("<binary payload omitted>");
    }
}

fn is_yolo_mode(approval_policy: ApprovalPolicy, sandbox_mode: SandboxMode) -> bool {
    approval_policy == ApprovalPolicy::Never && sandbox_mode == SandboxMode::DangerFullAccess
}

fn runtime_mode_label(approval_policy: ApprovalPolicy, sandbox_mode: SandboxMode) -> &'static str {
    if is_yolo_mode(approval_policy, sandbox_mode) {
        "yolo"
    } else if sandbox_mode == SandboxMode::ReadOnly {
        "readonly"
    } else {
        "safe"
    }
}

fn approval_policy_label(policy: ApprovalPolicy) -> &'static str {
    match policy {
        ApprovalPolicy::Never => "never",
        ApprovalPolicy::OnFailure => "on-failure",
        ApprovalPolicy::OnRequest => "on-request",
        ApprovalPolicy::Untrusted => "untrusted",
    }
}

fn sandbox_mode_label(mode: SandboxMode) -> &'static str {
    match mode {
        SandboxMode::ReadOnly => "read-only",
        SandboxMode::WorkspaceWrite => "workspace-write",
        SandboxMode::DangerFullAccess => "danger-full-access",
    }
}

fn restored_active_turn_id(turns: &[mli_types::TurnRecord]) -> Option<mli_types::LocalTurnId> {
    turns.iter().rev().find_map(|turn| {
        matches!(
            turn.status,
            mli_types::TurnStatus::Starting
                | mli_types::TurnStatus::Streaming
                | mli_types::TurnStatus::WaitingApproval
        )
        .then_some(turn.id)
    })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    use super::{
        AppClient, ApprovalResolutionOutcome, ClientMessage, SkillTokenMatch, TranscriptApp,
        collect_request_user_input_response_with, decode_notification, filter_artifacts,
        filter_skills, filter_threads, finalize_restored_transcript, match_skill_token,
        parse_leading_skill_token, preferred_artifact_file_index, replay_transcript_from_raw,
        restore_artifact_event, restored_active_turn_id,
    };
    use mli_protocol::{
        AgentMessageDeltaNotification, ApprovalDecision, ApprovalRespondParams, ConfigReadResult,
        ConfigWriteResult,
        ArtifactUpdatedNotification, JsonRpcError, JsonRpcErrorPayload, JsonRpcNotification,
        JsonRpcResponse, RequestId, ServerNotification, ThreadReadResult,
        TurnCompletedNotification,
    };
    use mli_config::AppConfig;
    use mli_types::{
        AppState, ApprovalKind, ApprovalPolicy, ArtifactFilePayload, ArtifactId, ArtifactKind,
        ArtifactManifest, ArtifactPreview, ArtifactReadBundle, AssistantMessageCell,
        ConnectionState, HistoryCellModel, LocalThreadId, LocalTurnId, PendingApproval,
        SandboxMode, SkillDescriptor, SkillScope, ThreadListItem, ThreadRecord, TranscriptEvent,
        TranscriptEventSource, TurnRecord, TurnStatus, UserMessageCell, utc_now,
    };

    fn test_client_with_notifications(notifications: Vec<ServerNotification>) -> AppClient {
        let messages = notifications
            .into_iter()
            .map(|notification| ClientMessage::Notification(Box::new(notification)))
            .collect::<Vec<_>>();
        test_client_with_messages(messages)
    }

    fn test_client_with_messages(messages: Vec<ClientMessage>) -> AppClient {
        let mut child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("spawn test client: {error}"));
        let stdin = child
            .stdin
            .take()
            .unwrap_or_else(|| panic!("missing child stdin"));
        let (tx, rx) = mpsc::channel();
        for message in messages {
            tx.send(message)
                .unwrap_or_else(|error| panic!("queue test message: {error}"));
        }
        drop(tx);
        AppClient {
            child,
            stdin,
            rx,
            buffered_notifications: VecDeque::new(),
            stderr_log: Arc::new(Mutex::new(String::new())),
            next_request_id: 0,
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("mli-tui-tests-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create temp dir: {error}"));
        path
    }

    fn artifact_manifest(
        artifact_id: ArtifactId,
        thread_id: LocalThreadId,
        turn_id: LocalTurnId,
        title: &str,
    ) -> ArtifactManifest {
        ArtifactManifest {
            id: artifact_id,
            version: 1,
            local_thread_id: thread_id,
            local_turn_id: turn_id,
            kind: ArtifactKind::JobSnapshot,
            title: title.to_owned(),
            created_at: utc_now(),
            updated_at: utc_now(),
            summary: "summary".to_owned(),
            tags: vec!["job".to_owned()],
            primary_path: PathBuf::from("report.md"),
            extra_paths: vec![PathBuf::from("report.json")],
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn rendered_frame_is_disabled_by_default() {
        let client = test_client_with_messages(Vec::new());
        let app = TranscriptApp::new(client);

        assert!(app.rendered_frame().is_none());
    }

    #[test]
    fn rendered_frame_includes_clear_sequence_when_enabled() {
        let client = test_client_with_messages(Vec::new());
        let mut app = TranscriptApp::new(client);
        app.interactive_rendering = true;
        app.state
            .transcript
            .history
            .push(HistoryCellModel::UserMessage(UserMessageCell {
                text: "hello".to_owned(),
            }));

        let frame = app
            .rendered_frame()
            .unwrap_or_else(|| panic!("expected interactive frame"));

        assert!(frame.starts_with("\u{1b}[2J\u{1b}[H"));
        assert!(frame.contains("you> hello"));
    }

    #[test]
    fn toggle_yolo_mode_updates_runtime_defaults() {
        let mut config = AppConfig::default();
        let written = {
            config.codex.approval_policy = ApprovalPolicy::Never;
            config.codex.sandbox_mode = SandboxMode::DangerFullAccess;
            config.clone()
        };
        let client = test_client_with_messages(vec![
            ClientMessage::Response(JsonRpcResponse {
                id: RequestId::Integer(1),
                result: serde_json::to_value(ConfigReadResult {
                    config: serde_json::to_value(AppConfig::default())
                        .unwrap_or_else(|error| panic!("encode default config: {error}")),
                })
                .unwrap_or_else(|error| panic!("encode config/read result: {error}")),
            }),
            ClientMessage::Response(JsonRpcResponse {
                id: RequestId::Integer(2),
                result: serde_json::to_value(ConfigWriteResult {
                    config: serde_json::to_value(written.clone())
                        .unwrap_or_else(|error| panic!("encode written config: {error}")),
                })
                .unwrap_or_else(|error| panic!("encode config/write result: {error}")),
            }),
        ]);
        let mut app = TranscriptApp::new(client);
        app.state.runtime.approval_policy = Some(ApprovalPolicy::OnRequest);
        app.state.runtime.sandbox_mode = Some(SandboxMode::WorkspaceWrite);

        app.toggle_yolo_mode()
            .unwrap_or_else(|error| panic!("toggle yolo mode: {error}"));

        assert_eq!(app.state.runtime.approval_policy, Some(ApprovalPolicy::Never));
        assert_eq!(
            app.state.runtime.sandbox_mode,
            Some(SandboxMode::DangerFullAccess)
        );
        assert!(matches!(
            app.state.transcript.history.last(),
            Some(HistoryCellModel::Status(cell))
                if cell.message.contains("Default mode set to yolo")
        ));
    }

    #[test]
    fn mode_command_supports_readonly() {
        let mut config = AppConfig::default();
        let written = {
            config.codex.approval_policy = ApprovalPolicy::OnRequest;
            config.codex.sandbox_mode = SandboxMode::ReadOnly;
            config.clone()
        };
        let client = test_client_with_messages(vec![
            ClientMessage::Response(JsonRpcResponse {
                id: RequestId::Integer(1),
                result: serde_json::to_value(ConfigReadResult {
                    config: serde_json::to_value(AppConfig::default())
                        .unwrap_or_else(|error| panic!("encode default config: {error}")),
                })
                .unwrap_or_else(|error| panic!("encode config/read result: {error}")),
            }),
            ClientMessage::Response(JsonRpcResponse {
                id: RequestId::Integer(2),
                result: serde_json::to_value(ConfigWriteResult {
                    config: serde_json::to_value(written.clone())
                        .unwrap_or_else(|error| panic!("encode written config: {error}")),
                })
                .unwrap_or_else(|error| panic!("encode config/write result: {error}")),
            }),
        ]);
        let mut app = TranscriptApp::new(client);

        app.set_mode_command("/mode readonly")
            .unwrap_or_else(|error| panic!("set readonly mode: {error}"));

        assert_eq!(app.state.runtime.approval_policy, Some(ApprovalPolicy::OnRequest));
        assert_eq!(app.state.runtime.sandbox_mode, Some(SandboxMode::ReadOnly));
    }

    #[test]
    fn decode_notification_accepts_plan_updates() {
        let decoded = decode_notification(JsonRpcNotification {
            method: "turn/plan/updated".to_owned(),
            params: Some(serde_json::json!({
                "thread_id": LocalThreadId::new(),
                "turn_id": LocalTurnId::new(),
                "summary": "Inspect logs | [in_progress] Patch parser"
            })),
        })
        .unwrap_or_else(|error| panic!("decode turn/plan/updated: {error}"));

        assert!(matches!(
            decoded,
            Some(ServerNotification::PlanUpdated { params })
                if params.summary.contains("[in_progress] Patch parser")
        ));
    }

    #[test]
    fn parse_leading_skill_token_extracts_skill_and_remainder() {
        let parsed = parse_leading_skill_token("$hf-dataset-audit inspect this dataset")
            .unwrap_or_else(|| panic!("expected parsed skill token"));
        assert_eq!(parsed.0, "hf-dataset-audit");
        assert_eq!(parsed.1, "inspect this dataset");
    }

    #[test]
    fn parse_leading_skill_token_ignores_plain_text() {
        assert!(parse_leading_skill_token("inspect this dataset").is_none());
        assert!(parse_leading_skill_token("$").is_none());
    }

    #[test]
    fn restored_active_turn_id_prefers_latest_live_turn() {
        let thread_id = LocalThreadId::new();
        let mut completed = TurnRecord::new(thread_id, "completed".to_owned());
        completed.status = TurnStatus::Completed;
        let mut waiting = TurnRecord::new(thread_id, "waiting".to_owned());
        waiting.status = TurnStatus::WaitingApproval;

        let restored = restored_active_turn_id(&[completed, waiting.clone()]);

        assert_eq!(restored, Some(waiting.id));
    }

    #[test]
    fn match_skill_token_reports_ambiguous_duplicates() {
        let skills = vec![
            SkillDescriptor {
                name: "hf-dataset-audit".to_owned(),
                description: "Repo".to_owned(),
                short_description: None,
                path: PathBuf::from(".agents/skills/hf-dataset-audit/SKILL.md"),
                scope: SkillScope::Repo,
                enabled: true,
            },
            SkillDescriptor {
                name: "hf-dataset-audit".to_owned(),
                description: "Bundled".to_owned(),
                short_description: None,
                path: PathBuf::from("skills/system/hf-dataset-audit/SKILL.md"),
                scope: SkillScope::Bundled,
                enabled: true,
            },
        ];

        let matched = match_skill_token("hf-dataset-audit", &skills);

        assert!(matches!(matched, SkillTokenMatch::Ambiguous(found) if found.len() == 2));
    }

    #[test]
    fn match_skill_token_returns_unique_skill() {
        let skill = SkillDescriptor {
            name: "hf-jobs-operator".to_owned(),
            description: "Jobs".to_owned(),
            short_description: None,
            path: PathBuf::from("skills/system/hf-jobs-operator/SKILL.md"),
            scope: SkillScope::Bundled,
            enabled: true,
        };

        let matched = match_skill_token("hf-jobs-operator", std::slice::from_ref(&skill));

        assert!(matches!(matched, SkillTokenMatch::Unique(found) if found == skill));
    }

    #[test]
    fn resolve_inline_skill_prefers_selected_skill_for_matching_token() {
        let client = test_client_with_messages(Vec::new());
        let mut app = TranscriptApp::new(client);
        let skill = SkillDescriptor {
            name: "hf-dataset-audit".to_owned(),
            description: "Repo".to_owned(),
            short_description: None,
            path: PathBuf::from(".agents/skills/hf-dataset-audit/SKILL.md"),
            scope: SkillScope::Repo,
            enabled: true,
        };
        app.selected_skill = Some(skill.clone());

        let resolved = app
            .resolve_inline_skill("$hf-dataset-audit inspect this dataset")
            .unwrap_or_else(|error| panic!("resolve inline skill from selection: {error}"))
            .unwrap_or_else(|| panic!("expected matching selected skill"));

        assert_eq!(resolved.0, Some(skill));
        assert_eq!(resolved.1, "inspect this dataset");
    }

    #[test]
    fn filter_skills_matches_name_scope_and_path_case_insensitively() {
        let skills = vec![
            SkillDescriptor {
                name: "hf-dataset-audit".to_owned(),
                description: "Inspect datasets".to_owned(),
                short_description: None,
                path: PathBuf::from("skills/system/hf-dataset-audit/SKILL.md"),
                scope: SkillScope::Bundled,
                enabled: true,
            },
            SkillDescriptor {
                name: "paper-review".to_owned(),
                description: "Summarize papers".to_owned(),
                short_description: None,
                path: PathBuf::from(".agents/skills/paper-review/SKILL.md"),
                scope: SkillScope::Repo,
                enabled: true,
            },
        ];

        let filtered = filter_skills(&skills, Some("SYSTEM/HF-DATASET"));

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "hf-dataset-audit");
    }

    #[test]
    fn filter_threads_matches_title_and_status_case_insensitively() {
        let transcript_path = PathBuf::from(".ml-intern/threads/thread/transcript.jsonl");
        let artifact_root = PathBuf::from(".ml-intern/threads/thread/artifacts");
        let mut running = ThreadRecord::new(
            PathBuf::from("/tmp/project"),
            Some("Dataset triage".to_owned()),
            None,
            ApprovalPolicy::OnRequest,
            SandboxMode::WorkspaceWrite,
            transcript_path.clone(),
            artifact_root.clone(),
        );
        running.status = mli_types::ThreadStatus::Running;
        let mut idle = ThreadRecord::new(
            PathBuf::from("/tmp/project"),
            Some("Paper notes".to_owned()),
            None,
            ApprovalPolicy::OnRequest,
            SandboxMode::WorkspaceWrite,
            transcript_path,
            artifact_root,
        );
        idle.status = mli_types::ThreadStatus::Idle;
        let items = vec![
            ThreadListItem {
                thread: running,
                selected: false,
            },
            ThreadListItem {
                thread: idle,
                selected: false,
            },
        ];

        let filtered = filter_threads(&items, Some("RUNNING"));

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].thread.title.as_deref(), Some("Dataset triage"));
    }

    #[test]
    fn filter_artifacts_matches_kind_tag_and_title() {
        let thread_id = LocalThreadId::new();
        let turn_id = LocalTurnId::new();
        let artifacts = vec![
            ArtifactManifest {
                id: ArtifactId::new(),
                version: 1,
                local_thread_id: thread_id,
                local_turn_id: turn_id,
                kind: ArtifactKind::JobSnapshot,
                title: "Nightly job".to_owned(),
                created_at: utc_now(),
                updated_at: utc_now(),
                summary: "nightly status".to_owned(),
                tags: vec!["hf".to_owned(), "gpu".to_owned()],
                primary_path: PathBuf::from("report.md"),
                extra_paths: vec![],
                metadata: serde_json::json!({}),
            },
            ArtifactManifest {
                id: ArtifactId::new(),
                version: 1,
                local_thread_id: thread_id,
                local_turn_id: turn_id,
                kind: ArtifactKind::PaperReport,
                title: "Survey notes".to_owned(),
                created_at: utc_now(),
                updated_at: utc_now(),
                summary: "paper shortlist".to_owned(),
                tags: vec!["research".to_owned()],
                primary_path: PathBuf::from("report.md"),
                extra_paths: vec![],
                metadata: serde_json::json!({}),
            },
        ];

        let filtered = filter_artifacts(&artifacts, Some("GPU"));

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].title, "Nightly job");
    }

    #[test]
    fn item_notifications_project_command_patch_and_plan_cells() {
        let thread_id = LocalThreadId::new();
        let turn_id = LocalTurnId::new();
        let client = test_client_with_messages(Vec::new());
        let mut app = TranscriptApp::new(client);

        app.apply_notification(ServerNotification::ItemStarted {
            params: mli_protocol::ItemStartedNotification {
                thread_id,
                turn_id,
                item: serde_json::json!({
                    "type": "commandExecution",
                    "id": "cmd-1",
                    "command": "cargo test",
                    "status": "inProgress"
                }),
            },
        })
        .unwrap_or_else(|error| panic!("apply item started: {error}"));
        app.apply_notification(ServerNotification::ItemCompleted {
            params: mli_protocol::ItemCompletedNotification {
                thread_id,
                turn_id,
                item: serde_json::json!({
                    "type": "commandExecution",
                    "id": "cmd-1",
                    "command": "cargo test",
                    "status": "completed",
                    "aggregatedOutput": "ok"
                }),
            },
        })
        .unwrap_or_else(|error| panic!("apply command completed: {error}"));
        app.apply_notification(ServerNotification::ItemCompleted {
            params: mli_protocol::ItemCompletedNotification {
                thread_id,
                turn_id,
                item: serde_json::json!({
                    "type": "fileChange",
                    "id": "patch-1",
                    "status": "completed",
                    "changes": [
                        {"path": "src/app.rs", "kind": "update"}
                    ]
                }),
            },
        })
        .unwrap_or_else(|error| panic!("apply patch completed: {error}"));
        app.apply_notification(ServerNotification::ItemCompleted {
            params: mli_protocol::ItemCompletedNotification {
                thread_id,
                turn_id,
                item: serde_json::json!({
                    "type": "plan",
                    "id": "plan-1",
                    "text": "1. Inspect logs\\n2. Fix parser"
                }),
            },
        })
        .unwrap_or_else(|error| panic!("apply plan completed: {error}"));

        assert!(matches!(
            &app.state.transcript.history[0],
            HistoryCellModel::ExecCommand(cell) if cell.command == "cargo test"
        ));
        assert!(matches!(
            &app.state.transcript.history[1],
            HistoryCellModel::ExecOutput(cell) if cell.output == "ok" && !cell.streaming
        ));
        assert!(matches!(
            &app.state.transcript.history[2],
            HistoryCellModel::PatchSummary(cell)
                if cell.summary.contains("Patch completed") && cell.summary.contains("src/app.rs (update)")
        ));
        assert!(matches!(
            &app.state.transcript.history[3],
            HistoryCellModel::PlanUpdate(cell) if cell.summary.contains("Inspect logs")
        ));
    }

    #[test]
    fn command_execution_output_delta_appends_live_output_before_completion() {
        let thread_id = LocalThreadId::new();
        let turn_id = LocalTurnId::new();
        let client = test_client_with_messages(Vec::new());
        let mut app = TranscriptApp::new(client);

        app.apply_notification(ServerNotification::ItemStarted {
            params: mli_protocol::ItemStartedNotification {
                thread_id,
                turn_id,
                item: serde_json::json!({
                    "type": "commandExecution",
                    "id": "cmd-1",
                    "command": "python long_job.py",
                    "status": "inProgress"
                }),
            },
        })
        .unwrap_or_else(|error| panic!("apply item started: {error}"));
        app.apply_notification(ServerNotification::CommandExecutionOutputDelta {
            params: mli_protocol::CommandExecutionOutputDeltaNotification {
                thread_id,
                turn_id,
                item_id: "cmd-1".to_owned(),
                delta: "epoch 1\n".to_owned(),
            },
        })
        .unwrap_or_else(|error| panic!("apply command output delta 1: {error}"));
        app.apply_notification(ServerNotification::CommandExecutionOutputDelta {
            params: mli_protocol::CommandExecutionOutputDeltaNotification {
                thread_id,
                turn_id,
                item_id: "cmd-1".to_owned(),
                delta: "epoch 2\n".to_owned(),
            },
        })
        .unwrap_or_else(|error| panic!("apply command output delta 2: {error}"));

        assert_eq!(app.state.transcript.history.len(), 2);
        assert!(matches!(
            &app.state.transcript.history[1],
            HistoryCellModel::ExecOutput(cell)
                if cell.command == "python long_job.py"
                    && cell.output == "epoch 1\nepoch 2\n"
                    && cell.streaming
        ));

        app.apply_notification(ServerNotification::ItemCompleted {
            params: mli_protocol::ItemCompletedNotification {
                thread_id,
                turn_id,
                item: serde_json::json!({
                    "type": "commandExecution",
                    "id": "cmd-1",
                    "command": "python long_job.py",
                    "status": "completed",
                    "aggregatedOutput": "epoch 1\nepoch 2\n"
                }),
            },
        })
        .unwrap_or_else(|error| panic!("apply command completed: {error}"));

        assert_eq!(app.state.transcript.history.len(), 2);
        assert!(matches!(
            &app.state.transcript.history[1],
            HistoryCellModel::ExecOutput(cell)
                if cell.output == "epoch 1\nepoch 2\n" && !cell.streaming
        ));
    }

    #[test]
    fn replay_transcript_from_raw_projects_wrapped_item_events() {
        let thread_id = LocalThreadId::new();
        let turn_id = LocalTurnId::new();
        let raw = [
            TranscriptEvent {
                seq: 1,
                timestamp: utc_now(),
                thread_id,
                turn_id: Some(turn_id),
                source: TranscriptEventSource::UpstreamCodex,
                payload: serde_json::json!({
                    "event": "item_started",
                    "item": {
                        "type": "commandExecution",
                        "id": "cmd-1",
                        "command": "cargo test",
                        "status": "inProgress"
                    }
                }),
            },
            TranscriptEvent {
                seq: 2,
                timestamp: utc_now(),
                thread_id,
                turn_id: Some(turn_id),
                source: TranscriptEventSource::UpstreamCodex,
                payload: serde_json::json!({
                    "event": "command_execution_output_delta",
                    "item_id": "cmd-1",
                    "delta": "ok"
                }),
            },
            TranscriptEvent {
                seq: 3,
                timestamp: utc_now(),
                thread_id,
                turn_id: Some(turn_id),
                source: TranscriptEventSource::UpstreamCodex,
                payload: serde_json::json!({
                    "event": "item_completed",
                    "item": {
                        "type": "commandExecution",
                        "id": "cmd-1",
                        "command": "cargo test",
                        "status": "completed",
                        "aggregatedOutput": "ok"
                    }
                }),
            },
            TranscriptEvent {
                seq: 4,
                timestamp: utc_now(),
                thread_id,
                turn_id: Some(turn_id),
                source: TranscriptEventSource::UpstreamCodex,
                payload: serde_json::json!({
                    "event": "item_completed",
                    "item": {
                        "type": "fileChange",
                        "id": "patch-1",
                        "status": "completed",
                        "changes": [
                            {"path": "src/app.rs", "kind": "update"}
                        ]
                    }
                }),
            },
        ]
        .iter()
        .map(|event| {
            serde_json::to_string(event)
                .unwrap_or_else(|error| panic!("encode transcript event: {error}"))
        })
        .collect::<Vec<_>>()
        .join("\n");
        let mut state = AppState::default();

        replay_transcript_from_raw(&mut state, Path::new("transcript.jsonl"), &raw);

        assert!(matches!(
            &state.transcript.history[0],
            HistoryCellModel::ExecCommand(cell) if cell.command == "cargo test"
        ));
        assert!(matches!(
            &state.transcript.history[1],
            HistoryCellModel::ExecOutput(cell) if cell.output == "ok" && !cell.streaming
        ));
        assert!(matches!(
            &state.transcript.history[2],
            HistoryCellModel::PatchSummary(cell) if cell.summary.contains("src/app.rs (update)")
        ));
    }

    #[test]
    fn replay_transcript_from_raw_merges_command_output_delta_into_long_running_command() {
        let thread_id = LocalThreadId::new();
        let turn_id = LocalTurnId::new();
        let raw = [
            TranscriptEvent {
                seq: 1,
                timestamp: utc_now(),
                thread_id,
                turn_id: Some(turn_id),
                source: TranscriptEventSource::UpstreamCodex,
                payload: serde_json::json!({
                    "event": "item_started",
                    "item": {
                        "type": "commandExecution",
                        "id": "cmd-1",
                        "command": "python long_job.py",
                        "status": "inProgress"
                    }
                }),
            },
            TranscriptEvent {
                seq: 2,
                timestamp: utc_now(),
                thread_id,
                turn_id: Some(turn_id),
                source: TranscriptEventSource::UpstreamCodex,
                payload: serde_json::json!({
                    "event": "command_execution_output_delta",
                    "item_id": "cmd-1",
                    "delta": "epoch 1\n"
                }),
            },
            TranscriptEvent {
                seq: 3,
                timestamp: utc_now(),
                thread_id,
                turn_id: Some(turn_id),
                source: TranscriptEventSource::UpstreamCodex,
                payload: serde_json::json!({
                    "event": "command_execution_output_delta",
                    "item_id": "cmd-1",
                    "delta": "epoch 2\n"
                }),
            },
            TranscriptEvent {
                seq: 4,
                timestamp: utc_now(),
                thread_id,
                turn_id: Some(turn_id),
                source: TranscriptEventSource::UpstreamCodex,
                payload: serde_json::json!({
                    "event": "item_completed",
                    "item": {
                        "type": "commandExecution",
                        "id": "cmd-1",
                        "command": "python long_job.py",
                        "status": "completed",
                        "aggregatedOutput": "epoch 1\nepoch 2\n"
                    }
                }),
            },
        ]
        .iter()
        .map(|event| {
            serde_json::to_string(event)
                .unwrap_or_else(|error| panic!("encode transcript event: {error}"))
        })
        .collect::<Vec<_>>()
        .join("\n");
        let mut state = AppState::default();

        replay_transcript_from_raw(&mut state, Path::new("transcript.jsonl"), &raw);

        assert_eq!(state.transcript.history.len(), 2);
        assert!(matches!(
            &state.transcript.history[1],
            HistoryCellModel::ExecOutput(cell)
                if cell.command == "python long_job.py"
                    && cell.output == "epoch 1\nepoch 2\n"
                    && !cell.streaming
        ));
    }

    #[test]
    fn item_started_file_change_projects_patch_before_completion() {
        let thread_id = LocalThreadId::new();
        let turn_id = LocalTurnId::new();
        let client = test_client_with_messages(Vec::new());
        let mut app = TranscriptApp::new(client);

        app.apply_notification(ServerNotification::ItemStarted {
            params: mli_protocol::ItemStartedNotification {
                thread_id,
                turn_id,
                item: serde_json::json!({
                    "type": "fileChange",
                    "id": "patch-1",
                    "status": "inProgress",
                    "changes": [
                        {"path": "src/app.rs", "kind": "update"}
                    ]
                }),
            },
        })
        .unwrap_or_else(|error| panic!("apply fileChange item started: {error}"));

        assert!(matches!(
            app.state.transcript.history.last(),
            Some(HistoryCellModel::PatchSummary(cell))
                if cell.summary.contains("Patch inProgress")
                    && cell.summary.contains("src/app.rs (update)")
        ));
    }

    #[test]
    fn replay_transcript_from_raw_projects_file_change_start_marker() {
        let thread_id = LocalThreadId::new();
        let turn_id = LocalTurnId::new();
        let raw = serde_json::to_string(&TranscriptEvent {
            seq: 1,
            timestamp: utc_now(),
            thread_id,
            turn_id: Some(turn_id),
            source: TranscriptEventSource::UpstreamCodex,
            payload: serde_json::json!({
                "event": "item_started",
                "item": {
                    "type": "fileChange",
                    "id": "patch-1",
                    "status": "inProgress",
                    "changes": [
                        {"path": "src/app.rs", "kind": "update"}
                    ]
                }
            }),
        })
        .unwrap_or_else(|error| panic!("encode transcript event: {error}"));
        let mut state = AppState::default();

        replay_transcript_from_raw(&mut state, Path::new("transcript.jsonl"), &raw);

        assert!(matches!(
            state.transcript.history.last(),
            Some(HistoryCellModel::PatchSummary(cell))
                if cell.summary.contains("Patch inProgress")
                    && cell.summary.contains("src/app.rs (update)")
        ));
    }

    #[test]
    fn plan_updated_notification_pushes_plan_cell() {
        let thread_id = LocalThreadId::new();
        let turn_id = LocalTurnId::new();
        let client = test_client_with_messages(Vec::new());
        let mut app = TranscriptApp::new(client);

        app.apply_notification(ServerNotification::PlanUpdated {
            params: mli_protocol::PlanUpdatedNotification {
                thread_id,
                turn_id,
                summary: "Refine the fix | [completed] Inspect logs; [in_progress] Patch parser"
                    .to_owned(),
            },
        })
        .unwrap_or_else(|error| panic!("apply plan updated notification: {error}"));

        assert!(matches!(
            app.state.transcript.history.last(),
            Some(HistoryCellModel::PlanUpdate(cell))
                if cell.summary.contains("Refine the fix")
                    && cell.summary.contains("[in_progress] Patch parser")
        ));
    }

    #[test]
    fn replay_transcript_from_raw_restores_plan_updated_marker() {
        let thread_id = LocalThreadId::new();
        let turn_id = LocalTurnId::new();
        let raw = serde_json::to_string(&TranscriptEvent {
            seq: 1,
            timestamp: utc_now(),
            thread_id,
            turn_id: Some(turn_id),
            source: TranscriptEventSource::UpstreamCodex,
            payload: serde_json::json!({
                "event": "plan_updated",
                "explanation": "Refine the fix",
                "plan": [
                    {"step": "Inspect logs", "status": "completed"},
                    {"step": "Patch parser", "status": "inProgress"}
                ]
            }),
        })
        .unwrap_or_else(|error| panic!("encode transcript event: {error}"));
        let mut state = AppState::default();

        replay_transcript_from_raw(&mut state, Path::new("transcript.jsonl"), &raw);

        assert!(matches!(
            state.transcript.history.last(),
            Some(HistoryCellModel::PlanUpdate(cell))
                if cell.summary.contains("Refine the fix")
                    && cell.summary.contains("[completed] Inspect logs")
                    && cell.summary.contains("[in_progress] Patch parser")
        ));
    }

    #[test]
    fn preferred_artifact_file_index_prefers_primary_path() {
        let thread_id = LocalThreadId::new();
        let bundle = ArtifactReadBundle {
            manifest: ArtifactManifest {
                id: mli_types::ArtifactId::new(),
                version: 1,
                local_thread_id: thread_id,
                local_turn_id: mli_types::LocalTurnId::new(),
                kind: ArtifactKind::GenericMarkdown,
                title: "artifact".to_owned(),
                created_at: utc_now(),
                updated_at: utc_now(),
                summary: "summary".to_owned(),
                tags: Vec::new(),
                primary_path: PathBuf::from("report.md"),
                extra_paths: vec![PathBuf::from("report.json")],
                metadata: serde_json::json!({}),
            },
            files: vec![
                ArtifactFilePayload {
                    path: PathBuf::from("report.json"),
                    media_type: "application/json".to_owned(),
                    text: Some("{}".to_owned()),
                    base64: None,
                    read_error: None,
                },
                ArtifactFilePayload {
                    path: PathBuf::from("report.md"),
                    media_type: "text/markdown".to_owned(),
                    text: Some("# report".to_owned()),
                    base64: None,
                    read_error: None,
                },
            ],
        };

        let selected = preferred_artifact_file_index(&bundle);

        assert_eq!(selected, 1);
    }

    #[test]
    fn replay_transcript_from_raw_skips_corrupt_lines_and_keeps_valid_history() {
        let thread_id = LocalThreadId::new();
        let turn_id = mli_types::LocalTurnId::new();
        let user_event = TranscriptEvent {
            seq: 1,
            timestamp: utc_now(),
            thread_id,
            turn_id: Some(turn_id),
            source: TranscriptEventSource::User,
            payload: serde_json::json!({
                "input": [
                    {
                        "type": "text",
                        "text": "inspect dataset"
                    }
                ]
            }),
        };
        let assistant_event = TranscriptEvent {
            seq: 2,
            timestamp: utc_now(),
            thread_id,
            turn_id: Some(turn_id),
            source: TranscriptEventSource::UpstreamCodex,
            payload: serde_json::json!({
                "delta": "done"
            }),
        };
        let raw = format!(
            "{}\n{{ not valid json }}\n{}\n",
            serde_json::to_string(&user_event)
                .unwrap_or_else(|error| panic!("encode user event: {error}")),
            serde_json::to_string(&assistant_event)
                .unwrap_or_else(|error| panic!("encode assistant event: {error}"))
        );
        let mut state = AppState::default();

        replay_transcript_from_raw(&mut state, Path::new("/tmp/transcript.jsonl"), &raw);
        finalize_restored_transcript(&mut state);

        assert!(matches!(
            state.transcript.history.as_slice(),
            [
                HistoryCellModel::UserMessage(UserMessageCell { text }),
                HistoryCellModel::Warning(_),
                HistoryCellModel::AssistantMessage(_)
            ] if text == "inspect dataset"
        ));
        match &state.transcript.history[1] {
            HistoryCellModel::Warning(cell) => {
                assert!(cell.message.contains("Skipped corrupt transcript line 2"));
            }
            other => panic!("expected warning cell, got {other:?}"),
        }
        match &state.transcript.history[2] {
            HistoryCellModel::AssistantMessage(cell) => {
                assert_eq!(cell.text, "done");
                assert!(!cell.streaming);
            }
            other => panic!("expected assistant cell, got {other:?}"),
        }
    }

    #[test]
    fn resumed_live_thread_continues_draining_notifications() {
        let thread_id = LocalThreadId::new();
        let mut completed_turn = TurnRecord::new(thread_id, "resume".to_owned());
        completed_turn.status = TurnStatus::Completed;
        completed_turn.finished_at = Some(utc_now());
        let client = test_client_with_notifications(vec![
            ServerNotification::AgentMessageDelta {
                params: AgentMessageDeltaNotification {
                    thread_id,
                    turn_id: completed_turn.id,
                    item_id: "item-1".to_owned(),
                    delta: " + tail".to_owned(),
                },
            },
            ServerNotification::TurnCompleted {
                params: TurnCompletedNotification {
                    thread_id,
                    turn: completed_turn.clone(),
                },
            },
        ]);
        let mut app = TranscriptApp::new(client);
        app.state.active_thread_id = Some(thread_id);
        app.active_turn_id = Some(completed_turn.id);
        app.state.connection = ConnectionState::Streaming;
        app.state
            .transcript
            .history
            .push(HistoryCellModel::AssistantMessage(AssistantMessageCell {
                text: "partial".to_owned(),
                streaming: false,
            }));

        app.continue_resumed_live_thread_if_needed_with_interrupts(false)
            .unwrap_or_else(|error| panic!("continue resumed stream: {error}"));

        assert_eq!(app.state.connection, ConnectionState::Ready);
        assert!(app.active_turn_id.is_none());
        match &app.state.transcript.history[0] {
            HistoryCellModel::AssistantMessage(cell) => {
                assert_eq!(cell.text, "partial + tail");
                assert!(!cell.streaming);
            }
            other => panic!("expected assistant cell, got {other:?}"),
        }
        assert!(matches!(
            app.state.transcript.history.last(),
            Some(HistoryCellModel::AssistantMessage(_))
        ));
    }

    #[test]
    fn turn_completed_notification_returns_connection_to_ready() {
        let thread_id = LocalThreadId::new();
        let mut completed_turn = TurnRecord::new(thread_id, "done".to_owned());
        completed_turn.status = TurnStatus::Completed;
        completed_turn.finished_at = Some(utc_now());
        let client = test_client_with_notifications(Vec::new());
        let mut app = TranscriptApp::new(client);
        app.state.active_thread_id = Some(thread_id);
        app.active_turn_id = Some(completed_turn.id);
        app.state.connection = ConnectionState::Streaming;
        app.state
            .transcript
            .history
            .push(HistoryCellModel::AssistantMessage(AssistantMessageCell {
                text: "partial".to_owned(),
                streaming: true,
            }));

        app.apply_notification(ServerNotification::TurnCompleted {
            params: TurnCompletedNotification {
                thread_id,
                turn: completed_turn,
            },
        })
        .unwrap_or_else(|error| panic!("apply turn completed notification: {error}"));

        assert_eq!(app.state.connection, ConnectionState::Ready);
        assert!(app.active_turn_id.is_none());
        match &app.state.transcript.history[0] {
            HistoryCellModel::AssistantMessage(cell) => assert!(!cell.streaming),
            other => panic!("expected assistant cell, got {other:?}"),
        }
    }

    #[test]
    fn resume_thread_into_view_falls_back_to_local_snapshot_when_resume_fails() {
        let root = temp_dir("resume-fallback");
        let transcript_path = root.join("transcript.jsonl");
        let artifact_root = root.join("artifacts");
        fs::create_dir_all(&artifact_root)
            .unwrap_or_else(|error| panic!("create artifact root: {error}"));
        let mut thread = ThreadRecord::new(
            root.clone(),
            Some("resume fallback".to_owned()),
            None,
            ApprovalPolicy::OnRequest,
            SandboxMode::WorkspaceWrite,
            transcript_path.clone(),
            artifact_root,
        );
        thread.status = mli_types::ThreadStatus::Running;
        let turn = TurnRecord::new(thread.id, "inspect".to_owned());
        let transcript = TranscriptEvent {
            seq: 1,
            timestamp: utc_now(),
            thread_id: thread.id,
            turn_id: Some(turn.id),
            source: TranscriptEventSource::User,
            payload: serde_json::json!({
                "input": [
                    {
                        "type": "text",
                        "text": "inspect local thread"
                    }
                ]
            }),
        };
        fs::write(
            &transcript_path,
            format!(
                "{}\n",
                serde_json::to_string(&transcript)
                    .unwrap_or_else(|error| panic!("encode transcript event: {error}"))
            ),
        )
        .unwrap_or_else(|error| panic!("write transcript: {error}"));
        let read_result = ThreadReadResult {
            thread: thread.clone(),
            turns: vec![turn],
        };
        let client = test_client_with_messages(vec![
            ClientMessage::Error(JsonRpcError {
                error: JsonRpcErrorPayload {
                    code: -32001,
                    message: "resume boom".to_owned(),
                    data: None,
                },
                id: RequestId::Integer(1),
            }),
            ClientMessage::Response(JsonRpcResponse {
                id: RequestId::Integer(2),
                result: serde_json::to_value(&read_result)
                    .unwrap_or_else(|error| panic!("encode read result: {error}")),
            }),
        ]);
        let mut app = TranscriptApp::new(client);

        app.resume_thread_into_view(thread.id)
            .unwrap_or_else(|error| panic!("resume with fallback: {error}"));

        assert_eq!(app.state.active_thread_id, Some(thread.id));
        assert_eq!(app.state.connection, ConnectionState::Ready);
        assert!(app.active_turn_id.is_none());
        assert!(app.state.approvals.pending.is_none());
        assert!(matches!(
            app.state.transcript.history.first(),
            Some(HistoryCellModel::UserMessage(UserMessageCell { text }))
                if text == "inspect local thread"
        ));
        assert!(matches!(
            app.state.transcript.history.last(),
            Some(HistoryCellModel::Warning(cell))
                if cell
                    .message
                    .contains("Resume failed: app-server error -32001: resume boom. Loaded local transcript only.")
        ));
    }

    #[test]
    fn artifact_updated_notification_pushes_inline_update_cell() {
        let thread_id = LocalThreadId::new();
        let artifact_id = ArtifactId::new();
        let turn_id = LocalTurnId::new();
        let mut manifest = artifact_manifest(artifact_id, thread_id, turn_id, "Job snapshot");
        let original_preview = ArtifactPreview::JobSnapshot {
            job_id: "job-123".to_owned(),
            status: "running".to_owned(),
            hardware: Some("a10g".to_owned()),
            dashboard_url: None,
        };
        let client = test_client_with_messages(Vec::new());
        let mut app = TranscriptApp::new(client);
        app.state.artifacts.manifests.push(manifest.clone());

        manifest.updated_at = utc_now();
        app.apply_notification(ServerNotification::ArtifactUpdated {
            params: ArtifactUpdatedNotification {
                manifest: manifest.clone(),
                preview: original_preview.clone(),
            },
        })
        .unwrap_or_else(|error| panic!("apply artifact updated notification: {error}"));

        assert_eq!(app.state.artifacts.manifests.len(), 1);
        assert_eq!(app.state.artifacts.manifests[0].id, artifact_id);
        assert!(matches!(
            app.state.transcript.history.last(),
            Some(HistoryCellModel::ArtifactUpdated(cell))
                if cell.manifest.id == artifact_id && cell.preview == original_preview
        ));
    }

    #[test]
    fn replay_artifact_event_restores_update_as_artifact_cell() {
        let thread_id = LocalThreadId::new();
        let artifact_id = ArtifactId::new();
        let turn_id = LocalTurnId::new();
        let manifest = artifact_manifest(artifact_id, thread_id, turn_id, "Job snapshot");
        let created_payload = serde_json::json!({
            "manifest": manifest,
            "preview": {
                "kind": "job_snapshot",
                "job_id": "job-123",
                "status": "running",
                "hardware": "a10g",
                "dashboard_url": "https://hf.co/jobs/job-123"
            }
        });
        let mut state = AppState::default();

        restore_artifact_event(&mut state, &created_payload);
        restore_artifact_event(&mut state, &created_payload);

        assert!(matches!(
            state.transcript.history.first(),
            Some(HistoryCellModel::ArtifactCreated(_))
        ));
        assert!(matches!(
            state.transcript.history.last(),
            Some(HistoryCellModel::ArtifactUpdated(cell))
                if cell.manifest.id == artifact_id
        ));
    }

    #[test]
    fn submit_approval_response_failure_keeps_waiting_state_and_error_cell() {
        let approval = PendingApproval {
            id: "approval-1".to_owned(),
            kind: ApprovalKind::CommandExecution,
            title: "Approve command".to_owned(),
            description: "run tests".to_owned(),
            raw_payload: serde_json::json!({}),
        };
        let client = test_client_with_messages(vec![ClientMessage::Error(JsonRpcError {
            error: JsonRpcErrorPayload {
                code: -32002,
                message: "send failed".to_owned(),
                data: None,
            },
            id: RequestId::Integer(1),
        })]);
        let mut app = TranscriptApp::new(client);
        app.state.connection = ConnectionState::WaitingApproval;
        app.state.approvals.pending = Some(approval.clone());

        let outcome = app
            .submit_approval_response(
                &approval,
                ApprovalRespondParams {
                    approval_id: approval.id.clone(),
                    decision: ApprovalDecision::Approve,
                    answers: None,
                },
            )
            .unwrap_or_else(|error| panic!("submit approval response: {error}"));

        assert!(matches!(outcome, ApprovalResolutionOutcome::Deferred));
        assert_eq!(app.state.connection, ConnectionState::WaitingApproval);
        assert_eq!(
            app.state
                .approvals
                .pending
                .as_ref()
                .map(|pending| pending.id.as_str()),
            Some("approval-1")
        );
        assert!(matches!(
            app.state.transcript.history.last(),
            Some(HistoryCellModel::Error(cell))
                if cell
                    .message
                    .contains("Failed to send approval approval-1: app-server error -32002: send failed. Use /approval to retry.")
        ));
    }

    #[test]
    fn send_prompt_requires_pending_approval_to_be_resolved_first() {
        let approval = PendingApproval {
            id: "approval-2".to_owned(),
            kind: ApprovalKind::CommandExecution,
            title: "Approve command".to_owned(),
            description: "run tests".to_owned(),
            raw_payload: serde_json::json!({}),
        };
        let client = test_client_with_messages(Vec::new());
        let mut app = TranscriptApp::new(client);
        app.state.connection = ConnectionState::WaitingApproval;
        app.state.approvals.pending = Some(approval);

        app.send_prompt("continue anyway".to_owned())
            .unwrap_or_else(|error| panic!("send prompt while approval pending: {error}"));

        assert!(matches!(
            app.state.transcript.history.last(),
            Some(HistoryCellModel::Warning(cell))
                if cell
                    .message
                    .contains("Resolve the pending approval first with /approval.")
        ));
    }

    #[test]
    fn collect_request_user_input_response_with_supports_option_selection_and_other_text() {
        let approval = PendingApproval {
            id: "approval-3".to_owned(),
            kind: ApprovalKind::RequestUserInput,
            title: "Need more detail".to_owned(),
            description: "Collect dataset context".to_owned(),
            raw_payload: serde_json::json!({
                "questions": [
                    {
                        "id": "priority",
                        "header": "Priority",
                        "question": "Choose a priority",
                        "options": [
                            {"label": "low", "description": "Background work"},
                            {"label": "high", "description": "Needs action"}
                        ]
                    },
                    {
                        "id": "notes",
                        "header": "Notes",
                        "question": "Add context",
                        "isOther": true,
                        "options": [
                            {"label": "sync", "description": "Sync first"},
                            {"label": "skip", "description": "Skip for now"}
                        ]
                    }
                ]
            }),
        };
        let mut answers = VecDeque::from(["2".to_owned(), "ship today".to_owned()]);

        let (decision, collected) = collect_request_user_input_response_with(&approval, |_| {
            answers
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("missing scripted answer"))
        })
        .unwrap_or_else(|error| panic!("collect request_user_input answers: {error}"));

        assert_eq!(decision, ApprovalDecision::Approve);
        let collected = collected.unwrap_or_else(|| panic!("missing collected answers"));
        assert_eq!(
            collected
                .get("priority")
                .map(|answer| answer.answers.clone()),
            Some(vec!["high".to_owned()])
        );
        assert_eq!(
            collected.get("notes").map(|answer| answer.answers.clone()),
            Some(vec!["ship today".to_owned()])
        );
    }

    #[test]
    fn collect_request_user_input_response_with_blank_answer_rejects() {
        let approval = PendingApproval {
            id: "approval-4".to_owned(),
            kind: ApprovalKind::RequestUserInput,
            title: "Need more detail".to_owned(),
            description: "Collect dataset context".to_owned(),
            raw_payload: serde_json::json!({
                "questions": [
                    {
                        "id": "priority",
                        "header": "Priority",
                        "question": "Choose a priority",
                        "options": [
                            {"label": "low", "description": "Background work"}
                        ]
                    }
                ]
            }),
        };

        let (decision, collected) =
            collect_request_user_input_response_with(&approval, |_| Ok(String::new()))
                .unwrap_or_else(|error| panic!("collect request_user_input cancel: {error}"));

        assert_eq!(decision, ApprovalDecision::Reject);
        assert!(collected.is_none());
    }

    #[test]
    fn request_interrupt_failure_keeps_live_turn_and_surfaces_error() {
        let thread_id = LocalThreadId::new();
        let turn_id = mli_types::LocalTurnId::new();
        let client = test_client_with_messages(vec![ClientMessage::Error(JsonRpcError {
            error: JsonRpcErrorPayload {
                code: -32003,
                message: "interrupt failed".to_owned(),
                data: None,
            },
            id: RequestId::Integer(1),
        })]);
        let mut app = TranscriptApp::new(client);
        app.state.active_thread_id = Some(thread_id);
        app.active_turn_id = Some(turn_id);
        app.state.connection = ConnectionState::Streaming;

        app.request_interrupt()
            .unwrap_or_else(|error| panic!("request interrupt: {error}"));

        assert_eq!(app.active_turn_id, Some(turn_id));
        assert_eq!(app.state.connection, ConnectionState::Streaming);
        assert!(matches!(
            app.state.transcript.history.last(),
            Some(HistoryCellModel::Error(cell))
                if cell
                    .message
                    .contains("Failed to send interrupt for turn")
                    && cell.message.contains("app-server error -32003: interrupt failed")
        ));
    }
}
