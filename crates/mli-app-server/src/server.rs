use std::fs;
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use mli_config::{AppConfig, AppPaths};
use mli_protocol::{
    ApprovalRespondParams, ArtifactListParams, ArtifactReadParams, ClientCapabilities, ClientInfo,
    ConfigReadResult, ConfigWriteParams, JsonRpcError, JsonRpcErrorPayload, JsonRpcMessage,
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RequestId, ServerNotification,
    SkillsListParams, ThreadReadParams, ThreadResumeParams, ThreadStartParams, TurnInterruptParams,
    TurnStartParams,
};
use mli_runtime::{RuntimeSession, default_initialize_params};

pub struct AppServer {
    config: AppConfig,
    paths: AppPaths,
    runtime: Arc<Mutex<RuntimeSession>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl AppServer {
    pub fn from_current_dir() -> Result<Self> {
        let cwd = std::env::current_dir().context("failed to read current directory")?;
        let (config, paths) = AppConfig::load_for_cwd(&cwd)?;
        let runtime = RuntimeSession::from_config(config.clone(), paths.clone())?;
        Ok(Self {
            config,
            paths,
            runtime: Arc::new(Mutex::new(runtime)),
            writer: Arc::new(Mutex::new(Box::new(io::stdout()))),
        })
    }

    pub fn serve_stdio(&mut self) -> Result<()> {
        self.spawn_notification_pump();
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let line = line.context("failed to read stdin line")?;
            if line.trim().is_empty() {
                continue;
            }
            let message: JsonRpcMessage = serde_json::from_str(&line)
                .with_context(|| format!("invalid request line: {line}"))?;
            match message {
                JsonRpcMessage::Request(request) => self.handle_request(request)?,
                JsonRpcMessage::Notification(notification) => {
                    self.handle_notification(notification)?
                }
                JsonRpcMessage::Response(_) | JsonRpcMessage::Error(_) => {
                    return Err(anyhow!("client sent a response/error object unexpectedly"));
                }
            }
        }
        Ok(())
    }

    fn handle_notification(&mut self, notification: JsonRpcNotification) -> Result<()> {
        if notification.method == "initialized" {
            let mut runtime = self
                .runtime
                .lock()
                .map_err(|_| anyhow!("runtime mutex poisoned"))?;
            runtime.initialized()?;
        }
        Ok(())
    }

