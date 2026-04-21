# ml-intern-codex Technical Specification

## 1. Scope and Baseline

This specification defines the v1 architecture for `ml-intern-codex`.

Baseline constraints:

- upstream execution backend: installed `codex-cli 0.120.0`
- upstream transport: stdio JSONL app-server protocol
- local product transport: stdio JSONL app-server protocol
- UI style: CodexPotter-inspired transcript-first TUI
- runtime model: local single-user only
- MCP: removed entirely
- model routing: delegated entirely to upstream Codex

## 2. Architectural Summary

`ml-intern-codex` is a three-plane local system:

1. **Presentation plane**: Rust TUI
2. **Control plane**: local Rust app-server wrapper
3. **Execution plane**: upstream `codex app-server`

ML-specific behavior is provided by:

- bundled and user skills
- generated runtime instructions
- local helper runtimes
- artifact persistence and indexing

### 2.1 High-level diagram

```text
+-------------------------------+
|           User / TTY          |
+---------------+---------------+
                |
                v
+-------------------------------+
|           Rust TUI            |
| transcript, picker, overlays  |
+---------------+---------------+
                | stdio JSONL
                v
+-------------------------------+
|     Local App-Server Wrapper  |
| session state, bridge, index  |
| skills view, artifacts, TUI   |
+-------+---------------+-------+
        |               |
        |               +-----------------------------+
        |                                             |
        v                                             v
+----------------------+               +-------------------------------+
| Upstream Codex       |               | Local State / Workspace Files |
| app-server (stdio)   |               | config, skills, artifacts, db |
+----------+-----------+               +-------------------------------+
           |
           v
+-------------------------------+
| Codex execution + shell tools |
| uv / uvx / python / npx / sh  |
+-------------------------------+
```

## 3. Dependency Layering

The codebase must follow this dependency direction:

```text
Types -> Config -> Repo -> Service -> Runtime -> UI
```

Rules:

- `UI` depends on `Runtime` abstractions only, never on raw repo persistence details.
- `Runtime` depends on `Service`, `Repo`, `Config`, and protocol DTOs.
- `Service` depends on `Repo`, `Config`, and `Types`, but never on TUI or transport concerns.
- `Repo` depends on `Config` and `Types` only.
- `Config` depends on `Types` only.
- `Types` depends on no product crate.

## 4. Proposed Workspace Layout

```text
ml-intern-codex/
  Cargo.toml
  rust-toolchain.toml
  .gitignore
  docs/
    PRD.md
    SPEC.md
  references/
    codex/
    CodexPotter/
    ml-intern/
  crates/
    mli-types/
    mli-config/
    mli-protocol/
    mli-repo/
    mli-artifacts/
    mli-skills/
    mli-services/
    mli-upstream-protocol/
    mli-codex-bridge/
    mli-runtime/
    mli-app-server/
    mli-tui/
    mli-cli/
  helpers/
    python/
      pyproject.toml
      src/mli_helpers/
    node/
      package.json
      src/
  skills/
    system/
      ml-runtime-conventions/
      hf-literature-research/
      hf-dataset-audit/
      hf-jobs-operator/
      hf-hub-maintainer/
      hf-space-troubleshooter/
  tests/
    e2e/
    fixtures/
```

### 4.1 Crate responsibilities

#### `mli-types`

Pure domain types, DTOs, enums, IDs, artifact schemas, view models, and state machine enums.

#### `mli-config`

Config file loading, merging, defaults, and runtime environment detection.

#### `mli-protocol`

Local app-server protocol DTOs shared by the TUI and local wrapper server.

#### `mli-repo`

Persistence repositories for threads, turns, artifacts, and logs.

#### `mli-artifacts`

Artifact manifest parsing, validation, indexing, preview generation, and file watching utilities.

#### `mli-skills`

Bundled skill registry, local skill discovery support, skill metadata adapters for TUI and wrapper code.

#### `mli-services`

Service layer containing thread services, artifact services, skill services, runtime environment services, and high-level use-case orchestration.

