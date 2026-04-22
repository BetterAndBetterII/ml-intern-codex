use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use anyhow::{Context, Result, anyhow};
use mli_upstream_protocol::{
    AgentMessageDeltaNotification, AskForApproval, ClientInfo,
    CommandExecutionOutputDeltaNotification, InitializeCapabilities, InitializeParams,
    InitializeResult, ItemCompletedNotification, ItemStartedNotification, JsonRpcError,
    JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RequestId, SandboxMode,
    ServerRequestResolvedNotification, ThreadResumeParams, ThreadResumeResult, ThreadStartParams,
    ThreadStartResult, TurnCompletedNotification, TurnInterruptParams, TurnInterruptResult,
    TurnStartParams, TurnStartResult, TurnStartedNotification, UpstreamEvent, UpstreamNotification,
    UpstreamServerRequest,
};
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct BridgeConfig {
    pub codex_bin: PathBuf,
    pub codex_home: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

pub trait CodexBridge: Send {
    fn initialize(&mut self) -> Result<InitializeResult>;
    fn thread_start(&mut self, params: ThreadStartParams) -> Result<ThreadStartResult>;
    fn thread_resume(&mut self, params: ThreadResumeParams) -> Result<ThreadResumeResult>;
    fn turn_start(&mut self, params: TurnStartParams) -> Result<TurnStartResult>;
    fn turn_interrupt(&mut self, params: TurnInterruptParams) -> Result<TurnInterruptResult>;
    fn respond_to_server_request(&mut self, request_id: RequestId, result: Value) -> Result<()>;
    fn recv_event(&mut self) -> Result<Option<UpstreamEvent>>;
    fn recv_event_blocking(&mut self) -> Result<Option<UpstreamEvent>>;
}

#[derive(Debug)]
enum BridgeMessage {
    Response(JsonRpcResponse),
    Error(JsonRpcError),
    Event(UpstreamEvent),
    Ignored,
}

pub struct ProcessCodexBridge {
    config: BridgeConfig,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    rx: Option<Receiver<BridgeMessage>>,
    next_request_id: i64,
    buffered_events: VecDeque<UpstreamEvent>,
    initialized: bool,
}

impl ProcessCodexBridge {
    pub fn new(config: BridgeConfig) -> Self {
        Self {
            config,
            child: None,
            stdin: None,
            rx: None,
            next_request_id: 0,
            buffered_events: VecDeque::new(),
            initialized: false,
        }
    }

    fn ensure_process(&mut self) -> Result<()> {
        if self.child.is_some() {
            return Ok(());
        }

        let mut command = Command::new(&self.config.codex_bin);
        command.arg("app-server");
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(codex_home) = &self.config.codex_home {
            command.env("CODEX_HOME", codex_home);
        }
        for (key, value) in &self.config.env {
            command.env(key, value);
        }

        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to spawn {} app-server",
                self.config.codex_bin.display()
            )
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("missing codex stdout"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("missing codex stdin"))?;
        let (tx, rx) = mpsc::channel();
        spawn_reader_thread(stdout, tx);
        self.stdin = Some(stdin);
        self.rx = Some(rx);
        self.child = Some(child);
        Ok(())
    }

    fn next_request_id(&mut self) -> RequestId {
        self.next_request_id += 1;
        RequestId::Integer(self.next_request_id)
    }

    fn send_request<P: serde::Serialize>(&mut self, method: &str, params: &P) -> Result<RequestId> {
        self.ensure_process()?;
        let request_id = self.next_request_id();
        let request = JsonRpcRequest {
            id: request_id.clone(),
            method: method.to_owned(),
            params: Some(
                serde_json::to_value(params).context("failed to serialize upstream params")?,
            ),
        };
        self.write_message(&JsonRpcMessage::Request(request))?;
        Ok(request_id)
    }

    fn send_notification<P: serde::Serialize>(
        &mut self,
        method: &str,
        params: Option<&P>,
    ) -> Result<()> {
        self.ensure_process()?;
        let notification = JsonRpcNotification {
            method: method.to_owned(),
            params: match params {
                Some(params) => Some(
                    serde_json::to_value(params)
                        .context("failed to serialize upstream notification")?,
                ),
                None => None,
            },
        };
        self.write_message(&JsonRpcMessage::Notification(notification))
    }

    fn write_message(&mut self, message: &JsonRpcMessage) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("codex stdin unavailable"))?;
        let line =
            serde_json::to_string(message).context("failed to encode upstream JSON-RPC line")?;
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush())
            .context("failed to write to codex app-server stdin")
    }

    fn wait_for_response(&mut self, request_id: &RequestId) -> Result<JsonRpcResponse> {
        let rx = self
            .rx
            .as_ref()
            .ok_or_else(|| anyhow!("bridge receiver unavailable"))?;
        loop {
            match rx.recv().context("upstream bridge channel closed")? {
                BridgeMessage::Response(response) if &response.id == request_id => {
                    return Ok(response);
                }
                BridgeMessage::Response(_) => continue,
                BridgeMessage::Error(error) if &error.id == request_id => {
                    return Err(anyhow!(
                        "upstream error {}: {}",
                        error.error.code,
                        error.error.message
                    ));
                }
                BridgeMessage::Error(_) => continue,
                BridgeMessage::Event(event) => self.buffered_events.push_back(event),
                BridgeMessage::Ignored => continue,
            }
        }
    }

    fn parse_result<T: serde::de::DeserializeOwned>(&self, response: JsonRpcResponse) -> Result<T> {
        serde_json::from_value(response.result)
            .context("failed to decode upstream response payload")
    }

    fn send_server_request_response<T: serde::Serialize>(
        &mut self,
        request_id: RequestId,
        result: &T,
    ) -> Result<()> {
        self.write_message(&JsonRpcMessage::Response(JsonRpcResponse {
            id: request_id,
            result: serde_json::to_value(result)
                .context("failed to serialize upstream response")?,
        }))
    }
}

