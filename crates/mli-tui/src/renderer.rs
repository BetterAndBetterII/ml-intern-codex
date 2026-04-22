use std::path::PathBuf;

use mli_types::{AppState, ArtifactEventCell, ArtifactPreview, HistoryCellModel};

pub fn render_app(state: &AppState, selected_skill: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str("=== ml-intern-codex ===\n");
    out.push_str(&format!(
        "cwd: {} | codex: {} | approval: {} | sandbox: {}\n",
        state
            .runtime
            .cwd
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "unknown".to_owned()),
        state.runtime.codex_version.as_deref().unwrap_or("unknown"),
        state
            .runtime
            .approval_policy
            .map(|policy| format!("{policy:?}"))
            .unwrap_or_else(|| "unknown".to_owned()),
        state
            .runtime
            .sandbox_mode
            .map(|mode| format!("{mode:?}"))
            .unwrap_or_else(|| "unknown".to_owned()),
    ));
    out.push_str(&format!(
        "connection: {:?} | active thread: {} | selected skill: {}\n\n",
        state.connection,
        state
            .active_thread_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "none".to_owned()),
        selected_skill.unwrap_or("none")
    ));
    out.push_str("Transcript\n----------\n");
    if state.transcript.history.is_empty() {
        out.push_str("(empty)\n");
    } else {
        for cell in &state.transcript.history {
            out.push_str(&format!("{}\n", render_history_cell(cell)));
        }
    }
    out.push_str(
        "\nCommands: type `$` for skills, or use /threads /skills /artifacts /approval /help /clear /quit\n",
    );
    out
}

pub(crate) fn render_history_cell(cell: &HistoryCellModel) -> String {
    match cell {
        HistoryCellModel::UserMessage(cell) => format!("you> {}", cell.text),
        HistoryCellModel::AssistantMessage(cell) => {
            if cell.streaming {
                format!("assistant~ {}", cell.text)
            } else {
                format!("assistant> {}", cell.text)
            }
        }
        HistoryCellModel::ExecCommand(cell) => format!("exec> {}", cell.command),
        HistoryCellModel::ExecOutput(cell) => {
            let prefix = if cell.streaming { "exec~" } else { "exec<" };
            format!("{prefix} {} => {}", cell.command, cell.output)
        }
        HistoryCellModel::PatchSummary(cell) => format!("patch> {}", cell.summary),
        HistoryCellModel::PlanUpdate(cell) => format!("plan> {}", cell.summary),
        HistoryCellModel::ApprovalRequest(cell) => format!("approval> {}", cell.approval.title),
        HistoryCellModel::ArtifactCreated(cell) => render_artifact_cell(cell, false),
        HistoryCellModel::ArtifactUpdated(cell) => render_artifact_cell(cell, true),
        HistoryCellModel::Warning(cell) => format!("warning> {}", cell.message),
        HistoryCellModel::Error(cell) => format!("error> {}", cell.message),
        HistoryCellModel::Status(cell) => format!("status> {}", cell.message),
    }
}

fn render_artifact_cell(cell: &ArtifactEventCell, updated: bool) -> String {
    let prefix = if updated { "artifact~" } else { "artifact+" };
    let path = render_artifact_path(&cell.manifest);
    match &cell.preview {
        ArtifactPreview::PaperReport {
            query,
            paper_count,
            headline,
        } => format!(
            "{prefix} {} | query={} | papers={} | recipe={} | path={}",
            cell.manifest.title, query, paper_count, headline, path
        ),
        ArtifactPreview::DatasetAudit {
            dataset,
            split_count,
            issue_count,
        } => format!(
            "{prefix} {} | dataset={} | splits={} | issues={} | path={}",
            cell.manifest.title, dataset, split_count, issue_count, path
        ),
        ArtifactPreview::JobSnapshot {
            job_id,
            status,
            hardware,
            dashboard_url,
        } => {
            let mut parts = vec![
                format!("{prefix} {}", cell.manifest.title),
                format!("job={job_id}"),
                format!("status={status}"),
            ];
            if let Some(hardware) = hardware {
                parts.push(format!("hardware={hardware}"));
            }
            if let Some(dashboard_url) = dashboard_url {
                parts.push(format!("url={dashboard_url}"));
            }
            parts.push(format!("path={path}"));
            parts.join(" | ")
        }
        ArtifactPreview::Generic { headline } => format!(
            "{prefix} {} | {} | path={}",
            cell.manifest.title, headline, path
        ),
    }
}

fn render_artifact_path(cell: &mli_types::ArtifactManifest) -> String {
    if cell.primary_path.is_absolute() {
        return cell.primary_path.display().to_string();
    }
    artifact_display_path(cell).display().to_string()
}

