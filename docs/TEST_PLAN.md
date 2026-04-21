# ml-intern-codex Test Plan

This document maps the canonical user flows from `docs/USER_FLOWS.md` to test coverage.

The goal is to make every implementation milestone answer three questions clearly:

- what user path is being verified
- how it is being verified
- whether failure blocks release

## Test Level Definitions

- `unit`: pure logic, DTOs, reducers, parsers, state transitions
- `integration`: multi-component behavior inside the workspace with fakes or local harnesses
- `snapshot`: deterministic UI rendering assertions
- `e2e`: near-real system validation using the actual local app, wrapper, and upstream `codex`

## Automation Definitions

- `automated`: runs in test commands or CI
- `manual`: human-operated verification script or checklist

## Release Blocking Policy

- `yes`: must pass before v1 release
- `no`: valuable coverage but does not block v1 ship on its own

## Coverage Matrix

| Flow ID | Flow Name | Level | Automation | Fixture / Harness | Blocking |
|---|---|---|---|---|---|
| `F001` | Startup and initialize local session | integration, e2e | automated + manual | fake codex binary, real codex binary | yes |
| `F002` | Start a new thread and send the first prompt | integration, e2e, snapshot | automated + manual | fake upstream app-server, transcript fixtures | yes |
| `F003` | Use the `$skill` picker and submit a skill-guided prompt | unit, integration, snapshot, e2e | automated + manual | skill fixtures, fake upstream input capture | yes |
| `F004` | Handle approval-required execution | integration, snapshot, e2e | automated + manual | fake upstream approval requests | yes |
| `F005` | Interrupt an active turn | integration, e2e | automated + manual | fake long-running turn harness | yes |
| `F006` | Resume a previously saved thread | integration, e2e | automated + manual | persisted thread fixtures, fake upstream resume harness | yes |
| `F007` | Run a dataset audit workflow and persist artifacts | integration, snapshot, e2e | automated + manual | artifact writer fixtures, helper runtime fixture | yes |
| `F008` | Run a paper research workflow and persist artifacts | integration, snapshot, manual e2e | automated + manual | paper artifact fixtures, helper runtime fixture | no |
| `F009` | Run an HF jobs workflow and persist artifacts | integration, snapshot, manual e2e | automated + manual | HF jobs artifact fixtures, helper runtime fixture | yes |
| `F010` | Browse and read artifacts from the TUI | unit, integration, snapshot, e2e | automated + manual | artifact index fixtures, viewer fixtures | yes |

## Detailed Cases

## F001 - Startup and initialize local session

### Test levels

- `integration`
- `e2e`

### Automation

- automated integration
- manual e2e

### Fixture / harness

- fake `codex` executable that returns version `0.120.0`
- fake `codex` executable that returns mismatched version
- temp home directory fixture
- temp cwd fixture
- real local `codex-cli 0.120.0` for manual verification

### Acceptance criteria

- wrapper initializes successfully when `codex` exists and matches version
- startup fails clearly when `codex` is missing
- startup fails clearly when version mismatches
- app home and runtime directories are created in expected locations
- TUI reaches ready state only on successful initialization

### Release blocking

- yes

## F002 - Start a new thread and send the first prompt

### Test levels

- `integration`
- `snapshot`
- `e2e`

### Automation

- automated integration
- automated snapshot
- manual e2e

### Fixture / harness

- fake upstream app-server script that emits:
  - initialize response
  - thread/start response
  - turn/start response
  - streaming agent deltas
  - turn completion
- transcript rendering snapshots

### Acceptance criteria

- local thread record is created
- local turn record is created
- user message appears in transcript
- assistant stream appears and completes
- transcript is persisted to `transcript.jsonl`
- thread returns to idle state after completion

### Release blocking

- yes

## F003 - Use the `$skill` picker and submit a skill-guided prompt

### Test levels

- `unit`
- `integration`
- `snapshot`
- `e2e`

### Automation

- automated unit
- automated integration
- automated snapshot
- manual e2e

### Fixture / harness

- bundled skill fixture directories
- repo-local skill fixture directories
- fake upstream app-server capture harness that records forwarded input payloads
- TUI skill popup snapshots

### Acceptance criteria

- typing `$` opens the picker
- picker filters skills correctly
- selected skill inserts a stable mention into the composer
- forwarded turn input preserves selected skill intent
- duplicate skill names can still be disambiguated through path-aware selection

### Release blocking

- yes

## F004 - Handle approval-required execution

### Test levels

- `integration`
- `snapshot`
- `e2e`

### Automation

- automated integration
- automated snapshot
- manual e2e

### Fixture / harness

- fake upstream app-server that emits approval requests for:
  - command execution
  - file change
  - permission request
  - request user input
- approval overlay snapshots

### Acceptance criteria

- approval request is normalized and displayed
- TUI enters waiting-approval state
- approving forwards the expected payload back upstream
- rejecting forwards the expected payload back upstream
- transcript records request and resolution

### Release blocking

- yes

## F005 - Interrupt an active turn

### Test levels

- `integration`
- `e2e`

### Automation

