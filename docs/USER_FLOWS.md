# ml-intern-codex User Flows

This document defines the primary user-facing flows for `ml-intern-codex`.

Each flow has:

- a stable flow ID
- an entry point
- preconditions
- the main path
- branch and failure paths
- expected transcript behavior
- expected persisted files

These flows are the canonical source for:

- implementation sequencing
- acceptance criteria
- test planning
- release-readiness review

## Flow Index

- `F001` Startup and initialize local session
- `F002` Start a new thread and send the first prompt
- `F003` Use the `$skill` picker and submit a skill-guided prompt
- `F004` Handle approval-required execution
- `F005` Interrupt an active turn
- `F006` Resume a previously saved thread
- `F007` Run a dataset audit workflow and persist artifacts
- `F008` Run a paper research workflow and persist artifacts
- `F009` Run an HF jobs workflow and persist artifacts
- `F010` Browse and read artifacts from the TUI

## F001 - Startup and initialize local session

### Entry

- User runs `ml-intern` in a repository root or subdirectory.

### Preconditions

- `codex` is installed and available on `PATH`.
- Installed version matches the pinned compatibility target `codex-cli 0.120.0`.
- The current working directory is readable.
- `~/.ml-intern-codex/` is writable or can be created.

### Main flow

1. CLI boots the TUI process.
2. TUI starts the local wrapper app-server over stdio.
3. Wrapper validates the upstream `codex` binary path and version.
4. Wrapper resolves config layers.
5. Wrapper prepares the local runtime home and skill roots.
6. TUI sends `initialize`.
7. Wrapper responds with runtime metadata.
8. TUI enters the ready state and renders the empty transcript plus composer.

### Branch and failure paths

- If `codex` is missing:
  - startup fails with a fatal error screen
  - no TUI interactive session is entered
- If `codex` version mismatches:
  - startup fails with a compatibility error
  - no thread is created
- If config parsing fails:
  - startup fails with a config error
- If app home creation fails:
  - startup fails with a filesystem error

### Expected transcript

- No thread transcript exists yet.
- The UI header shows:
  - current cwd
  - upstream codex version
  - default sandbox mode
  - default approval policy
- A lightweight status cell or startup banner may appear, but no turn items exist.

### Expected persisted files

- `~/.ml-intern-codex/config.toml` if first run default config is materialized
- `~/.ml-intern-codex/logs/tui/`
- `~/.ml-intern-codex/logs/app-server/`
- `~/.ml-intern-codex/runtime/codex-home/`
- `~/.ml-intern-codex/db/state.sqlite`

## F002 - Start a new thread and send the first prompt

### Entry

- User types plain text into the composer and submits it in ready state.

### Preconditions

- `F001` completed successfully.
- No blocking modal or approval overlay is open.
- Wrapper app-server is initialized.

### Main flow

1. TUI creates a `thread/start` request if no active thread exists.
2. Wrapper creates a local thread record.
3. Wrapper starts or attaches an upstream `codex app-server` process.
4. Wrapper performs upstream `initialize` and `thread/start`.
5. TUI sends `turn/start` with the user text input.
6. Wrapper creates a local turn record and forwards the turn to upstream Codex.
7. Upstream Codex streams item and delta events.
8. TUI renders assistant streaming output and side effects.
9. Turn completes successfully.
10. Thread returns to idle state.

### Branch and failure paths

- If upstream process fails to launch:
  - thread is marked error
  - error cell is inserted
- If upstream `thread/start` fails:
  - local thread remains persisted with error status
  - no active turn is created
- If `turn/start` fails after thread creation:
  - thread remains available for retry
  - turn is marked failed
- If stream disconnects unexpectedly:
  - wrapper emits an explicit runtime error
  - TUI shows disconnection state and the turn ends failed or interrupted

### Expected transcript

- User message cell with submitted prompt
- Session/thread start status cell if this is a brand-new thread
- Assistant streaming message cell
- Command, output, patch, or plan cells if upstream Codex emits them
- Final assistant message completion
- Turn completion status reflected in header

### Expected persisted files

- `<cwd>/.ml-intern/threads/<thread-id>/thread.json`
- `<cwd>/.ml-intern/threads/<thread-id>/transcript.jsonl`
- `<cwd>/.ml-intern/threads/<thread-id>/turns/<turn-id>.json`

## F003 - Use the `$skill` picker and submit a skill-guided prompt

### Entry

- User types `$` in the composer and chooses a skill from the popup.

### Preconditions

- `F001` completed successfully.
- At least one bundled, user, or repo-local skill is discoverable.
- `skills/list` succeeds for the current cwd.

### Main flow

1. User types `$`.
2. TUI opens the skill picker popup.
3. TUI requests the current skill list if not already cached.
4. User filters and selects a skill.
5. TUI inserts a structured skill mention or skill-linked text mention into the composer.
6. User completes the prompt and submits it.
7. Wrapper forwards user input to upstream Codex in a skill-preserving form.
8. Upstream Codex resolves the skill through the prepared skill roots.
9. The turn runs with skill-guided context.