#### `mli-upstream-protocol`

Typed subset/mirror of the upstream Codex app-server protocol pinned to `0.120.0`.

#### `mli-codex-bridge`

Bridge client for launching and communicating with upstream `codex app-server` over stdio.

#### `mli-runtime`

Core orchestration: session lifecycle, turn flow, event normalization, artifact watch registration, and local thread state machine.

#### `mli-app-server`

Local JSONL app-server implementation consumed by the TUI.

#### `mli-tui`

Transcript renderer, input composer, overlays, state reducer, and app-server client.

#### `mli-cli`

Top-level binaries:

- `ml-intern`
- `ml-intern-app-server`

## 5. File System Layout

The product uses both a user-global home and a project-local hidden directory.

### 5.1 User-global home

Path:

```text
~/.ml-intern-codex/
```

Contents:

```text
~/.ml-intern-codex/
  config.toml
  logs/
    app-server/
    tui/
  runtime/
    codex-home/
    generated-skills/
    sockets/
  db/
    state.sqlite
  cache/
    skill-index.json
```

Purpose:

- product config
- local logs
- wrapper-owned `CODEX_HOME` overlay
- local metadata database
- generated runtime skill material

### 5.2 Project-local state

Path:

```text
<cwd>/.ml-intern/
```

Contents:

```text
<cwd>/.ml-intern/
  config.toml
  threads/
    <local-thread-id>/
      thread.json
      transcript.jsonl
      turns/
        <turn-id>.json
      artifacts/
        <artifact-id>/
          artifact.json
          report.md
          report.json
          raw.txt
```

Purpose:

- thread-local user-visible session state
- transcript snapshotting
- durable ML domain artifacts
- per-project config overrides

## 6. Configuration Model

Configuration is resolved in this order, low to high precedence:

1. built-in defaults
2. user-global config `~/.ml-intern-codex/config.toml`
3. project-local config `<cwd>/.ml-intern/config.toml`
4. explicit CLI flags
5. per-thread overrides

### 6.1 Core config DTOs

```rust
pub struct AppConfig {
    pub codex: CodexConfig,
    pub ui: UiConfig,
    pub artifacts: ArtifactConfig,
    pub skills: SkillsConfig,
    pub runtime: RuntimeConfig,
}

pub struct CodexConfig {
    pub bin_path: PathBuf,
    pub expected_version: String,
    pub default_model: Option<String>,
    pub approval_policy: ApprovalPolicy,
    pub sandbox_mode: SandboxMode,
}

pub struct ArtifactConfig {
    pub project_root_dirname: String,
    pub auto_watch: bool,
    pub max_preview_bytes: usize,
}

pub struct SkillsConfig {
    pub bundled_enabled: bool,
    pub extra_user_roots: Vec<PathBuf>,
}

pub struct RuntimeConfig {
    pub bridge_start_timeout_ms: u64,
    pub interrupt_grace_timeout_ms: u64,
    pub upstream_idle_shutdown_secs: u64,
}
```

### 6.2 Required defaults

- `codex.expected_version = "0.120.0"`
- `codex.approval_policy = on-request`
- `codex.sandbox_mode = workspace-write`
- `artifacts.project_root_dirname = ".ml-intern"`
- `skills.bundled_enabled = true`

## 7. IDs and Core Domain Types

### 7.1 Identifiers

```rust
pub struct LocalThreadId(pub uuid::Uuid);
pub struct LocalTurnId(pub uuid::Uuid);
pub struct ArtifactId(pub uuid::Uuid);
pub struct UpstreamThreadId(pub String);
pub struct UpstreamTurnId(pub String);
```

### 7.2 Thread and turn DTOs

