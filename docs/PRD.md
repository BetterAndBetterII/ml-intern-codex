# ml-intern-codex PRD

## 1. Product Summary

`ml-intern-codex` is a local-first ML engineering terminal application built on top of the codex app-server protocol and the user's locally installed `codex` binary.

It replaces the previous `Python agent loop + LiteLLM + Anthropic/HF Router + FastAPI/SSE + React` stack with:

- a local Rust app-server wrapper around upstream `codex app-server`
- a Rust TUI inspired by CodexPotter's transcript-first interaction model
- a skill-first operating model for ML workflows
- local helper runtimes driven by `uv`, `uvx`, `python`, `npx`, and shell execution
- local artifact persistence for ML-specific outputs such as paper reports, dataset audits, and HF job summaries

This product does **not** embed Codex internally and does **not** fork Codex runtime logic into the app. It integrates against the protocol and behavior surface of the installed `codex-cli 0.120.0`.

## 2. Problem Statement

The old `ml-intern` architecture has four structural limitations:

1. It owns a custom agent runtime that must separately solve model routing, tool registration, context management, approvals, and UI streaming.
2. It couples product progress to a Python tool layer and MCP-based dynamic tool discovery.
3. It uses a web frontend and backend split that is unnecessary for a local single-user workflow.
4. It makes ML-specialized workflows difficult to persist and inspect in a durable local-first way.

The new product should instead treat Codex as the execution intelligence layer and focus product code on:

- session shaping
- skill packaging
- transcript rendering
- ML workflow conventions
- local artifact persistence and inspection

## 3. Product Goal

Build a local single-user ML engineering app that:

- uses the installed `codex` binary as the only model/runtime backend
- presents a polished Rust TUI with CodexPotter-style transcript rendering
- removes generic MCP usage entirely
- uses skills and helper runtimes to guide ML workflows
- saves important ML outputs to structured local files while also surfacing them in the TUI
- remains protocol-aligned with `codex-cli 0.120.0`

## 4. Target User

Primary user:

- a single developer or researcher working locally in a repository
- comfortable with terminal interaction
- wants Codex-level coding ergonomics plus ML workflow conventions
- needs persistent local artifacts for research, dataset inspection, and job operations

Secondary user:

- a power user who wants to extend the app with additional local skills and helper scripts

## 5. Core Product Principles

### 5.1 Codex-as-engine

The product must not implement its own LLM runtime. All intelligence execution is delegated to the installed `codex` runtime through the app-server protocol.

### 5.2 Local-first and single-user

The product targets one local user on one machine. No multi-user auth, remote control plane, or browser UI is required in v1.

### 5.3 Transcript-first UX

The TUI is primarily a transcript viewer and input surface. Rich domain outputs appear as transcript cells plus artifact viewers, not as a dashboard-first product.

### 5.4 Skill-first operation

ML-specific behavior is primarily expressed via skills and generated instructions, not via a generic dynamic MCP tool catalog.

### 5.5 Durable artifacts

Paper summaries, dataset audits, and HF job summaries must be saved to local files using a stable schema and also be inspectable from the TUI.

### 5.6 Minimal protocol divergence

Where practical, the local wrapper should preserve Codex app-server method and notification semantics so the product stays easy to reason about and upgrade.

## 6. In-Scope for v1

### 6.1 Runtime and platform

- Rust workspace from scratch in `/home/yuzhong/workspace/ml-intern-codex`
- new git repository and private remote repository
- local stdio app-server
- local Rust TUI
- wrapper bridge to installed `codex app-server`
- protocol compatibility baseline locked to `codex-cli 0.120.0`

### 6.2 Session model

- create thread
- resume thread
- list threads
- start turn
- interrupt turn
- persist thread metadata locally
- pass through Codex transcript events

### 6.3 Skill model

- bundled system skills for ML workflows
- user skills and repo-local skills discovery
- `$skill` picker in TUI
- skill list browsing
- skill-aware user input forwarding to Codex

