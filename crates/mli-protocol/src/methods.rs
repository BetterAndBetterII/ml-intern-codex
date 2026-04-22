use std::collections::BTreeMap;
use std::path::PathBuf;

use mli_types::{
    AppState, ArtifactId, ArtifactKind, ArtifactManifest, ArtifactReadBundle, LocalThreadId,
    LocalTurnId, StartThreadRequest, ThreadRecord, TurnRecord,
};
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    pub title: Option<String>,
    pub version: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    pub transcript_streaming: bool,
    pub skill_picker: bool,
    pub artifacts_overlay: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InitializeParams {
    pub client_info: ClientInfo,
    pub capabilities: ClientCapabilities,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub server_info: ServerInfo,
    pub protocol_version: String,
    pub upstream_codex_version: String,
    pub codex_bin: PathBuf,
    pub app_home: PathBuf,
}

pub type ThreadStartParams = StartThreadRequest;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreadStartResult {
    pub thread: ThreadRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreadResumeParams {
    pub thread_id: LocalThreadId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreadResumeResult {
    pub thread: ThreadRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreadListResult {
    pub threads: Vec<ThreadRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreadReadParams {
    pub thread_id: LocalThreadId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreadReadResult {
    pub thread: ThreadRecord,
    pub turns: Vec<TurnRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeInfoResult {
    pub codex_bin: PathBuf,
    pub codex_version: String,
    pub app_home: PathBuf,
    pub cwd: PathBuf,
    pub approval_policy: mli_types::ApprovalPolicy,
    pub sandbox_mode: mli_types::SandboxMode,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TurnStartParams {
    pub thread_id: LocalThreadId,
    pub input: Vec<UserInput>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TurnStartResult {
    pub turn: TurnRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TurnInterruptParams {
    pub thread_id: LocalThreadId,
    pub turn_id: LocalTurnId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Reject,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApprovalAnswer {
    pub answers: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApprovalRespondParams {
    pub approval_id: String,
    pub decision: ApprovalDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<BTreeMap<String, ApprovalAnswer>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ApprovalRespondResult {}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SkillsListParams {
    pub cwd: Option<PathBuf>,
    pub force_reload: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillsListResult {
    pub skills: Vec<mli_types::SkillDescriptor>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ArtifactListParams {
    pub thread_id: Option<LocalThreadId>,
    pub kind: Option<ArtifactKind>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactListResult {
    pub artifacts: Vec<ArtifactManifest>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactReadParams {
    pub artifact_id: ArtifactId,
}

pub type ArtifactReadResult = ArtifactReadBundle;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigReadResult {
    pub config: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigWriteParams {
    pub config: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigWriteResult {
    pub config: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UiStateResult {
    pub state: AppState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UserInput {
    Text { text: String },
    Image { url: String },
    LocalImage { path: PathBuf },
    Skill { name: String, path: PathBuf },
    Mention { name: String, path: String },
}

impl UserInput {
    pub fn summary(inputs: &[Self]) -> String {
        inputs
            .iter()
            .map(|input| match input {
                Self::Text { text } => text.clone(),
                Self::Image { url } => format!("[image:{url}]"),
                Self::LocalImage { path } => format!("[local-image:{}]", path.display()),
                Self::Skill { name, .. } => format!("${name}"),
                Self::Mention { name, .. } => format!("@{name}"),
            })
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_input_summary_keeps_skill_and_text_order() {
        let summary = UserInput::summary(&[
            UserInput::Skill {
                name: "hf-dataset-audit".to_owned(),
                path: PathBuf::from("skills/system/hf-dataset-audit/SKILL.md"),
            },
            UserInput::Text {
                text: "inspect this dataset".to_owned(),
            },
        ]);
        assert_eq!(summary, "$hf-dataset-audit inspect this dataset");
    }

    #[test]
    fn json_rpc_request_serializes_method_and_params() {
        let request = JsonRpcMessage::Request(JsonRpcRequest {
            id: RequestId::Integer(7),
            method: "thread/list".to_owned(),
            params: Some(serde_json::json!({})),
        });
        let value = match serde_json::to_value(request) {
            Ok(value) => value,
            Err(error) => panic!("serialize request: {error}"),
        };
        assert_eq!(value["method"], "thread/list");
        assert_eq!(value["id"], 7);
    }

    #[test]
    fn approval_respond_params_preserve_answers() {
        let params = ApprovalRespondParams {
            approval_id: "req-7".to_owned(),
            decision: ApprovalDecision::Approve,
            answers: Some(BTreeMap::from([(
                "dataset".to_owned(),
                ApprovalAnswer {
                    answers: vec!["c4".to_owned()],
                },
            )])),
        };
        let value = match serde_json::to_value(params) {
            Ok(value) => value,
            Err(error) => panic!("serialize approval params: {error}"),
        };
        assert_eq!(value["approval_id"], "req-7");
        assert_eq!(value["decision"], "approve");
        assert_eq!(value["answers"]["dataset"]["answers"][0], "c4");
    }
}