```rust
pub enum ThreadStatus {
    NotLoaded,
    Idle,
    Starting,
    Running,
    WaitingApproval,
    Interrupted,
    Error,
}

pub struct ThreadRecord {
    pub id: LocalThreadId,
    pub upstream_thread_id: Option<UpstreamThreadId>,
    pub cwd: PathBuf,
    pub title: Option<String>,
    pub status: ThreadStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub transcript_path: PathBuf,
    pub artifact_root: PathBuf,
}

pub enum TurnStatus {
    Pending,
    Starting,
    Streaming,
    WaitingApproval,
    Completed,
    Interrupted,
    Failed,
}

pub struct TurnRecord {
    pub id: LocalTurnId,
    pub local_thread_id: LocalThreadId,
    pub upstream_turn_id: Option<UpstreamTurnId>,
    pub status: TurnStatus,
    pub user_input_summary: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}
```

### 7.3 Skill DTOs

```rust
pub enum SkillScope {
    Bundled,
    User,
    Repo,
    Generated,
}

pub struct SkillDescriptor {
    pub name: String,
    pub description: String,
    pub short_description: Option<String>,
    pub path: PathBuf,
    pub scope: SkillScope,
    pub enabled: bool,
}
```

## 8. Artifact Model

Artifacts are first-class local persisted outputs.

### 8.1 Artifact kinds

```rust
pub enum ArtifactKind {
    PaperReport,
    DatasetAudit,
    JobSnapshot,
    JobLogExcerpt,
    JobRunbook,
    GenericMarkdown,
    GenericJson,
    GenericText,
}
```

### 8.2 Manifest DTO

```rust
pub struct ArtifactManifest {
    pub id: ArtifactId,
    pub version: u32,
    pub local_thread_id: LocalThreadId,
    pub local_turn_id: LocalTurnId,
    pub kind: ArtifactKind,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub summary: String,
    pub tags: Vec<String>,
    pub primary_path: PathBuf,
    pub extra_paths: Vec<PathBuf>,
    pub metadata: serde_json::Value,
}
```

### 8.3 Required file contract

Every artifact directory must contain:

- `artifact.json` (manifest)
- at least one content file referenced by `primary_path`

Optional:

- `report.md`
- `report.json`
- `raw.txt`
- `logs/`

### 8.4 Metadata contracts by kind

#### Paper report metadata

```json
{
  "query": "trl sft best practices",
  "paper_count": 12,
  "top_papers": ["..."],
  "recommended_recipe": "..."
}
```

#### Dataset audit metadata

```json
{
  "dataset": "org/name",
  "splits": ["train", "validation"],
  "row_counts": {"train": 1000},
  "issues": ["missing values", "label skew"]
}
```

#### Job snapshot metadata

```json
{
  "job_id": "...",
  "status": "running",
  "hardware": "a10g-large",
  "dashboard_url": "...",
  "duration_seconds": 1200
}
```

## 9. Skills System

### 9.1 Skill roots

The system resolves skills from these sources in priority order:

1. repo-local `<cwd>/.agents/skills`
2. user-global `~/.agents/skills`
3. bundled repo `skills/system`
4. generated runtime skills `~/.ml-intern-codex/runtime/generated-skills`

### 9.2 Bundled skills

Minimum bundled skills:

- `ml-runtime-conventions`
- `hf-literature-research`
- `hf-dataset-audit`
- `hf-jobs-operator`
- `hf-hub-maintainer`
- `hf-space-troubleshooter`

### 9.3 Skill format

Skill files follow upstream Codex `SKILL.md` conventions and should remain readable by upstream Codex directly.

### 9.4 Wrapper responsibilities for skills

The wrapper must:

- expose skill descriptors to the TUI
- generate runtime helper skill(s) describing artifact conventions and helper runtime locations
- prepare `CODEX_HOME` overlay so upstream Codex can see bundled/generated skills

### 9.5 Upstream Codex integration

The wrapper does **not** reimplement skill injection semantics.

Instead it ensures upstream Codex can discover the intended skill roots through:

- `CODEX_HOME` overlay generation
- stable skill directories
- optional generated instructions/AGENTS content

## 10. Helper Runtime Strategy

The product does not expose generic MCP tools.

Instead, Codex uses normal command execution and file access, guided by skills.

### 10.1 Required helper runtime lanes