fn artifact_display_path(manifest: &mli_types::ArtifactManifest) -> PathBuf {
    PathBuf::from(".ml-intern")
        .join("threads")
        .join(manifest.local_thread_id.to_string())
        .join("artifacts")
        .join(manifest.id.to_string())
        .join(&manifest.primary_path)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use mli_types::{
        ApprovalPolicy, ArtifactEventCell, ArtifactId, ArtifactKind, ArtifactManifest,
        ArtifactPreview, ExecOutputCell, HistoryCellModel, LocalThreadId, LocalTurnId,
        RuntimeBannerState, SandboxMode, utc_now,
    };

    #[test]
    fn render_app_includes_runtime_banner() {
        let state = AppState {
            runtime: RuntimeBannerState {
                cwd: Some(PathBuf::from("/tmp/project")),
                codex_version: Some("0.120.0".to_owned()),
                approval_policy: Some(ApprovalPolicy::OnRequest),
                sandbox_mode: Some(SandboxMode::WorkspaceWrite),
            },
            ..AppState::default()
        };

        let rendered = render_app(&state, Some("hf-dataset-audit"));

        assert!(rendered.contains("cwd: /tmp/project"));
        assert!(rendered.contains("codex: 0.120.0"));
        assert!(rendered.contains("approval: OnRequest"));
        assert!(rendered.contains("sandbox: WorkspaceWrite"));
    }

    fn manifest(title: &str) -> ArtifactManifest {
        ArtifactManifest {
            id: ArtifactId::new(),
            version: 1,
            local_thread_id: LocalThreadId::new(),
            local_turn_id: LocalTurnId::new(),
            kind: ArtifactKind::DatasetAudit,
            title: title.to_owned(),
            created_at: utc_now(),
            updated_at: utc_now(),
            summary: "summary".to_owned(),
            tags: vec![],
            primary_path: PathBuf::from("report.md"),
            extra_paths: vec![PathBuf::from("report.json")],
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn render_app_formats_structured_artifact_summaries() {
        let dataset_manifest = manifest("Dataset audit for demo");
        let paper_manifest = manifest("Paper report for diffusion");
        let job_manifest = manifest("Job snapshot for job-123");
        let state = AppState {
            transcript: mli_types::TranscriptState {
                history: vec![
                    HistoryCellModel::ArtifactCreated(ArtifactEventCell {
                        manifest: dataset_manifest.clone(),
                        preview: ArtifactPreview::DatasetAudit {
                            dataset: "demo/corpus".to_owned(),
                            split_count: 2,
                            issue_count: 1,
                        },
                    }),
                    HistoryCellModel::ArtifactCreated(ArtifactEventCell {
                        manifest: paper_manifest.clone(),
                        preview: ArtifactPreview::PaperReport {
                            query: "diffusion for tabular data".to_owned(),
                            paper_count: 4,
                            headline: "Start with the survey".to_owned(),
                        },
                    }),
                    HistoryCellModel::ArtifactCreated(ArtifactEventCell {
                        manifest: job_manifest.clone(),
                        preview: ArtifactPreview::JobSnapshot {
                            job_id: "job-123".to_owned(),
                            status: "running".to_owned(),
                            hardware: Some("a10g".to_owned()),
                            dashboard_url: Some("https://hf.co/jobs/job-123".to_owned()),
                        },
                    }),
                ],
            },
            ..AppState::default()
        };

        let rendered = render_app(&state, None);

        assert!(rendered.contains(
            "artifact+ Dataset audit for demo | dataset=demo/corpus | splits=2 | issues=1"
        ));
        assert!(rendered.contains("artifact+ Paper report for diffusion | query=diffusion for tabular data | papers=4 | recipe=Start with the survey"));
        assert!(!rendered.contains("artifact+ | job=job-123"));
        assert!(rendered.contains("artifact+ Job snapshot for job-123 | job=job-123 | status=running | hardware=a10g | url=https://hf.co/jobs/job-123"));
        assert!(rendered.contains(".ml-intern/threads/"));
    }

    #[test]
    fn render_app_distinguishes_artifact_updates() {
        let manifest = manifest("Job snapshot for job-123");
        let state = AppState {
            transcript: mli_types::TranscriptState {
                history: vec![HistoryCellModel::ArtifactUpdated(ArtifactEventCell {
                    manifest: manifest.clone(),
                    preview: ArtifactPreview::JobSnapshot {
                        job_id: "job-123".to_owned(),
                        status: "completed".to_owned(),
                        hardware: None,
                        dashboard_url: Some("https://hf.co/jobs/job-123".to_owned()),
                    },
                })],
            },
            ..AppState::default()
        };

        let rendered = render_app(&state, None);

        assert!(
            rendered
                .contains("artifact~ Job snapshot for job-123 | job=job-123 | status=completed")
        );
        assert!(rendered.contains("url=https://hf.co/jobs/job-123"));
        assert!(rendered.contains(".ml-intern/threads/"));
    }

    #[test]
    fn render_app_marks_live_command_output_with_exec_tilde() {
        let state = AppState {
            transcript: mli_types::TranscriptState {
                history: vec![HistoryCellModel::ExecOutput(ExecOutputCell {
                    item_id: "cmd-1".to_owned(),
                    command: "python long_job.py".to_owned(),
                    output: "epoch 1\n".to_owned(),
                    streaming: true,
                })],
            },
            ..AppState::default()
        };

        let rendered = render_app(&state, None);

        assert!(rendered.contains("exec~ python long_job.py => epoch 1"));
    }
}