    fn handle_request(&mut self, request: JsonRpcRequest) -> Result<()> {
        match request.method.as_str() {
            "initialize" => {
                let params = parse_params(request.params)?;
                let result = self
                    .runtime
                    .lock()
                    .map_err(|_| anyhow!("runtime mutex poisoned"))?
                    .initialize(params)?;
                self.write_response(request.id, &result)
            }
            "runtime/info" => {
                let result = self
                    .runtime
                    .lock()
                    .map_err(|_| anyhow!("runtime mutex poisoned"))?
                    .runtime_info()?;
                self.write_response(request.id, &result)
            }
            "thread/start" => {
                let params: ThreadStartParams = parse_params(request.params)?;
                let (result, notifications) = self
                    .runtime
                    .lock()
                    .map_err(|_| anyhow!("runtime mutex poisoned"))?
                    .start_thread(params)?;
                self.write_response(request.id, &result)?;
                self.write_notifications(notifications)
            }
            "thread/resume" => {
                let params: ThreadResumeParams = parse_params(request.params)?;
                let (result, notifications) = self
                    .runtime
                    .lock()
                    .map_err(|_| anyhow!("runtime mutex poisoned"))?
                    .resume_thread(params)?;
                self.write_response(request.id, &result)?;
                self.write_notifications(notifications)
            }
            "thread/list" => {
                let result = self
                    .runtime
                    .lock()
                    .map_err(|_| anyhow!("runtime mutex poisoned"))?
                    .list_threads()?;
                self.write_response(request.id, &result)
            }
            "thread/read" => {
                let params: ThreadReadParams = parse_params(request.params)?;
                let result = self
                    .runtime
                    .lock()
                    .map_err(|_| anyhow!("runtime mutex poisoned"))?
                    .read_thread(params)?;
                self.write_response(request.id, &result)
            }
            "turn/start" => {
                let params: TurnStartParams = parse_params(request.params)?;
                let result = self
                    .runtime
                    .lock()
                    .map_err(|_| anyhow!("runtime mutex poisoned"))?
                    .start_turn(params)?;
                self.write_response(request.id, &result)
            }
            "turn/interrupt" => {
                let params: TurnInterruptParams = parse_params(request.params)?;
                self.runtime
                    .lock()
                    .map_err(|_| anyhow!("runtime mutex poisoned"))?
                    .interrupt_turn(params)?;
                self.write_response(request.id, &serde_json::json!({}))
            }
            "approval/respond" => {
                let params: ApprovalRespondParams = parse_params(request.params)?;
                let result = self
                    .runtime
                    .lock()
                    .map_err(|_| anyhow!("runtime mutex poisoned"))?
                    .respond_to_approval(params)?;
                self.write_response(request.id, &result)
            }
            "skills/list" => {
                let params: SkillsListParams = parse_params_or_default(request.params)?;
                let result = self
                    .runtime
                    .lock()
                    .map_err(|_| anyhow!("runtime mutex poisoned"))?
                    .list_skills(params)?;
                self.write_response(request.id, &result)
            }
            "artifact/list" => {
                let params: ArtifactListParams = parse_params_or_default(request.params)?;
                let result = self
                    .runtime
                    .lock()
                    .map_err(|_| anyhow!("runtime mutex poisoned"))?
                    .list_artifacts(params)?;
                self.write_response(request.id, &result)
            }
            "artifact/read" => {
                let params: ArtifactReadParams = parse_params(request.params)?;
                let result = self
                    .runtime
                    .lock()
                    .map_err(|_| anyhow!("runtime mutex poisoned"))?
                    .read_artifact(params)?;
                self.write_response(request.id, &result)
            }
            "config/read" => {
                let value =
                    serde_json::to_value(&self.config).context("failed to encode config")?;
                self.write_response(request.id, &ConfigReadResult { config: value })
            }
            "config/write" => {
                let params: ConfigWriteParams = parse_params(request.params)?;
                let config: AppConfig = serde_json::from_value(params.config.clone())
                    .context("config/write expects a full AppConfig payload")?;
                let raw =
                    toml::to_string_pretty(&config).context("failed to encode TOML config")?;
                fs::write(&self.paths.project_config_path, raw).with_context(|| {
                    format!(
                        "failed to write {}",
                        self.paths.project_config_path.display()
                    )
                })?;
                self.config = config.clone();
                self.runtime
                    .lock()
                    .map_err(|_| anyhow!("runtime mutex poisoned"))?
                    .update_config(config.clone());
                self.write_response(
                    request.id,
                    &mli_protocol::ConfigWriteResult {
                        config: serde_json::to_value(config)
                            .context("failed to encode persisted config")?,
                    },
                )
            }
            other => self.write_error(
                request.id,
                -32601,
                &format!("unsupported method {other}"),
                None,
            ),
        }
    }

    fn spawn_notification_pump(&self) {
        let runtime = Arc::clone(&self.runtime);
        let writer = Arc::clone(&self.writer);
        thread::spawn(move || {
            loop {
                let notification = match runtime.lock() {
                    Ok(mut runtime) => match runtime.recv_notification() {
                        Ok(notification) => notification,
                        Err(error) => Some(ServerNotification::Error {
                            params: mli_protocol::ErrorNotification {
                                message: error.to_string(),
                            },
                        }),
                    },
                    Err(_) => Some(ServerNotification::Error {
                        params: mli_protocol::ErrorNotification {
                            message: "runtime mutex poisoned".to_owned(),
                        },
                    }),
                };

                if let Some(notification) = notification {
                    let message = JsonRpcMessage::Notification(JsonRpcNotification {
                        method: notification_method(&notification).to_owned(),
                        params: notification_params(&notification).ok(),
                    });
                    if let Ok(line) = serde_json::to_string(&message)
                        && let Ok(mut writer) = writer.lock()
                    {
                        let _ = writeln!(writer, "{line}");
                        let _ = writer.flush();
                    }
                    continue;
                }

                thread::sleep(Duration::from_millis(50));
            }
        });
    }

    fn write_response<T: serde::Serialize>(&self, id: RequestId, result: &T) -> Result<()> {
        let response = JsonRpcMessage::Response(JsonRpcResponse {
            id,
            result: serde_json::to_value(result).context("failed to serialize response")?,
        });
        self.write_line(&response)
    }

    fn write_error(
        &self,
        id: RequestId,
        code: i64,
        message: &str,
        data: Option<serde_json::Value>,
    ) -> Result<()> {
        let response = JsonRpcMessage::Error(JsonRpcError {
            id,
            error: JsonRpcErrorPayload {
                code,
                message: message.to_owned(),
                data,
            },
        });
        self.write_line(&response)
    }

    fn write_notifications(&self, notifications: Vec<ServerNotification>) -> Result<()> {
        for notification in notifications {
            let envelope = JsonRpcMessage::Notification(JsonRpcNotification {
                method: notification_method(&notification).to_owned(),
                params: Some(notification_params(&notification)?),
            });
            self.write_line(&envelope)?;
        }
        Ok(())
    }

