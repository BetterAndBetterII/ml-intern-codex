use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FakeCodexScenario {
    MissingCodex,
    VersionMismatch,
    StartupOnly,
    StreamingTurn,
    SkillsTurn,
    InterruptibleTurn,
    InterruptErrorTurn,
    CommandApprovalTurn,
    RequestUserInputTurn,
}

impl FakeCodexScenario {
    fn as_env(self) -> &'static str {
        match self {
            Self::MissingCodex => "missing-codex",
            Self::VersionMismatch => "version-mismatch",
            Self::StartupOnly => "startup-only",
            Self::StreamingTurn => "streaming-turn",
            Self::SkillsTurn => "skills-turn",
            Self::InterruptibleTurn => "interruptible-turn",
            Self::InterruptErrorTurn => "interrupt-error-turn",
            Self::CommandApprovalTurn => "command-approval-turn",
            Self::RequestUserInputTurn => "request-user-input-turn",
        }
    }

    fn has_fake_codex(self) -> bool {
        !matches!(self, Self::MissingCodex)
    }
}

struct TestEnv {
    home: PathBuf,
    cwd: PathBuf,
    bin_dir: PathBuf,
    capture_path: PathBuf,
    scenario: FakeCodexScenario,
}

impl TestEnv {
    fn new(name: &str, scenario: FakeCodexScenario) -> Self {
        let root = std::env::temp_dir().join(format!(
            "mli-cli-process-smoke-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home");
        let cwd = root.join("project");
        let bin_dir = root.join("bin");
        let capture_path = root.join("fake-codex-capture.jsonl");
        for dir in [&home, &cwd, &bin_dir] {
            fs::create_dir_all(dir)
                .unwrap_or_else(|error| panic!("create {}: {error}", dir.display()));
        }
        if scenario.has_fake_codex() {
            write_fake_codex(&bin_dir);
        }
        Self {
            home,
            cwd,
            bin_dir,
            capture_path,
            scenario,
        }
    }

    fn path_env(&self) -> OsString {
        let mut parts = vec![self.bin_dir.clone()];
        if self.scenario != FakeCodexScenario::MissingCodex {
            parts.extend(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ));
        }
        std::env::join_paths(parts).unwrap_or_else(|error| panic!("join PATH: {error}"))
    }

    fn workspace_root(&self) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap_or_else(|| panic!("derive workspace root from CARGO_MANIFEST_DIR"))
            .to_path_buf()
    }

    fn read_capture_lines(&self) -> Vec<Value> {
        if !self.capture_path.exists() {
            return Vec::new();
        }
        parse_json_lines(
            &fs::read(self.capture_path())
                .unwrap_or_else(|error| panic!("read {}: {error}", self.capture_path().display())),
        )
    }

    fn capture_path(&self) -> &Path {
        &self.capture_path
    }

    fn persisted_thread_dir(&self) -> PathBuf {
        let threads_root = self.cwd.join(".ml-intern/threads");
        let thread_dirs = fs::read_dir(&threads_root)
            .unwrap_or_else(|error| panic!("list {}: {error}", threads_root.display()))
            .map(|entry| {
                entry
                    .unwrap_or_else(|error| panic!("read thread dir entry: {error}"))
                    .path()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            thread_dirs.len(),
            1,
            "expected exactly one persisted thread in {}",
            threads_root.display()
        );
        thread_dirs
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("missing persisted thread dir"))
    }

    fn transcript_text(&self) -> String {
        let thread_dir = self.persisted_thread_dir();
        fs::read_to_string(thread_dir.join("transcript.jsonl"))
            .unwrap_or_else(|error| panic!("read transcript.jsonl: {error}"))
    }
}

struct AppServerSession {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    stderr_log: Arc<Mutex<String>>,
}

impl AppServerSession {
    fn spawn(env: &TestEnv) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ml-intern-app-server"))
            .current_dir(&env.cwd)
            .env("HOME", &env.home)
            .env("PATH", env.path_env())
            .env("MLI_INSTALL_ROOT", env.workspace_root())
            .env("MLI_FAKE_CODEX_SCENARIO", env.scenario.as_env())
            .env("MLI_FAKE_CODEX_CAPTURE", env.capture_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("spawn app-server session: {error}"));
        let stdout = child
            .stdout
            .take()
            .unwrap_or_else(|| panic!("missing app-server session stdout"));
        let stderr = child
            .stderr
            .take()
            .unwrap_or_else(|| panic!("missing app-server session stderr"));
        let stdin = child
            .stdin
            .take()
            .unwrap_or_else(|| panic!("missing app-server session stdin"));
        let (tx, rx) = mpsc::channel();
        let stderr_log = Arc::new(Mutex::new(String::new()));
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else {
                    break;
                };
                if line.trim().is_empty() {
                    continue;
                }
                let value = serde_json::from_str::<Value>(&line)
                    .unwrap_or_else(|error| panic!("parse app-server line `{line}`: {error}"));
                let _ = tx.send(value);
            }
        });
        let stderr_sink = Arc::clone(&stderr_log);
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
                if let Ok(mut buffer) = stderr_sink.lock() {
                    if !buffer.is_empty() {
                        buffer.push('\n');
                    }
                    buffer.push_str(trimmed);
                }
            }
        });
        Self {
            child,
            stdin,
            rx,
            stderr_log,
        }
    }

    fn send(&mut self, value: &Value) {
        writeln!(self.stdin, "{value}")
            .unwrap_or_else(|error| panic!("write app-server session request: {error}"));
        self.stdin
            .flush()
            .unwrap_or_else(|error| panic!("flush app-server session request: {error}"));
    }

    fn collect_until<F>(&mut self, mut predicate: F) -> Vec<Value>
    where
        F: FnMut(&[Value]) -> bool,
    {
        let mut messages = Vec::new();
        while !predicate(&messages) {
            let value = self
                .rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap_or_else(|error| {
                    panic!(
                        "timed out waiting for app-server output: {error}; {}",
                        self.failure_details()
                    )
                });
            messages.push(value);
        }
        messages
    }

    fn finish(self) {
        let AppServerSession {
            mut child,
            stdin,
            rx: _,
            stderr_log,
        } = self;
        drop(stdin);
        let status = child
            .wait()
            .unwrap_or_else(|error| panic!("wait for app-server session: {error}"));
        assert!(
            status.success(),
            "app-server session exited with {status}: {}",
            format_failure_details(&mut child, &stderr_log)
        );
    }

    fn failure_details(&mut self) -> String {
        let status = self
            .child
            .try_wait()
            .ok()
            .flatten()
            .map(|status| status.to_string())
            .unwrap_or_else(|| "still running".to_owned());
        let stderr = self
            .stderr_log
            .lock()
            .map(|buffer| buffer.trim().to_owned())
            .unwrap_or_default();
        if stderr.is_empty() {
            format!("status={status}, stderr=<empty>")
        } else {
            format!("status={status}, stderr={stderr}")
        }
    }
}

fn format_failure_details(child: &mut Child, stderr_log: &Arc<Mutex<String>>) -> String {
    let status = child
        .try_wait()
        .ok()
        .flatten()
        .map(|status| status.to_string())
        .unwrap_or_else(|| "still running".to_owned());
    let stderr = stderr_log
        .lock()
        .map(|buffer| buffer.trim().to_owned())
        .unwrap_or_default();
    if stderr.is_empty() {
        format!("status={status}, stderr=<empty>")
    } else {
        format!("status={status}, stderr={stderr}")
    }
}

#[test]
fn ml_intern_app_server_jsonl_smoke_bootstraps_runtime() {
    let env = TestEnv::new("app-server", FakeCodexScenario::StartupOnly);
    let output = run_app_server_with_requests(
        &env,
        &[
            json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "client_info": {
                        "name": "process-smoke",
                        "title": "process-smoke",
                        "version": "0.1.0"
                    },
                    "capabilities": {
                        "transcriptStreaming": true,
                        "skillPicker": true,
                        "artifactsOverlay": true
                    }
                }
            }),
            json!({"method": "initialized"}),
            json!({"id": 2, "method": "runtime/info"}),
            json!({"id": 3, "method": "skills/list"}),
            json!({"id": 4, "method": "thread/list"}),
            json!({"id": 5, "method": "artifact/list"}),
        ],
    );
    assert!(
        output.status.success(),
        "ml-intern-app-server failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let responses = parse_json_lines(&output.stdout);
    assert_eq!(responses.len(), 5);

    let initialize = response_result(&responses, 1);
    assert_eq!(initialize["upstreamCodexVersion"].as_str(), Some("0.120.0"));
    assert!(
        initialize["appHome"]
            .as_str()
            .unwrap_or_default()
            .ends_with(".ml-intern-codex")
    );

    let runtime_info = response_result(&responses, 2);
    let cwd_display = env.cwd.display().to_string();
    assert_eq!(runtime_info["codex_version"].as_str(), Some("0.120.0"));
    assert_eq!(runtime_info["cwd"].as_str(), Some(cwd_display.as_str()));

    let skills = response_result(&responses, 3);
    assert!(
        skills["skills"]
            .as_array()
            .is_some_and(|entries| !entries.is_empty()),
        "expected bundled skills in skills/list response"
    );

    let threads = response_result(&responses, 4);
    assert_eq!(threads["threads"], json!([]));

    let artifacts = response_result(&responses, 5);
    assert_eq!(artifacts["artifacts"], json!([]));

    assert!(env.home.join(".ml-intern-codex/config.toml").exists());
    assert!(env.home.join(".ml-intern-codex/db/state.sqlite").exists());
}

#[test]
fn ml_intern_app_server_initialize_fails_when_codex_is_missing() {
    let env = TestEnv::new("app-server-missing", FakeCodexScenario::MissingCodex);
    let output = run_app_server_with_requests(
        &env,
        &[json!({
            "id": 1,
            "method": "initialize",
            "params": {
                "client_info": {
                    "name": "process-smoke",
                    "title": "process-smoke",
                    "version": "0.1.0"
                },
                "capabilities": {
                    "transcriptStreaming": true
                }
            }
        })],
    );

    assert!(
        !output.status.success(),
        "expected initialize to fail without codex on PATH"
    );
    let stderr = String::from_utf8(output.stderr)
        .unwrap_or_else(|error| panic!("decode missing-codex stderr: {error}"));
    assert!(stderr.contains("failed to locate codex on PATH"));
    assert!(stderr.contains("codex was not found in PATH"));
}