### 6.4 ML workflow support

- HF jobs workflow support through skills and helper runtimes
- literature research workflow support through skills and helper runtimes
- dataset audit workflow support through skills and helper runtimes
- hub/repo maintenance workflow support through skills and helper runtimes

### 6.5 Artifact system

- workspace-local artifact directories
- structured artifact manifests
- artifact index and viewer
- transcript cells for newly created artifacts
- local file persistence for at least:
  - paper reports
  - dataset audits
  - HF job summaries/log snapshots

### 6.6 TUI capabilities

- transcript rendering
- live token/message streaming
- approval overlays
- skill picker
- artifacts list overlay
- artifact viewer overlay
- thread picker / resume flow
- slash commands for local navigation

## 7. Out of Scope for v1

- FastAPI backend
- React frontend
- LiteLLM
- Anthropic/HF Router support
- generic MCP client registry
- browser transport
- websocket transport
- multi-user auth and org membership
- CodexPotter multi-round clean-room workflow
- project progress-file orchestration
- autonomous background scheduler outside active user sessions

## 8. User Jobs To Be Done

### 8.1 General coding

- open the TUI in a project
- start a Codex-backed thread
- mention one or more skills
- ask for implementation/debugging/review work
- see transcript, commands, patches, approvals, and final output in one terminal flow

### 8.2 Literature research

- invoke a literature-oriented skill
- have Codex use local helpers and shell tooling to gather research
- save a paper report to a deterministic artifact path
- inspect the report from inside the TUI

### 8.3 Dataset audit

- invoke a dataset audit skill
- inspect data through helper runtimes
- save markdown and JSON audit outputs locally
- view summary inline and full details on demand

### 8.4 HF jobs operation

- invoke HF job helper flows from Codex
- save job snapshots or summaries locally
- see compact HF job state in transcript cells
- open a detailed artifact view later without re-running the command

### 8.5 Session continuity

- exit and reopen the app
- list prior threads
- resume a thread and continue working from existing Codex session history and local artifact files

## 9. Primary Use Cases

### 9.1 Start a new thread

1. User launches `ml-intern` in a repo.
2. TUI starts local app-server.
3. App-server validates installed Codex binary.
4. User enters a prompt, optionally using `$skill` mentions.
5. Wrapper starts upstream `codex app-server`, creates a thread, and starts the first turn.
6. Transcript streams in the TUI.

### 9.2 Create and inspect a dataset audit

1. User invokes a dataset audit-oriented prompt.
2. Codex uses skill instructions and helper runtimes.
3. Helper writes audit outputs into the workspace artifact root.
4. Wrapper detects new artifact manifest and emits `artifact/created`.
5. TUI shows a compact dataset audit cell.
6. User opens the artifact viewer and reads the full report.

### 9.3 Operate HF jobs

1. User asks Codex to inspect, submit, or summarize a job.
2. Codex uses shell plus helper CLI.
3. Job summary is written to local artifact files.
4. TUI shows the latest job status and file path.
5. User can later reopen the artifact from an overlay.

### 9.4 Resume a prior thread

1. User launches the TUI.
2. User opens the thread list.
3. User selects a prior thread.
4. Local app-server reattaches to or rehydrates the upstream Codex thread via wrapper metadata.
5. User continues the conversation.

## 10. UX Requirements

### 10.1 Transcript-first rendering

The main pane must prioritize readable transcript rendering over dashboards.

Required transcript content:

- user messages
- assistant messages and deltas
- shell command execution
- shell output summaries
- file patch summaries
- plan updates
- approvals and interrupts
- artifact-created summary cells
- warnings and error cells

### 10.2 Artifact visibility

Artifacts must be visible in two places:

- inline transcript summary cell when created or updated
- artifact browser overlay for later inspection

### 10.3 Skill discoverability

The user must be able to:

- discover bundled skills from a list
- invoke a skill through `$` mention
- understand which skill was selected

