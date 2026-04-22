use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ArtifactId, LocalThreadId, LocalTurnId};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    pub metadata: Value,
}

impl ArtifactManifest {
    pub fn parse_paper_report_metadata(&self) -> Result<PaperReportMetadata, serde_json::Error> {
        serde_json::from_value(self.metadata.clone())
    }

    pub fn parse_dataset_audit_metadata(&self) -> Result<DatasetAuditMetadata, serde_json::Error> {
        serde_json::from_value(self.metadata.clone())
    }

    pub fn parse_job_snapshot_metadata(&self) -> Result<JobSnapshotMetadata, serde_json::Error> {
        serde_json::from_value(self.metadata.clone())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ArtifactQuery {
    pub thread_id: Option<LocalThreadId>,
    pub kind: Option<ArtifactKind>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactFilePayload {
    pub path: PathBuf,
    pub media_type: String,
    pub text: Option<String>,
    pub base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactReadBundle {
    pub manifest: ArtifactManifest,
    pub files: Vec<ArtifactFilePayload>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactIndexWarning {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ArtifactScanResult {
    pub manifests: Vec<ArtifactManifest>,
    pub warnings: Vec<ArtifactIndexWarning>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArtifactPreview {
    PaperReport {
        query: String,
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
        dashboard_url: Option<String>,
    },
    Generic {
        headline: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaperReportMetadata {
    pub query: String,
    pub paper_count: usize,
    pub top_papers: Vec<String>,
    pub recommended_recipe: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatasetAuditMetadata {
    pub dataset: String,
    pub splits: Vec<String>,
    pub row_counts: Value,
    pub issues: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobSnapshotMetadata {
    pub job_id: String,
    pub status: String,
    pub hardware: Option<String>,
    pub dashboard_url: Option<String>,
    pub duration_seconds: Option<u64>,
}