#[test]
fn ml_intern_startup_smoke_renders_ready_banner_and_exits_cleanly() {
    let env = TestEnv::new("tui-startup", FakeCodexScenario::StartupOnly);
    let output = run_ml_intern_with_input(&env, "/quit\n");
    assert!(
        output.status.success(),
        "ml-intern failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("decode ml-intern stdout: {error}"));

    assert!(stdout.contains("=== ml-intern-codex ==="));
    assert!(stdout.contains("codex: 0.120.0"));
    assert!(stdout.contains("connection: Ready"));
    assert!(stdout.contains("Ready. Enter a prompt or use /help."));
    assert!(env.home.join(".ml-intern-codex/config.toml").exists());
    assert!(env.home.join(".ml-intern-codex/db/state.sqlite").exists());
}

#[test]
fn ml_intern_startup_allows_version_mismatch() {
    let env = TestEnv::new("tui-version-mismatch", FakeCodexScenario::VersionMismatch);
    let output = run_ml_intern_with_input(&env, "/quit\n");
    assert!(
        output.status.success(),
        "expected ml-intern startup to allow codex version mismatch: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("decode version-mismatch stdout: {error}"));
    assert!(stdout.contains("codex: 0.119.0"));
    assert!(stdout.contains("Ready. Enter a prompt or use /help."));
}

#[test]
fn ml_intern_prompt_smoke_streams_plan_updates_without_tty_and_persists_thread() {
    let env = TestEnv::new("tui-prompt", FakeCodexScenario::StreamingTurn);
    let output = run_ml_intern_with_input(&env, "inspect repo status\n/quit\n");
    assert!(
        output.status.success(),
        "ml-intern prompt smoke failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("decode ml-intern prompt stdout: {error}"));

    assert!(stdout.contains("you> inspect repo status"));
    assert!(stdout.contains("assistant> hello from fake codex"));
    assert!(stdout.contains("plan> Plan step | [in_progress] Do smoke"));
    let thread_dir = env.persisted_thread_dir();
    assert!(thread_dir.join("thread.json").exists());
    assert!(thread_dir.join("transcript.jsonl").exists());
    let transcript = env.transcript_text();
    assert!(transcript.contains("\"event\":\"plan_updated\""));
    assert!(transcript.contains("hello from fake codex"));
}

#[test]
fn ml_intern_skills_picker_smoke_selects_skill_and_forwards_turn_payload() {
    let env = TestEnv::new("tui-skills", FakeCodexScenario::SkillsTurn);
    let output = run_ml_intern_with_input(
        &env,
        "/skills\nml-runtime\n1\ninspect repo conventions\n/quit\n",
    );
    assert!(
        output.status.success(),
        "ml-intern skills smoke failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("decode ml-intern skills stdout: {error}"));

    assert!(stdout.contains("Filter skills (blank for all, q to cancel):"));
    assert!(stdout.contains("Skills:"));
    assert!(stdout.contains("ml-runtime-conventions [bundled]"));
    assert!(stdout.contains("Selected skill: ml-runtime-conventions [bundled]"));
    assert!(stdout.contains("assistant> skill payload captured"));

    let capture_lines = env.read_capture_lines();
    let turn_start = capture_lines
        .iter()
        .find(|entry| entry["kind"] == "turn_start_request")
        .unwrap_or_else(|| panic!("missing turn_start_request capture"));
    let input = turn_start["payload"]["params"]["input"]
        .as_array()
        .unwrap_or_else(|| panic!("missing forwarded input array"));
    assert_eq!(input.len(), 2);
    assert_eq!(input[0]["type"], json!("skill"));
    assert_eq!(input[0]["name"], json!("ml-runtime-conventions"));
    assert!(
        input[0]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("skills/system/ml-runtime-conventions/SKILL.md"))
    );
    assert_eq!(
        input[1],
        json!({
            "type": "text",
            "text": "inspect repo conventions"
        })
    );
}

#[test]
fn ml_intern_threads_resume_smoke_restores_saved_transcript() {
    let env = TestEnv::new("tui-resume", FakeCodexScenario::StreamingTurn);

    seed_streaming_thread_via_app_server(&env, "resume smoke seed prompt");
    let thread_dir = env.persisted_thread_dir();
    assert!(thread_dir.join("thread.json").exists());
    assert!(thread_dir.join("transcript.jsonl").exists());

    let resume_output = run_ml_intern_with_input(&env, "/threads\nresume smoke seed\n1\n/quit\n");
    assert!(
        resume_output.status.success(),
        "ml-intern resume smoke failed: {}",
        String::from_utf8_lossy(&resume_output.stderr)
    );

    let stdout = String::from_utf8(resume_output.stdout)
        .unwrap_or_else(|error| panic!("decode ml-intern resume stdout: {error}"));
    assert!(stdout.contains("Filter threads (blank for all, q to cancel):"));
    assert!(stdout.contains("Threads:"));
    assert!(stdout.contains("resume smoke seed prompt"));
    assert!(stdout.contains("you> resume smoke seed prompt"));
    assert!(stdout.contains("assistant> hello from fake codex"));

    let capture_lines = env.read_capture_lines();
    assert!(
        capture_lines.iter().any(|entry| {
            entry["kind"] == "thread_resume_request"
                && entry["payload"]["params"]["threadId"] == "up-thread-1"
                && entry["payload"]["params"]["persistExtendedHistory"] == true
        }),
        "expected upstream thread/resume payload in fake codex capture"
    );
}

#[cfg(unix)]
#[test]
fn ml_intern_fullscreen_startup_smoke_renders_event_driven_layout() {
    let env = TestEnv::new("tui-fullscreen-pty", FakeCodexScenario::StartupOnly);
    let output = run_ml_intern_fullscreen_with_pty(&env, "/quit");

    assert_eq!(
        output.exit_code, 0,
        "ml-intern PTY fullscreen smoke failed:\n{}",
        output.output
    );
    assert!(output.output.contains("Session"));
    assert!(output.output.contains("Transcript"));
    assert!(output.output.contains("Composer"));
    assert!(output.output.contains("Ready. Enter a prompt or"));
    assert!(output.output.contains("use /help."));
}

#[cfg(unix)]
#[test]
fn ml_intern_fullscreen_utf8_prompt_smoke_reaches_upstream() {
    let env = TestEnv::new("tui-fullscreen-utf8", FakeCodexScenario::SkillsTurn);
    let output = run_ml_intern_fullscreen_prompt_with_pty(&env, "你好 数据集");

    assert_eq!(
        output.exit_code, 0,
        "ml-intern PTY fullscreen UTF-8 smoke failed:\n{}",
        output.output
    );
    assert!(output.output.contains("你好 数据集"));
    assert!(output.output.contains("skill payload captured"));

    let capture_lines = env.read_capture_lines();
    let turn_start = capture_lines
        .iter()
        .find(|entry| entry["kind"] == "turn_start_request")
        .unwrap_or_else(|| panic!("missing turn_start_request capture"));
    let input = turn_start["payload"]["params"]["input"]
        .as_array()
        .unwrap_or_else(|| panic!("missing forwarded input array"));
    assert_eq!(input.len(), 1);
    assert_eq!(
        input[0],
        json!({
            "type": "text",
            "text": "你好 数据集"
        })
    );
}

#[cfg(unix)]
#[test]
fn ml_intern_fullscreen_skill_picker_overlay_smoke_selects_skill_and_forwards_turn_payload() {
    let env = TestEnv::new("tui-fullscreen-skills", FakeCodexScenario::SkillsTurn);
    let output = run_ml_intern_fullscreen_skill_picker_with_pty(&env, "inspect repo conventions");

    assert_eq!(
        output.exit_code, 0,
        "ml-intern PTY fullscreen skill picker smoke failed:\n{}",
        output.output
    );
    assert!(output.output.contains("Skills"));
    assert!(output.output.contains("Esc close, Enter select"));
    assert!(output.output.contains("ml-runtime-conventions"));
    assert!(output.output.contains("selected skill"));
    assert!(output.output.contains("ml-runtime-conventions"));
    assert!(output.output.contains("skill payload captured"));

    let capture_lines = env.read_capture_lines();
    let turn_start = capture_lines
        .iter()
        .find(|entry| entry["kind"] == "turn_start_request")
        .unwrap_or_else(|| panic!("missing turn_start_request capture"));
    let input = turn_start["payload"]["params"]["input"]
        .as_array()
        .unwrap_or_else(|| panic!("missing forwarded input array"));
    assert_eq!(input.len(), 2);
    assert_eq!(input[0]["type"], json!("skill"));
    assert_eq!(input[0]["name"], json!("ml-runtime-conventions"));
    assert!(
        input[0]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("skills/system/ml-runtime-conventions/SKILL.md"))
    );
    assert_eq!(
        input[1],
        json!({
            "type": "text",
            "text": "inspect repo conventions"
        })
    );
}

#[cfg(unix)]
#[test]
fn ml_intern_fullscreen_command_approval_overlay_smoke_round_trips_decision() {
    let env = TestEnv::new(
        "tui-fullscreen-command-approval",
        FakeCodexScenario::CommandApprovalTurn,
    );
    let output = run_ml_intern_fullscreen_command_approval_with_pty(&env, "run risky command");

    assert_eq!(
        output.exit_code, 0,
        "ml-intern PTY fullscreen command approval smoke failed:\n{}",
        output.output
    );
    assert!(output.output.contains("Approve command: python risky.py"));
    assert!(output.output.contains("Enter submit"));
    assert!(output.output.contains("command approval result:"));
    assert!(output.output.contains("accept"));

    let capture_lines = env.read_capture_lines();
    assert_eq!(capture_lines.len(), 1);
    assert_eq!(capture_lines[0]["kind"], "client_response");
    assert_eq!(
        capture_lines[0]["payload"]["result"],
        json!({
            "decision": "accept"
        })
    );

    let transcript = env.transcript_text();
    assert!(transcript.contains("\"approval_id\":\"11\""));
    assert!(transcript.contains("\"decision\":\"approve\""));
    assert!(transcript.contains("python risky.py"));
}

#[cfg(unix)]
#[test]
fn ml_intern_fullscreen_request_user_input_overlay_smoke_collects_answers() {
    let env = TestEnv::new(
        "tui-fullscreen-request-user-input",
        FakeCodexScenario::RequestUserInputTurn,
    );
    let output = run_ml_intern_fullscreen_request_user_input_with_pty(
        &env,
        "inspect dataset readiness",
        "needs shuffle before training",
    );

    assert_eq!(
        output.exit_code, 0,
        "ml-intern PTY fullscreen request_user_input smoke failed:\n{}",
        output.output
    );
    assert!(output.output.contains("Tool needs more input"));
    assert!(output.output.contains("Dataset"));
    assert!(output.output.contains("Pick a dataset"));
    assert!(output.output.contains("Follow-up"));
    assert!(output.output.contains("Secret input is masked locally."));
    assert!(!output.output.contains("needs shuffle before training"));
    assert!(output.output.contains("dataset answers captured"));

    let capture_lines = env.read_capture_lines();
    assert_eq!(capture_lines.len(), 1);
    assert_eq!(capture_lines[0]["kind"], "client_response");
    assert_eq!(
        capture_lines[0]["payload"]["result"],
        json!({
            "answers": {
                "dataset": {
                    "answers": ["fineweb"]
                },
                "note": {
                    "answers": ["needs shuffle before training"]
                }
            }
        })
    );

    let transcript = env.transcript_text();
    assert!(transcript.contains("\"approval_id\":\"41\""));
    assert!(transcript.contains("\"decision\":\"approve\""));
    assert!(transcript.contains("\"dataset\":{\"answers\":[\"fineweb\"]}"));
    assert!(transcript.contains("needs shuffle before training"));
}

#[cfg(unix)]
#[test]
fn ml_intern_fullscreen_thread_picker_overlay_smoke_restores_saved_transcript() {
    let env = TestEnv::new("tui-fullscreen-resume", FakeCodexScenario::StreamingTurn);

    seed_streaming_thread_via_app_server(&env, "resume smoke seed prompt");
    let thread_dir = env.persisted_thread_dir();
    assert!(thread_dir.join("thread.json").exists());
    assert!(thread_dir.join("transcript.jsonl").exists());

    let output = run_ml_intern_fullscreen_thread_picker_with_pty(&env, "resume smoke seed");

    assert_eq!(
        output.exit_code, 0,
        "ml-intern PTY fullscreen thread picker smoke failed:\n{}",
        output.output
    );
    assert!(output.output.contains("Threads"));
    assert!(output.output.contains("Esc close, Enter resume"));
    assert!(output.output.contains("resume smoke seed prompt"));
    assert!(output.output.contains("you resume smoke seed prompt"));
    assert!(output.output.contains("hello from fake codex"));

    let capture_lines = env.read_capture_lines();
    assert!(
        capture_lines.iter().any(|entry| {
            entry["kind"] == "thread_resume_request"
                && entry["payload"]["params"]["threadId"] == "up-thread-1"
                && entry["payload"]["params"]["persistExtendedHistory"] == true
        }),
        "expected upstream thread/resume payload in fake codex capture"
    );
}

#[cfg(unix)]
#[test]
fn ml_intern_fullscreen_artifact_viewer_overlay_smoke_opens_primary_file_and_returns() {
    let env = TestEnv::new(
        "tui-fullscreen-artifact-viewer",
        FakeCodexScenario::StartupOnly,
    );
    let thread_id = "11111111-1111-4111-8111-111111111111";
    write_paper_report_artifact(
        &env,
        thread_id,
        "22222222-2222-4222-8222-222222222222",
        "33333333-3333-4333-8333-333333333333",
    );

    let output = run_ml_intern_fullscreen_artifact_viewer_with_pty(&env);

    assert_eq!(
        output.exit_code, 0,
        "ml-intern PTY fullscreen artifact viewer smoke failed:\n{}",
        output.output
    );
    assert!(output.output.contains("kind: PaperReport"));
    assert!(output.output.contains("File 1/3"));
    assert!(output.output.contains("# Paper Report:"));
    assert!(output.output.contains("\"paper_count\": 3"));
    assert!(output.output.contains("Filter: trl"));
}

#[cfg(unix)]
#[test]
fn ml_intern_fullscreen_artifact_viewer_overlay_smoke_shows_missing_file_read_error() {
    let env = TestEnv::new(
        "tui-fullscreen-artifact-viewer-read-error",
        FakeCodexScenario::StartupOnly,
    );
    let thread_id = "11111111-1111-4111-8111-111111111111";
    let artifact_id = "33333333-3333-4333-8333-333333333333";
    write_paper_report_artifact(
        &env,
        thread_id,
        "22222222-2222-4222-8222-222222222222",
        artifact_id,
    );
    fs::remove_file(
        env.cwd
            .join(".ml-intern/threads")
            .join(thread_id)
            .join("artifacts")
            .join(artifact_id)
            .join("raw.txt"),
    )
    .unwrap_or_else(|error| panic!("remove fullscreen paper raw.txt: {error}"));

    let output = run_ml_intern_fullscreen_artifact_viewer_read_error_with_pty(&env);

    assert_eq!(
        output.exit_code, 0,
        "ml-intern PTY fullscreen artifact viewer read error smoke failed:\n{}",
        output.output
    );
    assert!(output.output.contains("kind: PaperReport"));
    assert!(output.output.contains("File 3/3"));
    assert!(output.output.contains("failed to read file"));
    assert!(output.output.contains("Filter: trl"));
}

#[cfg(unix)]
#[test]
fn ml_intern_fullscreen_interrupt_smoke_uses_escape_and_returns_ready() {
    let env = TestEnv::new(
        "tui-fullscreen-interrupt-pty",
        FakeCodexScenario::InterruptibleTurn,
    );
    let output = run_ml_intern_fullscreen_interrupt_with_pty(&env, "interrupt me");

    assert_eq!(
        output.exit_code, 0,
        "ml-intern PTY fullscreen interrupt smoke failed:\n{}",
        output.output
    );
    assert!(output.output.contains("you interrupt me"));
    assert!(output.output.contains("still working"));
    assert!(output.output.contains("Interrupt requested."));
    assert!(
        output.output.matches("Ready overlays none").count() >= 2,
        "expected fullscreen connection to return to Ready after interrupt:\n{}",
        output.output
    );

    let capture_lines = env.read_capture_lines();
    assert!(
        capture_lines.iter().any(|entry| {
            entry["kind"] == "turn_interrupt_request"
                && entry["payload"]["params"]["threadId"] == "up-thread-1"
                && entry["payload"]["params"]["turnId"] == "up-turn-1"
        }),
        "expected upstream turn/interrupt payload in fake codex capture"
    );

    let transcript = env.transcript_text();
    assert!(transcript.contains("\"event\":\"interrupt_requested\""));
    assert!(transcript.contains("\"status\":\"interrupted\""));
    assert!(transcript.contains("still working"));
}

#[cfg(unix)]
#[test]
fn ml_intern_fullscreen_help_overlay_interrupt_smoke_prioritizes_live_interrupt() {
    let env = TestEnv::new(
        "tui-fullscreen-help-interrupt-pty",
        FakeCodexScenario::InterruptibleTurn,
    );
    let output = run_ml_intern_fullscreen_help_overlay_interrupt_with_pty(&env, "interrupt me");

    assert_eq!(
        output.exit_code, 0,
        "ml-intern PTY fullscreen help-overlay interrupt smoke failed:\n{}",
        output.output
    );
    assert!(output.output.contains("still working"));
    assert!(output.output.contains("Tab opens this help overlay."));
    assert!(output.output.contains("Interrupt requested."));
    assert!(
        output.output.matches("Ready overlays none").count() >= 2,
        "expected fullscreen connection to return to Ready after interrupting over help overlay:\n{}",
        output.output
    );

    let capture_lines = env.read_capture_lines();
    assert!(
        capture_lines.iter().any(|entry| {
            entry["kind"] == "turn_interrupt_request"
                && entry["payload"]["params"]["threadId"] == "up-thread-1"
                && entry["payload"]["params"]["turnId"] == "up-turn-1"
        }),
        "expected upstream turn/interrupt payload in fake codex capture"
    );
}

#[cfg(unix)]
#[test]
fn ml_intern_fullscreen_interrupt_failure_smoke_keeps_live_turn_and_surfaces_error() {
    let env = TestEnv::new(
        "tui-fullscreen-interrupt-failure-pty",
        FakeCodexScenario::InterruptErrorTurn,
    );
    let output = run_ml_intern_fullscreen_interrupt_failure_with_pty(&env, "interrupt me");

    assert_eq!(
        output.exit_code, 0,
        "ml-intern PTY fullscreen interrupt-failure smoke failed:\n{}",
        output.output
    );
    assert!(output.output.contains("still working"));
    assert!(output.output.contains("app-server closed"));
    assert!(output.output.contains("interrupt it first."));
    assert!(!output.output.contains("Thread status: Interrupted"));

    let capture_lines = env.read_capture_lines();
    assert!(
        capture_lines.iter().any(|entry| {
            entry["kind"] == "turn_interrupt_request"
                && entry["payload"]["params"]["threadId"] == "up-thread-1"
                && entry["payload"]["params"]["turnId"] == "up-turn-1"
        }),
        "expected upstream turn/interrupt payload in fake codex capture"
    );

    let transcript = env.transcript_text();
    assert!(!transcript.contains("\"event\":\"interrupt_requested\""));
    assert!(!transcript.contains("\"status\":\"interrupted\""));
}

#[cfg(unix)]
#[test]
fn ml_intern_fullscreen_busy_commands_smoke_require_interrupt_before_opening_pickers() {
    let env = TestEnv::new(
        "tui-fullscreen-busy-command-guard",
        FakeCodexScenario::InterruptibleTurn,
    );
    let output = run_ml_intern_fullscreen_busy_command_with_pty(&env, "interrupt me");

    assert_eq!(
        output.exit_code, 0,
        "ml-intern PTY fullscreen busy-command smoke failed:\n{}",
        output.output
    );
    assert!(output.output.contains("still working"));
    assert!(output.output.contains("Interrupt the active turn"));
    assert!(output.output.contains("before running /threads."));
    assert!(!output.output.contains("Enter resume"));
    assert!(output.output.contains("Interrupt requested."));
}

#[cfg(unix)]
#[test]
fn ml_intern_interrupt_smoke_uses_tty_escape_watcher_and_returns_ready() {
    let env = TestEnv::new("tui-interrupt-pty", FakeCodexScenario::InterruptibleTurn);
    let output = run_ml_intern_interrupt_with_pty(&env, "interrupt me");

    assert_eq!(
        output.exit_code, 0,
        "ml-intern PTY interrupt smoke failed:\n{}",
        output.output
    );
    assert!(output.output.contains("interrupt me"));
    assert!(output.output.contains("still working"));
    assert!(output.output.contains("status> Interrupt requested."));
    assert!(
        output.output.matches("connection: Ready").count() >= 2,
        "expected connection to return to Ready after interrupt:\n{}",
        output.output
    );

    let capture_lines = env.read_capture_lines();
    assert!(
        capture_lines.iter().any(|entry| {
            entry["kind"] == "turn_interrupt_request"
                && entry["payload"]["params"]["threadId"] == "up-thread-1"
                && entry["payload"]["params"]["turnId"] == "up-turn-1"
        }),
        "expected upstream turn/interrupt payload in fake codex capture"
    );

    let transcript = env.transcript_text();
    assert!(transcript.contains("\"event\":\"interrupt_requested\""));
    assert!(transcript.contains("\"status\":\"interrupted\""));
    assert!(transcript.contains("still working"));
}

#[test]
fn ml_intern_app_server_interrupt_smoke_round_trips_and_persists_transcript() {
    let env = TestEnv::new("app-server-interrupt", FakeCodexScenario::InterruptibleTurn);
    let mut session = AppServerSession::spawn(&env);

    session.send(&json!({
        "id": 1,
        "method": "initialize",
        "params": {
            "client_info": {
                "name": "process-smoke",
                "title": "process-smoke",
                "version": "0.1.0"
            },
            "capabilities": {
                "transcriptStreaming": true,
                "skillPicker": true,
                "artifactsOverlay": true
            }
        }
    }));
    let initialize_messages = session.collect_until(|messages| has_response(messages, 1));
    assert_eq!(
        response_result(&initialize_messages, 1)["upstreamCodexVersion"].as_str(),
        Some("0.120.0")
    );

    session.send(&json!({"method": "initialized"}));
    session.send(&json!({
        "id": 2,
        "method": "thread/start",
        "params": {
            "cwd": env.cwd,
            "title": "interrupt smoke",
            "model": null,
            "approval_policy": null,
            "sandbox_mode": null
        }
    }));
    let thread_messages = session.collect_until(|messages| has_response(messages, 2));
    let thread_id = response_result(&thread_messages, 2)["thread"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing local thread id"))
        .to_owned();

    session.send(&json!({
        "id": 3,
        "method": "turn/start",
        "params": {
            "thread_id": thread_id,
            "input": [
                {
                    "type": "text",
                    "text": "interrupt me"
                }
            ]
        }
    }));
    let turn_messages = session.collect_until(|messages| {
        has_response(messages, 3)
            && has_notification(messages, "turn/started")
            && has_notification(messages, "item/agentMessage/delta")
    });
    let turn_id = response_result(&turn_messages, 3)["turn"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing local turn id"))
        .to_owned();

    session.send(&json!({
        "id": 4,
        "method": "turn/interrupt",
        "params": {
            "thread_id": thread_id,
            "turn_id": turn_id
        }
    }));
    let interrupt_messages = session.collect_until(|messages| {
        has_response(messages, 4)
            && notification_status(messages, "turn/completed") == Some("interrupted")
    });
    session.finish();

    assert_eq!(response_result(&interrupt_messages, 4), json!({}));
    assert_eq!(
        notification_status(&interrupt_messages, "turn/completed"),
        Some("interrupted")
    );

    let capture_lines = env.read_capture_lines();
    assert!(
        capture_lines.iter().any(|entry| {
            entry["kind"] == "turn_interrupt_request"
                && entry["payload"]["params"]["threadId"] == "up-thread-1"
                && entry["payload"]["params"]["turnId"] == "up-turn-1"
        }),
        "expected upstream turn/interrupt payload in fake codex capture"
    );

    let transcript = env.transcript_text();
    assert!(transcript.contains("\"event\":\"interrupt_requested\""));
    assert!(transcript.contains("\"status\":\"interrupted\""));
    assert!(transcript.contains("still working"));
}

#[test]
fn ml_intern_app_server_artifact_smoke_detects_lists_and_reads_dataset_audit() {
    let env = TestEnv::new("app-server-artifact", FakeCodexScenario::InterruptibleTurn);
    let (mut session, thread_id, turn_id) =
        start_interruptible_turn_session(&env, "create dataset audit artifact");

    let artifact_id = "00000000-0000-4000-8000-000000000001";
    write_dataset_audit_artifact(&env, &thread_id, &turn_id, artifact_id);

    let artifact_messages =
        session.collect_until(|messages| has_notification(messages, "artifact/created"));
    let artifact_created = notification_params(&artifact_messages, "artifact/created");
    assert_eq!(
        artifact_created["manifest"]["title"].as_str(),
        Some("Dataset audit for demo/corpus")
    );
    assert_eq!(
        artifact_created["preview"]["dataset"].as_str(),
        Some("demo/corpus")
    );

    session.send(&json!({
        "id": 4,
        "method": "artifact/list",
        "params": {
            "thread_id": thread_id,
            "kind": null,
            "limit": 20
        }
    }));
    let list_messages = session.collect_until(|messages| has_response(messages, 4));
    let artifacts = response_result(&list_messages, 4)["artifacts"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("missing artifacts array"));
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0]["id"].as_str(), Some(artifact_id));

    session.send(&json!({
        "id": 5,
        "method": "artifact/read",
        "params": {
            "artifact_id": artifact_id
        }
    }));
    let read_messages = session.collect_until(|messages| has_response(messages, 5));
    let read_result = response_result(&read_messages, 5);
    assert_eq!(
        read_result["manifest"]["title"].as_str(),
        Some("Dataset audit for demo/corpus")
    );
    let files = read_result["files"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("missing artifact files"));
    assert!(files.iter().any(|file| {
        file["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("/report.md"))
    }));
    assert!(files.iter().any(|file| {
        file["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("/report.json"))
    }));

    session.send(&json!({
        "id": 6,
        "method": "turn/interrupt",
        "params": {
            "thread_id": thread_id,
            "turn_id": turn_id
        }
    }));
    let interrupt_messages = session.collect_until(|messages| {
        has_response(messages, 6)
            && notification_status(messages, "turn/completed") == Some("interrupted")
    });
    assert_eq!(response_result(&interrupt_messages, 6), json!({}));
    session.finish();

    let transcript = env.transcript_text();
    assert!(transcript.contains("Dataset audit for demo/corpus"));
    assert!(transcript.contains("\"event\":\"interrupt_requested\""));
}

#[test]
fn ml_intern_app_server_artifact_warning_smoke_skips_malformed_manifest() {
    let env = TestEnv::new(
        "app-server-artifact-warning",
        FakeCodexScenario::InterruptibleTurn,
    );
    let (mut session, thread_id, turn_id) =
        start_interruptible_turn_session(&env, "create artifact warning");

    let valid_artifact_id = "00000000-0000-4000-8000-000000000003";
    let broken_artifact_id = "00000000-0000-4000-8000-000000000004";
    write_dataset_audit_artifact(&env, &thread_id, &turn_id, valid_artifact_id);
    write_malformed_artifact_manifest(&env, &thread_id, broken_artifact_id);

    let warning_messages = session.collect_until(|messages| {
        has_notification(messages, "artifact/created") && has_notification(messages, "warning")
    });
    let created = notification_params(&warning_messages, "artifact/created");
    assert_eq!(
        created["manifest"]["title"].as_str(),
        Some("Dataset audit for demo/corpus")
    );
    let warning = notification_params(&warning_messages, "warning");
    let warning_message = warning["message"]
        .as_str()
        .unwrap_or_else(|| panic!("missing warning message"));
    assert!(warning_message.contains("Skipped malformed artifact manifest"));
    assert!(warning_message.contains(broken_artifact_id));
    assert!(warning_message.contains("artifact.json"));

    session.send(&json!({
        "id": 4,
        "method": "artifact/list",
        "params": {
            "thread_id": thread_id,
            "kind": null,
            "limit": 20
        }
    }));
    let list_messages = session.collect_until(|messages| has_response(messages, 4));
    let artifacts = response_result(&list_messages, 4)["artifacts"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("missing artifacts array"));
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0]["id"].as_str(), Some(valid_artifact_id));

    session.send(&json!({
        "id": 5,
        "method": "turn/interrupt",
        "params": {
            "thread_id": thread_id,
            "turn_id": turn_id
        }
    }));
    let interrupt_messages = session.collect_until(|messages| {
        has_response(messages, 5)
            && notification_status(messages, "turn/completed") == Some("interrupted")
    });
    assert_eq!(response_result(&interrupt_messages, 5), json!({}));
    session.finish();

    let transcript = env.transcript_text();
    assert!(transcript.contains("Dataset audit for demo/corpus"));
    assert!(transcript.contains("Skipped malformed artifact manifest"));
    assert!(transcript.contains(broken_artifact_id));
}

#[test]
fn ml_intern_app_server_job_artifact_update_smoke_emits_updated_notification() {
    let env = TestEnv::new(
        "app-server-job-update",
        FakeCodexScenario::InterruptibleTurn,
    );
    let (mut session, thread_id, turn_id) =
        start_interruptible_turn_session(&env, "track hf job artifact");

    let artifact_id = "00000000-0000-4000-8000-000000000002";
    write_job_snapshot_artifact(&env, &thread_id, &turn_id, artifact_id);

    let created_messages =
        session.collect_until(|messages| has_notification(messages, "artifact/created"));
    let created = notification_params(&created_messages, "artifact/created");
    assert_eq!(
        created["manifest"]["title"].as_str(),
        Some("Job snapshot for job-123")
    );
    assert_eq!(created["preview"]["job_id"].as_str(), Some("job-123"));
    assert_eq!(created["preview"]["status"].as_str(), Some("running"));
    assert_eq!(created["preview"]["hardware"].as_str(), Some("a10g-large"));
    assert_eq!(
        created["preview"]["dashboard_url"].as_str(),
        Some("https://hf.co/jobs/job-123")
    );

    // The helper manifest timestamps are second-granularity, so wait before rewriting.
    thread::sleep(Duration::from_millis(1100));
    write_job_snapshot_artifact_with(&env, &thread_id, &turn_id, artifact_id, "completed", "240");

    let updated_messages =
        session.collect_until(|messages| has_notification(messages, "artifact/updated"));
    let updated = notification_params(&updated_messages, "artifact/updated");
    assert_eq!(
        updated["manifest"]["summary"].as_str(),
        Some("job-123 is currently completed")
    );
    assert_eq!(updated["preview"]["job_id"].as_str(), Some("job-123"));
    assert_eq!(updated["preview"]["status"].as_str(), Some("completed"));
    assert_eq!(updated["preview"]["hardware"].as_str(), Some("a10g-large"));

    session.send(&json!({
        "id": 4,
        "method": "artifact/list",
        "params": {
            "thread_id": thread_id,
            "kind": null,
            "limit": 20
        }
    }));
    let list_messages = session.collect_until(|messages| has_response(messages, 4));
    let artifacts = response_result(&list_messages, 4)["artifacts"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("missing artifacts array"));
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0]["id"].as_str(), Some(artifact_id));
    assert_eq!(
        artifacts[0]["summary"].as_str(),
        Some("job-123 is currently completed")
    );

    session.send(&json!({
        "id": 5,
        "method": "artifact/read",
        "params": {
            "artifact_id": artifact_id
        }
    }));
    let read_messages = session.collect_until(|messages| has_response(messages, 5));
    let read_result = response_result(&read_messages, 5);
    assert_eq!(
        read_result["manifest"]["summary"].as_str(),
        Some("job-123 is currently completed")
    );
    let report_markdown = read_result["files"]
        .as_array()
        .and_then(|files| {
            files.iter().find_map(|file| {
                let is_report = file["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with("/report.md"));
                if is_report {
                    file["text"].as_str()
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| panic!("missing report.md payload"));
    assert!(report_markdown.contains("- Status: completed"));
    assert!(report_markdown.contains("- Duration (seconds): 240"));

    session.send(&json!({
        "id": 6,
        "method": "turn/interrupt",
        "params": {
            "thread_id": thread_id,
            "turn_id": turn_id
        }
    }));
    let interrupt_messages = session.collect_until(|messages| {
        has_response(messages, 6)
            && notification_status(messages, "turn/completed") == Some("interrupted")
    });
    assert_eq!(response_result(&interrupt_messages, 6), json!({}));
    session.finish();

    let transcript = env.transcript_text();
    assert!(transcript.contains("\"kind\":\"job_snapshot\""));
    assert!(transcript.contains("\"status\":\"running\""));
    assert!(transcript.contains("\"status\":\"completed\""));
}

#[test]
fn ml_intern_app_server_command_approval_smoke_round_trips_jsonl_and_persists_transcript() {
    let env = TestEnv::new(
        "app-server-command-approval",
        FakeCodexScenario::CommandApprovalTurn,
    );
    let (mut session, _thread_id, _turn_id, approval) =
        start_approval_turn_session(&env, "run risky command");

    assert_eq!(approval["approval"]["id"].as_str(), Some("11"));
    assert_eq!(
        approval["approval"]["kind"].as_str(),
        Some("command_execution")
    );
    assert_eq!(
        approval["approval"]["title"].as_str(),
        Some("Approve command: python risky.py")
    );

    session.send(&json!({
        "id": 4,
        "method": "approval/respond",
        "params": {
            "approval_id": "11",
            "decision": "approve"
        }
    }));
    let approval_messages = session.collect_until(|messages| {
        has_response(messages, 4)
            && has_notification(messages, "item/agentMessage/delta")
            && notification_status(messages, "turn/completed") == Some("completed")
    });
    session.finish();

    assert_eq!(response_result(&approval_messages, 4), json!({}));
    let capture_lines = env.read_capture_lines();
    assert_eq!(capture_lines.len(), 1);
    assert_eq!(capture_lines[0]["kind"], "client_response");
    assert_eq!(
        capture_lines[0]["payload"]["result"],
        json!({
            "decision": "accept"
        })
    );

    let transcript = env.transcript_text();
    assert!(transcript.contains("\"approval_id\":\"11\""));
    assert!(transcript.contains("\"decision\":\"approve\""));
    assert!(transcript.contains("python risky.py"));
    assert!(transcript.contains("command approval result"));
}

#[test]
fn ml_intern_app_server_request_user_input_smoke_round_trips_answers_and_persists_transcript() {
    let env = TestEnv::new(
        "app-server-request-user-input",
        FakeCodexScenario::RequestUserInputTurn,
    );
    let (mut session, _thread_id, _turn_id, approval) =
        start_approval_turn_session(&env, "inspect dataset readiness");

    assert_eq!(approval["approval"]["id"].as_str(), Some("41"));
    assert_eq!(
        approval["approval"]["kind"].as_str(),
        Some("request_user_input")
    );
    assert_eq!(
        approval["approval"]["title"].as_str(),
        Some("Tool needs more input")
    );

    session.send(&json!({
        "id": 4,
        "method": "approval/respond",
        "params": {
            "approval_id": "41",
            "decision": "approve",
            "answers": {
                "dataset": {
                    "answers": ["fineweb"]
                },
                "note": {
                    "answers": ["needs shuffle before training"]
                }
            }
        }
    }));
    let approval_messages = session.collect_until(|messages| {
        has_response(messages, 4)
            && has_notification(messages, "item/agentMessage/delta")
            && notification_status(messages, "turn/completed") == Some("completed")
    });
    session.finish();

    assert_eq!(response_result(&approval_messages, 4), json!({}));
    let capture_lines = env.read_capture_lines();
    assert_eq!(capture_lines.len(), 1);
    assert_eq!(capture_lines[0]["kind"], "client_response");
    assert_eq!(
        capture_lines[0]["payload"]["result"],
        json!({
            "answers": {
                "dataset": {
                    "answers": ["fineweb"]
                },
                "note": {
                    "answers": ["needs shuffle before training"]
                }
            }
        })
    );

    let transcript = env.transcript_text();
    assert!(transcript.contains("\"approval_id\":\"41\""));
    assert!(transcript.contains("\"decision\":\"approve\""));
    assert!(transcript.contains("\"dataset\":{\"answers\":[\"fineweb\"]}"));
    assert!(transcript.contains("needs shuffle before training"));
    assert!(transcript.contains("dataset answers captured"));
}

#[test]
fn ml_intern_command_approval_smoke_forwards_decision_and_persists_transcript() {
    let env = TestEnv::new(
        "tui-command-approval",
        FakeCodexScenario::CommandApprovalTurn,
    );
    let output = run_ml_intern_with_input(&env, "run risky command\ny\n/quit\n");
    assert!(
        output.status.success(),
        "ml-intern command approval smoke failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("decode ml-intern command approval stdout: {error}"));

    assert!(stdout.contains("approval> Approve command: python risky.py"));
    assert!(stdout.contains("Approval requested: Approve command: python risky.py"));
    assert!(stdout.contains("Approval 11: approved"));
    assert!(stdout.contains("assistant> command approval result: accept"));

    let capture_lines = env.read_capture_lines();
    assert_eq!(capture_lines.len(), 1);
    assert_eq!(capture_lines[0]["kind"], "client_response");
    assert_eq!(
        capture_lines[0]["payload"]["result"],
        json!({
            "decision": "accept"
        })
    );

    let transcript = env.transcript_text();
    assert!(transcript.contains("\"approval_id\":\"11\""));
    assert!(transcript.contains("\"decision\":\"approve\""));
    assert!(transcript.contains("python risky.py"));
}

#[test]
fn ml_intern_request_user_input_smoke_collects_answers_and_persists_transcript() {
    let env = TestEnv::new(
        "tui-request-user-input",
        FakeCodexScenario::RequestUserInputTurn,
    );
    let output = run_ml_intern_with_input(
        &env,
        "inspect dataset readiness\n2\nneeds shuffle before training\n/quit\n",
    );
    assert!(
        output.status.success(),
        "ml-intern request_user_input smoke failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("decode ml-intern request_user_input stdout: {error}"));

    assert!(stdout.contains("approval> Tool needs more input"));
    assert!(stdout.contains("Approval requested: Tool needs more input"));
    assert!(stdout.contains("Dataset: Pick a dataset"));
    assert!(stdout.contains("Follow-up: Add an operator note"));
    assert!(stdout.contains("Approval 41: approved"));
    assert!(stdout.contains("assistant> dataset answers captured"));

    let capture_lines = env.read_capture_lines();
    assert_eq!(capture_lines.len(), 1);
    assert_eq!(capture_lines[0]["kind"], "client_response");
    assert_eq!(
        capture_lines[0]["payload"]["result"],
        json!({
            "answers": {
                "dataset": {
                    "answers": ["fineweb"]
                },
                "note": {
                    "answers": ["needs shuffle before training"]
                }
            }
        })
    );

    let transcript = env.transcript_text();
    assert!(transcript.contains("\"approval_id\":\"41\""));
    assert!(transcript.contains("\"decision\":\"approve\""));
    assert!(transcript.contains("\"dataset\":{\"answers\":[\"fineweb\"]}"));
    assert!(transcript.contains("needs shuffle before training"));
}

#[test]
fn ml_intern_artifact_browser_smoke_filters_switches_files_and_shows_read_errors() {
    let env = TestEnv::new("tui-artifact-browser", FakeCodexScenario::StartupOnly);
    let thread_id = "11111111-1111-4111-8111-111111111111";
    write_paper_report_artifact(
        &env,
        thread_id,
        "22222222-2222-4222-8222-222222222222",
        "33333333-3333-4333-8333-333333333333",
    );
    fs::remove_file(
        env.cwd
            .join(".ml-intern/threads")
            .join(thread_id)
            .join("artifacts")
            .join("33333333-3333-4333-8333-333333333333")
            .join("raw.txt"),
    )
    .unwrap_or_else(|error| panic!("remove paper raw.txt: {error}"));
    write_job_snapshot_artifact(
        &env,
        thread_id,
        "44444444-4444-4444-8444-444444444444",
        "55555555-5555-4555-8555-555555555555",
    );

    let output = run_ml_intern_with_input(
        &env,
        "/artifacts\ntrl\n1\n2\n3\nq\n/artifacts\njob-123\n1\nq\n/quit\n",
    );
    assert!(
        output.status.success(),
        "ml-intern artifact browser smoke failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("decode ml-intern artifact browser stdout: {error}"));

    assert_eq!(stdout.match_indices("Filter artifacts").count(), 2);
    assert_eq!(stdout.match_indices("Artifacts:").count(), 2);
    assert!(stdout.contains("Paper report for trl sft best practices"));
    assert!(stdout.contains("# Paper Report: trl sft best practices"));
    assert!(stdout.contains("\"paper_count\": 3"));
    assert!(stdout.contains("Select another file number (blank/q to close):"));
    assert!(stdout.contains("[read error]"));
    assert!(stdout.contains("failed to read file"));
    assert!(stdout.contains("Job snapshot for job-123"));
    assert!(stdout.contains("# Job Snapshot: job-123"));
    assert!(stdout.contains("Dashboard: https://hf.co/jobs/job-123"));
}

fn run_app_server_with_requests(env: &TestEnv, requests: &[Value]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ml-intern-app-server"))
        .current_dir(&env.cwd)
        .env("HOME", &env.home)
        .env("PATH", env.path_env())
        .env("MLI_INSTALL_ROOT", env.workspace_root())
        .env("MLI_FAKE_CODEX_SCENARIO", env.scenario.as_env())
        .env("MLI_FAKE_CODEX_CAPTURE", env.capture_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn ml-intern-app-server: {error}"));
    let mut stdin = child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("missing app-server stdin"));
    for request in requests {
        if let Err(error) = writeln!(stdin, "{request}") {
            if error.kind() == std::io::ErrorKind::BrokenPipe {
                break;
            }
            panic!("write app-server request: {error}");
        }
    }
    drop(stdin);
    child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("wait for app-server: {error}"))
}

fn run_ml_intern_with_input(env: &TestEnv, input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ml-intern"))
        .arg("--app-server-bin")
        .arg(env!("CARGO_BIN_EXE_ml-intern-app-server"))
        .current_dir(&env.cwd)
        .env("HOME", &env.home)
        .env("PATH", env.path_env())
        .env("MLI_INSTALL_ROOT", env.workspace_root())
        .env("MLI_FAKE_CODEX_SCENARIO", env.scenario.as_env())
        .env("MLI_FAKE_CODEX_CAPTURE", env.capture_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn ml-intern: {error}"));
    let mut stdin = child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("missing ml-intern stdin"));
    stdin
        .write_all(input.as_bytes())
        .unwrap_or_else(|error| panic!("write ml-intern input: {error}"));
    drop(stdin);

    child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("wait for ml-intern: {error}"))
}

#[cfg(unix)]
struct PtyRunOutput {
    exit_code: i32,
    output: String,
}

#[cfg(unix)]
struct PtyInputStep<'a> {
    wait_for: &'a str,
    input: String,
    label: &'a str,
}

#[cfg(unix)]
impl<'a> PtyInputStep<'a> {
    fn new(wait_for: &'a str, input: impl Into<String>, label: &'a str) -> Self {
        Self {
            wait_for,
            input: input.into(),
            label,
        }
    }
}

#[cfg(unix)]
fn sh_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\"'\"'"))
}

#[cfg(unix)]
fn bytes_snapshot(log: &Arc<Mutex<Vec<u8>>>) -> String {
    let bytes = log.lock().map(|value| value.clone()).unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(unix)]
fn fullscreen_snapshot(log: &Arc<Mutex<Vec<u8>>>) -> String {
    render_ansi_screen(&bytes_snapshot(log))
}

#[cfg(unix)]
fn wait_for_script_output(
    stdout_log: &Arc<Mutex<Vec<u8>>>,
    stderr_log: &Arc<Mutex<Vec<u8>>>,
    child: &mut Child,
    pattern: &str,
    search_start: usize,
    timeout: Duration,
    label: &str,
) {
    let deadline = Instant::now() + timeout;
    loop {
        let snapshot = bytes_snapshot(stdout_log);
        let search_start = search_start.min(snapshot.len());
        if snapshot[search_start..].contains(pattern) {
            return;
        }
        match child.try_wait() {
            Ok(Some(status)) => panic!(
                "PTY smoke exited early while waiting for {label} ({status})\n--- CAPTURED PTY ---\n{}\n--- SCRIPT STDERR ---\n{}",
                snapshot,
                bytes_snapshot(stderr_log)
            ),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => panic!(
                "timed out waiting for {label}\n--- CAPTURED PTY ---\n{}\n--- SCRIPT STDERR ---\n{}",
                snapshot,
                bytes_snapshot(stderr_log)
            ),
            Err(error) => {
                panic!("inspect PTY smoke child status while waiting for {label}: {error}")
            }
        }
    }
}

#[cfg(unix)]
fn wait_for_fullscreen_output(
    stdout_log: &Arc<Mutex<Vec<u8>>>,
    stderr_log: &Arc<Mutex<Vec<u8>>>,
    child: &mut Child,
    pattern: &str,
    timeout: Duration,
    label: &str,
) {
    let deadline = Instant::now() + timeout;
    loop {
        let rendered = fullscreen_snapshot(stdout_log);
        if rendered.contains(pattern) {
            return;
        }
        match child.try_wait() {
            Ok(Some(status)) => panic!(
                "PTY smoke exited early while waiting for {label} ({status})\n--- RENDERED PTY ---\n{}\n--- RAW PTY ---\n{}\n--- SCRIPT STDERR ---\n{}",
                rendered,
                bytes_snapshot(stdout_log),
                bytes_snapshot(stderr_log)
            ),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => panic!(
                "timed out waiting for {label}\n--- RENDERED PTY ---\n{}\n--- RAW PTY ---\n{}\n--- SCRIPT STDERR ---\n{}",
                rendered,
                bytes_snapshot(stdout_log),
                bytes_snapshot(stderr_log)
            ),
            Err(error) => {
                panic!("inspect PTY smoke child status while waiting for {label}: {error}")
            }
        }
    }
}

#[cfg(unix)]
fn render_ansi_screen(raw: &str) -> String {
    let mut screen = AnsiScreen::default();
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' if chars.peek() == Some(&'[') => {
                chars.next();
                let mut sequence = String::new();
                for next in chars.by_ref() {
                    sequence.push(next);
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
                screen.apply_csi(&sequence);
            }
            '\u{1b}' => {}
            '\r' => screen.col = 0,
            '\n' => {
                screen.row = screen.row.saturating_add(1);
                screen.col = 0;
            }
            '\u{8}' => {
                screen.col = screen.col.saturating_sub(1);
            }
            ch if !ch.is_control() => screen.put(ch),
            _ => {}
        }
    }
    screen.render()
}

#[cfg(unix)]
#[derive(Default)]
struct AnsiScreen {
    cells: Vec<Vec<char>>,
    row: usize,
    col: usize,
}

#[cfg(unix)]
impl AnsiScreen {
    fn put(&mut self, ch: char) {
        let width = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        self.ensure(self.row, self.col + width.saturating_sub(1));
        self.cells[self.row][self.col] = ch;
        for continuation in 1..width {
            self.cells[self.row][self.col + continuation] = '\0';
        }
        self.col = self.col.saturating_add(width);
    }

    fn apply_csi(&mut self, sequence: &str) {
        let Some((index, final_byte)) = sequence.char_indices().last() else {
            return;
        };
        let params = &sequence[..index];
        match final_byte {
            'H' | 'f' => {
                let mut parts = params.split(';');
                let row = parts
                    .next()
                    .and_then(|value| value.trim_start_matches('?').parse::<usize>().ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(1);
                let col = parts
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(1);
                self.row = row - 1;
                self.col = col - 1;
            }
            'J' => self.clear_all(),
            'K' => self.clear_line_from_cursor(),
            'A' => {
                let amount = params.parse::<usize>().unwrap_or(1);
                self.row = self.row.saturating_sub(amount);
            }
            'B' => {
                let amount = params.parse::<usize>().unwrap_or(1);
                self.row = self.row.saturating_add(amount);
            }
            'C' => {
                let amount = params.parse::<usize>().unwrap_or(1);
                self.col = self.col.saturating_add(amount);
            }
            'D' => {
                let amount = params.parse::<usize>().unwrap_or(1);
                self.col = self.col.saturating_sub(amount);
            }
            'm' | 'h' | 'l' => {}
            _ => {}
        }
    }

    fn clear_all(&mut self) {
        for row in &mut self.cells {
            for cell in row.iter_mut() {
                *cell = ' ';
            }
        }
        self.row = 0;
        self.col = 0;
    }

    fn clear_line_from_cursor(&mut self) {
        self.ensure(self.row, self.col);
        for cell in self.cells[self.row].iter_mut().skip(self.col) {
            *cell = ' ';
        }
    }

    fn ensure(&mut self, row: usize, col: usize) {
        while self.cells.len() <= row {
            self.cells.push(Vec::new());
        }
        if self.cells[row].len() <= col {
            self.cells[row].resize(col + 1, ' ');
        }
    }

    fn render(&self) -> String {
        self.cells
            .iter()
            .map(|row| {
                row.iter()
                    .filter(|cell| **cell != '\0')
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(unix)]
fn run_ml_intern_fullscreen_script_with_pty(
    env: &TestEnv,
    steps: &[PtyInputStep<'_>],
) -> PtyRunOutput {
    let ml_intern_bin = env.workspace_root().join("target/debug/ml-intern");
    let app_server_bin = env
        .workspace_root()
        .join("target/debug/ml-intern-app-server");
    let command = format!(
        "{} --app-server-bin {}",
        sh_quote(&ml_intern_bin),
        sh_quote(&app_server_bin)
    );
    let mut child = Command::new("script")
        .arg("-qefc")
        .arg(&command)
        .arg("/dev/null")
        .current_dir(&env.cwd)
        .env("HOME", &env.home)
        .env("PATH", env.path_env())
        .env("SHELL", "/bin/sh")
        .env("TERM", "xterm-256color")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("MLI_INSTALL_ROOT", env.workspace_root())
        .env("MLI_FAKE_CODEX_SCENARIO", env.scenario.as_env())
        .env("MLI_FAKE_CODEX_CAPTURE", env.capture_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn ml-intern PTY smoke script: {error}"));
    let mut stdin = child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("missing PTY smoke stdin"));
    let stdout = child
        .stdout
        .take()
        .unwrap_or_else(|| panic!("missing PTY smoke stdout"));
    let stderr = child
        .stderr
        .take()
        .unwrap_or_else(|| panic!("missing PTY smoke stderr"));
    let stdout_log = Arc::new(Mutex::new(Vec::new()));
    let stderr_log = Arc::new(Mutex::new(Vec::new()));
    let stdout_sink = Arc::clone(&stdout_log);
    let stderr_sink = Arc::clone(&stderr_log);
    let stdout_reader = thread::spawn(move || {
        let mut reader = stdout;
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(bytes_read) => {
                    if let Ok(mut log) = stdout_sink.lock() {
                        log.extend_from_slice(&buffer[..bytes_read]);
                    }
                }
                Err(_) => break,
            }
        }
    });
    let stderr_reader = thread::spawn(move || {
        let mut reader = stderr;
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(bytes_read) => {
                    if let Ok(mut log) = stderr_sink.lock() {
                        log.extend_from_slice(&buffer[..bytes_read]);
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut rendered_frames = Vec::new();
    for step in steps {
        wait_for_fullscreen_output(
            &stdout_log,
            &stderr_log,
            &mut child,
            step.wait_for,
            Duration::from_secs(5),
            step.label,
        );
        let rendered = fullscreen_snapshot(&stdout_log);
        if rendered_frames.last() != Some(&rendered) {
            rendered_frames.push(rendered);
        }
        if !step.input.is_empty() {
            stdin
                .write_all(step.input.as_bytes())
                .and_then(|_| stdin.flush())
                .unwrap_or_else(|error| {
                    panic!("write PTY fullscreen step `{}`: {error}", step.label)
                });
        }
    }
    drop(stdin);

    let status = child
        .wait()
        .unwrap_or_else(|error| panic!("wait for PTY fullscreen smoke child: {error}"));
    stdout_reader
        .join()
        .unwrap_or_else(|_| panic!("join PTY fullscreen stdout reader"));
    stderr_reader
        .join()
        .unwrap_or_else(|_| panic!("join PTY fullscreen stderr reader"));

    let rendered = fullscreen_snapshot(&stdout_log);
    if rendered_frames.last() != Some(&rendered) {
        rendered_frames.push(rendered);
    }

    PtyRunOutput {
        exit_code: status.code().unwrap_or_default(),
        output: rendered_frames.join("\n\n--- FRAME ---\n\n"),
    }
}

#[cfg(unix)]
fn run_ml_intern_fullscreen_with_pty(env: &TestEnv, command_input: &str) -> PtyRunOutput {
    run_ml_intern_fullscreen_script_with_pty(
        env,
        &[PtyInputStep::new(
            "Ready. Enter a prompt or",
            format!("{command_input}\r"),
            "fullscreen ready banner",
        )],
    )
}

#[cfg(unix)]
fn run_ml_intern_fullscreen_prompt_with_pty(env: &TestEnv, prompt: &str) -> PtyRunOutput {
    run_ml_intern_fullscreen_script_with_pty(
        env,
        &[
            PtyInputStep::new(
                "Ready. Enter a prompt or",
                format!("{prompt}\r"),
                "fullscreen ready banner",
            ),
            PtyInputStep::new(
                "skill payload captured",
                "/quit\r",
                "fullscreen UTF-8 prompt completion",
            ),
        ],
    )
}

#[cfg(unix)]
fn run_ml_intern_fullscreen_skill_picker_with_pty(env: &TestEnv, prompt: &str) -> PtyRunOutput {
    run_ml_intern_fullscreen_script_with_pty(
        env,
        &[
            PtyInputStep::new("Ready. Enter a prompt or", "$", "fullscreen ready banner"),
            PtyInputStep::new(
                "Esc close, Enter select",
                "ml-runtime",
                "fullscreen skill picker opened",
            ),
            PtyInputStep::new(
                "ml-runtime-conventions",
                "\r",
                "fullscreen skill picker filtered",
            ),
            PtyInputStep::new(
                "selected skill",
                format!("{prompt}\r"),
                "fullscreen skill selected",
            ),
            PtyInputStep::new(
                "skill payload captured",
                "/quit\r",
                "fullscreen skill prompt completion",
            ),
        ],
    )
}

#[cfg(unix)]
fn run_ml_intern_fullscreen_command_approval_with_pty(env: &TestEnv, prompt: &str) -> PtyRunOutput {
    run_ml_intern_fullscreen_script_with_pty(
        env,
        &[
            PtyInputStep::new(
                "Ready. Enter a prompt or",
                format!("{prompt}\r"),
                "fullscreen ready banner",
            ),
            PtyInputStep::new(
                "Approve command: python risky.py",
                "\r",
                "fullscreen command approval overlay",
            ),
            PtyInputStep::new(
                "command approval result:",
                "/quit\r",
                "fullscreen command approval completion",
            ),
        ],
    )
}

#[cfg(unix)]
fn run_ml_intern_fullscreen_thread_picker_with_pty(env: &TestEnv, filter: &str) -> PtyRunOutput {
    run_ml_intern_fullscreen_script_with_pty(
        env,
        &[
            PtyInputStep::new(
                "Ready. Enter a prompt or",
                "/threads\r",
                "fullscreen ready banner",
            ),
            PtyInputStep::new(
                "Esc close, Enter resume",
                filter,
                "fullscreen thread picker opened",
            ),
            PtyInputStep::new(
                "resume smoke seed prompt",
                "\r",
                "fullscreen thread picker filtered",
            ),
            PtyInputStep::new(
                "hello from fake codex",
                "\u{3}",
                "fullscreen thread resumed",
            ),
        ],
    )
}

#[cfg(unix)]
fn run_ml_intern_fullscreen_request_user_input_with_pty(
    env: &TestEnv,
    prompt: &str,
    note: &str,
) -> PtyRunOutput {
    run_ml_intern_fullscreen_script_with_pty(
        env,
        &[
            PtyInputStep::new(
                "Ready. Enter a prompt or",
                format!("{prompt}\r"),
                "fullscreen ready banner",
            ),
            PtyInputStep::new(
                "Dataset",
                "2\r",
                "fullscreen request_user_input dataset question",
            ),
            PtyInputStep::new(
                "Follow-up",
                format!("{note}\r"),
                "fullscreen request_user_input follow-up question",
            ),
            PtyInputStep::new(
                "dataset answers captured",
                "/quit\r",
                "fullscreen request_user_input completion",
            ),
        ],
    )
}

#[cfg(unix)]
fn run_ml_intern_fullscreen_artifact_viewer_with_pty(env: &TestEnv) -> PtyRunOutput {
    run_ml_intern_fullscreen_script_with_pty(
        env,
        &[
            PtyInputStep::new(
                "Ready. Enter a prompt or",
                "/artifacts\r",
                "fullscreen ready banner",
            ),
            PtyInputStep::new("kind: PaperReport", "trl\r", "fullscreen artifact picker"),
            PtyInputStep::new(
                "# Paper Report:",
                "\u{1b}[C",
                "fullscreen artifact viewer primary file",
            ),
            PtyInputStep::new(
                "\"paper_count\": 3",
                "\u{1b}",
                "fullscreen artifact viewer secondary file",
            ),
            PtyInputStep::new(
                "Filter: trl",
                "\u{1b}",
                "fullscreen artifact viewer returned to picker",
            ),
            PtyInputStep::new("Transcript", "\u{3}", "fullscreen artifact picker closed"),
        ],
    )
}

#[cfg(unix)]
fn run_ml_intern_fullscreen_artifact_viewer_read_error_with_pty(env: &TestEnv) -> PtyRunOutput {
    run_ml_intern_fullscreen_script_with_pty(
        env,
        &[
            PtyInputStep::new(
                "Ready. Enter a prompt or",
                "/artifacts\r",
                "fullscreen ready banner",
            ),
            PtyInputStep::new("kind: PaperReport", "trl\r", "fullscreen artifact picker"),
            PtyInputStep::new(
                "# Paper Report:",
                "\u{1b}[C",
                "fullscreen artifact viewer primary file",
            ),
            PtyInputStep::new(
                "\"paper_count\": 3",
                "\u{1b}[C",
                "fullscreen artifact viewer secondary file",
            ),
            PtyInputStep::new(
                "failed to read file",
                "\u{1b}",
                "fullscreen artifact viewer read error",
            ),
            PtyInputStep::new(
                "Filter: trl",
                "\u{1b}",
                "fullscreen artifact viewer returned to picker",
            ),
            PtyInputStep::new("Transcript", "\u{3}", "fullscreen artifact picker closed"),
        ],
    )
}

#[cfg(unix)]
fn run_ml_intern_fullscreen_interrupt_with_pty(env: &TestEnv, prompt: &str) -> PtyRunOutput {
    run_ml_intern_fullscreen_script_with_pty(
        env,
        &[
            PtyInputStep::new(
                "Ready. Enter a prompt or",
                &(prompt.to_owned() + "\r"),
                "fullscreen ready banner",
            ),
            PtyInputStep::new(
                "still working",
                "\u{1b}",
                "fullscreen streaming assistant output",
            ),
            PtyInputStep::new(
                "Interrupt requested.",
                "\u{3}",
                "fullscreen returned ready after interrupt",
            ),
        ],
    )
}

#[cfg(unix)]
fn run_ml_intern_fullscreen_help_overlay_interrupt_with_pty(
    env: &TestEnv,
    prompt: &str,
) -> PtyRunOutput {
    run_ml_intern_fullscreen_script_with_pty(
        env,
        &[
            PtyInputStep::new(
                "Ready. Enter a prompt or",
                &(prompt.to_owned() + "\r"),
                "fullscreen ready banner",
            ),
            PtyInputStep::new(
                "still working",
                "\t",
                "fullscreen streaming assistant output",
            ),
            PtyInputStep::new(
                "Tab opens this help overlay.",
                "\u{3}",
                "fullscreen help overlay opened",
            ),
            PtyInputStep::new(
                "Interrupt requested.",
                "\u{3}",
                "fullscreen returned ready after help-overlay interrupt",
            ),
        ],
    )
}

#[cfg(unix)]
fn run_ml_intern_fullscreen_interrupt_failure_with_pty(
    env: &TestEnv,
    prompt: &str,
) -> PtyRunOutput {
    run_ml_intern_fullscreen_script_with_pty(
        env,
        &[
            PtyInputStep::new(
                "Ready. Enter a prompt or",
                &(prompt.to_owned() + "\r"),
                "fullscreen ready banner",
            ),
            PtyInputStep::new(
                "still working",
                "\u{1b}",
                "fullscreen streaming assistant output",
            ),
            PtyInputStep::new(
                "app-server closed",
                "again\r",
                "fullscreen interrupt failure error",
            ),
            PtyInputStep::new(
                "interrupt it first.",
                "\u{7f}\u{7f}\u{7f}\u{7f}\u{7f}/quit\r",
                "fullscreen live turn preserved after interrupt failure",
            ),
        ],
    )
}

#[cfg(unix)]
fn run_ml_intern_fullscreen_busy_command_with_pty(env: &TestEnv, prompt: &str) -> PtyRunOutput {
    run_ml_intern_fullscreen_script_with_pty(
        env,
        &[
            PtyInputStep::new(
                "Ready. Enter a prompt or",
                &(prompt.to_owned() + "\r"),
                "fullscreen ready banner",
            ),
            PtyInputStep::new(
                "still working",
                "/threads\r",
                "fullscreen streaming assistant output",
            ),
            PtyInputStep::new(
                "Interrupt the active turn before running /threads.",
                "\u{1b}",
                "fullscreen busy command warning",
            ),
            PtyInputStep::new(
                "Interrupt requested.",
                "\u{3}",
                "fullscreen returned ready after busy-command interrupt",
            ),
        ],
    )
}

#[cfg(unix)]
fn run_ml_intern_interrupt_with_pty(env: &TestEnv, prompt: &str) -> PtyRunOutput {
    let ml_intern_bin = env.workspace_root().join("target/debug/ml-intern");
    let app_server_bin = env
        .workspace_root()
        .join("target/debug/ml-intern-app-server");
    let command = format!(
        "{} --line-mode --app-server-bin {}",
        sh_quote(&ml_intern_bin),
        sh_quote(&app_server_bin)
    );
    let mut child = Command::new("script")
        .arg("-qefc")
        .arg(&command)
        .arg("/dev/null")
        .current_dir(&env.cwd)
        .env("HOME", &env.home)
        .env("PATH", env.path_env())
        .env("SHELL", "/bin/sh")
        .env("TERM", "xterm-256color")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("MLI_INSTALL_ROOT", env.workspace_root())
        .env("MLI_FAKE_CODEX_SCENARIO", env.scenario.as_env())
        .env("MLI_FAKE_CODEX_CAPTURE", env.capture_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn ml-intern PTY smoke script: {error}"));
    let mut stdin = child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("missing PTY smoke stdin"));
    let stdout = child
        .stdout
        .take()
        .unwrap_or_else(|| panic!("missing PTY smoke stdout"));
    let stderr = child
        .stderr
        .take()
        .unwrap_or_else(|| panic!("missing PTY smoke stderr"));
    let stdout_log = Arc::new(Mutex::new(Vec::new()));
    let stderr_log = Arc::new(Mutex::new(Vec::new()));
    let stdout_sink = Arc::clone(&stdout_log);
    let stderr_sink = Arc::clone(&stderr_log);
    let stdout_reader = thread::spawn(move || {
        let mut reader = stdout;
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(bytes_read) => {
                    if let Ok(mut log) = stdout_sink.lock() {
                        log.extend_from_slice(&buffer[..bytes_read]);
                    }
                }
                Err(_) => break,
            }
        }
    });
    let stderr_reader = thread::spawn(move || {
        let mut reader = stderr;
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(bytes_read) => {
                    if let Ok(mut log) = stderr_sink.lock() {
                        log.extend_from_slice(&buffer[..bytes_read]);
                    }
                }
                Err(_) => break,
            }
        }
    });

    wait_for_script_output(
        &stdout_log,
        &stderr_log,
        &mut child,
        "Ready. Enter a prompt or use /help.",
        0,
        Duration::from_secs(5),
        "ml-intern ready banner",
    );
    wait_for_script_output(
        &stdout_log,
        &stderr_log,
        &mut child,
        "/quit\r\n> ",
        0,
        Duration::from_secs(5),
        "interactive prompt",
    );
    thread::sleep(Duration::from_millis(50));

    stdin
        .write_all(prompt.as_bytes())
        .and_then(|_| stdin.write_all(b"\r"))
        .and_then(|_| stdin.flush())
        .unwrap_or_else(|error| panic!("write PTY smoke prompt: {error}"));
    wait_for_script_output(
        &stdout_log,
        &stderr_log,
        &mut child,
        "assistant~ still working",
        0,
        Duration::from_secs(5),
        "streaming assistant output",
    );

    stdin
        .write_all(&[0x1b])
        .and_then(|_| stdin.flush())
        .unwrap_or_else(|error| panic!("write PTY smoke escape: {error}"));
    wait_for_script_output(
        &stdout_log,
        &stderr_log,
        &mut child,
        "status> Interrupt requested.",
        0,
        Duration::from_secs(5),
        "interrupt request status",
    );

    thread::sleep(Duration::from_millis(200));
    stdin
        .write_all(b"/quit\r")
        .and_then(|_| stdin.flush())
        .unwrap_or_else(|error| panic!("write PTY smoke quit: {error}"));
    drop(stdin);

    let deadline = Instant::now() + Duration::from_secs(5);
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or_default(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                panic!(
                    "ml-intern PTY smoke did not exit after /quit\n--- CAPTURED PTY ---\n{}\n--- SCRIPT STDERR ---\n{}",
                    bytes_snapshot(&stdout_log),
                    bytes_snapshot(&stderr_log)
                );
            }
            Err(error) => panic!("inspect PTY smoke child status: {error}"),
        }
    };
    let _ = stdout_reader.join();
    let _ = stderr_reader.join();
    PtyRunOutput {
        exit_code,
        output: bytes_snapshot(&stdout_log),
    }
}

fn parse_json_lines(stdout: &[u8]) -> Vec<Value> {
    String::from_utf8(stdout.to_vec())
        .unwrap_or_else(|error| panic!("decode app-server stdout: {error}"))
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|error| panic!("parse JSON line `{line}`: {error}"))
        })
        .collect()
}

