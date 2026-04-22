use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use mli_codex_bridge::{
    BridgeConfig, CodexBridge, ProcessCodexBridge, approval_policy_to_upstream,
    sandbox_mode_to_upstream,
};
use mli_config::{AppConfig, AppPaths};
use mli_protocol::{
    AgentMessageDeltaNotification, ApprovalDecision, ApprovalRespondParams, ApprovalRespondResult,
    ArtifactCreatedNotification, ArtifactListParams, ArtifactListResult, ArtifactReadParams,
    ArtifactReadResult, ClientCapabilities, ClientInfo,
    CommandExecutionOutputDeltaNotification as LocalCommandExecutionOutputDeltaNotification,
    ErrorNotification as LocalErrorNotification, InitializeParams, InitializeResult,
    ItemCompletedNotification as LocalItemCompletedNotification,
    ItemStartedNotification as LocalItemStartedNotification,
    PlanUpdatedNotification as LocalPlanUpdatedNotification, RuntimeInfoResult,
    RuntimeStatusChangedNotification, ServerInfo, ServerNotification, SkillsListParams,
    SkillsListResult, ThreadListResult, ThreadReadParams, ThreadReadResult, ThreadResumeParams,
    ThreadResumeResult, ThreadStartParams, ThreadStartResult, TurnCompletedNotification,
    TurnInterruptParams, TurnStartParams, TurnStartResult, UserInput, WarningNotification,
};
use mli_repo::{FsThreadRepo, FsTranscriptRepo, FsTurnRepo, ThreadRepo, TranscriptRepo, TurnRepo};
use mli_services::{
    ArtifactService, LocalArtifactService, LocalRuntimeEnvironmentService, LocalSkillService,
    LocalThreadService, RuntimeEnvironmentService, SkillService, ThreadService, TurnService,
};
use mli_types::{
    ApprovalKind, ArtifactId, LocalThreadId, LocalTurnId, PendingApproval, ThreadRecord,
    ThreadStatus, TranscriptEvent, TranscriptEventSource, TurnRecord, TurnStatus, UpstreamThreadId,
    UpstreamTurnId, utc_now,
};
use mli_upstream_protocol::{
    CommandExecutionApprovalDecision, CommandExecutionOutputDeltaNotification,
    CommandExecutionRequestApprovalParams, CommandExecutionRequestApprovalResponse,
    FileChangeApprovalDecision, FileChangeRequestApprovalParams, FileChangeRequestApprovalResponse,
    GrantedPermissionProfile, PermissionGrantScope, PermissionsRequestApprovalParams,
    PermissionsRequestApprovalResponse, RequestId as UpstreamRequestId,
    ThreadResumeParams as UpstreamThreadResumeParams,
    ThreadStartParams as UpstreamThreadStartParams, ToolRequestUserInputAnswer,
    ToolRequestUserInputParams, ToolRequestUserInputResponse,
    TurnInterruptParams as UpstreamTurnInterruptParams, TurnStartParams as UpstreamTurnStartParams,
    TurnStatus as UpstreamTurnStatus, UpstreamEvent, UpstreamNotification, UpstreamServerRequest,
    UserInput as UpstreamUserInput,
};

use crate::state::{can_transition_thread, can_transition_turn};

pub struct RuntimeSession<B: CodexBridge = ProcessCodexBridge> {
    config: AppConfig,
    paths: AppPaths,
    environment_service: LocalRuntimeEnvironmentService,
    thread_service: LocalThreadService,
    skill_service: LocalSkillService,
    artifact_service: LocalArtifactService,
    thread_repo: FsThreadRepo,
    turn_repo: FsTurnRepo,
    transcript_repo: FsTranscriptRepo,
    bridge: B,
    thread_seq: HashMap<LocalThreadId, u64>,
    upstream_threads: HashMap<String, LocalThreadId>,
    upstream_turns: HashMap<String, (LocalThreadId, LocalTurnId)>,
    known_artifacts: HashMap<LocalThreadId, HashMap<ArtifactId, chrono::DateTime<chrono::Utc>>>,
    artifact_warnings: HashMap<LocalThreadId, HashMap<std::path::PathBuf, String>>,
    pending_approvals: HashMap<String, PendingApprovalContext>,
    pending_notifications: VecDeque<ServerNotification>,
}

#[derive(Clone, Debug)]
struct PendingApprovalContext {
    request_id: UpstreamRequestId,
    thread_id: LocalThreadId,
    turn_id: LocalTurnId,
    request: UpstreamServerRequest,
}

struct PendingApprovalRegistration {
    request_id: UpstreamRequestId,
    upstream_thread_id: String,
    upstream_turn_id: String,
    kind: ApprovalKind,
    title: String,
    description: String,
    raw_payload: serde_json::Value,
    request: UpstreamServerRequest,
}

impl RuntimeSession<ProcessCodexBridge> {
    pub fn from_config(config: AppConfig, paths: AppPaths) -> Result<Self> {
        let environment_service =
            LocalRuntimeEnvironmentService::new(config.clone(), paths.clone());
        let bridge = ProcessCodexBridge::new(BridgeConfig {
            codex_bin: environment_service.resolve_codex_bin()?,
            codex_home: Some(paths.codex_home_dir.clone()),
            env: bridge_environment(&paths)?,
        });
        Ok(Self::new(config, paths, bridge))
    }
}

impl<B: CodexBridge> RuntimeSession<B> {
    pub fn new(config: AppConfig, paths: AppPaths, bridge: B) -> Self {
        Self {
            environment_service: LocalRuntimeEnvironmentService::new(config.clone(), paths.clone()),
            thread_service: LocalThreadService::new(config.clone(), paths.clone()),
            skill_service: LocalSkillService::new(config.clone(), paths.clone()),
            artifact_service: LocalArtifactService::new(paths.clone()),
            thread_repo: FsThreadRepo::new(paths.clone()),
            turn_repo: FsTurnRepo::new(paths.clone()),
            transcript_repo: FsTranscriptRepo::new(paths.clone()),
            config,
            paths,
            bridge,
            thread_seq: HashMap::new(),
            upstream_threads: HashMap::new(),
            upstream_turns: HashMap::new(),
            known_artifacts: HashMap::new(),
            artifact_warnings: HashMap::new(),
            pending_approvals: HashMap::new(),
            pending_notifications: VecDeque::new(),
        }
    }