- `uv run`
- `uvx`
- `python -m`
- `npx`
- shell commands

### 10.2 Helper packaging

The product ships helper code under:

- `helpers/python`
- `helpers/node`

The wrapper prepends helper launcher paths to the upstream process environment.

### 10.3 Artifact helper contract

Python helpers must provide a stable artifact writer API so ML domain outputs are consistently saved.

Example logical helper commands:

- `python -m mli_helpers.artifacts.write_paper_report`
- `python -m mli_helpers.artifacts.write_dataset_audit`
- `python -m mli_helpers.artifacts.write_job_snapshot`

Node helper space is reserved for ecosystem tasks that are materially easier in JS, but Python is the default helper runtime in v1.

## 11. Local App-Server External Protocol

The local app-server speaks JSONL over stdio and deliberately mirrors a subset of Codex app-server semantics.

### 11.1 Framing

- one JSON object per line
- no websocket in v1
- request/response/notification flow model matches Codex mental model

### 11.2 Method set

Supported methods:

- `initialize`
- `initialized`
- `runtime/info`
- `thread/start`
- `thread/resume`
- `thread/list`
- `thread/read`
- `turn/start`
- `turn/interrupt`
- `skills/list`
- `artifact/list`
- `artifact/read`
- `config/read`
- `config/write`

### 11.3 Method DTOs

#### `initialize`

```rust
pub struct InitializeParams {
    pub client_info: ClientInfo,
    pub capabilities: ClientCapabilities,
}

pub struct InitializeResult {
    pub server_info: ServerInfo,
    pub protocol_version: String,
    pub upstream_codex_version: String,
    pub codex_bin: PathBuf,
    pub app_home: PathBuf,
}
```

#### `runtime/info`

```rust
pub struct RuntimeInfoResult {
    pub codex_bin: PathBuf,
    pub codex_version: String,
    pub app_home: PathBuf,
    pub cwd: PathBuf,
    pub approval_policy: ApprovalPolicy,
    pub sandbox_mode: SandboxMode,
}
```

#### `thread/start`

```rust
pub struct ThreadStartParams {
    pub cwd: PathBuf,
    pub title: Option<String>,
    pub model: Option<String>,
    pub approval_policy: Option<ApprovalPolicy>,
    pub sandbox_mode: Option<SandboxMode>,
}

pub struct ThreadStartResult {
    pub thread: ThreadRecord,
}
```

#### `thread/resume`

```rust
pub struct ThreadResumeParams {
    pub thread_id: LocalThreadId,
}

pub struct ThreadResumeResult {
    pub thread: ThreadRecord,
}
```

#### `thread/list`

```rust
pub struct ThreadListResult {
    pub threads: Vec<ThreadRecord>,
}
```

#### `thread/read`

```rust
pub struct ThreadReadParams {
    pub thread_id: LocalThreadId,
}

pub struct ThreadReadResult {
    pub thread: ThreadRecord,
    pub turns: Vec<TurnRecord>,
}
```

#### `turn/start`

```rust
pub struct TurnStartParams {
    pub thread_id: LocalThreadId,
    pub input: Vec<UserInput>,
}

pub struct TurnStartResult {
    pub turn: TurnRecord,
}
```

#### `turn/interrupt`

```rust
pub struct TurnInterruptParams {
    pub thread_id: LocalThreadId,
    pub turn_id: LocalTurnId,
}
```

#### `skills/list`

```rust
pub struct SkillsListParams {
    pub cwd: Option<PathBuf>,
    pub force_reload: Option<bool>,
}

pub struct SkillsListResult {
    pub skills: Vec<SkillDescriptor>,
}
```

#### `artifact/list`

```rust
pub struct ArtifactListParams {
    pub thread_id: Option<LocalThreadId>,
    pub kind: Option<ArtifactKind>,
    pub limit: Option<usize>,
}

pub struct ArtifactListResult {
    pub artifacts: Vec<ArtifactManifest>,
}
```

#### `artifact/read`

