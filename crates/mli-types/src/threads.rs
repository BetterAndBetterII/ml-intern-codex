use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ApprovalPolicy, SandboxMode};
use crate::{LocalThreadId, LocalTurnId, UpstreamThreadId, UpstreamTurnId, utc_now};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    NotLoaded,
    Idle,
    Starting,
    Running,
    WaitingApproval,
    Interrupted,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThreadRecord {
    pub id: LocalThreadId,
    pub upstream_thread_id: Option<UpstreamThreadId>,
    pub cwd: PathBuf,
    pub title: Option<String>,
    pub model: Option<String>,
    pub approval_policy: ApprovalPolicy,
    pub sandbox_mode: SandboxMode,
    pub status: ThreadStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub transcript_path: PathBuf,
    pub artifact_root: PathBuf,
}

impl ThreadRecord {
    pub fn new(
        cwd: PathBuf,
        title: Option<String>,
        model: Option<String>,
        approval_policy: ApprovalPolicy,
        sandbox_mode: SandboxMode,
        transcript_path: PathBuf,
        artifact_root: PathBuf,
    ) -> Self {
        let now = utc_now();
        Self {
            id: LocalThreadId::new(),
            upstream_thread_id: None,
            cwd,
            title,
            model,
            approval_policy,
            sandbox_mode,
            status: ThreadStatus::Starting,
            created_at: now,
            updated_at: now,
            transcript_path,
            artifact_root,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Pending,
    Starting,
    Streaming,
    WaitingApproval,
    Completed,
    Interrupted,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TurnRecord {
    pub id: LocalTurnId,
    pub local_thread_id: LocalThreadId,
    pub upstream_turn_id: Option<UpstreamTurnId>,
    pub status: TurnStatus,
    pub user_input_summary: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl TurnRecord {
    pub fn new(local_thread_id: LocalThreadId, user_input_summary: String) -> Self {
        Self {
            id: LocalTurnId::new(),
            local_thread_id,
            upstream_turn_id: None,
            status: TurnStatus::Pending,
            user_input_summary,
            started_at: utc_now(),
            finished_at: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StartThreadRequest {
    pub cwd: PathBuf,
    pub title: Option<String>,
    pub model: Option<String>,
    pub approval_policy: Option<ApprovalPolicy>,
    pub sandbox_mode: Option<SandboxMode>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StartTurnRequest {
    pub thread_id: LocalThreadId,
    pub user_input_summary: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptEventSource {
    User,
    Wrapper,
    UpstreamCodex,
    ArtifactSystem,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranscriptEvent {
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    pub thread_id: LocalThreadId,
    pub turn_id: Option<LocalTurnId>,
    pub source: TranscriptEventSource,
    pub payload: Value,
}