    pub fn initialize(&mut self, _params: InitializeParams) -> Result<InitializeResult> {
        let codex_version = self.environment_service.validate_codex_version()?;
        let codex_bin = self.environment_service.resolve_codex_bin()?;
        self.environment_service
            .prepare_codex_home_overlay(&self.paths.cwd)?;
        let _ = self.bridge.initialize()?;
        Ok(InitializeResult {
            server_info: ServerInfo {
                name: "ml-intern-codex-app-server".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            protocol_version: "0.1.0".to_owned(),
            upstream_codex_version: codex_version,
            codex_bin,
            app_home: self.paths.app_home.clone(),
        })
    }

    pub fn initialized(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn runtime_info(&mut self) -> Result<RuntimeInfoResult> {
        Ok(RuntimeInfoResult {
            codex_bin: self.environment_service.resolve_codex_bin()?,
            codex_version: self.environment_service.validate_codex_version()?,
            app_home: self.paths.app_home.clone(),
            cwd: self.paths.cwd.clone(),
            approval_policy: self.config.codex.approval_policy,
            sandbox_mode: self.config.codex.sandbox_mode,
        })
    }

    pub fn start_thread(
        &mut self,
        params: ThreadStartParams,
    ) -> Result<(ThreadStartResult, Vec<ServerNotification>)> {
        self.environment_service
            .prepare_codex_home_overlay(&params.cwd)?;
        let mut thread = self.thread_service.start_thread(params.clone())?;
        let upstream = self.bridge.thread_start(UpstreamThreadStartParams {
            model: thread.model.clone(),
            cwd: Some(thread.cwd.display().to_string()),
            approval_policy: Some(approval_policy_to_upstream(thread.approval_policy)),
            sandbox: Some(sandbox_mode_to_upstream(thread.sandbox_mode)),
            base_instructions: None,
            developer_instructions: None,
            experimental_raw_events: true,
            persist_extended_history: true,
        })?;
        thread.upstream_thread_id = Some(UpstreamThreadId::from(upstream.thread.id.clone()));
        thread.status = ThreadStatus::Idle;
        self.thread_repo.update(&thread)?;
        self.upstream_threads.insert(upstream.thread.id, thread.id);
        self.sync_known_artifacts(thread.id)?;
        self.append_transcript_event(thread.id, None, TranscriptEventSource::Wrapper, &thread)?;
        let notifications = vec![
            ServerNotification::RuntimeStatusChanged {
                params: RuntimeStatusChangedNotification {
                    status: "ready".to_owned(),
                },
            },
            ServerNotification::ThreadStarted {
                params: mli_protocol::ThreadStartedNotification {
                    thread: thread.clone(),
                },
            },
            ServerNotification::ThreadStatusChanged {
                params: mli_protocol::ThreadStatusChangedNotification {
                    thread: thread.clone(),
                },
            },
        ];
        Ok((ThreadStartResult { thread }, notifications))
    }

    pub fn resume_thread(
        &mut self,
        params: ThreadResumeParams,
    ) -> Result<(ThreadResumeResult, Vec<ServerNotification>)> {
        let thread = self.thread_service.resume_thread(params.thread_id)?;
        let upstream_thread_id = thread
            .upstream_thread_id
            .clone()
            .ok_or_else(|| anyhow!("thread {} has no upstream thread id", thread.id))?;
        let upstream = self.bridge.thread_resume(UpstreamThreadResumeParams {
            thread_id: upstream_thread_id.as_str().to_owned(),
            model: thread.model.clone(),
            cwd: Some(thread.cwd.display().to_string()),
            approval_policy: Some(approval_policy_to_upstream(thread.approval_policy)),
            sandbox: Some(sandbox_mode_to_upstream(thread.sandbox_mode)),
            base_instructions: None,
            developer_instructions: None,
            persist_extended_history: true,
        })?;
        self.upstream_threads.insert(upstream.thread.id, thread.id);
        let active_turn = self.rehydrate_active_turn_binding(&thread)?;
        if matches!(thread.status, ThreadStatus::WaitingApproval) {
            self.rehydrate_pending_approval(&thread, active_turn.as_ref().map(|turn| turn.id))?;
        }
        self.sync_known_artifacts(thread.id)?;
        let notifications = vec![ServerNotification::ThreadStatusChanged {
            params: mli_protocol::ThreadStatusChangedNotification {
                thread: thread.clone(),
            },
        }];
        Ok((ThreadResumeResult { thread }, notifications))
    }

    pub fn list_threads(&self) -> Result<ThreadListResult> {
        Ok(ThreadListResult {
            threads: self.thread_service.list_threads()?,
        })
    }

    pub fn read_thread(&self, params: ThreadReadParams) -> Result<ThreadReadResult> {
        let details = self.thread_service.read_thread(params.thread_id)?;
        Ok(ThreadReadResult {
            thread: details.thread,
            turns: details.turns,
        })
    }

    pub fn list_skills(&self, params: SkillsListParams) -> Result<SkillsListResult> {
        Ok(SkillsListResult {
            skills: self
                .skill_service
                .list_skills(params.cwd.as_deref(), params.force_reload.unwrap_or(false))?,
        })
    }

    pub fn list_artifacts(&self, params: ArtifactListParams) -> Result<ArtifactListResult> {
        Ok(ArtifactListResult {
            artifacts: self
                .artifact_service
                .list_artifacts(mli_types::ArtifactQuery {
                    thread_id: params.thread_id,
                    kind: params.kind,
                    limit: params.limit,
                })?,
        })
    }

    pub fn read_artifact(&self, params: ArtifactReadParams) -> Result<ArtifactReadResult> {
        self.artifact_service.read_artifact(params.artifact_id)
    }

    pub fn start_turn(&mut self, params: TurnStartParams) -> Result<TurnStartResult> {
        let TurnStartParams { thread_id, input } = params;
        let summary = UserInput::summary(&input);
        let mut turn = self
            .thread_service
            .start_turn(mli_types::StartTurnRequest {
                thread_id,
                user_input_summary: summary,
            })?;
        let mut thread = self
            .thread_repo
            .get(thread_id)?
            .ok_or_else(|| anyhow!("unknown thread {}", thread_id))?;
        let upstream_thread_id = thread
            .upstream_thread_id
            .clone()
            .ok_or_else(|| anyhow!("thread {} has no upstream binding", thread.id))?;
        self.sync_known_artifacts(thread.id)?;
        self.append_transcript_event(
            thread.id,
            Some(turn.id),
            TranscriptEventSource::User,
            &serde_json::json!({ "input": input.clone() }),
        )?;
        let upstream = self.bridge.turn_start(UpstreamTurnStartParams {
            thread_id: upstream_thread_id.as_str().to_owned(),
            input: input.into_iter().map(into_upstream_input).collect(),
            cwd: Some(thread.cwd.clone()),
            approval_policy: Some(approval_policy_to_upstream(thread.approval_policy)),
            model: thread.model.clone(),
        })?;
        turn.upstream_turn_id = Some(UpstreamTurnId::from(upstream.turn.id.clone()));
        if can_transition_turn(&turn.status, &TurnStatus::Streaming) {
            turn.status = TurnStatus::Streaming;
        }
        self.turn_repo.update(&turn)?;
        if can_transition_thread(&thread.status, &ThreadStatus::Running) {
            thread.status = ThreadStatus::Running;
        }
        self.thread_repo.update(&thread)?;
        self.upstream_turns
            .insert(upstream.turn.id, (thread.id, turn.id));
        Ok(TurnStartResult { turn })
    }

    pub fn interrupt_turn(&mut self, params: TurnInterruptParams) -> Result<()> {
        let turn = self
            .turn_repo
            .get(params.thread_id, params.turn_id)?
            .ok_or_else(|| anyhow!("unknown turn {}", params.turn_id))?;
        let upstream_turn_id = turn
            .upstream_turn_id
            .clone()
            .ok_or_else(|| anyhow!("turn {} has no upstream binding", turn.id))?;
        let thread = self
            .thread_repo
            .get(params.thread_id)?
            .ok_or_else(|| anyhow!("unknown thread {}", params.thread_id))?;
        let upstream_thread_id = thread
            .upstream_thread_id
            .clone()
            .ok_or_else(|| anyhow!("thread {} has no upstream binding", thread.id))?;
        let _ = self.bridge.turn_interrupt(UpstreamTurnInterruptParams {
            thread_id: upstream_thread_id.as_str().to_owned(),
            turn_id: upstream_turn_id.as_str().to_owned(),
        })?;
        self.thread_service
            .interrupt_turn(params.thread_id, params.turn_id)?;
        self.append_transcript_event(
            params.thread_id,
            Some(params.turn_id),
            TranscriptEventSource::Wrapper,
            &serde_json::json!({
                "event": "interrupt_requested",
            }),
        )
    }

    pub fn respond_to_approval(
        &mut self,
        params: ApprovalRespondParams,
    ) -> Result<ApprovalRespondResult> {
        let context = self
            .pending_approvals
            .get(&params.approval_id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown approval {}", params.approval_id))?;
        let result = approval_response_value(&context.request, &params)?;
        self.bridge
            .respond_to_server_request(context.request_id.clone(), result)?;

        self.pending_approvals.remove(&params.approval_id);
        let thread = self.restore_after_approval(context.thread_id, context.turn_id)?;
        self.pending_notifications
            .push_back(ServerNotification::ThreadStatusChanged {
                params: mli_protocol::ThreadStatusChangedNotification { thread },
            });
        self.append_transcript_event(
            context.thread_id,
            Some(context.turn_id),
            TranscriptEventSource::Wrapper,
            &params,
        )?;
        Ok(ApprovalRespondResult::default())
    }

    pub fn recv_notification(&mut self) -> Result<Option<ServerNotification>> {
        if let Some(notification) = self.pending_notifications.pop_front() {
            return Ok(Some(notification));
        }
        self.poll_artifact_notifications()?;
        if let Some(notification) = self.pending_notifications.pop_front() {
            return Ok(Some(notification));
        }
        match self.bridge.recv_event()? {
            Some(event) => self.normalize_event(event),
            None => Ok(None),
        }
    }

    pub fn recv_notification_blocking(&mut self) -> Result<Option<ServerNotification>> {
        if let Some(notification) = self.pending_notifications.pop_front() {
            return Ok(Some(notification));
        }
        self.poll_artifact_notifications()?;
        if let Some(notification) = self.pending_notifications.pop_front() {
            return Ok(Some(notification));
        }
        match self.bridge.recv_event_blocking()? {
            Some(event) => self.normalize_event(event),
            None => Ok(None),
        }
    }

    pub fn has_pending_notifications(&self) -> bool {
        !self.pending_notifications.is_empty()
    }

    pub fn has_active_turns(&self) -> bool {
        !self.upstream_turns.is_empty()
    }

    fn poll_artifact_notifications(&mut self) -> Result<()> {
        let watched_threads = self.known_artifacts.keys().copied().collect::<Vec<_>>();
        for thread_id in watched_threads {
            for notification in self.scan_artifact_changes(thread_id)? {
                self.enqueue_notification(notification)?;
            }
        }
        Ok(())
    }

    fn normalize_event(&mut self, event: UpstreamEvent) -> Result<Option<ServerNotification>> {
        match event {
            UpstreamEvent::Notification(notification) => self.normalize_notification(notification),
            UpstreamEvent::ServerRequest(request) => self.normalize_server_request(*request),
        }
    }

    fn normalize_notification(
        &mut self,
        notification: UpstreamNotification,
    ) -> Result<Option<ServerNotification>> {
        match notification {
            UpstreamNotification::TurnStarted { params } => {
                let (thread_id, turn_id) = self.lookup_local_turn(&params.turn.id)?;
                let mut turn = self
                    .turn_repo
                    .get(thread_id, turn_id)?
                    .ok_or_else(|| anyhow!("turn {turn_id} missing"))?;
                turn.status = TurnStatus::Streaming;
                self.turn_repo.update(&turn)?;
                self.append_transcript_event(
                    thread_id,
                    Some(turn_id),
                    TranscriptEventSource::UpstreamCodex,
                    &params,
                )?;
                Ok(Some(ServerNotification::TurnStarted {
                    params: mli_protocol::TurnStartedNotification { thread_id, turn },
                }))
            }
            UpstreamNotification::ItemStarted { params } => {
                let (thread_id, turn_id) =
                    self.lookup_thread_turn(&params.thread_id, &params.turn_id)?;
                self.append_transcript_event(
                    thread_id,
                    Some(turn_id),
                    TranscriptEventSource::UpstreamCodex,
                    &serde_json::json!({
                        "event": "item_started",
                        "item": params.item.clone(),
                    }),
                )?;
                Ok(Some(ServerNotification::ItemStarted {
                    params: LocalItemStartedNotification {
                        thread_id,
                        turn_id,
                        item: params.item,
                    },
                }))
            }
            UpstreamNotification::ItemCompleted { params } => {
                let (thread_id, turn_id) =
                    self.lookup_thread_turn(&params.thread_id, &params.turn_id)?;
                self.append_transcript_event(
                    thread_id,
                    Some(turn_id),
                    TranscriptEventSource::UpstreamCodex,
                    &serde_json::json!({
                        "event": "item_completed",
                        "item": params.item.clone(),
                    }),
                )?;
                Ok(Some(ServerNotification::ItemCompleted {
                    params: LocalItemCompletedNotification {
                        thread_id,
                        turn_id,
                        item: params.item,
                    },
                }))
            }
            UpstreamNotification::AgentMessageDelta { params } => {
                let (thread_id, turn_id) =
                    self.lookup_thread_turn(&params.thread_id, &params.turn_id)?;
                self.append_transcript_event(
                    thread_id,
                    Some(turn_id),
                    TranscriptEventSource::UpstreamCodex,
                    &params,
                )?;
                Ok(Some(ServerNotification::AgentMessageDelta {
                    params: AgentMessageDeltaNotification {
                        thread_id,
                        turn_id,
                        item_id: params.item_id,
                        delta: params.delta,
                    },
                }))
            }
            UpstreamNotification::CommandExecutionOutputDelta { params } => {
                let (thread_id, turn_id) =
                    self.lookup_thread_turn(&params.thread_id, &params.turn_id)?;
                self.append_transcript_event(
                    thread_id,
                    Some(turn_id),
                    TranscriptEventSource::UpstreamCodex,
                    &command_output_delta_payload(&params),
                )?;
                Ok(Some(ServerNotification::CommandExecutionOutputDelta {
                    params: LocalCommandExecutionOutputDeltaNotification {
                        thread_id,
                        turn_id,
                        item_id: params.item_id,
                        delta: params.delta,
                    },
                }))
            }
            UpstreamNotification::TurnPlanUpdated { params } => {
                let (thread_id, turn_id) = self.lookup_local_turn(&params.turn_id)?;
                let summary = Self::summarize_upstream_plan_update(
                    params.explanation.as_deref(),
                    &params.plan,
                );
                self.append_transcript_event(
                    thread_id,
                    Some(turn_id),
                    TranscriptEventSource::UpstreamCodex,
                    &serde_json::json!({
                        "event": "plan_updated",
                        "explanation": params.explanation,
                        "plan": params.plan,
                    }),
                )?;
                Ok(Some(ServerNotification::PlanUpdated {
                    params: LocalPlanUpdatedNotification {
                        thread_id,
                        turn_id,
                        summary,
                    },
                }))
            }
            UpstreamNotification::TurnCompleted { params } => {
                let (thread_id, turn_id) = self.lookup_local_turn(&params.turn.id)?;
                let mut turn = self
                    .turn_repo
                    .get(thread_id, turn_id)?
                    .ok_or_else(|| anyhow!("turn {turn_id} missing"))?;
                let mut thread = self
                    .thread_repo
                    .get(thread_id)?
                    .ok_or_else(|| anyhow!("thread {thread_id} missing"))?;
                let upstream_turn_id = params.turn.id.clone();
                let final_turn_status =
                    match params.turn.status.unwrap_or(UpstreamTurnStatus::Completed) {
                        UpstreamTurnStatus::Completed => TurnStatus::Completed,
                        UpstreamTurnStatus::Interrupted => TurnStatus::Interrupted,
                        UpstreamTurnStatus::Failed => TurnStatus::Failed,
                        UpstreamTurnStatus::InProgress => TurnStatus::Streaming,
                    };
                if can_transition_turn(&turn.status, &final_turn_status) {
                    turn.status = final_turn_status.clone();
                }
                turn.finished_at = Some(utc_now());
                self.turn_repo.update(&turn)?;
                thread.status = match final_turn_status {
                    TurnStatus::Interrupted => ThreadStatus::Interrupted,
                    TurnStatus::Failed => ThreadStatus::Error,
                    _ => ThreadStatus::Idle,
                };
                thread.updated_at = utc_now();
                self.thread_repo.update(&thread)?;
                self.append_transcript_event(
                    thread_id,
                    Some(turn_id),
                    TranscriptEventSource::UpstreamCodex,
                    &params,
                )?;
                self.upstream_turns.remove(&upstream_turn_id);
                self.pending_notifications
                    .push_back(ServerNotification::ThreadStatusChanged {
                        params: mli_protocol::ThreadStatusChangedNotification {
                            thread: thread.clone(),
                        },
                    });
                for artifact_notification in self.scan_artifact_changes(thread_id)? {
                    self.enqueue_notification(artifact_notification)?;
                }
                Ok(Some(ServerNotification::TurnCompleted {
                    params: TurnCompletedNotification {
                        thread_id,
                        turn: turn.clone(),
                    },
                }))
            }
            UpstreamNotification::ServerRequestResolved { params } => {
                self.resolve_server_request(params.request_id)
            }
            UpstreamNotification::Error { params } => {
                let (thread_id, turn_id) =
                    self.lookup_thread_turn(&params.thread_id, &params.turn_id)?;
                self.append_transcript_event(
                    thread_id,
                    Some(turn_id),
                    TranscriptEventSource::UpstreamCodex,
                    &params,
                )?;
                Ok(Some(ServerNotification::Error {
                    params: LocalErrorNotification {
                        message: params.error.message,
                    },
                }))
            }
        }
    }

    fn normalize_server_request(
        &mut self,
        request: UpstreamServerRequest,
    ) -> Result<Option<ServerNotification>> {
        let request_clone = request.clone();
        match &request {
            UpstreamServerRequest::CommandExecutionRequestApproval { id, params } => self
                .register_pending_approval(PendingApprovalRegistration {
                    request_id: id.clone(),
                    upstream_thread_id: params.thread_id.clone(),
                    upstream_turn_id: params.turn_id.clone(),
                    kind: ApprovalKind::CommandExecution,
                    title: command_approval_title(params),
                    description: command_approval_description(params),
                    raw_payload: serde_json::to_value(params)
                        .context("failed to serialize approval payload")?,
                    request: request_clone,
                }),
            UpstreamServerRequest::FileChangeRequestApproval { id, params } => self
                .register_pending_approval(PendingApprovalRegistration {
                    request_id: id.clone(),
                    upstream_thread_id: params.thread_id.clone(),
                    upstream_turn_id: params.turn_id.clone(),
                    kind: ApprovalKind::FileChange,
                    title: "Approve file changes".to_owned(),
                    description: file_change_approval_description(params),
                    raw_payload: serde_json::to_value(params)
                        .context("failed to serialize approval payload")?,
                    request: request_clone,
                }),
            UpstreamServerRequest::PermissionsRequestApproval { id, params } => self
                .register_pending_approval(PendingApprovalRegistration {
                    request_id: id.clone(),
                    upstream_thread_id: params.thread_id.clone(),
                    upstream_turn_id: params.turn_id.clone(),
                    kind: ApprovalKind::PermissionRequest,
                    title: "Approve additional permissions".to_owned(),
                    description: permissions_approval_description(params),
                    raw_payload: serde_json::to_value(params)
                        .context("failed to serialize approval payload")?,
                    request: request_clone,
                }),
            UpstreamServerRequest::ToolRequestUserInput { id, params } => self
                .register_pending_approval(PendingApprovalRegistration {
                    request_id: id.clone(),
                    upstream_thread_id: params.thread_id.clone(),
                    upstream_turn_id: params.turn_id.clone(),
                    kind: ApprovalKind::RequestUserInput,
                    title: "Tool needs more input".to_owned(),
                    description: request_user_input_description(params),
                    raw_payload: serde_json::to_value(params)
                        .context("failed to serialize approval payload")?,
                    request: request_clone,
                }),
        }
    }

    fn register_pending_approval(
        &mut self,
        registration: PendingApprovalRegistration,
    ) -> Result<Option<ServerNotification>> {
        let PendingApprovalRegistration {
            request_id,
            upstream_thread_id,
            upstream_turn_id,
            kind,
            title,
            description,
            raw_payload,
            request,
        } = registration;
        let (thread_id, turn_id) =
            self.lookup_thread_turn(&upstream_thread_id, &upstream_turn_id)?;
        let mut thread = self
            .thread_repo
            .get(thread_id)?
            .ok_or_else(|| anyhow!("thread {thread_id} missing"))?;
        let mut turn = self
            .turn_repo
            .get(thread_id, turn_id)?
            .ok_or_else(|| anyhow!("turn {turn_id} missing"))?;
        if can_transition_turn(&turn.status, &TurnStatus::WaitingApproval) {
            turn.status = TurnStatus::WaitingApproval;
            self.turn_repo.update(&turn)?;
        }
        if can_transition_thread(&thread.status, &ThreadStatus::WaitingApproval) {
            thread.status = ThreadStatus::WaitingApproval;
            thread.updated_at = utc_now();
            self.thread_repo.update(&thread)?;
            self.pending_notifications
                .push_back(ServerNotification::ThreadStatusChanged {
                    params: mli_protocol::ThreadStatusChangedNotification {
                        thread: thread.clone(),
                    },
                });
        }
        let approval_id = upstream_request_id_to_string(&request_id);
        let approval = PendingApproval {
            id: approval_id.clone(),
            kind,
            title,
            description,
            raw_payload,
        };
        self.pending_approvals.insert(
            approval_id,
            PendingApprovalContext {
                request_id,
                thread_id,
                turn_id,
                request,
            },
        );
        let notification = mli_protocol::ApprovalRequestNotification {
            approval: approval.clone(),
        };
        self.append_transcript_event(
            thread_id,
            Some(turn_id),
            TranscriptEventSource::Wrapper,
            &notification,
        )?;
        Ok(Some(ServerNotification::ApprovalRequested {
            params: notification,
        }))
    }

    fn resolve_server_request(
        &mut self,
        request_id: UpstreamRequestId,
    ) -> Result<Option<ServerNotification>> {
        let approval_id = upstream_request_id_to_string(&request_id);
        let Some(context) = self.pending_approvals.remove(&approval_id) else {
            return Ok(None);
        };
        let thread = self.restore_after_approval(context.thread_id, context.turn_id)?;
        self.append_transcript_event(
            context.thread_id,
            Some(context.turn_id),
            TranscriptEventSource::Wrapper,
            &serde_json::json!({
                "approval_id": approval_id,
                "resolved": true,
            }),
        )?;
        Ok(Some(ServerNotification::ThreadStatusChanged {
            params: mli_protocol::ThreadStatusChangedNotification { thread },
        }))
    }

    fn restore_after_approval(
        &mut self,
        thread_id: LocalThreadId,
        turn_id: LocalTurnId,
    ) -> Result<ThreadRecord> {
        let mut thread = self
            .thread_repo
            .get(thread_id)?
            .ok_or_else(|| anyhow!("thread {thread_id} missing"))?;
        let mut turn = self
            .turn_repo
            .get(thread_id, turn_id)?
            .ok_or_else(|| anyhow!("turn {turn_id} missing"))?;
        if can_transition_turn(&turn.status, &TurnStatus::Streaming) {
            turn.status = TurnStatus::Streaming;
            self.turn_repo.update(&turn)?;
        }
        if can_transition_thread(&thread.status, &ThreadStatus::Running) {
            thread.status = ThreadStatus::Running;
            thread.updated_at = utc_now();
            self.thread_repo.update(&thread)?;
        }
        Ok(thread)
    }

    fn lookup_local_turn(&self, upstream_turn_id: &str) -> Result<(LocalThreadId, LocalTurnId)> {
        self.upstream_turns
            .get(upstream_turn_id)
            .copied()
            .ok_or_else(|| anyhow!("unknown upstream turn {upstream_turn_id}"))
    }

    fn lookup_thread_turn(
        &self,
        upstream_thread_id: &str,
        upstream_turn_id: &str,
    ) -> Result<(LocalThreadId, LocalTurnId)> {
        let (thread_id, turn_id) = self.lookup_local_turn(upstream_turn_id)?;
        let expected_thread = self
            .upstream_threads
            .get(upstream_thread_id)
            .copied()
            .ok_or_else(|| anyhow!("unknown upstream thread {upstream_thread_id}"))?;
        if expected_thread != thread_id {
            return Err(anyhow!(
                "upstream thread/turn mismatch for {upstream_turn_id}"
            ));
        }
        Ok((thread_id, turn_id))
    }

    fn summarize_upstream_plan_update(
        explanation: Option<&str>,
        plan: &[mli_upstream_protocol::TurnPlanStep],
    ) -> String {
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
                            match entry.status {
                                mli_upstream_protocol::TurnPlanStepStatus::Pending => "pending",
                                mli_upstream_protocol::TurnPlanStepStatus::InProgress =>
                                    "in_progress",
                                mli_upstream_protocol::TurnPlanStepStatus::Completed => {
                                    "completed"
                                }
                            },
                            entry.step.trim()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            );
        }
        parts.join(" | ")
    }

    fn append_transcript_event<T: serde::Serialize>(
        &mut self,
        thread_id: LocalThreadId,
        turn_id: Option<LocalTurnId>,
        source: TranscriptEventSource,
        payload: &T,
    ) -> Result<()> {
        let seq = self.thread_seq.entry(thread_id).or_insert(0);
        *seq += 1;
        self.transcript_repo.append(&TranscriptEvent {
            seq: *seq,
            timestamp: utc_now(),
            thread_id,
            turn_id,
            source,
            payload: serde_json::to_value(payload)
                .context("failed to serialize transcript payload")?,
        })
    }

    fn sync_known_artifacts(&mut self, thread_id: LocalThreadId) -> Result<()> {
        let scan = self
            .artifact_service
            .scan_artifacts(mli_types::ArtifactQuery {
                thread_id: Some(thread_id),
                kind: None,
                limit: None,
            })?;
        let entry = self.known_artifacts.entry(thread_id).or_default();
        entry.clear();
        for manifest in scan.manifests {
            entry.insert(manifest.id, manifest.updated_at);
        }
        let warnings = self.artifact_warnings.entry(thread_id).or_default();
        warnings.clear();
        for warning in scan.warnings {
            warnings.insert(warning.path, warning.message);
        }
        Ok(())
    }

    fn scan_artifact_changes(
        &mut self,
        thread_id: LocalThreadId,
    ) -> Result<Vec<ServerNotification>> {
        let scan = self
            .artifact_service
            .scan_artifacts(mli_types::ArtifactQuery {
                thread_id: Some(thread_id),
                kind: None,
                limit: None,
            })?;
        let known = self.known_artifacts.entry(thread_id).or_default();
        let known_warnings = self.artifact_warnings.entry(thread_id).or_default();
        let mut notifications = Vec::new();
        let mut warning_paths = HashSet::new();
        for warning in scan.warnings {
            warning_paths.insert(warning.path.clone());
            let should_emit = match known_warnings.get(&warning.path) {
                Some(message) => message != &warning.message,
                None => true,
            };
            known_warnings.insert(warning.path.clone(), warning.message.clone());
            if should_emit {
                notifications.push(ServerNotification::Warning {
                    params: WarningNotification {
                        message: format!(
                            "Skipped malformed artifact manifest {}: {}",
                            warning.path.display(),
                            warning.message
                        ),
                        thread_id: Some(thread_id),
                        turn_id: None,
                    },
                });
            }
        }
        known_warnings.retain(|path, _| warning_paths.contains(path));

        for manifest in scan.manifests {
            let preview = self.artifact_service.preview(&manifest);
            match known.get(&manifest.id) {
                None => notifications.push(ServerNotification::ArtifactCreated {
                    params: ArtifactCreatedNotification {
                        manifest: manifest.clone(),
                        preview: preview.clone(),
                    },
                }),
                Some(updated_at) if updated_at != &manifest.updated_at => {
                    notifications.push(ServerNotification::ArtifactUpdated {
                        params: mli_protocol::ArtifactUpdatedNotification {
                            manifest: manifest.clone(),
                            preview: preview.clone(),
                        },
                    });
                }
                _ => {}
            }
            known.insert(manifest.id, manifest.updated_at);
        }
        Ok(notifications)
    }

    fn enqueue_notification(&mut self, notification: ServerNotification) -> Result<()> {
        match &notification {
            ServerNotification::ArtifactCreated { params } => {
                self.append_transcript_event(
                    params.manifest.local_thread_id,
                    Some(params.manifest.local_turn_id),
                    TranscriptEventSource::ArtifactSystem,
                    params,
                )?;
            }
            ServerNotification::ArtifactUpdated { params } => {
                self.append_transcript_event(
                    params.manifest.local_thread_id,
                    Some(params.manifest.local_turn_id),
                    TranscriptEventSource::ArtifactSystem,
                    params,
                )?;
            }
            ServerNotification::Warning { params } => {
                if let Some(thread_id) = params.thread_id {
                    self.append_transcript_event(
                        thread_id,
                        params.turn_id,
                        TranscriptEventSource::ArtifactSystem,
                        params,
                    )?;
                }
            }
            _ => {}
        }
        self.pending_notifications.push_back(notification);
        Ok(())
    }

    fn rehydrate_active_turn_binding(
        &mut self,
        thread: &ThreadRecord,
    ) -> Result<Option<TurnRecord>> {
        let Some(upstream_thread_id) = &thread.upstream_thread_id else {
            return Ok(None);
        };
        self.upstream_threads
            .insert(upstream_thread_id.as_str().to_owned(), thread.id);
        self.upstream_turns
            .retain(|_, (thread_id, _)| *thread_id != thread.id);
        let turns = self.turn_repo.list_by_thread(thread.id)?;
        let active_turn = turns.into_iter().rev().find(|turn| {
            turn.upstream_turn_id.is_some()
                && matches!(
                    turn.status,
                    TurnStatus::Starting | TurnStatus::Streaming | TurnStatus::WaitingApproval
                )
        });
        if let Some(turn) = active_turn.clone()
            && let Some(upstream_turn_id) = &turn.upstream_turn_id
        {
            self.upstream_turns
                .insert(upstream_turn_id.as_str().to_owned(), (thread.id, turn.id));
        }
        Ok(active_turn)
    }

    fn rehydrate_pending_approval(
        &mut self,
        thread: &ThreadRecord,
        turn_id: Option<LocalTurnId>,
    ) -> Result<()> {
        self.pending_approvals
            .retain(|_, context| context.thread_id != thread.id);
        let Some(turn_id) = turn_id else {
            return Ok(());
        };
        let Some(approval) = self.find_unresolved_approval(thread.id, turn_id)? else {
            return Ok(());
        };
        let request_id = request_id_from_string(&approval.id);
        let request = request_from_pending_approval(request_id.clone(), &approval)?;
        self.pending_approvals.insert(
            approval.id.clone(),
            PendingApprovalContext {
                request_id,
                thread_id: thread.id,
                turn_id,
                request,
            },
        );
        Ok(())
    }

    fn find_unresolved_approval(
        &self,
        thread_id: LocalThreadId,
        turn_id: LocalTurnId,
    ) -> Result<Option<PendingApproval>> {
        let mut pending = None;
        for event in self.transcript_repo.list(thread_id)? {
            if event.turn_id != Some(turn_id) || event.source != TranscriptEventSource::Wrapper {
                continue;
            }
            if let Ok(notification) = serde_json::from_value::<
                mli_protocol::ApprovalRequestNotification,
            >(event.payload.clone())
            {
                pending = Some(notification.approval);
                continue;
            }
            let Some(approval_id) = event
                .payload
                .get("approval_id")
                .and_then(|value| value.as_str())
            else {
                continue;
            };
            let resolved = event
                .payload
                .get("resolved")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
                || event.payload.get("decision").is_some();
            if resolved
                && pending
                    .as_ref()
                    .is_some_and(|approval| approval.id == approval_id)
            {
                pending = None;
            }
        }
        Ok(pending)
    }
}

fn bridge_environment(paths: &AppPaths) -> Result<Vec<(String, String)>> {
    let mut env = vec![
        (
            "MLI_INSTALL_ROOT".to_owned(),
            paths.install_root.display().to_string(),
        ),
        (
            "MLI_HELPER_PYTHON_SRC".to_owned(),
            paths.helper_python_src.display().to_string(),
        ),
        (
            "MLI_HELPER_NODE_SRC".to_owned(),
            paths.helper_node_src.display().to_string(),
        ),
    ];
    env.push((
        "PYTHONPATH".to_owned(),
        prepend_env_path("PYTHONPATH", &paths.helper_python_src)?,
    ));
    Ok(env)
}

fn command_output_delta_payload(
    params: &CommandExecutionOutputDeltaNotification,
) -> serde_json::Value {
    serde_json::json!({
        "event": "command_execution_output_delta",
        "item_id": params.item_id,
        "delta": params.delta,
    })
}

fn prepend_env_path(var_name: &str, prefix: &Path) -> Result<String> {
    let mut paths = vec![prefix.to_path_buf()];
    if let Some(existing) = std::env::var_os(var_name) {
        paths.extend(std::env::split_paths(&existing));
    }
    let joined = std::env::join_paths(paths)
        .with_context(|| format!("failed to join {var_name} search path"))?;
    Ok(joined.to_string_lossy().into_owned())
}

fn approval_response_value(
    request: &UpstreamServerRequest,
    params: &ApprovalRespondParams,
) -> Result<serde_json::Value> {
    match request {
        UpstreamServerRequest::CommandExecutionRequestApproval { .. } => {
            serde_json::to_value(CommandExecutionRequestApprovalResponse {
                decision: match params.decision {
                    ApprovalDecision::Approve => CommandExecutionApprovalDecision::Accept,
                    ApprovalDecision::Reject => CommandExecutionApprovalDecision::Decline,
                },
            })
            .context("failed to encode command approval response")
        }
        UpstreamServerRequest::FileChangeRequestApproval { .. } => {
            serde_json::to_value(FileChangeRequestApprovalResponse {
                decision: match params.decision {
                    ApprovalDecision::Approve => FileChangeApprovalDecision::Accept,
                    ApprovalDecision::Reject => FileChangeApprovalDecision::Decline,
                },
            })
            .context("failed to encode file approval response")
        }
        UpstreamServerRequest::PermissionsRequestApproval {
            params: request, ..
        } => serde_json::to_value(PermissionsRequestApprovalResponse {
            permissions: match params.decision {
                ApprovalDecision::Approve => {
                    GrantedPermissionProfile::from(request.permissions.clone())
                }
                ApprovalDecision::Reject => GrantedPermissionProfile::default(),
            },
            scope: PermissionGrantScope::Turn,
        })
        .context("failed to encode permissions approval response"),
        UpstreamServerRequest::ToolRequestUserInput { .. } => {
            let answers = match params.decision {
                ApprovalDecision::Approve => params
                    .answers
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(id, answer)| {
                        (
                            id,
                            ToolRequestUserInputAnswer {
                                answers: answer.answers,
                            },
                        )
                    })
                    .collect(),
                ApprovalDecision::Reject => HashMap::new(),
            };
            serde_json::to_value(ToolRequestUserInputResponse { answers })
                .context("failed to encode request_user_input response")
        }
    }
}