```rust
pub struct ArtifactReadParams {
    pub artifact_id: ArtifactId,
}

pub struct ArtifactReadResult {
    pub manifest: ArtifactManifest,
    pub files: Vec<ArtifactFilePayload>,
}

pub struct ArtifactFilePayload {
    pub path: PathBuf,
    pub media_type: String,
    pub text: Option<String>,
    pub base64: Option<String>,
}
```

## 12. Notifications

### 12.1 Pass-through notifications

Whenever possible, the wrapper forwards upstream Codex turn/item notifications in normalized form:

- `thread/started`
- `thread/statusChanged`
- `turn/started`
- `turn/completed`
- `item/started`
- `item/completed`
- `item/agentMessage/delta`
- `error`

### 12.2 Product-specific notifications

Additional local notifications:

- `runtime/statusChanged`
- `skills/changed`
- `artifact/created`
- `artifact/updated`

### 12.3 Notification DTOs

```rust
pub struct ArtifactCreatedNotification {
    pub manifest: ArtifactManifest,
    pub preview: ArtifactPreview,
}

pub enum ArtifactPreview {
    PaperReport {
        paper_count: usize,
        headline: String,
    },
    DatasetAudit {
        dataset: String,
        split_count: usize,
        issue_count: usize,
    },
    JobSnapshot {
        job_id: String,
        status: String,
        hardware: Option<String>,
    },
    Generic {
        headline: String,
    },
}
```

## 13. Upstream Codex Bridge

### 13.1 Responsibilities

`mli-codex-bridge` owns:

- locating `codex` binary
- validating version compatibility
- launching upstream `codex app-server`
- performing upstream initialize handshake
- starting or resuming upstream threads
- forwarding turn requests
- reading upstream notifications
- shutting down or recreating upstream process on failure

### 13.2 Process model

V1 process model:

- one wrapper process per TUI session
- at most one active upstream Codex process per loaded thread
- wrapper may lazily start the upstream process on first thread activity

### 13.3 `CODEX_HOME` overlay strategy

The wrapper generates a product-owned overlay home under:

```text
~/.ml-intern-codex/runtime/codex-home/
```

It contains:

- generated `AGENTS.md` or equivalent instruction file
- generated skill roots
- wrapper-managed config files
- symlinked or copied upstream-compatible config where needed

Goals:

- keep product skills and runtime instructions isolated
- avoid mutating the user's canonical Codex home unexpectedly
- make upstream Codex skill discovery deterministic for this app

### 13.4 Upstream compatibility policy

The bridge is hard-pinned to `codex-cli 0.120.0` for v1.

If the installed version mismatches:

- `initialize` returns a clear compatibility error
- TUI shows a fatal startup message
- no thread is started

## 14. Runtime Orchestration

`mli-runtime` is the control plane above the bridge.

### 14.1 Responsibilities

- local thread lifecycle
- mapping local thread IDs to upstream thread IDs
- turn lifecycle management
- event normalization
- local transcript snapshot writing
- artifact watcher registration
- approval state handling for the TUI

### 14.2 Thread lifecycle state machine

```text
NotLoaded
  -> Starting
  -> Idle
  -> Running
  -> WaitingApproval
  -> Running
  -> Idle

Idle
  -> Running
  -> Interrupted
  -> Idle

Any active state
  -> Error
```

### 14.2.1 Transition rules

- `thread/start` creates local thread record and moves to `Starting`.
- successful upstream thread creation moves to `Idle`.
- `turn/start` moves thread to `Running`.
- upstream approval request moves to `WaitingApproval`.
- approval response moves back to `Running`.
- `turn/completed` moves thread to `Idle`.
- interrupt moves thread to `Interrupted`, then back to `Idle` after cleanup.

### 14.3 Turn lifecycle state machine

```text
Pending -> Starting -> Streaming -> WaitingApproval -> Streaming -> Completed
Pending -> Starting -> Streaming -> Interrupted
Pending -> Starting -> Failed
```

### 14.4 Upstream process state machine

