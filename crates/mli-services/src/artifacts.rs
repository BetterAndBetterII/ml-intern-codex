use anyhow::{Result, anyhow};
use mli_artifacts::{build_preview, read_artifact_bundle};
use mli_config::AppPaths;
use mli_repo::{ArtifactRepo, FsArtifactRepo};
use mli_types::{
    ArtifactId, ArtifactManifest, ArtifactPreview, ArtifactQuery, ArtifactReadBundle,
    ArtifactScanResult,
};

pub trait ArtifactService {
    fn scan_artifacts(&self, query: ArtifactQuery) -> Result<ArtifactScanResult>;
    fn list_artifacts(&self, query: ArtifactQuery) -> Result<Vec<ArtifactManifest>>;
    fn read_artifact(&self, id: ArtifactId) -> Result<ArtifactReadBundle>;
    fn register_or_update(&self, manifest: ArtifactManifest) -> Result<()>;
    fn preview(&self, manifest: &ArtifactManifest) -> ArtifactPreview;
}

#[derive(Clone)]
pub struct LocalArtifactService {
    repo: FsArtifactRepo,
}

impl LocalArtifactService {
    pub fn new(paths: AppPaths) -> Self {
        Self {
            repo: FsArtifactRepo::new(paths),
        }
    }
}

impl ArtifactService for LocalArtifactService {
    fn scan_artifacts(&self, query: ArtifactQuery) -> Result<ArtifactScanResult> {
        self.repo.scan(query)
    }

    fn list_artifacts(&self, query: ArtifactQuery) -> Result<Vec<ArtifactManifest>> {
        self.repo.list(query)
    }

    fn read_artifact(&self, id: ArtifactId) -> Result<ArtifactReadBundle> {
        let manifest = self
            .repo
            .get(id)?
            .ok_or_else(|| anyhow!("artifact {id} does not exist"))?;
        let artifact_dir = self
            .repo
            .find_artifact_dir(id)?
            .ok_or_else(|| anyhow!("artifact directory missing for {id}"))?;
        read_artifact_bundle(&artifact_dir, &manifest)
    }

    fn register_or_update(&self, manifest: ArtifactManifest) -> Result<()> {
        self.repo.upsert_manifest(&manifest)
    }

    fn preview(&self, manifest: &ArtifactManifest) -> ArtifactPreview {
        build_preview(manifest)
    }
}
