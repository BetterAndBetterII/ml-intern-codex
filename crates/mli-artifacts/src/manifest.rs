use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use mli_types::{ArtifactFilePayload, ArtifactManifest, ArtifactReadBundle};

pub fn read_manifest_file(path: &Path) -> Result<ArtifactManifest> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read manifest {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse manifest {}", path.display()))
}

pub fn read_artifact_bundle(
    artifact_dir: &Path,
    manifest: &ArtifactManifest,
) -> Result<ArtifactReadBundle> {
    let mut files = Vec::new();
    let mut paths = vec![manifest.primary_path.clone()];
    paths.extend(manifest.extra_paths.iter().cloned());

    for relative_path in paths {
        let resolved = resolve_artifact_file_path(artifact_dir, &relative_path);
        files.push(read_file_payload(&resolved));
    }

    Ok(ArtifactReadBundle {
        manifest: manifest.clone(),
        files,
    })
}

pub fn resolve_artifact_file_path(artifact_dir: &Path, declared_path: &Path) -> PathBuf {
    if declared_path.is_absolute() {
        declared_path.to_path_buf()
    } else {
        artifact_dir.join(declared_path)
    }
}

pub fn read_file_payload(path: &Path) -> ArtifactFilePayload {
    let media_type = media_type_for(path).to_owned();
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return ArtifactFilePayload {
                path: path.to_path_buf(),
                media_type,
                text: None,
                base64: None,
                read_error: Some(format!("failed to read file {}: {error}", path.display())),
            };
        }
    };
    let (text, base64) = match String::from_utf8(bytes.clone()) {
        Ok(text) => (Some(text), None),
        Err(_) => (None, Some(STANDARD.encode(bytes))),
    };

    ArtifactFilePayload {
        path: path.to_path_buf(),
        media_type,
        text,
        base64,
        read_error: None,
    }
}

fn media_type_for(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("md") => "text/markdown",
        Some("json") => "application/json",
        Some("txt") | Some("log") => "text/plain",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use mli_types::{
        ArtifactId, ArtifactKind, ArtifactManifest, LocalThreadId, LocalTurnId, utc_now,
    };

    use super::read_artifact_bundle;

    #[test]
    fn read_artifact_bundle_keeps_missing_files_as_read_errors() {
        let artifact_dir = std::env::temp_dir().join(format!(
            "mli-artifact-bundle-test-{}-{}",
            std::process::id(),
            utc_now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&artifact_dir);
        fs::create_dir_all(&artifact_dir)
            .unwrap_or_else(|error| panic!("create artifact dir: {error}"));
        fs::write(artifact_dir.join("report.md"), "# report\n")
            .unwrap_or_else(|error| panic!("write report: {error}"));

        let manifest = ArtifactManifest {
            id: ArtifactId::new(),
            version: 1,
            local_thread_id: LocalThreadId::new(),
            local_turn_id: LocalTurnId::new(),
            kind: ArtifactKind::GenericMarkdown,
            title: "report".to_owned(),
            created_at: utc_now(),
            updated_at: utc_now(),
            summary: "summary".to_owned(),
            tags: Vec::new(),
            primary_path: "report.md".into(),
            extra_paths: vec!["missing.json".into()],
            metadata: serde_json::json!({}),
        };

        let bundle = read_artifact_bundle(&artifact_dir, &manifest)
            .unwrap_or_else(|error| panic!("read artifact bundle: {error}"));

        assert_eq!(bundle.files.len(), 2);
        assert_eq!(bundle.files[0].text.as_deref(), Some("# report\n"));
        assert_eq!(bundle.files[0].read_error, None);
        assert!(
            bundle.files[1]
                .read_error
                .as_deref()
                .is_some_and(|message| message.contains("failed to read file"))
        );
        assert_eq!(bundle.files[1].text, None);
    }
}