```text
NotStarted -> Launching -> Handshaking -> Ready -> ThreadBound -> TurnRunning
TurnRunning -> Ready
Any -> Restarting -> Launching
Any -> Exited
```

## 15. Transcript Persistence

The runtime writes transcript snapshots to:

```text
<cwd>/.ml-intern/threads/<thread-id>/transcript.jsonl
```

Each line is a local normalized event envelope:

```rust
pub struct TranscriptEvent {
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    pub thread_id: LocalThreadId,
    pub turn_id: Option<LocalTurnId>,
    pub source: TranscriptEventSource,
    pub payload: serde_json::Value,
}
```

Sources:

- `user`
- `wrapper`
- `upstream_codex`
- `artifact_system`

This file is append-only and intended for local replay/debugging, not as the primary UI state source during a live session.

## 16. Repository Layer

`mli-repo` provides persistence APIs.

### 16.1 Backing stores

- SQLite for indexes and metadata
- JSON files for user-visible thread snapshots
- JSONL for append-only transcripts

### 16.2 Repository interfaces

```rust
pub trait ThreadRepo {
    fn create(&self, thread: &ThreadRecord) -> Result<()>;
    fn update(&self, thread: &ThreadRecord) -> Result<()>;
    fn get(&self, id: LocalThreadId) -> Result<Option<ThreadRecord>>;
    fn list(&self) -> Result<Vec<ThreadRecord>>;
}

pub trait TurnRepo {
    fn create(&self, turn: &TurnRecord) -> Result<()>;
    fn update(&self, turn: &TurnRecord) -> Result<()>;
    fn list_by_thread(&self, thread_id: LocalThreadId) -> Result<Vec<TurnRecord>>;
}

pub trait ArtifactRepo {
    fn upsert_manifest(&self, manifest: &ArtifactManifest) -> Result<()>;
    fn get(&self, id: ArtifactId) -> Result<Option<ArtifactManifest>>;
    fn list(&self, query: ArtifactQuery) -> Result<Vec<ArtifactManifest>>;
}
```

## 16A. Service Layer

`mli-services` contains use-case level orchestration that is above repositories and below runtime.

### 16A.1 Core services

```rust
pub trait ThreadService {
    fn start_thread(&self, req: StartThreadRequest) -> Result<ThreadRecord>;
    fn resume_thread(&self, id: LocalThreadId) -> Result<ThreadRecord>;
    fn list_threads(&self) -> Result<Vec<ThreadRecord>>;
}

pub trait TurnService {
    fn start_turn(&self, req: StartTurnRequest) -> Result<TurnRecord>;
    fn interrupt_turn(&self, thread_id: LocalThreadId, turn_id: LocalTurnId) -> Result<()>;
}

pub trait SkillService {
    fn list_skills(&self, cwd: Option<&Path>, force_reload: bool) -> Result<Vec<SkillDescriptor>>;
}

pub trait ArtifactService {
    fn list_artifacts(&self, query: ArtifactQuery) -> Result<Vec<ArtifactManifest>>;
    fn read_artifact(&self, id: ArtifactId) -> Result<ArtifactReadResult>;
    fn register_or_update(&self, manifest: ArtifactManifest) -> Result<()>;
}

pub trait RuntimeEnvironmentService {
    fn resolve_codex_bin(&self) -> Result<PathBuf>;
    fn validate_codex_version(&self) -> Result<String>;
    fn prepare_codex_home_overlay(&self, cwd: &Path) -> Result<CodexHomeOverlay>;
}
```

### 16A.2 Service rules

- services may depend on repos, config, skills, and artifact libraries
- services may not depend on TUI widgets or protocol transport framing
- runtime may compose multiple services into live session flows

## 17. Artifact Watcher Design

`mli-artifacts` owns a file watcher attached to each active thread artifact root.

### 17.1 Watch targets

```text
<cwd>/.ml-intern/threads/<thread-id>/artifacts/
```

### 17.2 Behavior

- debounce file changes
- detect new `artifact.json`
- parse manifest
- index artifact
- compute preview
- emit `artifact/created` or `artifact/updated`

