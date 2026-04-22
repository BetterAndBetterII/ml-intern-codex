use std::cmp::Reverse;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use mli_config::AppPaths;
use mli_types::{LocalThreadId, ThreadRecord};

pub trait ThreadRepo {
    fn create(&self, thread: &ThreadRecord) -> Result<()>;
    fn update(&self, thread: &ThreadRecord) -> Result<()>;
    fn get(&self, id: LocalThreadId) -> Result<Option<ThreadRecord>>;
    fn list(&self) -> Result<Vec<ThreadRecord>>;
}

#[derive(Clone, Debug)]
pub struct FsThreadRepo {
    paths: AppPaths,
}

impl FsThreadRepo {
    pub fn new(paths: AppPaths) -> Self {
        Self { paths }
    }
}

impl ThreadRepo for FsThreadRepo {
    fn create(&self, thread: &ThreadRecord) -> Result<()> {
        let thread_dir = self.paths.thread_dir(thread.id);
        fs::create_dir_all(self.paths.turns_dir(thread.id))
            .with_context(|| format!("failed to create {}", thread_dir.display()))?;
        fs::create_dir_all(self.paths.artifacts_dir(thread.id))
            .with_context(|| format!("failed to create artifacts dir for {}", thread.id))?;
        if !self.paths.transcript_file(thread.id).exists() {
            fs::write(self.paths.transcript_file(thread.id), b"")
                .with_context(|| format!("failed to create transcript for {}", thread.id))?;
        }
        write_json_file(&self.paths.thread_file(thread.id), thread)
    }

    fn update(&self, thread: &ThreadRecord) -> Result<()> {
        write_json_file(&self.paths.thread_file(thread.id), thread)
    }

    fn get(&self, id: LocalThreadId) -> Result<Option<ThreadRecord>> {
        let path = self.paths.thread_file(id);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(read_json_file(&path)?))
    }

    fn list(&self) -> Result<Vec<ThreadRecord>> {
        let mut threads = Vec::new();
        if !self.paths.threads_root.exists() {
            return Ok(threads);
        }
        for entry in fs::read_dir(&self.paths.threads_root)
            .with_context(|| format!("failed to read {}", self.paths.threads_root.display()))?
        {
            let entry = entry?;
            let path = entry.path().join("thread.json");
            if path.exists() {
                threads.push(read_json_file(&path)?);
            }
        }
        threads.sort_by_key(|thread| Reverse(thread.updated_at));
        Ok(threads)
    }
}

pub(crate) fn write_json_file<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let body = serde_json::to_vec_pretty(value).context("failed to serialize JSON")?;
    fs::write(path, body).with_context(|| format!("failed to write {}", path.display()))
}

pub(crate) fn read_json_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}
