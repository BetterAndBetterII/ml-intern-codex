use mli_types::{ArtifactManifest, ArtifactPreview, PendingApproval, ThreadRecord, TurnRecord};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreadStartedNotification {
    pub thread: ThreadRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreadStatusChangedNotification {
    pub thread: ThreadRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TurnStartedNotification {
    pub thread_id: mli_types::LocalThreadId,
    pub turn: TurnRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TurnCompletedNotification {
    pub thread_id: mli_types::LocalThreadId,
    pub turn: TurnRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanUpdatedNotification {
    pub thread_id: mli_types::LocalThreadId,
    pub turn_id: mli_types::LocalTurnId,
    pub summary: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemStartedNotification {
    pub thread_id: mli_types::LocalThreadId,
    pub turn_id: mli_types::LocalTurnId,
    pub item: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemCompletedNotification {
    pub thread_id: mli_types::LocalThreadId,
    pub turn_id: mli_types::LocalTurnId,
    pub item: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentMessageDeltaNotification {
    pub thread_id: mli_types::LocalThreadId,
    pub turn_id: mli_types::LocalTurnId,
    pub item_id: String,
    pub delta: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandExecutionOutputDeltaNotification {
    pub thread_id: mli_types::LocalThreadId,
    pub turn_id: mli_types::LocalTurnId,
    pub item_id: String,
    pub delta: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeStatusChangedNotification {
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillsChangedNotification {
    pub skills: Vec<mli_types::SkillDescriptor>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactCreatedNotification {
    pub manifest: ArtifactManifest,
    pub preview: ArtifactPreview,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactUpdatedNotification {
    pub manifest: ArtifactManifest,
    pub preview: ArtifactPreview,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApprovalRequestNotification {
    pub approval: PendingApproval,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErrorNotification {
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WarningNotification {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<mli_types::LocalThreadId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<mli_types::LocalTurnId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "camelCase")]
pub enum ServerNotification {
    #[serde(rename = "thread/started")]
    ThreadStarted { params: ThreadStartedNotification },
    #[serde(rename = "thread/statusChanged")]
    ThreadStatusChanged {
        params: ThreadStatusChangedNotification,
    },
    #[serde(rename = "turn/started")]
    TurnStarted { params: TurnStartedNotification },
    #[serde(rename = "turn/completed")]
    TurnCompleted { params: TurnCompletedNotification },
    #[serde(rename = "turn/plan/updated")]
    PlanUpdated { params: PlanUpdatedNotification },
    #[serde(rename = "item/started")]
    ItemStarted { params: ItemStartedNotification },
    #[serde(rename = "item/completed")]
    ItemCompleted { params: ItemCompletedNotification },
    #[serde(rename = "item/agentMessage/delta")]
    AgentMessageDelta {
        params: AgentMessageDeltaNotification,
    },
    #[serde(rename = "item/commandExecution/outputDelta")]
    CommandExecutionOutputDelta {
        params: CommandExecutionOutputDeltaNotification,
    },
    #[serde(rename = "runtime/statusChanged")]
    RuntimeStatusChanged {
        params: RuntimeStatusChangedNotification,
    },
    #[serde(rename = "skills/changed")]
    SkillsChanged { params: SkillsChangedNotification },
    #[serde(rename = "artifact/created")]
    ArtifactCreated { params: ArtifactCreatedNotification },
    #[serde(rename = "artifact/updated")]
    ArtifactUpdated { params: ArtifactUpdatedNotification },
    #[serde(rename = "approval/requested")]
    ApprovalRequested { params: ApprovalRequestNotification },
    #[serde(rename = "warning")]
    Warning { params: WarningNotification },
    #[serde(rename = "error")]
    Error { params: ErrorNotification },
}