### Branch and failure paths

- If `skills/list` fails:
  - picker shows an error state
  - user can still submit plain text without a skill
- If a skill becomes unreadable after selection:
  - upstream Codex may surface a skill-read warning
  - turn continues with best-effort fallback
- If duplicate skill names exist:
  - picker prefers exact selected path
  - structured mention avoids ambiguity

### Expected transcript

- User message cell contains either:
  - rendered skill mention
  - or text that clearly includes the skill token
- Optional warning cell if skill loading fails
- Assistant transcript reflects skill-guided behavior

### Expected persisted files

- Standard thread and turn persistence from `F002`
- No artifact files are required by this flow alone
- Optional skill cache/index update under `~/.ml-intern-codex/cache/skill-index.json`

## F004 - Handle approval-required execution

### Entry

- Upstream Codex requests approval for command execution, file change, permissions, or tool user input.

### Preconditions

- An active turn is in progress.
- TUI is connected to the wrapper app-server.

### Main flow

1. Upstream Codex emits an approval request.
2. Wrapper normalizes the request into local approval DTOs.
3. TUI inserts an approval marker cell and opens the approval overlay.
4. User reviews the request.
5. User approves or rejects.
6. TUI sends the decision back to the wrapper.
7. Wrapper forwards the decision to upstream Codex.
8. Turn resumes streaming or completes.

### Branch and failure paths

- If the user rejects:
  - transcript records rejection
  - upstream Codex may continue with an alternate path or stop that branch
- If the approval response fails to send:
  - TUI stays in waiting state and surfaces an error
- If the wrapper loses upstream connectivity during approval:
  - approval overlay closes with an error
  - turn enters failed or interrupted state

### Expected transcript

- Approval request cell with concise summary
- Optional detailed overlay content
- Approval decision reflected inline
- Continued assistant output after approval if execution resumes

### Expected persisted files

- Standard thread and turn persistence from `F002`
- Approval request and decision events appended to `transcript.jsonl`

## F005 - Interrupt an active turn

### Entry

- User triggers interrupt during a streaming turn.

### Preconditions

- A turn is currently active and not yet completed.

### Main flow

1. User presses the interrupt key or uses the interrupt action.
2. TUI sends `turn/interrupt`.
3. Wrapper forwards interrupt to upstream Codex.
4. Upstream Codex stops the current turn.
5. Wrapper updates local turn and thread state.
6. TUI marks the turn interrupted and returns to ready/idle state.

### Branch and failure paths

- If the turn already ended before interrupt arrives:
  - interrupt is treated as a no-op
  - TUI refreshes into the terminal completed state
- If upstream ignores or delays interrupt:
  - TUI shows pending interrupt status until resolved
- If wrapper crashes during interrupt:
  - state is recovered from local transcript and persisted turn metadata on restart

### Expected transcript

- Streaming assistant cell stops growing
- Interrupt status cell or inline interruption marker appears
- Header and footer reflect idle state afterward

### Expected persisted files

- `<turn-id>.json` updated with interrupted status
- `transcript.jsonl` includes interrupt request and resolution events

## F006 - Resume a previously saved thread

### Entry

- User opens the thread picker and selects a prior thread.

### Preconditions

- At least one local thread record exists under the project state root.
- Local metadata store is readable.

### Main flow

1. User opens the thread picker.
2. TUI calls `thread/list`.
3. Wrapper returns local thread metadata.
4. User selects one thread.
5. TUI calls `thread/resume`.
6. Wrapper loads local thread state and transcript metadata.
7. Wrapper starts or reconnects an upstream Codex process and rebinds the thread.
8. TUI restores transcript history and marks the thread active.
9. User can send another prompt.

### Branch and failure paths

- If the local thread metadata exists but upstream resume fails:
  - thread stays inspectable
  - resume returns a clear recovery error
- If transcript replay partially fails:
  - TUI still resumes using available persisted metadata
  - corrupted events are skipped with warnings
- If no threads exist:
  - picker shows empty state

### Expected transcript

- Prior transcript history is visible
- Active thread indicator changes to the resumed thread
- Optional status cell notes successful resume or degraded recovery

### Expected persisted files

- Existing files from earlier flows are reused
- `thread.json` updated with latest `updated_at` and active status
- Optional new turn files if the user continues after resume

## F007 - Run a dataset audit workflow and persist artifacts

### Entry

- User submits a dataset-audit-oriented prompt, typically with a dataset skill.

### Preconditions

- `F003` is available if the user wants guided skill selection.
- Helper runtime is available:
  - `uv`, `uvx`, or `python`
- Workspace artifact root is writable.

### Main flow