    fn write_line<T: serde::Serialize>(&self, value: &T) -> Result<()> {
        let line = serde_json::to_string(value).context("failed to encode JSONL message")?;
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("writer mutex poisoned"))?;
        writeln!(writer, "{line}").context("failed to write JSONL message")?;
        writer.flush().context("failed to flush JSONL message")
    }
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Option<serde_json::Value>) -> Result<T> {
    let value = params.ok_or_else(|| anyhow!("missing params"))?;
    serde_json::from_value(value).context("invalid params")
}

fn parse_params_or_default<T>(params: Option<serde_json::Value>) -> Result<T>
where
    T: serde::de::DeserializeOwned + Default,
{
    match params {
        Some(value) => serde_json::from_value(value).context("invalid params"),
        None => Ok(T::default()),
    }
}

fn notification_method(notification: &ServerNotification) -> &'static str {
    match notification {
        ServerNotification::ThreadStarted { .. } => "thread/started",
        ServerNotification::ThreadStatusChanged { .. } => "thread/statusChanged",
        ServerNotification::TurnStarted { .. } => "turn/started",
        ServerNotification::TurnCompleted { .. } => "turn/completed",
        ServerNotification::PlanUpdated { .. } => "turn/plan/updated",
        ServerNotification::ItemStarted { .. } => "item/started",
        ServerNotification::ItemCompleted { .. } => "item/completed",
        ServerNotification::AgentMessageDelta { .. } => "item/agentMessage/delta",
        ServerNotification::CommandExecutionOutputDelta { .. } => {
            "item/commandExecution/outputDelta"
        }
        ServerNotification::RuntimeStatusChanged { .. } => "runtime/statusChanged",
        ServerNotification::SkillsChanged { .. } => "skills/changed",
        ServerNotification::ArtifactCreated { .. } => "artifact/created",
        ServerNotification::ArtifactUpdated { .. } => "artifact/updated",
        ServerNotification::ApprovalRequested { .. } => "approval/requested",
        ServerNotification::Warning { .. } => "warning",
        ServerNotification::Error { .. } => "error",
    }
}

pub fn run_stdio_server() -> Result<()> {
    let mut server = AppServer::from_current_dir()?;
    server.serve_stdio()
}

pub fn default_client_info() -> (ClientInfo, ClientCapabilities) {
    let params = default_initialize_params();
    (params.client_info, params.capabilities)
}

fn notification_params(notification: &ServerNotification) -> Result<serde_json::Value> {
    match notification {
        ServerNotification::ThreadStarted { params } => {
            serde_json::to_value(params).context("failed to serialize thread/started")
        }
        ServerNotification::ThreadStatusChanged { params } => {
            serde_json::to_value(params).context("failed to serialize thread/statusChanged")
        }
        ServerNotification::TurnStarted { params } => {
            serde_json::to_value(params).context("failed to serialize turn/started")
        }
        ServerNotification::TurnCompleted { params } => {
            serde_json::to_value(params).context("failed to serialize turn/completed")
        }
        ServerNotification::PlanUpdated { params } => {
            serde_json::to_value(params).context("failed to serialize turn/plan/updated")
        }
        ServerNotification::ItemStarted { params } => {
            serde_json::to_value(params).context("failed to serialize item/started")
        }
        ServerNotification::ItemCompleted { params } => {
            serde_json::to_value(params).context("failed to serialize item/completed")
        }
        ServerNotification::AgentMessageDelta { params } => {
            serde_json::to_value(params).context("failed to serialize item/agentMessage/delta")
        }
        ServerNotification::CommandExecutionOutputDelta { params } => serde_json::to_value(params)
            .context("failed to serialize item/commandExecution/outputDelta"),
        ServerNotification::RuntimeStatusChanged { params } => {
            serde_json::to_value(params).context("failed to serialize runtime/statusChanged")
        }
        ServerNotification::SkillsChanged { params } => {
            serde_json::to_value(params).context("failed to serialize skills/changed")
        }
        ServerNotification::ArtifactCreated { params } => {
            serde_json::to_value(params).context("failed to serialize artifact/created")
        }
        ServerNotification::ArtifactUpdated { params } => {
            serde_json::to_value(params).context("failed to serialize artifact/updated")
        }
        ServerNotification::ApprovalRequested { params } => {
            serde_json::to_value(params).context("failed to serialize approval/requested")
        }
        ServerNotification::Warning { params } => {
            serde_json::to_value(params).context("failed to serialize warning")
        }
        ServerNotification::Error { params } => {
            serde_json::to_value(params).context("failed to serialize error")
        }
    }
}