fn request_id_from_string(value: &str) -> UpstreamRequestId {
    value
        .parse::<i64>()
        .map(UpstreamRequestId::Integer)
        .unwrap_or_else(|_| UpstreamRequestId::String(value.to_owned()))
}

fn request_from_pending_approval(
    request_id: UpstreamRequestId,
    approval: &PendingApproval,
) -> Result<UpstreamServerRequest> {
    match approval.kind {
        ApprovalKind::CommandExecution => {
            Ok(UpstreamServerRequest::CommandExecutionRequestApproval {
                id: request_id,
                params: serde_json::from_value(approval.raw_payload.clone())
                    .context("failed to decode persisted command approval payload")?,
            })
        }
        ApprovalKind::FileChange => Ok(UpstreamServerRequest::FileChangeRequestApproval {
            id: request_id,
            params: serde_json::from_value(approval.raw_payload.clone())
                .context("failed to decode persisted file-change approval payload")?,
        }),
        ApprovalKind::PermissionRequest => Ok(UpstreamServerRequest::PermissionsRequestApproval {
            id: request_id,
            params: serde_json::from_value(approval.raw_payload.clone())
                .context("failed to decode persisted permission approval payload")?,
        }),
        ApprovalKind::RequestUserInput => Ok(UpstreamServerRequest::ToolRequestUserInput {
            id: request_id,
            params: serde_json::from_value(approval.raw_payload.clone())
                .context("failed to decode persisted request_user_input payload")?,
        }),
    }
}