- automated integration
- manual e2e

### Fixture / harness

- fake upstream app-server with a long-running streaming turn
- interrupt timing harness

### Acceptance criteria

- interrupt request is sent to wrapper and upstream
- turn transitions to interrupted
- streaming stops
- thread returns to an interactive ready or idle state
- transcript records interruption

### Release blocking

- yes

## F006 - Resume a previously saved thread

### Test levels

- `integration`
- `e2e`

### Automation

- automated integration
- manual e2e

### Fixture / harness

- prebuilt local thread fixtures:
  - valid thread metadata
  - transcript history
  - turn history
- fake upstream resume harness

### Acceptance criteria

- thread list returns persisted threads
- selecting a thread restores visible transcript history
- wrapper can reconnect the thread when upstream resume succeeds
- resume failure remains inspectable and does not corrupt local metadata

### Release blocking

- yes

## F007 - Run a dataset audit workflow and persist artifacts

### Test levels

- `integration`
- `snapshot`
- `e2e`

### Automation

- automated integration
- automated snapshot
- manual e2e

### Fixture / harness

- helper runtime fixture that writes a valid dataset audit artifact
- malformed artifact fixture
- artifact watcher harness
- dataset audit summary cell snapshots

### Acceptance criteria

- artifact watcher detects new dataset audit manifest
- artifact is indexed correctly
- TUI shows the expected dataset audit summary cell
- markdown and JSON payloads are readable through `artifact/read`
- malformed manifests produce warnings rather than crashes

### Release blocking

- yes

## F008 - Run a paper research workflow and persist artifacts

### Test levels

- `integration`
- `snapshot`
- `e2e`

### Automation

- automated integration
- automated snapshot
- manual e2e

### Fixture / harness

- helper runtime fixture that writes a valid paper report artifact
- paper report cell snapshots
- artifact viewer markdown and JSON fixtures

### Acceptance criteria

- paper report artifact is indexed
- transcript shows the expected paper report summary cell
- saved report files are accessible from the viewer
- partial failures do not crash runtime

### Release blocking

- no

## F009 - Run an HF jobs workflow and persist artifacts

### Test levels

- `integration`
- `snapshot`
- `e2e`

### Automation

- automated integration
- automated snapshot
- manual e2e

### Fixture / harness

- helper runtime fixture that writes valid HF jobs artifact manifests
- running-job and completed-job fixtures
- HF jobs summary cell snapshots

### Acceptance criteria

- job snapshot artifact is indexed
- transcript shows compact HF jobs cell with status and path
- viewer can read the saved report and optional raw payloads
- repeated updates can produce `artifact/updated` without duplicating corrupted state

### Release blocking

- yes

## F010 - Browse and read artifacts from the TUI

### Test levels

- `unit`
- `integration`
- `snapshot`
- `e2e`

### Automation

- automated unit
- automated integration
- automated snapshot
- manual e2e

### Fixture / harness

- artifact index fixtures
- artifact directories with:
  - markdown primary files
  - JSON files
  - text files
  - missing primary file cases
- artifact list and artifact viewer snapshots

### Acceptance criteria

- artifact list loads from indexed manifests
- selecting an artifact loads the expected file payloads
- markdown, JSON, and text render correctly
- missing files produce explicit errors without crashing the TUI

### Release blocking

- yes

## Cross-Cutting Test Suites

These suites validate shared behavior across multiple flows.

### C001 - DTO and protocol serialization

- Level: `unit`
- Automation: automated
- Fixture:
  - local app-server DTO roundtrip samples
  - upstream protocol DTO roundtrip samples
- Acceptance:
  - all request/response/notification DTOs serialize and deserialize deterministically
- Blocking:
  - yes

### C002 - State machine transition correctness

- Level: `unit`
- Automation: automated
- Fixture:
  - thread lifecycle transition tables
  - turn lifecycle transition tables
  - connection state transition tables
- Acceptance:
  - invalid transitions are rejected
  - valid transitions produce the expected next state
- Blocking:
  - yes

### C003 - Artifact watcher robustness

- Level: `integration`
- Automation: automated
- Fixture:
  - delayed file write fixture
  - malformed manifest fixture
  - partial directory fixture
- Acceptance:
  - watcher emits create/update only for valid manifests
  - malformed artifacts produce warnings only
- Blocking:
  - yes

### C004 - Transcript rendering snapshots

- Level: `snapshot`
- Automation: automated
- Fixture:
  - user message cells
  - assistant message cells
  - approval cells
  - artifact summary cells
  - warning and error cells
- Acceptance:
  - snapshots remain stable and intentional
- Blocking:
  - yes

## Manual Release Checklist

The following manual checklist must be run before v1 release:

1. Start app with real `codex-cli 0.120.0`
2. Create a new thread and complete one prompt
3. Use `$skill` picker and submit a skill-guided prompt
4. Trigger and resolve one approval
5. Interrupt one active turn
6. Generate one dataset audit artifact
7. Generate one HF jobs artifact
8. Open the artifact list and read both artifacts
9. Restart the app and resume the previous thread

If any blocking flow fails, release is blocked.

