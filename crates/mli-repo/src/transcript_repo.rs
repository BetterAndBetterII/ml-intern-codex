use std::fs::{self, OpenOptions};
use std::io::Write;

use anyhow::{Context, Result};
use mli_config::AppPaths;
use mli_types::{LocalThreadId, TranscriptEvent};

pub trait TranscriptRepo {
    fn append(&self, event: &TranscriptEvent) -> Result<()>;
    fn list(&self, thread_id: LocalThreadId) -> Result<Vec<TranscriptEvent>>;
}

#[derive(Clone, Debug)]
pub struct FsTranscriptRepo {
    paths: AppPaths,
}

impl FsTranscriptRepo {
    pub fn new(paths: AppPaths) -> Self {
        Self { paths }
    }
}

impl TranscriptRepo for FsTranscriptRepo {
    fn append(&self, event: &TranscriptEvent) -> Result<()> {
        let path = self.paths.transcript_file(event.thread_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let line = serde_json::to_string(event).context("failed to serialize transcript event")?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        writeln!(file, "{line}").with_context(|| format!("failed to append {}", path.display()))
    }

    fn list(&self, thread_id: LocalThreadId) -> Result<Vec<TranscriptEvent>> {
        let path = self.paths.transcript_file(thread_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        raw.lines()
            .map(|line| serde_json::from_str(line).context("invalid transcript event"))
            .collect()
    }
}
