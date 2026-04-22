use anyhow::{Context, Result, anyhow};
use mli_config::{AppConfig, AppPaths};
use mli_repo::{FsThreadRepo, FsTurnRepo, ThreadRepo, TurnRepo};
use mli_types::{
    LocalThreadId, LocalTurnId, StartThreadRequest, StartTurnRequest, ThreadRecord, ThreadStatus,
    TurnRecord, TurnStatus, utc_now,
};

#[derive(Clone, Debug)]
pub struct ThreadReadModel {
    pub thread: ThreadRecord,
    pub turns: Vec<TurnRecord>,
}

pub trait ThreadService {
    fn start_thread(&self, req: StartThreadRequest) -> Result<ThreadRecord>;
    fn resume_thread(&self, id: LocalThreadId) -> Result<ThreadRecord>;
    fn list_threads(&self) -> Result<Vec<ThreadRecord>>;
    fn read_thread(&self, id: LocalThreadId) -> Result<ThreadReadModel>;
}

pub trait TurnService {
    fn start_turn(&self, req: StartTurnRequest) -> Result<TurnRecord>;
    fn interrupt_turn(&self, thread_id: LocalThreadId, turn_id: LocalTurnId) -> Result<()>;
}

#[derive(Clone)]
pub struct LocalThreadService {
    config: AppConfig,
    paths: AppPaths,
    thread_repo: FsThreadRepo,
    turn_repo: FsTurnRepo,
}

impl LocalThreadService {
    pub fn new(config: AppConfig, paths: AppPaths) -> Self {
        let thread_repo = FsThreadRepo::new(paths.clone());
        let turn_repo = FsTurnRepo::new(paths.clone());
        Self {
            config,
            paths,
            thread_repo,
            turn_repo,
        }
    }

    pub fn mark_thread_status(
        &self,
        thread_id: LocalThreadId,
        status: ThreadStatus,
    ) -> Result<ThreadRecord> {
        let mut thread = self
            .thread_repo
            .get(thread_id)?
            .ok_or_else(|| anyhow!("unknown thread {thread_id}"))?;
        thread.status = status;
        thread.updated_at = utc_now();
        self.thread_repo.update(&thread)?;
        Ok(thread)
    }

    pub fn mark_turn_status(
        &self,
        thread_id: LocalThreadId,
        turn_id: LocalTurnId,
        status: TurnStatus,
    ) -> Result<TurnRecord> {
        let mut turn = self
            .turn_repo
            .get(thread_id, turn_id)?
            .ok_or_else(|| anyhow!("unknown turn {turn_id}"))?;
        turn.status = status.clone();
        if matches!(
            status,
            TurnStatus::Completed | TurnStatus::Interrupted | TurnStatus::Failed
        ) {
            turn.finished_at = Some(utc_now());
        }
        self.turn_repo.update(&turn)?;
        Ok(turn)
    }
}

impl ThreadService for LocalThreadService {
    fn start_thread(&self, req: StartThreadRequest) -> Result<ThreadRecord> {
        let approval_policy = req
            .approval_policy
            .unwrap_or(self.config.codex.approval_policy);
        let sandbox_mode = req.sandbox_mode.unwrap_or(self.config.codex.sandbox_mode);
        let temp_thread_id = LocalThreadId::new();
        let transcript_path = self.paths.transcript_file(temp_thread_id);
        let artifact_root = self.paths.artifacts_dir(temp_thread_id);
        let mut thread = ThreadRecord::new(
            req.cwd,
            req.title,
            req.model
                .or_else(|| self.config.codex.default_model.clone()),
            approval_policy,
            sandbox_mode,
            transcript_path,
            artifact_root,
        );
        thread.id = temp_thread_id;
        self.thread_repo.create(&thread)?;
        Ok(thread)
    }

    fn resume_thread(&self, id: LocalThreadId) -> Result<ThreadRecord> {
        let mut thread = self
            .thread_repo
            .get(id)?
            .ok_or_else(|| anyhow!("thread {id} does not exist"))?;
        thread.updated_at = utc_now();
        if matches!(
            thread.status,
            ThreadStatus::NotLoaded | ThreadStatus::Error | ThreadStatus::Interrupted
        ) {
            thread.status = ThreadStatus::Idle;
        }
        self.thread_repo.update(&thread)?;
        Ok(thread)
    }

    fn list_threads(&self) -> Result<Vec<ThreadRecord>> {
        self.thread_repo.list()
    }

    fn read_thread(&self, id: LocalThreadId) -> Result<ThreadReadModel> {
        let thread = self
            .thread_repo
            .get(id)?
            .ok_or_else(|| anyhow!("thread {id} does not exist"))?;
        let turns = self.turn_repo.list_by_thread(id)?;
        Ok(ThreadReadModel { thread, turns })
    }
}

impl TurnService for LocalThreadService {
    fn start_turn(&self, req: StartTurnRequest) -> Result<TurnRecord> {
        let mut thread = self
            .thread_repo
            .get(req.thread_id)?
            .ok_or_else(|| anyhow!("thread {} does not exist", req.thread_id))?;
        let mut turn = TurnRecord::new(req.thread_id, req.user_input_summary);
        turn.status = TurnStatus::Starting;
        self.turn_repo.create(&turn)?;
        thread.status = ThreadStatus::Running;
        thread.updated_at = utc_now();
        self.thread_repo.update(&thread)?;
        Ok(turn)
    }

    fn interrupt_turn(&self, thread_id: LocalThreadId, turn_id: LocalTurnId) -> Result<()> {
        self.mark_turn_status(thread_id, turn_id, TurnStatus::Interrupted)
            .with_context(|| format!("failed to interrupt turn {turn_id}"))?;
        self.mark_thread_status(thread_id, ThreadStatus::Interrupted)
            .with_context(|| format!("failed to update thread {thread_id}"))?;
        Ok(())
    }
}