fn upstream_request_id_to_string(request_id: &UpstreamRequestId) -> String {
    match request_id {
        UpstreamRequestId::String(value) => value.clone(),
        UpstreamRequestId::Integer(value) => value.to_string(),
    }
}

fn command_approval_title(params: &CommandExecutionRequestApprovalParams) -> String {
    match &params.command {
        Some(command) => format!("Approve command: {command}"),
        None => "Approve command execution".to_owned(),
    }
}

fn command_approval_description(params: &CommandExecutionRequestApprovalParams) -> String {
    let mut parts = Vec::new();
    if let Some(reason) = &params.reason {
        parts.push(reason.clone());
    }
    if let Some(cwd) = &params.cwd {
        parts.push(format!("cwd: {}", cwd.display()));
    }
    if parts.is_empty() {
        "Codex wants to run a command.".to_owned()
    } else {
        parts.join(" | ")
    }
}

fn file_change_approval_description(params: &FileChangeRequestApprovalParams) -> String {
    let mut parts = Vec::new();
    if let Some(reason) = &params.reason {
        parts.push(reason.clone());
    }
    if let Some(root) = &params.grant_root {
        parts.push(format!("grant root: {}", root.display()));
    }
    if parts.is_empty() {
        "Codex wants to write files.".to_owned()
    } else {
        parts.join(" | ")
    }
}