impl CodexBridge for ProcessCodexBridge {
    fn initialize(&mut self) -> Result<InitializeResult> {
        if self.initialized {
            return Ok(InitializeResult::default());
        }
        let request_id = self.send_request(
            "initialize",
            &InitializeParams {
                client_info: ClientInfo {
                    name: "ml-intern-codex".to_owned(),
                    title: Some("ml-intern-codex".to_owned()),
                    version: env!("CARGO_PKG_VERSION").to_owned(),
                },
                capabilities: Some(InitializeCapabilities {
                    experimental_api: true,
                }),
            },
        )?;
        let response = self.wait_for_response(&request_id)?;
        self.send_notification::<()>("initialized", None)?;
        self.initialized = true;
        self.parse_result(response)
    }

    fn thread_start(&mut self, params: ThreadStartParams) -> Result<ThreadStartResult> {
        self.initialize()?;
        let request_id = self.send_request("thread/start", &params)?;
        let response = self.wait_for_response(&request_id)?;
        self.parse_result(response)
    }

    fn thread_resume(&mut self, params: ThreadResumeParams) -> Result<ThreadResumeResult> {
        self.initialize()?;
        let request_id = self.send_request("thread/resume", &params)?;
        let response = self.wait_for_response(&request_id)?;
        self.parse_result(response)
    }

    fn turn_start(&mut self, params: TurnStartParams) -> Result<TurnStartResult> {
        self.initialize()?;
        let request_id = self.send_request("turn/start", &params)?;
        let response = self.wait_for_response(&request_id)?;
        self.parse_result(response)
    }

    fn turn_interrupt(&mut self, params: TurnInterruptParams) -> Result<TurnInterruptResult> {
        self.initialize()?;
        let request_id = self.send_request("turn/interrupt", &params)?;
        let response = self.wait_for_response(&request_id)?;
        self.parse_result(response)
    }

    fn respond_to_server_request(&mut self, request_id: RequestId, result: Value) -> Result<()> {
        self.send_server_request_response(request_id, &result)
    }

    fn recv_event(&mut self) -> Result<Option<UpstreamEvent>> {
        if let Some(event) = self.buffered_events.pop_front() {
            return Ok(Some(event));
        }
        let Some(rx) = self.rx.as_ref() else {
            return Ok(None);
        };
        match rx.try_recv() {
            Ok(BridgeMessage::Event(event)) => Ok(Some(event)),
            Ok(BridgeMessage::Response(_))
            | Ok(BridgeMessage::Error(_))
            | Ok(BridgeMessage::Ignored) => Ok(None),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Ok(None),
        }
    }

    fn recv_event_blocking(&mut self) -> Result<Option<UpstreamEvent>> {
        if let Some(event) = self.buffered_events.pop_front() {
            return Ok(Some(event));
        }
        let Some(rx) = self.rx.as_ref() else {
            return Ok(None);
        };
        loop {
            match rx.recv() {
                Ok(BridgeMessage::Event(event)) => return Ok(Some(event)),
                Ok(BridgeMessage::Response(_))
                | Ok(BridgeMessage::Error(_))
                | Ok(BridgeMessage::Ignored) => continue,
                Err(_) => return Ok(None),
            }
        }
    }
}

impl Drop for ProcessCodexBridge {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
    }
}

