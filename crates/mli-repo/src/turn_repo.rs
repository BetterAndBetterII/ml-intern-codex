use std::fs;

use anyhow::{Context, Result};
use mli_config::AppPaths;
use mli_types::{LocalThreadId, LocalTurnId, TurnRecord};

use crate::thread_repo::{read_json_file, write_json_file};

pub trait TurnRepo {
    fn create(&self, turn: &TurnRecord) -> Result<()>;
    fn update(&self, turn: &TurnRecord) -> Result<()>;
    fn get(&self, thread_id: LocalThreadId, turn_id: LocalTurnId) -> Result<Option<TurnRecord>>;
    fn list_by_thread(&self, thread_id: LocalThreadId) -> Result<Vec<TurnRecord>>;
}

#[derive(Clone, Debug)]
pub struct FsTurnRepo {
    paths: AppPaths,
}

impl FsTurnRepo {
    pub fn new(paths: AppPaths) -> Self {
        Self { paths }
    }

    fn turn_path(&self, thread_id: LocalThreadId, turn_id: LocalTurnId) -> std::path::PathBuf {
        self.paths
            .turns_dir(thread_id)
            .join(format!("{turn_id}.json"))
    }
}

impl TurnRepo for FsTurnRepo {
    fn create(&self, turn: &TurnRecord) -> Result<()> {
        write_json_file(&self.turn_path(turn.local_thread_id, turn.id), turn)
    }

    fn update(&self, turn: &TurnRecord) -> Result<()> {
        write_json_file(&self.turn_path(turn.local_thread_id, turn.id), turn)
    }

    fn get(&self, thread_id: LocalThreadId, turn_id: LocalTurnId) -> Result<Option<TurnRecord>> {
        let path = self.turn_path(thread_id, turn_id);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(read_json_file(&path)?))
    }

    fn list_by_thread(&self, thread_id: LocalThreadId) -> Result<Vec<TurnRecord>> {
        let turns_dir = self.paths.turns_dir(thread_id);
        let mut turns = Vec::new();
        if !turns_dir.exists() {
            return Ok(turns);
        }
        for entry in fs::read_dir(&turns_dir)
            .with_context(|| format!("failed to read {}", turns_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                turns.push(read_json_file(&path)?);
            }
        }
        turns.sort_by_key(|turn| turn.started_at);
        Ok(turns)
    }
}