### 17.3 Failure handling

If manifest parsing fails:

- do not crash runtime
- emit warning event
- mark artifact as invalid in logs

## 18. TUI Design

The TUI is transcript-first.

### 18.1 Main layout

```text
+------------------------------------------------------------+
| Header / thread / status / model / approval / sandbox      |
+------------------------------------------------------------+
|                                                            |
| Transcript viewport                                         |
|                                                            |
+------------------------------------------------------------+
| Composer / prompt buffer / skill popup / footer            |
+------------------------------------------------------------+
```

### 18.2 Overlays

V1 overlays:

- thread picker
- skill picker
- artifact list
- artifact viewer
- approval dialog
- help / slash command list

### 18.3 TUI state DTOs

```rust
pub struct AppState {
    pub connection: ConnectionState,
    pub active_thread_id: Option<LocalThreadId>,
    pub threads: Vec<ThreadListItem>,
    pub transcript: TranscriptState,
    pub composer: ComposerState,
    pub artifacts: ArtifactUiState,
    pub approvals: ApprovalUiState,
}
```

### 18.4 Transcript cell model

```rust
pub enum HistoryCellModel {
    UserMessage(UserMessageCell),
    AssistantMessage(AssistantMessageCell),
    ExecCommand(ExecCommandCell),
    ExecOutput(ExecOutputCell),
    PatchSummary(PatchSummaryCell),
    PlanUpdate(PlanUpdateCell),
    ApprovalRequest(ApprovalCell),
    ArtifactCreated(ArtifactCreatedCell),
    Warning(WarningCell),
    Error(ErrorCell),
    Status(StatusCell),
}
```

### 18.5 Artifact-created cell behavior

Every `artifact/created` notification produces an `ArtifactCreated` cell containing:

- artifact kind badge
- title
- compact domain summary
- saved local path
- hint to open viewer

#### HF job summary cell

Compact rendering fields:

- job id
- status
- hardware if present
- dashboard URL if present
- local file path

#### Dataset audit cell

Compact rendering fields:

- dataset name
- splits counted
- issue count
- local file path

#### Paper report cell

Compact rendering fields:

- query or title
- paper count
- headline recommendation
- local file path

### 18.6 Artifact viewer overlay

The artifact viewer must support:

- metadata header
- switching between available files
- markdown rendering for `.md`
- pretty JSON rendering for `.json`
- plain text rendering fallback

### 18.7 Skill picker

Behavior:

- popup opens when user types `$`
- list filtered by fuzzy match
- selecting an item inserts a structured skill mention when possible
- skill list comes from `skills/list`

### 18.8 Slash commands

Minimum slash commands:

- `/threads`
- `/skills`
- `/artifacts`
- `/help`
- `/clear`

## 19. Event-to-UI Projection

`mli-tui` should not render raw protocol directly.

Instead it reduces protocol messages into view models.

### 19.1 Projection rules

- `item/agentMessage/delta` appends to current assistant streaming cell
- `item/completed` for command execution creates `ExecCommand` or `ExecOutput` cells
- `turn/completed` finalizes streaming cell and clears busy state
- `artifact/created` adds artifact summary cell and updates artifact overlay state
- approval request notifications open approval overlay and insert approval marker cell

## 20. Approval Model

Approvals remain upstream Codex behavior. The wrapper normalizes them for the TUI.

### 20.1 Approval DTO

```rust
pub struct PendingApproval {
    pub id: String,
    pub kind: ApprovalKind,
    pub title: String,
    pub description: String,
    pub raw_payload: serde_json::Value,
}
```

Kinds:

- command execution
- file change
- permission request
- request user input

### 20.2 UI behavior

- pending approval moves app state to `WaitingApproval`
- approval is surfaced both inline and in overlay
- user decisions are sent back through local app-server to wrapper, then to upstream Codex

## 21. Local App-Server State Machine

