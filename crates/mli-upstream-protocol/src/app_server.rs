use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Hash)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Integer(i64),
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Notification(JsonRpcNotification),
    Response(JsonRpcResponse),
    Error(JsonRpcError),
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct JsonRpcRequest {
    pub id: RequestId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct JsonRpcNotification {
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct JsonRpcResponse {
    pub id: RequestId,
    pub result: Value,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct JsonRpcError {
    pub error: JsonRpcErrorPayload,
    pub id: RequestId,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct JsonRpcErrorPayload {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub client_info: ClientInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<InitializeCapabilities>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    pub title: Option<String>,
    pub version: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct InitializeCapabilities {
    #[serde(default)]
    pub experimental_api: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_info: Option<ServerInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_home: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AskForApproval {
    Untrusted,
    OnFailure,
    OnRequest,
    Never,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartParams {
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub approval_policy: Option<AskForApproval>,
    pub sandbox: Option<SandboxMode>,
    pub base_instructions: Option<String>,
    pub developer_instructions: Option<String>,
    #[serde(default)]
    pub experimental_raw_events: bool,
    #[serde(default)]
    pub persist_extended_history: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResumeParams {
    pub thread_id: String,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub approval_policy: Option<AskForApproval>,
    pub sandbox: Option<SandboxMode>,
    pub base_instructions: Option<String>,
    pub developer_instructions: Option<String>,
    #[serde(default)]
    pub persist_extended_history: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    pub id: String,
    #[serde(default)]
    pub preview: String,
    #[serde(default)]
    pub cwd: PathBuf,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartResult {
    pub thread: ThreadSummary,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<AskForApproval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<Value>,
}

pub type ThreadResumeResult = ThreadStartResult;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UserInput {
    Text { text: String },
    Image { url: String },
    LocalImage { path: PathBuf },
    Skill { name: String, path: PathBuf },
    Mention { name: String, path: String },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    Completed,
    Interrupted,
    Failed,
    InProgress,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TurnError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_details: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnSummary {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TurnStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<TurnError>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams {
    pub thread_id: String,
    pub input: Vec<UserInput>,
    pub cwd: Option<PathBuf>,
    pub approval_policy: Option<AskForApproval>,
    pub model: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartResult {
    pub turn: TurnSummary,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptResult {}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CommandExecutionApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CommandExecutionRequestApprovalParams {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_actions: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_permissions: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_approval_context: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_execpolicy_amendment: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_network_policy_amendments: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_decisions: Option<Vec<Value>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CommandExecutionRequestApprovalResponse {
    pub decision: CommandExecutionApprovalDecision,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileChangeApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeRequestApprovalParams {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_root: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FileChangeRequestApprovalResponse {
    pub decision: FileChangeApprovalDecision,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdditionalNetworkPermissions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdditionalFileSystemPermissions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<Vec<PathBuf>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write: Option<Vec<PathBuf>>,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RequestPermissionProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<AdditionalNetworkPermissions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_system: Option<AdditionalFileSystemPermissions>,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GrantedPermissionProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<AdditionalNetworkPermissions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_system: Option<AdditionalFileSystemPermissions>,
}

impl From<RequestPermissionProfile> for GrantedPermissionProfile {
    fn from(value: RequestPermissionProfile) -> Self {
        Self {
            network: value.network,
            file_system: value.file_system,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum PermissionGrantScope {
    #[default]
    Turn,
    Session,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsRequestApprovalParams {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub permissions: RequestPermissionProfile,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsRequestApprovalResponse {
    pub permissions: GrantedPermissionProfile,
    #[serde(default)]
    pub scope: PermissionGrantScope,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolRequestUserInputOption {
    pub label: String,
    pub description: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolRequestUserInputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    #[serde(default)]
    pub is_other: bool,
    #[serde(default)]
    pub is_secret: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<ToolRequestUserInputOption>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolRequestUserInputParams {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub questions: Vec<ToolRequestUserInputQuestion>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolRequestUserInputAnswer {
    pub answers: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolRequestUserInputResponse {
    pub answers: HashMap<String, ToolRequestUserInputAnswer>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartedNotification {
    pub thread_id: String,
    pub turn: TurnSummary,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TurnCompletedNotification {
    pub thread_id: String,
    pub turn: TurnSummary,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ItemStartedNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item: Value,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ItemCompletedNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item: Value,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnPlanStep {
    pub step: String,
    pub status: TurnPlanStepStatus,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TurnPlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnPlanUpdatedNotification {
    pub turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    pub plan: Vec<TurnPlanStep>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageDeltaNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub delta: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandExecutionOutputDeltaNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub delta: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerRequestResolvedNotification {
    pub thread_id: String,
    pub request_id: RequestId,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ErrorNotification {
    pub error: TurnError,
    pub will_retry: bool,
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "method")]
pub enum UpstreamNotification {
    #[serde(rename = "turn/started")]
    TurnStarted { params: TurnStartedNotification },
    #[serde(rename = "turn/completed")]
    TurnCompleted { params: TurnCompletedNotification },
    #[serde(rename = "turn/plan/updated")]
    TurnPlanUpdated { params: TurnPlanUpdatedNotification },
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
    #[serde(rename = "serverRequest/resolved")]
    ServerRequestResolved {
        params: ServerRequestResolvedNotification,
    },
    #[serde(rename = "error")]
    Error { params: ErrorNotification },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "method")]
pub enum UpstreamServerRequest {
    #[serde(rename = "item/commandExecution/requestApproval")]
    CommandExecutionRequestApproval {
        id: RequestId,
        params: CommandExecutionRequestApprovalParams,
    },
    #[serde(rename = "item/fileChange/requestApproval")]
    FileChangeRequestApproval {
        id: RequestId,
        params: FileChangeRequestApprovalParams,
    },
    #[serde(rename = "item/permissions/requestApproval")]
    PermissionsRequestApproval {
        id: RequestId,
        params: PermissionsRequestApprovalParams,
    },
    #[serde(rename = "item/tool/requestUserInput")]
    ToolRequestUserInput {
        id: RequestId,
        params: ToolRequestUserInputParams,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum UpstreamEvent {
    Notification(UpstreamNotification),
    ServerRequest(Box<UpstreamServerRequest>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_request_decodes_request_user_input_payload() {
        let value = serde_json::json!({
            "method": "item/tool/requestUserInput",
            "id": 41,
            "params": {
                "threadId": "thr-1",
                "turnId": "turn-1",
                "itemId": "tool-1",
                "questions": [
                    {
                        "id": "dataset",
                        "header": "Dataset",
                        "question": "Pick a dataset",
                        "options": [
                            { "label": "c4", "description": "Use C4" }
                        ]
                    }
                ]
            }
        });
        let request: UpstreamServerRequest = match serde_json::from_value(value) {
            Ok(request) => request,
            Err(error) => panic!("decode request_user_input request: {error}"),
        };
        match request {
            UpstreamServerRequest::ToolRequestUserInput { id, params } => {
                assert_eq!(id, RequestId::Integer(41));
                assert_eq!(params.questions.len(), 1);
                assert_eq!(params.questions[0].header, "Dataset");
            }
            other => panic!("unexpected request variant: {other:?}"),
        }
    }

    #[test]
    fn permissions_response_serializes_granted_subset() {
        let response = PermissionsRequestApprovalResponse {
            permissions: GrantedPermissionProfile {
                network: None,
                file_system: Some(AdditionalFileSystemPermissions {
                    read: None,
                    write: Some(vec![PathBuf::from("/tmp/workspace")]),
                }),
            },
            scope: PermissionGrantScope::Turn,
        };
        let value = match serde_json::to_value(response) {
            Ok(value) => value,
            Err(error) => panic!("serialize permissions response: {error}"),
        };
        assert_eq!(value["scope"], "turn");
        assert_eq!(
            value["permissions"]["fileSystem"]["write"][0],
            "/tmp/workspace"
        );
    }

    #[test]
    fn upstream_notification_decodes_server_request_resolved() {
        let value = serde_json::json!({
            "method": "serverRequest/resolved",
            "params": {
                "threadId": "thr-1",
                "requestId": 7
            }
        });
        let notification: UpstreamNotification = match serde_json::from_value(value) {
            Ok(notification) => notification,
            Err(error) => panic!("decode serverRequest/resolved notification: {error}"),
        };
        match notification {
            UpstreamNotification::ServerRequestResolved { params } => {
                assert_eq!(params.thread_id, "thr-1");
                assert_eq!(params.request_id, RequestId::Integer(7));
            }
            other => panic!("unexpected notification variant: {other:?}"),
        }
    }

    #[test]
    fn upstream_notification_decodes_turn_plan_updated() {
        let value = serde_json::json!({
            "method": "turn/plan/updated",
            "params": {
                "turnId": "turn-1",
                "explanation": "Refine the fix",
                "plan": [
                    {"step": "Inspect logs", "status": "completed"},
                    {"step": "Patch parser", "status": "inProgress"}
                ]
            }
        });
        let notification: UpstreamNotification = match serde_json::from_value(value) {
            Ok(notification) => notification,
            Err(error) => panic!("decode turn/plan/updated notification: {error}"),
        };
        match notification {
            UpstreamNotification::TurnPlanUpdated { params } => {
                assert_eq!(params.turn_id, "turn-1");
                assert_eq!(params.explanation.as_deref(), Some("Refine the fix"));
                assert_eq!(params.plan.len(), 2);
                assert_eq!(params.plan[1].status, TurnPlanStepStatus::InProgress);
            }
            other => panic!("unexpected notification variant: {other:?}"),
        }
    }

    #[test]
    fn upstream_notification_decodes_command_execution_output_delta() {
        let value = serde_json::json!({
            "method": "item/commandExecution/outputDelta",
            "params": {
                "threadId": "thr-1",
                "turnId": "turn-1",
                "itemId": "cmd-1",
                "delta": "line 1\n"
            }
        });
        let notification: UpstreamNotification = match serde_json::from_value(value) {
            Ok(notification) => notification,
            Err(error) => panic!("decode item/commandExecution/outputDelta notification: {error}"),
        };
        match notification {
            UpstreamNotification::CommandExecutionOutputDelta { params } => {
                assert_eq!(params.thread_id, "thr-1");
                assert_eq!(params.turn_id, "turn-1");
                assert_eq!(params.item_id, "cmd-1");
                assert_eq!(params.delta, "line 1\n");
            }
            other => panic!("unexpected notification variant: {other:?}"),
        }
    }
}