1. User submits a dataset audit prompt.
2. Upstream Codex follows skill guidance and uses local helper runtime or shell commands.
3. Dataset audit logic analyzes schema, splits, row counts, samples, and issues.
4. Helper writes a dataset audit artifact directory.
5. Artifact watcher detects the new `artifact.json`.
6. Wrapper indexes the artifact and emits `artifact/created`.
7. TUI inserts a compact dataset audit cell.
8. User can open the full artifact later.

### Branch and failure paths

- If helper runtime is missing:
  - assistant may surface a runtime-not-found error
  - no artifact is created
- If audit succeeds partially but artifact write fails:
  - turn may contain assistant summary
  - artifact watcher does not emit creation
- If manifest is malformed:
  - TUI shows warning
  - raw files may still exist on disk

### Expected transcript

- User message cell
- Assistant messages and any command cells used for the audit
- Artifact-created cell with:
  - dataset name
  - split count
  - issue count
  - saved local path

### Expected persisted files

- Standard thread and turn files
- `<cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id>/artifact.json`
- `<cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id>/report.md`
- `<cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id>/report.json`
- Optional `<cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id>/raw.txt`

## F008 - Run a paper research workflow and persist artifacts

### Entry

- User submits a literature or paper-research-oriented prompt.

### Preconditions

- Helper runtime is available.
- Workspace artifact root is writable.

### Main flow

1. User submits the paper research prompt.
2. Upstream Codex uses skill instructions and local helper commands.
3. Research logic gathers papers, extracts summaries, and prepares a recommendation.
4. Helper writes a paper report artifact directory.
5. Artifact watcher indexes the report.
6. TUI shows an artifact-created paper report cell.

### Branch and failure paths

- If network-dependent research cannot complete:
  - assistant may explain the limitation in transcript
  - no artifact is created or an incomplete artifact is written
- If report markdown is written but manifest is missing:
  - no formal artifact event is emitted
- If raw research output is too large:
  - report persists summarized content
  - raw output may be truncated into `raw.txt`

### Expected transcript

- User prompt
- Research or shell execution cells as emitted by upstream Codex
- Artifact-created paper report cell with:
  - query
  - paper count
  - recommended recipe headline
  - saved local path

### Expected persisted files

- Standard thread and turn files
- `<cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id>/artifact.json`
- `<cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id>/report.md`
- `<cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id>/report.json`
- Optional raw supporting outputs

## F009 - Run an HF jobs workflow and persist artifacts

### Entry

- User submits an HF jobs operation prompt such as inspect, summarize, or prepare a runbook.

### Preconditions

- Helper runtime is available.
- Required local environment for the workflow is available.
- Workspace artifact root is writable.

### Main flow

1. User submits the HF jobs prompt.
2. Upstream Codex follows skill guidance and uses helper/runtime commands.
3. Workflow captures job metadata, status, logs, or a derived summary.
4. Helper writes an HF jobs artifact directory.
5. Artifact watcher indexes the artifact.
6. TUI inserts an HF jobs summary cell.
7. User can open the full artifact later.

### Branch and failure paths

- If job lookup fails:
  - transcript shows a workflow error
  - no artifact is created
- If logs are too large:
  - artifact stores summarized excerpt and optional raw file
- If the job is still running:
  - artifact is created with a non-terminal status
  - later refresh may emit `artifact/updated`

### Expected transcript

- User prompt
- Command/output cells as emitted during job operations
- Artifact-created or artifact-updated cell with:
  - job id
  - status
  - hardware if known
  - dashboard URL if known
  - saved local path

### Expected persisted files

- Standard thread and turn files
- `<cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id>/artifact.json`
- `<cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id>/report.md`
- Optional:
  - `report.json`
  - `raw.txt`
  - `logs/`

## F010 - Browse and read artifacts from the TUI

### Entry

- User opens the artifact list overlay and selects one artifact.

### Preconditions

- At least one valid artifact manifest has been indexed.
- Artifact files are still present on disk.

### Main flow

1. User opens the artifact list overlay.
2. TUI calls `artifact/list`.
3. Wrapper returns indexed artifact manifests.
4. User filters or selects one artifact.
5. TUI calls `artifact/read`.
6. Wrapper reads manifest and referenced files.
7. TUI opens the artifact viewer overlay.
8. User switches between markdown, JSON, or text files if multiple are present.

### Branch and failure paths

- If an artifact file is missing:
  - viewer opens with partial content or an explicit file-missing error
- If manifest exists but primary file is unreadable:
  - metadata remains visible
  - content pane shows read error
- If no artifacts exist:
  - list overlay shows empty state

### Expected transcript

- Opening the artifact browser does not require a new transcript cell.
- Existing artifact-created cells remain the discovery point in transcript.
- Optional status messages may appear only for read failures.

### Expected persisted files

- No new files are required for read-only browsing.
- Access may update in-memory caches but not canonical artifact payloads.