fn has_response(messages: &[Value], id: i64) -> bool {
    messages
        .iter()
        .any(|value| value.get("id") == Some(&json!(id)) && value.get("result").is_some())
}

fn response_result(responses: &[Value], id: i64) -> Value {
    responses
        .iter()
        .find(|value| value.get("id") == Some(&json!(id)))
        .and_then(|value| value.get("result"))
        .cloned()
        .unwrap_or_else(|| panic!("missing response result for id {id}"))
}

fn notification_params(messages: &[Value], method: &str) -> Value {
    messages
        .iter()
        .find(|value| value.get("method") == Some(&json!(method)))
        .and_then(|value| value.get("params"))
        .cloned()
        .unwrap_or_else(|| panic!("missing notification params for method {method}"))
}

fn has_notification(messages: &[Value], method: &str) -> bool {
    messages
        .iter()
        .any(|value| value.get("method") == Some(&json!(method)))
}

fn notification_status<'a>(messages: &'a [Value], method: &str) -> Option<&'a str> {
    messages
        .iter()
        .find(|value| value.get("method") == Some(&json!(method)))
        .and_then(|value| value.get("params"))
        .and_then(|params| params.get("turn"))
        .and_then(|turn| turn.get("status"))
        .and_then(Value::as_str)
}

fn write_dataset_audit_artifact(env: &TestEnv, thread_id: &str, turn_id: &str, artifact_id: &str) {
    let artifact_dir = create_artifact_dir(env, thread_id, artifact_id);
    let output = Command::new("python3")
        .arg("-m")
        .arg("mli_helpers.artifacts.write_dataset_audit")
        .arg("--artifact-dir")
        .arg(&artifact_dir)
        .arg("--turn-id")
        .arg(turn_id)
        .arg("--dataset")
        .arg("demo/corpus")
        .arg("--split")
        .arg("train")
        .arg("--row-count")
        .arg("train=12")
        .arg("--issue")
        .arg("missing labels")
        .env(
            "PYTHONPATH",
            env.workspace_root().join("helpers/python/src"),
        )
        .output()
        .unwrap_or_else(|error| panic!("run dataset audit helper: {error}"));
    assert!(
        output.status.success(),
        "dataset audit helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_malformed_artifact_manifest(env: &TestEnv, thread_id: &str, artifact_id: &str) {
    let artifact_dir = create_artifact_dir(env, thread_id, artifact_id);
    fs::write(artifact_dir.join("artifact.json"), "{not-json")
        .unwrap_or_else(|error| panic!("write malformed artifact manifest: {error}"));
}

fn write_paper_report_artifact(env: &TestEnv, thread_id: &str, turn_id: &str, artifact_id: &str) {
    run_artifact_helper(
        env,
        "mli_helpers.artifacts.write_paper_report",
        create_artifact_dir(env, thread_id, artifact_id),
        &[
            "--turn-id",
            turn_id,
            "--query",
            "trl sft best practices",
            "--paper-count",
            "3",
            "--top-paper",
            "Paper A",
            "--top-paper",
            "Paper B",
            "--recommended-recipe",
            "Use packed sequences with careful masking",
            "--raw-text",
            "internal shortlist notes",
        ],
    );
}

fn write_job_snapshot_artifact(env: &TestEnv, thread_id: &str, turn_id: &str, artifact_id: &str) {
    write_job_snapshot_artifact_with(env, thread_id, turn_id, artifact_id, "running", "120");
}

fn write_job_snapshot_artifact_with(
    env: &TestEnv,
    thread_id: &str,
    turn_id: &str,
    artifact_id: &str,
    status: &str,
    duration_seconds: &str,
) {
    run_artifact_helper(
        env,
        "mli_helpers.artifacts.write_job_snapshot",
        create_artifact_dir(env, thread_id, artifact_id),
        &[
            "--turn-id",
            turn_id,
            "--job-id",
            "job-123",
            "--status",
            status,
            "--hardware",
            "a10g-large",
            "--dashboard-url",
            "https://hf.co/jobs/job-123",
            "--duration-seconds",
            duration_seconds,
            "--raw-text",
            "tail -f trainer.log",
        ],
    );
}

fn start_interruptible_turn_session(
    env: &TestEnv,
    prompt: &str,
) -> (AppServerSession, String, String) {
    let mut session = AppServerSession::spawn(env);

    session.send(&json!({
        "id": 1,
        "method": "initialize",
        "params": {
            "client_info": {
                "name": "process-smoke",
                "title": "process-smoke",
                "version": "0.1.0"
            },
            "capabilities": {
                "transcriptStreaming": true,
                "skillPicker": true,
                "artifactsOverlay": true
            }
        }
    }));
    let initialize_messages = session.collect_until(|messages| has_response(messages, 1));
    assert_eq!(
        response_result(&initialize_messages, 1)["upstreamCodexVersion"].as_str(),
        Some("0.120.0")
    );

    session.send(&json!({"method": "initialized"}));
    session.send(&json!({
        "id": 2,
        "method": "thread/start",
        "params": {
            "cwd": env.cwd,
            "title": "artifact smoke",
            "model": null,
            "approval_policy": null,
            "sandbox_mode": null
        }
    }));
    let thread_messages = session.collect_until(|messages| has_response(messages, 2));
    let thread_id = response_result(&thread_messages, 2)["thread"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing local thread id"))
        .to_owned();

    session.send(&json!({
        "id": 3,
        "method": "turn/start",
        "params": {
            "thread_id": thread_id,
            "input": [
                {
                    "type": "text",
                    "text": prompt
                }
            ]
        }
    }));
    let turn_messages = session.collect_until(|messages| {
        has_response(messages, 3)
            && has_notification(messages, "turn/started")
            && has_notification(messages, "item/agentMessage/delta")
    });
    let turn_id = response_result(&turn_messages, 3)["turn"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing local turn id"))
        .to_owned();

    (session, thread_id, turn_id)
}

fn seed_streaming_thread_via_app_server(env: &TestEnv, prompt: &str) {
    let mut session = AppServerSession::spawn(env);

    session.send(&json!({
        "id": 1,
        "method": "initialize",
        "params": {
            "client_info": {
                "name": "process-smoke",
                "title": "process-smoke",
                "version": "0.1.0"
            },
            "capabilities": {
                "transcriptStreaming": true,
                "skillPicker": true,
                "artifactsOverlay": true
            }
        }
    }));
    let initialize_messages = session.collect_until(|messages| has_response(messages, 1));
    assert_eq!(
        response_result(&initialize_messages, 1)["upstreamCodexVersion"].as_str(),
        Some("0.120.0")
    );

    session.send(&json!({"method": "initialized"}));
    session.send(&json!({
        "id": 2,
        "method": "thread/start",
        "params": {
            "cwd": env.cwd,
            "title": prompt,
            "model": null,
            "approval_policy": null,
            "sandbox_mode": null
        }
    }));
    let thread_messages = session.collect_until(|messages| has_response(messages, 2));
    let thread_id = response_result(&thread_messages, 2)["thread"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing local thread id"))
        .to_owned();

    session.send(&json!({
        "id": 3,
        "method": "turn/start",
        "params": {
            "thread_id": thread_id,
            "input": [
                {
                    "type": "text",
                    "text": prompt
                }
            ]
        }
    }));
    let turn_messages = session.collect_until(|messages| {
        has_response(messages, 3)
            && has_notification(messages, "item/agentMessage/delta")
            && has_notification(messages, "turn/completed")
    });
    assert!(
        has_notification(&turn_messages, "turn/plan/updated"),
        "expected seeded streaming thread to record the fake plan update"
    );

    session.finish();
}

fn start_approval_turn_session(
    env: &TestEnv,
    prompt: &str,
) -> (AppServerSession, String, String, Value) {
    let mut session = AppServerSession::spawn(env);

    session.send(&json!({
        "id": 1,
        "method": "initialize",
        "params": {
            "client_info": {
                "name": "process-smoke",
                "title": "process-smoke",
                "version": "0.1.0"
            },
            "capabilities": {
                "transcriptStreaming": true,
                "skillPicker": true,
                "artifactsOverlay": true
            }
        }
    }));
    let initialize_messages = session.collect_until(|messages| has_response(messages, 1));
    assert_eq!(
        response_result(&initialize_messages, 1)["upstreamCodexVersion"].as_str(),
        Some("0.120.0")
    );

    session.send(&json!({"method": "initialized"}));
    session.send(&json!({
        "id": 2,
        "method": "thread/start",
        "params": {
            "cwd": env.cwd,
            "title": "approval smoke",
            "model": null,
            "approval_policy": null,
            "sandbox_mode": null
        }
    }));
    let thread_messages = session.collect_until(|messages| has_response(messages, 2));
    let thread_id = response_result(&thread_messages, 2)["thread"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing local thread id"))
        .to_owned();

    session.send(&json!({
        "id": 3,
        "method": "turn/start",
        "params": {
            "thread_id": thread_id,
            "input": [
                {
                    "type": "text",
                    "text": prompt
                }
            ]
        }
    }));
    let turn_messages = session.collect_until(|messages| {
        has_response(messages, 3)
            && has_notification(messages, "turn/started")
            && has_notification(messages, "approval/requested")
    });
    let turn_id = response_result(&turn_messages, 3)["turn"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing local turn id"))
        .to_owned();
    let approval = notification_params(&turn_messages, "approval/requested");

    (session, thread_id, turn_id, approval)
}

fn create_artifact_dir(env: &TestEnv, thread_id: &str, artifact_id: &str) -> PathBuf {
    let artifact_dir = env
        .cwd
        .join(".ml-intern/threads")
        .join(thread_id)
        .join("artifacts")
        .join(artifact_id);
    fs::create_dir_all(&artifact_dir)
        .unwrap_or_else(|error| panic!("create artifact dir {}: {error}", artifact_dir.display()));
    artifact_dir
}

fn run_artifact_helper(env: &TestEnv, module: &str, artifact_dir: PathBuf, extra_args: &[&str]) {
    let output = Command::new("python3")
        .arg("-m")
        .arg(module)
        .arg("--artifact-dir")
        .arg(&artifact_dir)
        .args(extra_args)
        .env(
            "PYTHONPATH",
            env.workspace_root().join("helpers/python/src"),
        )
        .output()
        .unwrap_or_else(|error| panic!("run {module} helper: {error}"));
    assert!(
        output.status.success(),
        "{module} helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_fake_codex(bin_dir: &Path) {
    let codex = bin_dir.join("codex");
    let temp_codex = bin_dir.join("codex.tmp");
    fs::write(
        &temp_codex,
        r#"#!/usr/bin/env python3
import json
import os
import sys

SCENARIO = os.environ.get("MLI_FAKE_CODEX_SCENARIO", "startup-only")
CAPTURE_PATH = os.environ.get("MLI_FAKE_CODEX_CAPTURE")

def respond(message, result):
    print(json.dumps({
        "id": message["id"],
        "result": result,
    }), flush=True)

def respond_error(request, code, error_message):
    print(json.dumps({
        "id": request["id"],
        "error": {
            "code": code,
            "message": error_message,
        }
    }), flush=True)

def notify(method, params):
    print(json.dumps({
        "method": method,
        "params": params,
    }), flush=True)

def capture(kind, payload):
    if not CAPTURE_PATH:
        return
    with open(CAPTURE_PATH, "a", encoding="utf-8") as fh:
        fh.write(json.dumps({
            "kind": kind,
            "payload": payload,
        }) + "\n")

if len(sys.argv) > 1 and sys.argv[1] == "--version":
    if SCENARIO == "version-mismatch":
        print("codex-cli 0.119.0")
    else:
        print("codex-cli 0.120.0")
    raise SystemExit(0)

if len(sys.argv) > 1 and sys.argv[1] == "app-server":
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        message = json.loads(line)
        if "result" in message and "id" in message and "method" not in message:
            capture("client_response", message)
            if SCENARIO == "command-approval-turn" and message["id"] == 11:
                decision = message["result"].get("decision", "unknown")
                notify("item/agentMessage/delta", {
                    "threadId": "up-thread-1",
                    "turnId": "up-turn-1",
                    "itemId": "agent-approval",
                    "delta": f"command approval result: {decision}"
                })
                notify("turn/completed", {
                    "threadId": "up-thread-1",
                    "turn": {
                        "id": "up-turn-1",
                        "status": "completed"
                    }
                })
            elif SCENARIO == "request-user-input-turn" and message["id"] == 41:
                notify("item/agentMessage/delta", {
                    "threadId": "up-thread-1",
                    "turnId": "up-turn-1",
                    "itemId": "agent-user-input",
                    "delta": "dataset answers captured"
                })
                notify("turn/completed", {
                    "threadId": "up-thread-1",
                    "turn": {
                        "id": "up-turn-1",
                        "status": "completed"
                    }
                })
            continue
        method = message.get("method")
        if method == "initialize":
            respond(message, {
                "serverInfo": {
                    "name": "fake-codex",
                    "version": "0.120.0"
                }
            })
        elif method == "initialized":
            continue
        elif method == "thread/start" and SCENARIO in {
            "streaming-turn",
            "skills-turn",
            "interruptible-turn",
            "interrupt-error-turn",
            "command-approval-turn",
            "request-user-input-turn",
        }:
            cwd = message["params"].get("cwd") or os.getcwd()
            respond(message, {
                "thread": {
                    "id": "up-thread-1",
                    "preview": "streaming smoke thread",
                    "cwd": cwd
                },
                "model": "gpt-5.4-mini",
                "cwd": cwd
            })
        elif method == "thread/resume" and SCENARIO in {
            "streaming-turn",
            "skills-turn",
            "interruptible-turn",
            "interrupt-error-turn",
            "command-approval-turn",
            "request-user-input-turn",
        }:
            capture("thread_resume_request", message)
            cwd = message["params"].get("cwd") or os.getcwd()
            respond(message, {
                "thread": {
                    "id": message["params"]["threadId"],
                    "preview": "streaming smoke thread",
                    "cwd": cwd
                },
                "model": "gpt-5.4-mini",
                "cwd": cwd
            })
        elif method == "turn/start" and SCENARIO == "streaming-turn":
            respond(message, {
                "turn": {
                    "id": "up-turn-1",
                    "status": "inProgress"
                }
            })
            notify("turn/started", {
                "threadId": "up-thread-1",
                "turn": {
                    "id": "up-turn-1",
                    "status": "inProgress"
                }
            })
            notify("item/agentMessage/delta", {
                "threadId": "up-thread-1",
                "turnId": "up-turn-1",
                "itemId": "agent-1",
                "delta": "hello from fake codex"
            })
            notify("turn/plan/updated", {
                "turnId": "up-turn-1",
                "explanation": "Plan step",
                "plan": [
                    {
                        "step": "Do smoke",
                        "status": "inProgress"
                    }
                ]
            })
            notify("turn/completed", {
                "threadId": "up-thread-1",
                "turn": {
                    "id": "up-turn-1",
                    "status": "completed"
                }
            })
        elif method == "turn/start" and SCENARIO == "skills-turn":
            capture("turn_start_request", message)
            respond(message, {
                "turn": {
                    "id": "up-turn-1",
                    "status": "inProgress"
                }
            })
            notify("turn/started", {
                "threadId": "up-thread-1",
                "turn": {
                    "id": "up-turn-1",
                    "status": "inProgress"
                }
            })
            notify("item/agentMessage/delta", {
                "threadId": "up-thread-1",
                "turnId": "up-turn-1",
                "itemId": "agent-skill",
                "delta": "skill payload captured"
            })
            notify("turn/completed", {
                "threadId": "up-thread-1",
                "turn": {
                    "id": "up-turn-1",
                    "status": "completed"
                }
            })
        elif method == "turn/start" and SCENARIO == "interruptible-turn":
            respond(message, {
                "turn": {
                    "id": "up-turn-1",
                    "status": "inProgress"
                }
            })
            notify("turn/started", {
                "threadId": "up-thread-1",
                "turn": {
                    "id": "up-turn-1",
                    "status": "inProgress"
                }
            })
            notify("item/agentMessage/delta", {
                "threadId": "up-thread-1",
                "turnId": "up-turn-1",
                "itemId": "agent-interrupt",
                "delta": "still working"
            })
        elif method == "turn/start" and SCENARIO == "interrupt-error-turn":
            respond(message, {
                "turn": {
                    "id": "up-turn-1",
                    "status": "inProgress"
                }
            })
            notify("turn/started", {
                "threadId": "up-thread-1",
                "turn": {
                    "id": "up-turn-1",
                    "status": "inProgress"
                }
            })
            notify("item/agentMessage/delta", {
                "threadId": "up-thread-1",
                "turnId": "up-turn-1",
                "itemId": "agent-interrupt-error",
                "delta": "still working"
            })
        elif method == "turn/start" and SCENARIO == "command-approval-turn":
            respond(message, {
                "turn": {
                    "id": "up-turn-1",
                    "status": "inProgress"
                }
            })
            notify("turn/started", {
                "threadId": "up-thread-1",
                "turn": {
                    "id": "up-turn-1",
                    "status": "inProgress"
                }
            })
            print(json.dumps({
                "method": "item/commandExecution/requestApproval",
                "id": 11,
                "params": {
                    "threadId": "up-thread-1",
                    "turnId": "up-turn-1",
                    "itemId": "cmd-approval-1",
                    "command": "python risky.py",
                    "reason": "Needs confirmation before writing outputs",
                    "cwd": os.getcwd()
                }
            }), flush=True)
        elif method == "turn/start" and SCENARIO == "request-user-input-turn":
            respond(message, {
                "turn": {
                    "id": "up-turn-1",
                    "status": "inProgress"
                }
            })
            notify("turn/started", {
                "threadId": "up-thread-1",
                "turn": {
                    "id": "up-turn-1",
                    "status": "inProgress"
                }
            })
            print(json.dumps({
                "method": "item/tool/requestUserInput",
                "id": 41,
                "params": {
                    "threadId": "up-thread-1",
                    "turnId": "up-turn-1",
                    "itemId": "tool-approval-1",
                    "questions": [
                        {
                            "id": "dataset",
                            "header": "Dataset",
                            "question": "Pick a dataset",
                            "options": [
                                {
                                    "label": "c4",
                                    "description": "Use the C4 demo subset"
                                },
                                {
                                    "label": "fineweb",
                                    "description": "Use the FineWeb training slice"
                                }
                            ]
                        },
                        {
                            "id": "note",
                            "header": "Follow-up",
                            "question": "Add an operator note",
                            "isOther": True,
                            "isSecret": True
                        }
                    ]
                }
            }), flush=True)
        elif method == "turn/interrupt" and SCENARIO == "interruptible-turn":
            capture("turn_interrupt_request", message)
            respond(message, {})
            notify("turn/completed", {
                "threadId": "up-thread-1",
                "turn": {
                    "id": "up-turn-1",
                    "status": "interrupted"
                }
            })
        elif method == "turn/interrupt" and SCENARIO == "interrupt-error-turn":
            capture("turn_interrupt_request", message)
            respond_error(message, -32003, "interrupt failed")
        else:
            print(json.dumps({
                "id": message.get("id", 0),
                "error": {
                    "code": -32601,
                    "message": f"unsupported {method}"
                }
            }), flush=True)
    raise SystemExit(0)

raise SystemExit(1)
"#,
    )
    .unwrap_or_else(|error| panic!("write fake codex: {error}"));
    make_executable(&temp_codex);
    fs::rename(&temp_codex, &codex).unwrap_or_else(|error| panic!("install fake codex: {error}"));
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .unwrap_or_else(|error| panic!("stat fake codex: {error}"))
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .unwrap_or_else(|error| panic!("chmod fake codex: {error}"));
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}