```text
Booting -> Uninitialized -> Ready
Ready -> ThreadActive
ThreadActive -> TurnActive
TurnActive -> WaitingApproval
WaitingApproval -> TurnActive
TurnActive -> ThreadActive
ThreadActive -> Ready
Any -> FatalError
```

## 22. Connection State Machine (TUI)

```text
Booting -> Connecting -> Initializing -> Ready -> Streaming
Streaming -> WaitingApproval -> Streaming
Streaming -> Ready
Any -> Disconnected
Disconnected -> Reconnecting -> Ready
```

## 23. Resume Model

Resume uses local metadata as the source of truth and upstream thread IDs as bridge references.

### 23.1 Resume path

1. TUI calls `thread/list`.
2. User selects thread.
3. TUI calls `thread/resume`.
4. Wrapper loads local thread record.
5. Wrapper re-establishes upstream codex process and resumes or reconstructs context using the saved upstream thread ID.
6. TUI reloads transcript state from repo + live runtime.

### 23.2 Missing upstream thread handling

If upstream thread cannot be resumed:

- wrapper surfaces a recovery error
- local thread remains available for inspection
- future recovery tools may support fork/recreate, but v1 only guarantees graceful failure

## 24. Logging and Observability

### 24.1 Log targets

- wrapper logs: `~/.ml-intern-codex/logs/app-server/`
- TUI logs: `~/.ml-intern-codex/logs/tui/`
- transcript logs: `<cwd>/.ml-intern/threads/<thread-id>/transcript.jsonl`

### 24.2 Required structured log fields

- timestamp
- local_thread_id
- local_turn_id
- upstream_thread_id if available
- upstream_turn_id if available
- component
- event_type
- severity

## 25. Error Handling

### 25.1 Startup errors

- missing codex binary
- unsupported codex version
- corrupted config
- app home not writable

These are fatal and must stop startup with explicit user-readable messages.

### 25.2 Session errors

- upstream process exits unexpectedly
- malformed protocol payload
- thread record missing
- transcript write failure
- artifact parse failure

These should produce transcript-visible errors where possible and preserve local state.

## 26. Security and Safety Model

V1 is single-user local software.

Guardrails:

- approvals are preserved, not bypassed by the wrapper
- helper runtime paths are explicit and discoverable
- no hidden remote transport is introduced by the product itself
- local file artifact paths remain under workspace-local `.ml-intern`

## 27. Testing Strategy

### 27.1 Unit tests

- config merge behavior
- DTO serialization
- state machine transitions
- artifact preview generation
- skill descriptor loading

### 27.2 Integration tests

- wrapper with fake upstream app-server
- artifact watcher end-to-end
- thread persistence + resume
- protocol request/response flow

### 27.3 Snapshot tests

- transcript cells
- artifact summary cells
- artifact viewer rendering
- skill picker UI
- approval overlay UI

### 27.4 Manual E2E tests

- real installed `codex-cli 0.120.0`
- start thread
- skill mention
- approval flow
- dataset audit artifact
- HF job artifact
- resume flow

## 28. Implementation Readiness Checklist

This specification is implementation-ready if the following remain true:

- the upstream compatibility target stays pinned to `0.120.0`
- wrapper architecture remains the chosen integration boundary
- artifacts remain workspace-local and manifest-based
- TUI remains transcript-first rather than dashboard-first
- bundled skills remain the main ML specialization mechanism

## 29. Deferred Items

Not part of v1 implementation, but explicitly reserved:

- websocket transport
- browser client
- remote multi-user mode
- plugin marketplace support
- CodexPotter multi-round orchestration
- custom ML dashboard panes beyond artifact overlays

## 30. Locked Design Decisions

- local wrapper app-server sits between TUI and upstream Codex
- upstream Codex remains the only model/runtime backend
- upstream protocol usage is pinned to installed `codex-cli 0.120.0`
- product-local protocol is Codex-inspired and JSONL over stdio
- skills drive ML specialization; generic MCP is gone
- local artifacts are canonical for paper reports, dataset audits, and HF job summaries
- TUI is transcript-first with overlays for artifacts, skills, and threads