### 10.4 Local file visibility

Every ML artifact shown in the TUI must reveal where it was saved on disk.

### 10.5 Graceful approvals

Because Codex approvals remain part of the runtime, the TUI must provide approval interaction flows aligned with Codex transcript semantics.

## 11. Functional Requirements

### 11.1 Runtime

- detect and validate `codex` binary at startup
- reject unsupported versions below or above the pinned compatibility band for v1
- launch upstream `codex app-server` over stdio
- support initialize/start/resume/turn/interrupt flows

### 11.2 Skills

- expose a skills list through local app-server
- support bundled system skills shipped with the product
- support repo-local and user-global skill roots
- support structured skill inputs and text mentions

### 11.3 Artifact persistence

- create deterministic local artifact directories
- write artifact manifests with stable schema
- persist artifact index locally
- allow artifact listing and reading through the local app-server

### 11.4 HF jobs visualization

- represent HF job artifacts with compact transcript summaries
- support reading detailed HF job artifacts from TUI overlay
- preserve raw file outputs on disk

### 11.5 Resume and history

- store thread index locally
- store transcript metadata locally
- resume from prior thread sessions

## 12. Non-Functional Requirements

### 12.1 Reliability

- if upstream Codex crashes, the wrapper must surface a clear error and preserve local metadata
- malformed artifact outputs must not crash the TUI
- thread index corruption must be detectable and recoverable

### 12.2 Performance

- app startup should feel interactive on a developer laptop
- transcript rendering should remain smooth for long sessions
- artifact indexing should be incremental and cheap

### 12.3 Maintainability

- strict layered Rust workspace
- well-defined DTOs and state machines
- minimal coupling between TUI rendering and upstream protocol transport

### 12.4 Extensibility

- new bundled skills can be added without changing the core protocol
- new artifact kinds can be added without reworking transcript plumbing

## 13. Success Criteria

The v1 product is successful if all of the following are true:

1. A user can open the TUI, start a Codex-backed thread, and complete normal coding tasks without any LiteLLM or MCP dependency.
2. A user can invoke skill-driven ML workflows and see them operate through transcript-first UX.
3. Paper reports, dataset audits, and HF job summaries are saved to disk using a stable schema and can be reopened from the TUI.
4. The product works entirely through the installed `codex-cli 0.120.0` and local helper runtimes.
5. The architecture is cleanly separated enough that future implementation can proceed crate-by-crate without major redesign.

## 14. Acceptance Scenarios

### 14.1 New thread smoke test

- launch TUI
- create thread
- send prompt
- receive streaming response
- interrupt and recover

### 14.2 Skill invocation smoke test

- open `$skill` picker
- select bundled skill
- send prompt with structured skill input
- verify upstream Codex turn receives skill context correctly

### 14.3 Dataset audit smoke test

- run dataset audit workflow
- artifact directory is created under workspace-local artifact root
- transcript shows audit summary cell
- overlay opens saved markdown and JSON outputs

### 14.4 HF jobs smoke test

- run HF jobs workflow
- transcript shows compact job summary cell
- raw artifact files exist on disk
- job artifact reopens later from overlay

### 14.5 Resume smoke test

- close app
- reopen app
- list thread
- resume thread successfully

## 15. Risks

- upstream Codex protocol drift beyond `0.120.0`
- helper runtime availability differences across user machines
- artifact generation conventions becoming inconsistent if not enforced by helper tools
- over-customizing TUI beyond transcript-first scope

## 16. Product Decision Log

Locked decisions:

- wrapper bridge architecture: yes
- protocol baseline: `codex-cli 0.120.0`
- transport: stdio only in v1
- runtime backend: installed Codex only
- UI model: transcript-first Rust TUI in CodexPotter style
- ML outputs: TUI-visible and also persisted to local files
- generic MCP: removed entirely
- LiteLLM/Anthropic/HF Router: removed entirely
