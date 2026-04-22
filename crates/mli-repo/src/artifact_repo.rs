use std::cmp::Reverse;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use mli_config::AppPaths;
use mli_types::{
    ArtifactId, ArtifactIndexWarning, ArtifactManifest, ArtifactQuery, ArtifactScanResult,
    LocalThreadId,
};

use crate::thread_repo::{read_json_file, write_json_file};

pub trait ArtifactRepo {
    fn upsert_manifest(&self, manifest: &ArtifactManifest) -> Result<()>;
    fn get(&self, id: ArtifactId) -> Result<Option<ArtifactManifest>>;
    fn scan(&self, query: ArtifactQuery) -> Result<ArtifactScanResult>;
    fn list(&self, query: ArtifactQuery) -> Result<Vec<ArtifactManifest>>;
}

#[derive(Clone, Debug)]
pub struct FsArtifactRepo {
    paths: AppPaths,
}

impl FsArtifactRepo {
    pub fn new(paths: AppPaths) -> Self {
        Self { paths }
    }

    pub fn artifact_dir(&self, thread_id: LocalThreadId, artifact_id: ArtifactId) -> PathBuf {
        self.paths
            .artifacts_dir(thread_id)
            .join(artifact_id.to_string())
    }

    pub fn find_artifact_dir(&self, artifact_id: ArtifactId) -> Result<Option<PathBuf>> {
        if !self.paths.threads_root.exists() {
            return Ok(None);
        }
        for thread_entry in fs::read_dir(&self.paths.threads_root)
            .with_context(|| format!("failed to read {}", self.paths.threads_root.display()))?
        {
            let thread_entry = thread_entry?;
            let candidate = thread_entry
                .path()
                .join("artifacts")
                .join(artifact_id.to_string());
            if candidate.exists() {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    fn manifest_path(&self, thread_id: LocalThreadId, artifact_id: ArtifactId) -> PathBuf {
        self.artifact_dir(thread_id, artifact_id)
            .join("artifact.json")
    }
}

impl ArtifactRepo for FsArtifactRepo {
    fn upsert_manifest(&self, manifest: &ArtifactManifest) -> Result<()> {
        write_json_file(
            &self.manifest_path(manifest.local_thread_id, manifest.id),
            manifest,
        )
    }

    fn get(&self, id: ArtifactId) -> Result<Option<ArtifactManifest>> {
        if let Some(dir) = self.find_artifact_dir(id)? {
            return Ok(Some(read_json_file(&dir.join("artifact.json"))?));
        }
        Ok(None)
    }

    fn scan(&self, query: ArtifactQuery) -> Result<ArtifactScanResult> {
        let mut manifests = Vec::new();
        let mut warnings = Vec::new();
        let thread_ids = if let Some(thread_id) = query.thread_id {
            vec![thread_id]
        } else {
            collect_thread_ids(&self.paths)?
        };

        for thread_id in thread_ids {
            let artifacts_dir = self.paths.artifacts_dir(thread_id);
            if !artifacts_dir.exists() {
                continue;
            }
            for entry in fs::read_dir(&artifacts_dir)
                .with_context(|| format!("failed to read {}", artifacts_dir.display()))?
            {
                let entry = entry?;
                let manifest_path = entry.path().join("artifact.json");
                if !manifest_path.exists() {
                    continue;
                }
                let manifest: ArtifactManifest = match read_json_file(&manifest_path) {
                    Ok(manifest) => manifest,
                    Err(error) => {
                        warnings.push(ArtifactIndexWarning {
                            path: manifest_path.clone(),
                            message: error.to_string(),
                        });
                        continue;
                    }
                };
                if query
                    .kind
                    .as_ref()
                    .is_some_and(|kind| kind != &manifest.kind)
                {
                    continue;
                }
                manifests.push(manifest);
            }
        }

        manifests.sort_by_key(|manifest| Reverse(manifest.updated_at));
        if let Some(limit) = query.limit {
            manifests.truncate(limit);
        }
        Ok(ArtifactScanResult {
            manifests,
            warnings,
        })
    }

    fn list(&self, query: ArtifactQuery) -> Result<Vec<ArtifactManifest>> {
        Ok(self.scan(query)?.manifests)
    }
}

fn collect_thread_ids(paths: &AppPaths) -> Result<Vec<LocalThreadId>> {
    let mut ids = Vec::new();
    if !paths.threads_root.exists() {
        return Ok(ids);
    }
    for entry in fs::read_dir(&paths.threads_root)
        .with_context(|| format!("failed to read {}", paths.threads_root.display()))?
    {
        let entry = entry?;
        let Some(file_name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        if let Ok(thread_id) = file_name.parse() {
            ids.push(thread_id);
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use mli_types::utc_now;
    use mli_types::{ArtifactKind, LocalTurnId};

    fn temp_dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("mli-artifact-repo-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create temp dir: {error}"));
        path
    }

    fn app_paths(root: &std::path::Path) -> AppPaths {
        AppPaths {
            cwd: root.to_path_buf(),
            install_root: root.to_path_buf(),
            app_home: root.join("home"),
            user_config_path: root.join("home/config.toml"),
            user_logs_tui_dir: root.join("home/logs/tui"),
            user_logs_app_server_dir: root.join("home/logs/app-server"),
            runtime_dir: root.join("home/runtime"),
            codex_home_dir: root.join("home/runtime/codex-home"),
            generated_skills_dir: root.join("home/runtime/generated-skills"),
            cache_dir: root.join("home/cache"),
            db_dir: root.join("home/db"),
            db_path: root.join("home/db/state.sqlite"),
            project_root: root.join(".ml-intern"),
            project_config_path: root.join(".ml-intern/config.toml"),
            threads_root: root.join(".ml-intern/threads"),
            bundled_skills_root: root.join("skills/system"),
            helper_python_src: root.join("helpers/python/src"),
            helper_node_src: root.join("helpers/node/src"),
        }
    }

    fn manifest(thread_id: LocalThreadId) -> ArtifactManifest {
        ArtifactManifest {
            id: ArtifactId::new(),
            version: 1,
            local_thread_id: thread_id,
            local_turn_id: LocalTurnId::new(),
            kind: ArtifactKind::DatasetAudit,
            title: "audit".to_owned(),
            created_at: utc_now(),
            updated_at: utc_now(),
            summary: "summary".to_owned(),
            tags: vec!["dataset".to_owned()],
            primary_path: PathBuf::from("report.md"),
            extra_paths: vec![PathBuf::from("report.json")],
            metadata: json!({
                "dataset": "org/name",
                "splits": ["train"],
                "row_counts": {"train": 1},
                "issues": []
            }),
        }
    }

    #[test]
    fn scan_skips_malformed_manifest_and_keeps_valid_entries() {
        let root = temp_dir("scan");
        let paths = app_paths(&root);
        let repo = FsArtifactRepo::new(paths.clone());
        let thread_id = LocalThreadId::new();

        let valid = manifest(thread_id);
        repo.upsert_manifest(&valid)
            .unwrap_or_else(|error| panic!("write valid manifest: {error}"));

        let broken_dir = paths
            .artifacts_dir(thread_id)
            .join(ArtifactId::new().to_string());
        fs::create_dir_all(&broken_dir)
            .unwrap_or_else(|error| panic!("create broken dir: {error}"));
        fs::write(broken_dir.join("artifact.json"), "{not-json")
            .unwrap_or_else(|error| panic!("write broken manifest: {error}"));

        let scan = repo
            .scan(ArtifactQuery {
                thread_id: Some(thread_id),
                kind: None,
                limit: None,
            })
            .unwrap_or_else(|error| panic!("scan artifacts: {error}"));

        assert_eq!(scan.manifests.len(), 1);
        assert_eq!(scan.manifests[0].id, valid.id);
        assert_eq!(scan.warnings.len(), 1);
        assert!(scan.warnings[0].path.ends_with("artifact.json"));
    }
}
