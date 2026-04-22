use mli_types::{ArtifactKind, ArtifactManifest, ArtifactPreview};

pub fn build_preview(manifest: &ArtifactManifest) -> ArtifactPreview {
    match manifest.kind {
        ArtifactKind::PaperReport => match manifest.parse_paper_report_metadata() {
            Ok(metadata) => ArtifactPreview::PaperReport {
                query: metadata.query,
                paper_count: metadata.paper_count,
                headline: metadata.recommended_recipe,
            },
            Err(_) => ArtifactPreview::Generic {
                headline: manifest.summary.clone(),
            },
        },
        ArtifactKind::DatasetAudit => match manifest.parse_dataset_audit_metadata() {
            Ok(metadata) => ArtifactPreview::DatasetAudit {
                dataset: metadata.dataset,
                split_count: metadata.splits.len(),
                issue_count: metadata.issues.len(),
            },
            Err(_) => ArtifactPreview::Generic {
                headline: manifest.summary.clone(),
            },
        },
        ArtifactKind::JobSnapshot | ArtifactKind::JobLogExcerpt | ArtifactKind::JobRunbook => {
            match manifest.parse_job_snapshot_metadata() {
                Ok(metadata) => ArtifactPreview::JobSnapshot {
                    job_id: metadata.job_id,
                    status: metadata.status,
                    hardware: metadata.hardware,
                    dashboard_url: metadata.dashboard_url,
                },
                Err(_) => ArtifactPreview::Generic {
                    headline: manifest.summary.clone(),
                },
            }
        }
        ArtifactKind::GenericMarkdown | ArtifactKind::GenericJson | ArtifactKind::GenericText => {
            ArtifactPreview::Generic {
                headline: manifest.summary.clone(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use mli_types::utc_now;
    use mli_types::{
        ArtifactId, ArtifactKind, ArtifactManifest, ArtifactPreview, LocalThreadId, LocalTurnId,
    };

    fn manifest(kind: ArtifactKind, metadata: serde_json::Value) -> ArtifactManifest {
        ArtifactManifest {
            id: ArtifactId::new(),
            version: 1,
            local_thread_id: LocalThreadId::new(),
            local_turn_id: LocalTurnId::new(),
            kind,
            title: "artifact".to_owned(),
            created_at: utc_now(),
            updated_at: utc_now(),
            summary: "summary".to_owned(),
            tags: vec![],
            primary_path: "report.md".into(),
            extra_paths: vec![],
            metadata,
        }
    }

    #[test]
    fn dataset_audit_preview_reads_structured_metadata() {
        let preview = build_preview(&manifest(
            ArtifactKind::DatasetAudit,
            json!({
                "dataset": "org/name",
                "splits": ["train", "validation"],
                "row_counts": {"train": 10},
                "issues": ["missing values"]
            }),
        ));
        match preview {
            ArtifactPreview::DatasetAudit {
                dataset,
                split_count,
                issue_count,
            } => {
                assert_eq!(dataset, "org/name");
                assert_eq!(split_count, 2);
                assert_eq!(issue_count, 1);
            }
            other => panic!("unexpected preview: {other:?}"),
        }
    }

    #[test]
    fn paper_report_preview_reads_query_and_recipe() {
        let preview = build_preview(&manifest(
            ArtifactKind::PaperReport,
            json!({
                "query": "diffusion for tabular data",
                "paper_count": 4,
                "top_papers": ["Paper A"],
                "recommended_recipe": "Start with the survey and compare ablations."
            }),
        ));
        match preview {
            ArtifactPreview::PaperReport {
                query,
                paper_count,
                headline,
            } => {
                assert_eq!(query, "diffusion for tabular data");
                assert_eq!(paper_count, 4);
                assert_eq!(headline, "Start with the survey and compare ablations.");
            }
            other => panic!("unexpected preview: {other:?}"),
        }
    }

    #[test]
    fn job_snapshot_preview_reads_dashboard_url() {
        let preview = build_preview(&manifest(
            ArtifactKind::JobSnapshot,
            json!({
                "job_id": "job-123",
                "status": "running",
                "hardware": "a10g",
                "dashboard_url": "https://hf.co/jobs/job-123",
                "duration_seconds": 42
            }),
        ));
        match preview {
            ArtifactPreview::JobSnapshot {
                job_id,
                status,
                hardware,
                dashboard_url,
            } => {
                assert_eq!(job_id, "job-123");
                assert_eq!(status, "running");
                assert_eq!(hardware.as_deref(), Some("a10g"));
                assert_eq!(dashboard_url.as_deref(), Some("https://hf.co/jobs/job-123"));
            }
            other => panic!("unexpected preview: {other:?}"),
        }
    }

    #[test]
    fn generic_preview_falls_back_to_summary() {
        let preview = build_preview(&manifest(ArtifactKind::GenericMarkdown, json!({})));
        match preview {
            ArtifactPreview::Generic { headline } => assert_eq!(headline, "summary"),
            other => panic!("unexpected preview: {other:?}"),
        }
    }
}