fn permissions_approval_description(params: &PermissionsRequestApprovalParams) -> String {
    let mut parts = Vec::new();
    if let Some(reason) = &params.reason {
        parts.push(reason.clone());
    }
    if params
        .permissions
        .network
        .as_ref()
        .and_then(|network| network.enabled)
        .unwrap_or(false)
    {
        parts.push("network".to_owned());
    }
    if let Some(file_system) = &params.permissions.file_system {
        if let Some(read) = &file_system.read
            && !read.is_empty()
        {
            parts.push(format!("read: {}", join_paths(read)));
        }
        if let Some(write) = &file_system.write
            && !write.is_empty()
        {
            parts.push(format!("write: {}", join_paths(write)));
        }
    }
    if parts.is_empty() {
        "Codex requested extra permissions.".to_owned()
    } else {
        parts.join(" | ")
    }
}

fn request_user_input_description(params: &ToolRequestUserInputParams) -> String {
    let question_count = params.questions.len();
    let headers = params
        .questions
        .iter()
        .take(2)
        .map(|question| question.header.clone())
        .collect::<Vec<_>>();
    if headers.is_empty() {
        format!("{question_count} question(s)")
    } else {
        format!("{question_count} question(s): {}", headers.join(", "))
    }
}

fn join_paths(paths: &[std::path::PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn into_upstream_input(input: UserInput) -> UpstreamUserInput {
    match input {
        UserInput::Text { text } => UpstreamUserInput::Text { text },
        UserInput::Image { url } => UpstreamUserInput::Image { url },
        UserInput::LocalImage { path } => UpstreamUserInput::LocalImage { path },
        UserInput::Skill { name, path } => UpstreamUserInput::Skill { name, path },
        UserInput::Mention { name, path } => UpstreamUserInput::Mention { name, path },
    }
}

pub fn default_initialize_params() -> InitializeParams {
    InitializeParams {
        client_info: ClientInfo {
            name: "ml-intern-codex-tui".to_owned(),
            title: Some("ml-intern-codex".to_owned()),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        capabilities: ClientCapabilities {
            transcript_streaming: true,
            skill_picker: true,
            artifacts_overlay: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use mli_config::AppPaths;
    use mli_protocol::ApprovalRequestNotification;
    use mli_repo::{ThreadRepo, TranscriptRepo, TurnRepo};
    use mli_types::{ApprovalPolicy, SandboxMode, utc_now};

    #[derive(Default)]
    struct StubBridge;

    impl CodexBridge for StubBridge {
        fn initialize(&mut self) -> Result<mli_upstream_protocol::InitializeResult> {
            Ok(mli_upstream_protocol::InitializeResult::default())
        }

        fn thread_start(
            &mut self,
            _params: UpstreamThreadStartParams,
        ) -> Result<mli_upstream_protocol::ThreadStartResult> {
            panic!("thread_start should not be called in resume tests");
        }

        fn thread_resume(
            &mut self,
            params: UpstreamThreadResumeParams,
        ) -> Result<mli_upstream_protocol::ThreadResumeResult> {
            Ok(mli_upstream_protocol::ThreadResumeResult {
                thread: mli_upstream_protocol::ThreadSummary {
                    id: params.thread_id,
                    preview: String::new(),
                    cwd: params.cwd.map(PathBuf::from).unwrap_or_default(),
                },
                model: String::new(),
                cwd: PathBuf::new(),
                approval_policy: None,
                sandbox: None,
            })
        }

        fn turn_start(
            &mut self,
            _params: UpstreamTurnStartParams,
        ) -> Result<mli_upstream_protocol::TurnStartResult> {
            panic!("turn_start should not be called in resume tests");
        }

        fn turn_interrupt(
            &mut self,
            _params: UpstreamTurnInterruptParams,
        ) -> Result<mli_upstream_protocol::TurnInterruptResult> {
            Ok(mli_upstream_protocol::TurnInterruptResult::default())
        }

        fn respond_to_server_request(
            &mut self,
            _request_id: UpstreamRequestId,
            _result: serde_json::Value,
        ) -> Result<()> {
            Ok(())
        }

        fn recv_event(&mut self) -> Result<Option<UpstreamEvent>> {
            Ok(None)
        }

        fn recv_event_blocking(&mut self) -> Result<Option<UpstreamEvent>> {
            Ok(None)
        }
    }

    struct QueueBridge {
        events: VecDeque<UpstreamEvent>,
    }

    impl CodexBridge for QueueBridge {
        fn initialize(&mut self) -> Result<mli_upstream_protocol::InitializeResult> {
            Ok(mli_upstream_protocol::InitializeResult::default())
        }

        fn thread_start(
            &mut self,
            _params: UpstreamThreadStartParams,
        ) -> Result<mli_upstream_protocol::ThreadStartResult> {
            panic!("thread_start should not be called in queue bridge tests");
        }

        fn thread_resume(
            &mut self,
            params: UpstreamThreadResumeParams,
        ) -> Result<mli_upstream_protocol::ThreadResumeResult> {
            Ok(mli_upstream_protocol::ThreadResumeResult {
                thread: mli_upstream_protocol::ThreadSummary {
                    id: params.thread_id,
                    preview: String::new(),
                    cwd: params.cwd.map(PathBuf::from).unwrap_or_default(),
                },
                model: String::new(),
                cwd: PathBuf::new(),
                approval_policy: None,
                sandbox: None,
            })
        }

        fn turn_start(
            &mut self,
            _params: UpstreamTurnStartParams,
        ) -> Result<mli_upstream_protocol::TurnStartResult> {
            panic!("turn_start should not be called in queue bridge tests");
        }

        fn turn_interrupt(
            &mut self,
            _params: UpstreamTurnInterruptParams,
        ) -> Result<mli_upstream_protocol::TurnInterruptResult> {
            Ok(mli_upstream_protocol::TurnInterruptResult::default())
        }

        fn respond_to_server_request(
            &mut self,
            _request_id: UpstreamRequestId,
            _result: serde_json::Value,
        ) -> Result<()> {
            Ok(())
        }

        fn recv_event(&mut self) -> Result<Option<UpstreamEvent>> {
            Ok(self.events.pop_front())
        }

        fn recv_event_blocking(&mut self) -> Result<Option<UpstreamEvent>> {
            Ok(self.events.pop_front())
        }
    }

    #[derive(Default)]
    struct CapturingBridge {
        events: VecDeque<UpstreamEvent>,
        responses: Vec<(UpstreamRequestId, serde_json::Value)>,
    }

    impl CodexBridge for CapturingBridge {
        fn initialize(&mut self) -> Result<mli_upstream_protocol::InitializeResult> {
            Ok(mli_upstream_protocol::InitializeResult::default())
        }

        fn thread_start(
            &mut self,
            _params: UpstreamThreadStartParams,
        ) -> Result<mli_upstream_protocol::ThreadStartResult> {
            panic!("thread_start should not be called in capturing bridge tests");
        }

        fn thread_resume(
            &mut self,
            params: UpstreamThreadResumeParams,
        ) -> Result<mli_upstream_protocol::ThreadResumeResult> {
            Ok(mli_upstream_protocol::ThreadResumeResult {
                thread: mli_upstream_protocol::ThreadSummary {
                    id: params.thread_id,
                    preview: String::new(),
                    cwd: params.cwd.map(PathBuf::from).unwrap_or_default(),
                },
                model: String::new(),
                cwd: PathBuf::new(),
                approval_policy: None,
                sandbox: None,
            })
        }

        fn turn_start(
            &mut self,
            _params: UpstreamTurnStartParams,
        ) -> Result<mli_upstream_protocol::TurnStartResult> {
            panic!("turn_start should not be called in capturing bridge tests");
        }

        fn turn_interrupt(
            &mut self,
            _params: UpstreamTurnInterruptParams,
        ) -> Result<mli_upstream_protocol::TurnInterruptResult> {
            Ok(mli_upstream_protocol::TurnInterruptResult::default())
        }

        fn respond_to_server_request(
            &mut self,
            request_id: UpstreamRequestId,
            result: serde_json::Value,
        ) -> Result<()> {
            self.responses.push((request_id, result));
            Ok(())
        }

        fn recv_event(&mut self) -> Result<Option<UpstreamEvent>> {
            Ok(self.events.pop_front())
        }

        fn recv_event_blocking(&mut self) -> Result<Option<UpstreamEvent>> {
            Ok(self.events.pop_front())
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("mli-runtime-session-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create temp dir: {error}"));
        path
    }

    fn app_paths(root: &std::path::Path) -> AppPaths {
        AppPaths {
            cwd: root.to_path_buf(),
            install_root: root.to_path_buf(),
            app_home: root.join("home"),
            user_config_path: root.join("home/config.toml"),
            user_logs_tui_dir: root.join("home/logs/tui"),
            user_logs_app_server_dir: root.join("home/logs/app-server"),
            runtime_dir: root.join("home/runtime"),
            codex_home_dir: root.join("home/runtime/codex-home"),
            generated_skills_dir: root.join("home/runtime/generated-skills"),
            cache_dir: root.join("home/cache"),
            db_dir: root.join("home/db"),
            db_path: root.join("home/db/state.sqlite"),
            project_root: root.join(".ml-intern"),
            project_config_path: root.join(".ml-intern/config.toml"),
            threads_root: root.join(".ml-intern/threads"),
            bundled_skills_root: root.join("skills/system"),
            helper_python_src: root.join("helpers/python/src"),
            helper_node_src: root.join("helpers/node/src"),
        }
    }

    fn thread_record(paths: &AppPaths, status: ThreadStatus) -> ThreadRecord {
        let thread_id = LocalThreadId::new();
        let mut record = ThreadRecord::new(
            paths.cwd.clone(),
            Some("resume me".to_owned()),
            None,
            ApprovalPolicy::OnRequest,
            SandboxMode::WorkspaceWrite,
            paths.transcript_file(thread_id),
            paths.artifacts_dir(thread_id),
        );
        record.id = thread_id;
        record.status = status;
        record.updated_at = utc_now();
        record.upstream_thread_id = Some(UpstreamThreadId::from("upstream-thread-1".to_owned()));
        record
    }

    fn turn_record(thread: &ThreadRecord, status: TurnStatus) -> TurnRecord {
        let mut turn = TurnRecord::new(thread.id, "resume turn".to_owned());
        turn.status = status;
        turn.upstream_turn_id = Some(UpstreamTurnId::from("upstream-turn-1".to_owned()));
        turn
    }

    fn assert_resume_preserves_status(status: ThreadStatus) {
        let root = temp_dir(match status {
            ThreadStatus::Running => "running",
            ThreadStatus::WaitingApproval => "approval",
            _ => "other",
        });
        let paths = app_paths(&root);
        paths
            .ensure_base_layout()
            .unwrap_or_else(|error| panic!("create base layout: {error}"));
        let repo = FsThreadRepo::new(paths.clone());
        let thread = thread_record(&paths, status.clone());
        repo.create(&thread)
            .unwrap_or_else(|error| panic!("create thread: {error}"));

        let mut session = RuntimeSession::new(AppConfig::default(), paths.clone(), StubBridge);
        let (result, notifications) = session
            .resume_thread(ThreadResumeParams {
                thread_id: thread.id,
            })
            .unwrap_or_else(|error| panic!("resume thread: {error}"));

        assert_eq!(result.thread.status, status);
        let persisted = repo
            .get(thread.id)
            .unwrap_or_else(|error| panic!("read thread: {error}"))
            .unwrap_or_else(|| panic!("missing persisted thread"));
        assert_eq!(persisted.status, status);

        match &notifications[..] {
            [ServerNotification::ThreadStatusChanged { params }] => {
                assert_eq!(params.thread.status, status);
            }
            other => panic!("unexpected notifications: {other:?}"),
        }
    }

    #[test]
    fn resume_thread_keeps_running_status() {
        assert_resume_preserves_status(ThreadStatus::Running);
    }

    #[test]
    fn resume_thread_keeps_waiting_approval_status() {
        assert_resume_preserves_status(ThreadStatus::WaitingApproval);
    }

    #[test]
    fn resume_thread_rehydrates_running_turn_for_interrupts() {
        let root = temp_dir("running-turn");
        let paths = app_paths(&root);
        paths
            .ensure_base_layout()
            .unwrap_or_else(|error| panic!("create base layout: {error}"));
        let thread_repo = FsThreadRepo::new(paths.clone());
        let turn_repo = FsTurnRepo::new(paths.clone());
        let thread = thread_record(&paths, ThreadStatus::Running);
        let turn = turn_record(&thread, TurnStatus::Streaming);
        thread_repo
            .create(&thread)
            .unwrap_or_else(|error| panic!("create thread: {error}"));
        turn_repo
            .create(&turn)
            .unwrap_or_else(|error| panic!("create turn: {error}"));

        let mut session = RuntimeSession::new(AppConfig::default(), paths.clone(), StubBridge);
        session
            .resume_thread(ThreadResumeParams {
                thread_id: thread.id,
            })
            .unwrap_or_else(|error| panic!("resume thread: {error}"));
        session
            .interrupt_turn(TurnInterruptParams {
                thread_id: thread.id,
                turn_id: turn.id,
            })
            .unwrap_or_else(|error| panic!("interrupt resumed turn: {error}"));
    }

    #[test]
    fn resume_thread_rehydrates_waiting_approval_context() {
        let root = temp_dir("pending-approval");
        let paths = app_paths(&root);
        paths
            .ensure_base_layout()
            .unwrap_or_else(|error| panic!("create base layout: {error}"));
        let thread_repo = FsThreadRepo::new(paths.clone());
        let turn_repo = FsTurnRepo::new(paths.clone());
        let transcript_repo = FsTranscriptRepo::new(paths.clone());
        let thread = thread_record(&paths, ThreadStatus::WaitingApproval);
        let turn = turn_record(&thread, TurnStatus::WaitingApproval);
        let approval = PendingApproval {
            id: "42".to_owned(),
            kind: ApprovalKind::PermissionRequest,
            title: "Approve additional permissions".to_owned(),
            description: "network".to_owned(),
            raw_payload: serde_json::json!({
                "threadId": "upstream-thread-1",
                "turnId": "upstream-turn-1",
                "itemId": "perm-item-1",
                "reason": "Need network",
                "permissions": {
                    "network": { "enabled": true }
                }
            }),
        };
        thread_repo
            .create(&thread)
            .unwrap_or_else(|error| panic!("create thread: {error}"));
        turn_repo
            .create(&turn)
            .unwrap_or_else(|error| panic!("create turn: {error}"));
        transcript_repo
            .append(&TranscriptEvent {
                seq: 1,
                timestamp: utc_now(),
                thread_id: thread.id,
                turn_id: Some(turn.id),
                source: TranscriptEventSource::Wrapper,
                payload: serde_json::to_value(ApprovalRequestNotification {
                    approval: approval.clone(),
                })
                .unwrap_or_else(|error| panic!("encode approval transcript: {error}")),
            })
            .unwrap_or_else(|error| panic!("append approval transcript: {error}"));

        let mut session = RuntimeSession::new(AppConfig::default(), paths.clone(), StubBridge);
        session
            .resume_thread(ThreadResumeParams {
                thread_id: thread.id,
            })
            .unwrap_or_else(|error| panic!("resume thread: {error}"));
        session
            .respond_to_approval(ApprovalRespondParams {
                approval_id: approval.id,
                decision: ApprovalDecision::Reject,
                answers: None,
            })
            .unwrap_or_else(|error| panic!("respond to restored approval: {error}"));
    }

    #[test]
    fn command_execution_output_delta_is_forwarded_and_persisted_for_replay() {
        let root = temp_dir("command-output-delta");
        let paths = app_paths(&root);
        paths
            .ensure_base_layout()
            .unwrap_or_else(|error| panic!("create base layout: {error}"));
        let thread_repo = FsThreadRepo::new(paths.clone());
        let turn_repo = FsTurnRepo::new(paths.clone());
        let transcript_repo = FsTranscriptRepo::new(paths.clone());
        let thread = thread_record(&paths, ThreadStatus::Running);
        let turn = turn_record(&thread, TurnStatus::Streaming);
        thread_repo
            .create(&thread)
            .unwrap_or_else(|error| panic!("create thread: {error}"));
        turn_repo
            .create(&turn)
            .unwrap_or_else(|error| panic!("create turn: {error}"));

        let mut session = RuntimeSession::new(
            AppConfig::default(),
            paths.clone(),
            QueueBridge {
                events: VecDeque::from([UpstreamEvent::Notification(
                    UpstreamNotification::CommandExecutionOutputDelta {
                        params: CommandExecutionOutputDeltaNotification {
                            thread_id: thread
                                .upstream_thread_id
                                .clone()
                                .unwrap_or_else(|| panic!("missing upstream thread id"))
                                .to_string(),
                            turn_id: turn
                                .upstream_turn_id
                                .clone()
                                .unwrap_or_else(|| panic!("missing upstream turn id"))
                                .to_string(),
                            item_id: "cmd-1".to_owned(),
                            delta: "chunk 1\n".to_owned(),
                        },
                    },
                )]),
            },
        );
        session
            .resume_thread(ThreadResumeParams {
                thread_id: thread.id,
            })
            .unwrap_or_else(|error| panic!("resume thread: {error}"));

        let notification = session
            .recv_notification()
            .unwrap_or_else(|error| panic!("receive notification: {error}"))
            .unwrap_or_else(|| panic!("missing command output delta notification"));
        match notification {
            ServerNotification::CommandExecutionOutputDelta { params } => {
                assert_eq!(params.thread_id, thread.id);
                assert_eq!(params.turn_id, turn.id);
                assert_eq!(params.item_id, "cmd-1");
                assert_eq!(params.delta, "chunk 1\n");
            }
            other => panic!("unexpected notification: {other:?}"),
        }

        let transcript = transcript_repo
            .list(thread.id)
            .unwrap_or_else(|error| panic!("list transcript: {error}"));
        match transcript.as_slice() {
            [event] => {
                assert_eq!(event.source, TranscriptEventSource::UpstreamCodex);
                assert_eq!(
                    event.payload,
                    serde_json::json!({
                        "event": "command_execution_output_delta",
                        "item_id": "cmd-1",
                        "delta": "chunk 1\n"
                    })
                );
            }
            other => panic!("unexpected transcript events: {other:?}"),
        }
    }

    #[test]
    fn request_user_input_answers_are_forwarded_back_to_upstream() {
        let root = temp_dir("request-user-input");
        let paths = app_paths(&root);
        paths
            .ensure_base_layout()
            .unwrap_or_else(|error| panic!("create base layout: {error}"));
        let thread_repo = FsThreadRepo::new(paths.clone());
        let turn_repo = FsTurnRepo::new(paths.clone());
        let thread = thread_record(&paths, ThreadStatus::Running);
        let turn = turn_record(&thread, TurnStatus::Streaming);
        thread_repo
            .create(&thread)
            .unwrap_or_else(|error| panic!("create thread: {error}"));
        turn_repo
            .create(&turn)
            .unwrap_or_else(|error| panic!("create turn: {error}"));

        let request_id = UpstreamRequestId::Integer(41);
        let mut session = RuntimeSession::new(
            AppConfig::default(),
            paths.clone(),
            CapturingBridge {
                events: VecDeque::from([UpstreamEvent::ServerRequest(Box::new(
                    UpstreamServerRequest::ToolRequestUserInput {
                        id: request_id.clone(),
                        params: ToolRequestUserInputParams {
                            thread_id: thread
                                .upstream_thread_id
                                .clone()
                                .unwrap_or_else(|| panic!("missing upstream thread id"))
                                .to_string(),
                            turn_id: turn
                                .upstream_turn_id
                                .clone()
                                .unwrap_or_else(|| panic!("missing upstream turn id"))
                                .to_string(),
                            item_id: "tool-1".to_owned(),
                            questions: vec![mli_upstream_protocol::ToolRequestUserInputQuestion {
                                id: "dataset".to_owned(),
                                header: "Dataset".to_owned(),
                                question: "Pick the dataset".to_owned(),
                                is_other: false,
                                is_secret: false,
                                options: Some(vec![
                                    mli_upstream_protocol::ToolRequestUserInputOption {
                                        label: "demo".to_owned(),
                                        description: "Use the demo dataset".to_owned(),
                                    },
                                    mli_upstream_protocol::ToolRequestUserInputOption {
                                        label: "prod".to_owned(),
                                        description: "Use the production dataset".to_owned(),
                                    },
                                ]),
                            }],
                        },
                    },
                ))]),
                responses: Vec::new(),
            },
        );
        session
            .resume_thread(ThreadResumeParams {
                thread_id: thread.id,
            })
            .unwrap_or_else(|error| panic!("resume thread: {error}"));

        let notification = session
            .recv_notification()
            .unwrap_or_else(|error| panic!("receive approval request: {error}"))
            .unwrap_or_else(|| panic!("missing approval request notification"));
        let approval_id = match notification {
            ServerNotification::ApprovalRequested { params } => params.approval.id,
            other => panic!("unexpected notification: {other:?}"),
        };

        session
            .respond_to_approval(ApprovalRespondParams {
                approval_id,
                decision: ApprovalDecision::Approve,
                answers: Some(std::collections::BTreeMap::from([(
                    "dataset".to_owned(),
                    mli_protocol::ApprovalAnswer {
                        answers: vec!["prod".to_owned()],
                    },
                )])),
            })
            .unwrap_or_else(|error| panic!("respond to request_user_input approval: {error}"));

        assert_eq!(session.bridge.responses.len(), 1);
        let (captured_id, captured_value) = &session.bridge.responses[0];
        assert_eq!(captured_id, &request_id);
        assert_eq!(
            captured_value,
            &serde_json::json!({
                "answers": {
                    "dataset": {
                        "answers": ["prod"]
                    }
                }
            })
        );
    }
}
