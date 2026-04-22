use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ArtifactManifest, ArtifactPreview, LocalThreadId, ThreadRecord};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    Never,
    OnFailure,
    OnRequest,
    Untrusted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    CommandExecution,
    FileChange,
    PermissionRequest,
    RequestUserInput,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingApproval {
    pub id: String,
    pub kind: ApprovalKind,
    pub title: String,
    pub description: String,
    pub raw_payload: Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Booting,
    Connecting,
    Initializing,
    Ready,
    Streaming,
    WaitingApproval,
    Disconnected,
    Reconnecting,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreadListItem {
    pub thread: ThreadRecord,
    pub selected: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TranscriptState {
    pub history: Vec<HistoryCellModel>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ComposerState {
    pub buffer: String,
    pub cursor: usize,
    pub skill_query: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ArtifactUiState {
    pub manifests: Vec<ArtifactManifest>,
    pub selected_artifact_id: Option<crate::ArtifactId>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ApprovalUiState {
    pub pending: Option<PendingApproval>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RuntimeBannerState {
    pub cwd: Option<PathBuf>,
    pub codex_version: Option<String>,
    pub approval_policy: Option<ApprovalPolicy>,
    pub sandbox_mode: Option<SandboxMode>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppState {
    pub connection: ConnectionState,
    pub active_thread_id: Option<LocalThreadId>,
    pub runtime: RuntimeBannerState,
    pub threads: Vec<ThreadListItem>,
    pub transcript: TranscriptState,
    pub composer: ComposerState,
    pub artifacts: ArtifactUiState,
    pub approvals: ApprovalUiState,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            connection: ConnectionState::Booting,
            active_thread_id: None,
            runtime: RuntimeBannerState::default(),
            threads: Vec::new(),
            transcript: TranscriptState::default(),
            composer: ComposerState::default(),
            artifacts: ArtifactUiState::default(),
            approvals: ApprovalUiState::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserMessageCell {
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssistantMessageCell {
    pub text: String,
    pub streaming: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecCommandCell {
    pub item_id: String,
    pub command: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecOutputCell {
    pub item_id: String,
    pub command: String,
    pub output: String,
    pub streaming: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PatchSummaryCell {
    pub summary: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanUpdateCell {
    pub summary: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApprovalCell {
    pub approval: PendingApproval,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactEventCell {
    pub manifest: ArtifactManifest,
    pub preview: ArtifactPreview,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WarningCell {
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErrorCell {
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatusCell {
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HistoryCellModel {
    UserMessage(UserMessageCell),
    AssistantMessage(AssistantMessageCell),
    ExecCommand(ExecCommandCell),
    ExecOutput(ExecOutputCell),
    PatchSummary(PatchSummaryCell),
    PlanUpdate(PlanUpdateCell),
    ApprovalRequest(ApprovalCell),
    ArtifactCreated(ArtifactEventCell),
    ArtifactUpdated(ArtifactEventCell),
    Warning(WarningCell),
    Error(ErrorCell),
    Status(StatusCell),
}
