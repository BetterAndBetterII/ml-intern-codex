use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use mli_types::{ApprovalPolicy, SandboxMode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppConfig {
    pub codex: CodexConfig,
    pub ui: UiConfig,
    pub artifacts: ArtifactConfig,
    pub skills: SkillsConfig,
    pub runtime: RuntimeConfig,
}

impl AppConfig {
    pub fn load_for_cwd(cwd: &Path) -> Result<(Self, AppPaths)> {
        let paths = AppPaths::for_cwd(cwd)?;
        paths.ensure_base_layout()?;
        paths.ensure_bootstrap_files(&Self::default())?;

        let mut config = Self::default();
        for candidate in [&paths.user_config_path, &paths.project_config_path] {
            if candidate.exists() {
                let raw = fs::read_to_string(candidate)
                    .with_context(|| format!("failed to read config {}", candidate.display()))?;
                let parsed = parse_raw_config(&raw)
                    .with_context(|| format!("failed to parse config {}", candidate.display()))?;
                config.merge(parsed);
            }
        }

        Ok((config, paths))
    }

    pub fn merge(&mut self, partial: PartialAppConfig) {
        self.codex.merge(partial.codex);
        self.ui.merge(partial.ui);
        self.artifacts.merge(partial.artifacts);
        self.skills.merge(partial.skills);
        self.runtime.merge(partial.runtime);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CodexConfig {
    pub bin_path: PathBuf,
    pub expected_version: String,
    pub default_model: Option<String>,
    pub approval_policy: ApprovalPolicy,
    pub sandbox_mode: SandboxMode,
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            bin_path: PathBuf::from("codex"),
            expected_version: "0.120.0".to_owned(),
            default_model: None,
            approval_policy: ApprovalPolicy::OnRequest,
            sandbox_mode: SandboxMode::WorkspaceWrite,
        }
    }
}

impl CodexConfig {
    fn merge(&mut self, partial: PartialCodexConfig) {
        if let Some(bin_path) = partial.bin_path {
            self.bin_path = bin_path;
        }
        if let Some(expected_version) = partial.expected_version {
            self.expected_version = expected_version;
        }
        if let Some(default_model) = partial.default_model {
            self.default_model = Some(default_model);
        }
        if let Some(approval_policy) = partial.approval_policy {
            self.approval_policy = approval_policy;
        }
        if let Some(sandbox_mode) = partial.sandbox_mode {
            self.sandbox_mode = sandbox_mode;
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UiConfig {
    pub startup_banner: bool,
    pub theme: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            startup_banner: true,
            theme: "ml-intern-classic".to_owned(),
        }
    }
}

impl UiConfig {
    fn merge(&mut self, partial: PartialUiConfig) {
        if let Some(startup_banner) = partial.startup_banner {
            self.startup_banner = startup_banner;
        }
        if let Some(theme) = partial.theme {
            self.theme = theme;
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactConfig {
    pub project_root_dirname: String,
    pub auto_watch: bool,
    pub max_preview_bytes: usize,
}

impl Default for ArtifactConfig {
    fn default() -> Self {
        Self {
            project_root_dirname: ".ml-intern".to_owned(),
            auto_watch: true,
            max_preview_bytes: 32 * 1024,
        }
    }
}

impl ArtifactConfig {
    fn merge(&mut self, partial: PartialArtifactConfig) {
        if let Some(project_root_dirname) = partial.project_root_dirname {
            self.project_root_dirname = project_root_dirname;
        }
        if let Some(auto_watch) = partial.auto_watch {
            self.auto_watch = auto_watch;
        }
        if let Some(max_preview_bytes) = partial.max_preview_bytes {
            self.max_preview_bytes = max_preview_bytes;
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillsConfig {
    pub bundled_enabled: bool,
    pub extra_user_roots: Vec<PathBuf>,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            bundled_enabled: true,
            extra_user_roots: Vec::new(),
        }
    }
}

impl SkillsConfig {
    fn merge(&mut self, partial: PartialSkillsConfig) {
        if let Some(bundled_enabled) = partial.bundled_enabled {
            self.bundled_enabled = bundled_enabled;
        }
        if let Some(extra_user_roots) = partial.extra_user_roots {
            self.extra_user_roots = extra_user_roots;
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub bridge_start_timeout_ms: u64,
    pub interrupt_grace_timeout_ms: u64,
    pub upstream_idle_shutdown_secs: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            bridge_start_timeout_ms: 10_000,
            interrupt_grace_timeout_ms: 5_000,
            upstream_idle_shutdown_secs: 120,
        }
    }
}

impl RuntimeConfig {
    fn merge(&mut self, partial: PartialRuntimeConfig) {
        if let Some(bridge_start_timeout_ms) = partial.bridge_start_timeout_ms {
            self.bridge_start_timeout_ms = bridge_start_timeout_ms;
        }
        if let Some(interrupt_grace_timeout_ms) = partial.interrupt_grace_timeout_ms {
            self.interrupt_grace_timeout_ms = interrupt_grace_timeout_ms;
        }
        if let Some(upstream_idle_shutdown_secs) = partial.upstream_idle_shutdown_secs {
            self.upstream_idle_shutdown_secs = upstream_idle_shutdown_secs;
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct PartialAppConfig {
    #[serde(default)]
    pub codex: PartialCodexConfig,
    #[serde(default)]
    pub ui: PartialUiConfig,
    #[serde(default)]
    pub artifacts: PartialArtifactConfig,
    #[serde(default)]
    pub skills: PartialSkillsConfig,
    #[serde(default)]
    pub runtime: PartialRuntimeConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct PartialCodexConfig {
    pub bin_path: Option<PathBuf>,
    pub expected_version: Option<String>,
    pub default_model: Option<String>,
    pub approval_policy: Option<ApprovalPolicy>,
    pub sandbox_mode: Option<SandboxMode>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct PartialUiConfig {
    pub startup_banner: Option<bool>,
    pub theme: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct PartialArtifactConfig {
    pub project_root_dirname: Option<String>,
    pub auto_watch: Option<bool>,
    pub max_preview_bytes: Option<usize>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct PartialSkillsConfig {
    pub bundled_enabled: Option<bool>,
    pub extra_user_roots: Option<Vec<PathBuf>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct PartialRuntimeConfig {
    pub bridge_start_timeout_ms: Option<u64>,
    pub interrupt_grace_timeout_ms: Option<u64>,
    pub upstream_idle_shutdown_secs: Option<u64>,
}

pub fn parse_raw_config(raw: &str) -> Result<PartialAppConfig> {
    toml::from_str(raw).context("invalid TOML config")
}

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub cwd: PathBuf,
    pub install_root: PathBuf,
    pub app_home: PathBuf,
    pub user_config_path: PathBuf,
    pub user_logs_tui_dir: PathBuf,
    pub user_logs_app_server_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub codex_home_dir: PathBuf,
    pub generated_skills_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub db_dir: PathBuf,
    pub db_path: PathBuf,
    pub project_root: PathBuf,
    pub project_config_path: PathBuf,
    pub threads_root: PathBuf,
    pub bundled_skills_root: PathBuf,
    pub helper_python_src: PathBuf,
    pub helper_node_src: PathBuf,
}

impl AppPaths {
    pub fn for_cwd(cwd: &Path) -> Result<Self> {
        let home_dir = dirs::home_dir().ok_or_else(|| anyhow!("home directory is unavailable"))?;
        let install_root = resolve_install_root()?;
        let app_home = home_dir.join(".ml-intern-codex");
        let project_root = cwd.join(".ml-intern");
        let runtime_dir = app_home.join("runtime");
        Ok(Self {
            cwd: cwd.to_path_buf(),
            install_root: install_root.clone(),
            user_config_path: app_home.join("config.toml"),
            user_logs_tui_dir: app_home.join("logs/tui"),
            user_logs_app_server_dir: app_home.join("logs/app-server"),
            cache_dir: app_home.join("cache"),
            db_dir: app_home.join("db"),
            db_path: app_home.join("db/state.sqlite"),
            codex_home_dir: runtime_dir.join("codex-home"),
            generated_skills_dir: runtime_dir.join("generated-skills"),
            runtime_dir,
            project_config_path: project_root.join("config.toml"),
            threads_root: project_root.join("threads"),
            bundled_skills_root: install_root.join("skills/system"),
            helper_python_src: install_root.join("helpers/python/src"),
            helper_node_src: install_root.join("helpers/node/src"),
            project_root,
            app_home,
        })
    }

    pub fn ensure_base_layout(&self) -> Result<()> {
        for dir in [
            &self.app_home,
            &self.user_logs_tui_dir,
            &self.user_logs_app_server_dir,
            &self.runtime_dir,
            &self.codex_home_dir,
            &self.generated_skills_dir,
            &self.cache_dir,
            &self.db_dir,
            &self.project_root,
            &self.threads_root,
        ] {
            fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        Ok(())
    }

    pub fn ensure_bootstrap_files(&self, default_config: &AppConfig) -> Result<()> {
        if !self.user_config_path.exists() {
            let raw = toml::to_string_pretty(default_config)
                .context("failed to encode default config TOML")?;
            fs::write(&self.user_config_path, raw)
                .with_context(|| format!("failed to write {}", self.user_config_path.display()))?;
        }
        fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&self.db_path)
            .with_context(|| format!("failed to create {}", self.db_path.display()))?;
        Ok(())
    }

    pub fn thread_dir(&self, thread_id: mli_types::LocalThreadId) -> PathBuf {
        self.threads_root.join(thread_id.to_string())
    }

    pub fn thread_file(&self, thread_id: mli_types::LocalThreadId) -> PathBuf {
        self.thread_dir(thread_id).join("thread.json")
    }

    pub fn turns_dir(&self, thread_id: mli_types::LocalThreadId) -> PathBuf {
        self.thread_dir(thread_id).join("turns")
    }

    pub fn transcript_file(&self, thread_id: mli_types::LocalThreadId) -> PathBuf {
        self.thread_dir(thread_id).join("transcript.jsonl")
    }

    pub fn artifacts_dir(&self, thread_id: mli_types::LocalThreadId) -> PathBuf {
        self.thread_dir(thread_id).join("artifacts")
    }
}

fn resolve_install_root() -> Result<PathBuf> {
    if let Some(raw_root) = std::env::var_os("MLI_INSTALL_ROOT") {
        let root = PathBuf::from(raw_root);
        if has_install_assets(&root) {
            return Ok(root);
        }
        return Err(anyhow!(
            "MLI_INSTALL_ROOT {} does not contain bundled skills/helpers",
            root.display()
        ));
    }

    let mut candidates = Vec::new();
    if let Ok(current_exe) = std::env::current_exe() {
        let mut next = current_exe.parent().map(Path::to_path_buf);
        while let Some(candidate) = next {
            push_unique_candidate(&mut candidates, candidate.clone());
            next = candidate.parent().map(Path::to_path_buf);
        }
    }
    push_unique_candidate(&mut candidates, workspace_root_from_manifest()?);

    for candidate in candidates {
        if has_install_assets(&candidate) {
            return Ok(candidate);
        }
    }

    Err(anyhow!(
        "failed to locate ml-intern-codex install root containing skills/system and helpers/python/src"
    ))
}

fn workspace_root_from_manifest() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("failed to derive workspace root from CARGO_MANIFEST_DIR"))
}

fn has_install_assets(root: &Path) -> bool {
    root.join("skills/system").exists() && root.join("helpers/python/src").exists()
}

fn push_unique_candidate(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if candidates.iter().all(|existing| existing != &candidate) {
        candidates.push(candidate);
    }
}