fn spawn_reader_thread(stdout: impl std::io::Read + Send + 'static, tx: Sender<BridgeMessage>) {
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else {
                let _ = tx.send(BridgeMessage::Ignored);
                break;
            };
            if line.trim().is_empty() {
                continue;
            }
            let message = match serde_json::from_str::<JsonRpcMessage>(&line) {
                Ok(message) => message,
                Err(_) => {
                    let _ = tx.send(BridgeMessage::Ignored);
                    continue;
                }
            };
            let bridge_message = match message {
                JsonRpcMessage::Response(response) => BridgeMessage::Response(response),
                JsonRpcMessage::Error(error) => BridgeMessage::Error(error),
                JsonRpcMessage::Notification(notification) => {
                    match decode_notification(notification) {
                        Ok(Some(notification)) => {
                            BridgeMessage::Event(UpstreamEvent::Notification(notification))
                        }
                        Ok(None) => BridgeMessage::Ignored,
                        Err(_) => BridgeMessage::Ignored,
                    }
                }
                JsonRpcMessage::Request(request) => match decode_server_request(request) {
                    Ok(Some(request)) => {
                        BridgeMessage::Event(UpstreamEvent::ServerRequest(Box::new(request)))
                    }
                    Ok(None) => BridgeMessage::Ignored,
                    Err(_) => BridgeMessage::Ignored,
                },
            };
            let _ = tx.send(bridge_message);
        }
    });
}

fn decode_notification(notification: JsonRpcNotification) -> Result<Option<UpstreamNotification>> {
    let method = notification.method.clone();
    let params = notification.params.unwrap_or(Value::Null);
    let notification = match method.as_str() {
        "turn/started" => Some(UpstreamNotification::TurnStarted {
            params: serde_json::from_value::<TurnStartedNotification>(params)
                .context("failed to decode turn/started")?,
        }),
        "turn/completed" => Some(UpstreamNotification::TurnCompleted {
            params: serde_json::from_value::<TurnCompletedNotification>(params)
                .context("failed to decode turn/completed")?,
        }),
        "turn/plan/updated" => Some(UpstreamNotification::TurnPlanUpdated {
            params: serde_json::from_value::<mli_upstream_protocol::TurnPlanUpdatedNotification>(
                params,
            )
            .context("failed to decode turn/plan/updated")?,
        }),
        "item/started" => Some(UpstreamNotification::ItemStarted {
            params: serde_json::from_value::<ItemStartedNotification>(params)
                .context("failed to decode item/started")?,
        }),
        "item/completed" => Some(UpstreamNotification::ItemCompleted {
            params: serde_json::from_value::<ItemCompletedNotification>(params)
                .context("failed to decode item/completed")?,
        }),
        "item/agentMessage/delta" => Some(UpstreamNotification::AgentMessageDelta {
            params: serde_json::from_value::<AgentMessageDeltaNotification>(params)
                .context("failed to decode item/agentMessage/delta")?,
        }),
        "item/commandExecution/outputDelta" => {
            Some(UpstreamNotification::CommandExecutionOutputDelta {
                params: serde_json::from_value::<CommandExecutionOutputDeltaNotification>(params)
                    .context("failed to decode item/commandExecution/outputDelta")?,
            })
        }
        "serverRequest/resolved" => Some(UpstreamNotification::ServerRequestResolved {
            params: serde_json::from_value::<ServerRequestResolvedNotification>(params)
                .context("failed to decode serverRequest/resolved")?,
        }),
        "error" => Some(UpstreamNotification::Error {
            params: serde_json::from_value(params).context("failed to decode upstream error")?,
        }),
        _ => None,
    };
    Ok(notification)
}

fn decode_server_request(request: JsonRpcRequest) -> Result<Option<UpstreamServerRequest>> {
    let method = request.method.clone();
    let request = match method.as_str() {
        "item/commandExecution/requestApproval"
        | "item/fileChange/requestApproval"
        | "item/permissions/requestApproval"
        | "item/tool/requestUserInput" => Some(
            serde_json::from_value::<UpstreamServerRequest>(
                serde_json::to_value(request).context("failed to encode server request")?,
            )
            .context("failed to decode upstream server request")?,
        ),
        _ => None,
    };
    Ok(request)
}

pub fn approval_policy_to_upstream(policy: mli_types::ApprovalPolicy) -> AskForApproval {
    match policy {
        mli_types::ApprovalPolicy::Never => AskForApproval::Never,
        mli_types::ApprovalPolicy::OnFailure => AskForApproval::OnFailure,
        mli_types::ApprovalPolicy::OnRequest => AskForApproval::OnRequest,
        mli_types::ApprovalPolicy::Untrusted => AskForApproval::Untrusted,
    }
}

pub fn sandbox_mode_to_upstream(mode: mli_types::SandboxMode) -> SandboxMode {
    match mode {
        mli_types::SandboxMode::ReadOnly => SandboxMode::ReadOnly,
        mli_types::SandboxMode::WorkspaceWrite => SandboxMode::WorkspaceWrite,
        mli_types::SandboxMode::DangerFullAccess => SandboxMode::DangerFullAccess,
    }
}
