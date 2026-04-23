use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use mli_config::{AppConfig, AppPaths};

#[derive(Clone, Debug)]
pub struct CodexHomeOverlay {
    pub root: PathBuf,
    pub agents_path: PathBuf,
    pub generated_skills_root: PathBuf,
}

pub trait RuntimeEnvironmentService {
    fn resolve_codex_bin(&self) -> Result<PathBuf>;
    fn validate_codex_version(&self) -> Result<String>;
    fn prepare_codex_home_overlay(&self, cwd: &Path) -> Result<CodexHomeOverlay>;
}

#[derive(Clone)]
pub struct LocalRuntimeEnvironmentService {
    config: AppConfig,
    paths: AppPaths,
}

impl LocalRuntimeEnvironmentService {
    pub fn new(config: AppConfig, paths: AppPaths) -> Self {
        Self { config, paths }
    }
}

impl RuntimeEnvironmentService for LocalRuntimeEnvironmentService {
    fn resolve_codex_bin(&self) -> Result<PathBuf> {
        let configured = &self.config.codex.bin_path;
        if configured.components().count() > 1 {
            if configured.exists() {
                return Ok(configured.clone());
            }
            return Err(anyhow!(
                "configured codex binary {} does not exist",
                configured.display()
            ));
        }
        resolve_bin_on_path(configured)
            .with_context(|| format!("failed to locate {} on PATH", configured.display()))
    }

    fn validate_codex_version(&self) -> Result<String> {
        let codex_bin = self.resolve_codex_bin()?;
        let output = Command::new(&codex_bin)
            .arg("--version")
            .output()
            .with_context(|| format!("failed to run {} --version", codex_bin.display()))?;
        if !output.status.success() {
            return Err(anyhow!(
                "{} --version exited with status {}",
                codex_bin.display(),
                output.status
            ));
        }
        let stdout =
            String::from_utf8(output.stdout).context("codex --version output is not utf-8")?;
        let version = stdout
            .split_whitespace()
            .last()
            .ok_or_else(|| anyhow!("unable to parse codex version from `{stdout}`"))?
            .to_owned();
        Ok(version)
    }

    fn prepare_codex_home_overlay(&self, cwd: &Path) -> Result<CodexHomeOverlay> {
        fs::create_dir_all(&self.paths.codex_home_dir)
            .with_context(|| format!("failed to create {}", self.paths.codex_home_dir.display()))?;
        fs::create_dir_all(&self.paths.generated_skills_dir).with_context(|| {
            format!(
                "failed to create generated skills dir {}",
                self.paths.generated_skills_dir.display()
            )
        })?;
        let user_codex_home = user_codex_home()?;
        sync_overlay_config(&user_codex_home, &self.paths.codex_home_dir, cwd)?;
        sync_optional_file(
            &user_codex_home.join("auth.json"),
            &self.paths.codex_home_dir.join("auth.json"),
        )?;
        sync_optional_file(
            &user_codex_home.join("installation_id"),
            &self.paths.codex_home_dir.join("installation_id"),
        )?;
        sync_optional_file(
            &user_codex_home.join("version.json"),
            &self.paths.codex_home_dir.join("version.json"),
        )?;
        let agents_path = self.paths.codex_home_dir.join("AGENTS.md");
        let generated_skill_dir = self
            .paths
            .generated_skills_dir
            .join("runtime-artifact-contract");
        let helper_python_src = self.paths.helper_python_src.display();
        fs::create_dir_all(&generated_skill_dir)
            .with_context(|| format!("failed to create {}", generated_skill_dir.display()))?;
        fs::write(
            &agents_path,
            compose_overlay_agents(&user_codex_home, cwd, &self.paths),
        )
        .with_context(|| format!("failed to write {}", agents_path.display()))?;
        fs::write(
            generated_skill_dir.join("SKILL.md"),
            format!(
                "---\nname: runtime-artifact-contract\ndescription: Write artifacts into the canonical ml-intern-codex thread artifact tree and emit artifact.json manifests.\nmetadata:\n  short-description: Persist canonical runtime artifacts\n---\n\n# runtime-artifact-contract\n\nWrite artifacts into `<cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id>/` and always emit `artifact.json`.\n\n## Concrete helper lanes\n\n- dataset audit: `PYTHONPATH={helper_python_src} python3 -m mli_helpers.artifacts.write_dataset_audit --artifact-dir <cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id> --turn-id <local-turn-id> ...`\n- paper report: `PYTHONPATH={helper_python_src} python3 -m mli_helpers.artifacts.write_paper_report --artifact-dir <cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id> --turn-id <local-turn-id> ...`\n- job snapshot: `PYTHONPATH={helper_python_src} python3 -m mli_helpers.artifacts.write_job_snapshot --artifact-dir <cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id> --turn-id <local-turn-id> ...`\n"
            ),
        )
        .with_context(|| "failed to write generated runtime skill".to_owned())?;
        sync_repo_skill_links(
            cwd,
            &self.paths.bundled_skills_root,
            &self.paths.generated_skills_dir,
        )?;
        Ok(CodexHomeOverlay {
            root: self.paths.codex_home_dir.clone(),
            agents_path,
            generated_skills_root: self.paths.generated_skills_dir.clone(),
        })
    }
}

fn user_codex_home() -> Result<PathBuf> {
    let home_dir = dirs::home_dir().ok_or_else(|| anyhow!("home directory is unavailable"))?;
    Ok(home_dir.join(".codex"))
}

fn sync_overlay_config(user_codex_home: &Path, overlay_home: &Path, cwd: &Path) -> Result<()> {
    let mut contents = fs::read_to_string(user_codex_home.join("config.toml")).unwrap_or_default();
    let project_block = format!("[projects.\"{}\"]", cwd.display());
    if !contents.contains(&project_block) {
        if !contents.trim().is_empty() {
            contents.push('\n');
            contents.push('\n');
        }
        contents.push_str(&project_block);
        contents.push('\n');
        contents.push_str("trust_level = \"trusted\"\n");
    }
    fs::write(overlay_home.join("config.toml"), contents).with_context(|| {
        format!(
            "failed to write {}",
            overlay_home.join("config.toml").display()
        )
    })
}

fn sync_optional_file(source: &Path, destination: &Path) -> Result<()> {
    if source.exists() {
        if destination.exists() {
            fs::remove_file(destination).with_context(|| {
                format!(
                    "failed to replace existing overlay file {}",
                    destination.display()
                )
            })?;
        }
        fs::copy(source, destination).with_context(|| {
            format!(
                "failed to copy user state {} into overlay {}",
                source.display(),
                destination.display()
            )
        })?;
    } else if destination.exists() {
        fs::remove_file(destination).with_context(|| {
            format!(
                "failed to remove stale overlay file {} after source disappeared",
                destination.display()
            )
        })?;
    }
    Ok(())
}

fn compose_overlay_agents(user_codex_home: &Path, cwd: &Path, paths: &AppPaths) -> String {
    let runtime_section = format!(
        "# ml-intern-codex runtime overlay\n\n- cwd: {}\n- install root: {}\n- artifact root: {}\n- generated skills: {}\n- helper python src: {}\n- helper node src: {}\n",
        cwd.display(),
        paths.install_root.display(),
        paths.project_root.display(),
        paths.generated_skills_dir.display(),
        paths.helper_python_src.display(),
        paths.helper_node_src.display(),
    );
    match fs::read_to_string(user_codex_home.join("AGENTS.md")) {
        Ok(user_agents) if !user_agents.trim().is_empty() => {
            format!("{user_agents}\n\n---\n\n{runtime_section}")
        }
        _ => runtime_section,
    }
}

fn sync_repo_skill_links(cwd: &Path, bundled_root: &Path, generated_root: &Path) -> Result<()> {
    let repo_skills_root = cwd.join(".agents").join("skills");
    fs::create_dir_all(&repo_skills_root)
        .with_context(|| format!("failed to create repo skill root {}", repo_skills_root.display()))?;
    sync_linked_skill_dirs(bundled_root, &repo_skills_root)?;
    sync_linked_skill_dirs(generated_root, &repo_skills_root)?;
    Ok(())
}

fn sync_linked_skill_dirs(source_root: &Path, destination_root: &Path) -> Result<()> {
    if !source_root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(source_root)
        .with_context(|| format!("failed to read source skill root {}", source_root.display()))?
    {
        let entry = entry.with_context(|| {
            format!(
                "failed to iterate source skill root {}",
                source_root.display()
            )
        })?;
        let source_path = entry.path();
        if !source_path.is_dir() {
            continue;
        }
        let destination = destination_root.join(entry.file_name());
        sync_skill_link(&source_path, &destination)?;
    }
    Ok(())
}

fn sync_skill_link(source: &Path, destination: &Path) -> Result<()> {
    if let Ok(target) = fs::read_link(destination)
        && target == source
    {
        return Ok(());
    }
    if destination.exists() || destination.symlink_metadata().is_ok() {
        let metadata = destination.symlink_metadata().with_context(|| {
            format!(
                "failed to inspect existing repo skill path {}",
                destination.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            fs::remove_file(destination).with_context(|| {
                format!("failed to replace repo skill symlink {}", destination.display())
            })?;
        } else {
            // Preserve any real repo-managed skill directory/file the user already created.
            return Ok(());
        }
    }
    create_dir_symlink(source, destination)
}

#[cfg(unix)]
fn create_dir_symlink(source: &Path, destination: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source, destination).with_context(|| {
        format!(
            "failed to symlink repo skill {} -> {}",
            destination.display(),
            source.display()
        )
    })
}

#[cfg(not(unix))]
fn create_dir_symlink(source: &Path, destination: &Path) -> Result<()> {
    copy_dir_recursive(source, destination)
}

#[cfg(not(unix))]
fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("failed to create copied skill dir {}", destination.display()))?;
    for entry in fs::read_dir(source)
        .with_context(|| format!("failed to read copied skill dir {}", source.display()))?
    {
        let entry = entry.with_context(|| {
            format!("failed to iterate copied skill dir {}", source.display())
        })?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir_recursive(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "failed to copy skill file {} -> {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn resolve_bin_on_path(command_name: &Path) -> Result<PathBuf> {
    let path_env = std::env::var_os("PATH").ok_or_else(|| anyhow!("PATH is not set"))?;
    for directory in std::env::split_paths(&path_env) {
        let candidate = directory.join(command_name);
        if is_executable_file(&candidate) {
            return Ok(candidate);
        }
    }
    Err(anyhow!("{} was not found in PATH", command_name.display()))
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    use super::{LocalRuntimeEnvironmentService, RuntimeEnvironmentService, resolve_bin_on_path};
    use mli_config::{AppConfig, AppPaths};
    use mli_types::utc_now;

    struct PathGuard {
        original: Option<OsString>,
    }

    struct HomeGuard {
        original: Option<OsString>,
    }

    fn test_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|error| panic!("lock runtime environment tests: {error}"))
    }

    impl PathGuard {
        fn set(value: &std::ffi::OsStr) -> Self {
            let original = std::env::var_os("PATH");
            // Tests only mutate PATH inside the current process and restore it on drop.
            unsafe { std::env::set_var("PATH", value) };
            Self { original }
        }
    }

    impl Drop for PathGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => {
                    // Restore the caller PATH after the test finishes.
                    unsafe { std::env::set_var("PATH", value) };
                }
                None => {
                    unsafe { std::env::remove_var("PATH") };
                }
            }
        }
    }

    impl HomeGuard {
        fn set(value: &Path) -> Self {
            let original = std::env::var_os("HOME");
            unsafe { std::env::set_var("HOME", value) };
            Self { original }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => unsafe { std::env::set_var("HOME", value) },
                None => unsafe { std::env::remove_var("HOME") },
            }
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mli-runtime-env-{name}-{}-{}",
            std::process::id(),
            utc_now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create temp dir: {error}"));
        path
    }

    #[cfg(unix)]
    fn make_executable(path: &PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path)
            .unwrap_or_else(|error| panic!("stat candidate: {error}"))
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)
            .unwrap_or_else(|error| panic!("chmod candidate: {error}"));
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &PathBuf) {}

    fn app_paths(root: &Path) -> AppPaths {
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

    fn fake_codex(root: &Path, version: &str) -> PathBuf {
        let codex = root.join("bin/codex");
        fs::create_dir_all(codex.parent().unwrap_or(root))
            .unwrap_or_else(|error| panic!("create fake codex dir: {error}"));
        fs::write(&codex, format!("#!/bin/sh\necho 'codex-cli {version}'\n"))
            .unwrap_or_else(|error| panic!("write fake codex: {error}"));
        make_executable(&codex);
        codex
    }

    #[test]
    fn resolve_bin_on_path_finds_first_executable_match() {
        let _lock = test_lock();
        let first_dir = temp_dir("first");
        let second_dir = temp_dir("second");
        let candidate = second_dir.join("codex");
        fs::write(&candidate, "#!/bin/sh\nexit 0\n")
            .unwrap_or_else(|error| panic!("write candidate: {error}"));
        make_executable(&candidate);
        let joined = std::env::join_paths([first_dir, second_dir])
            .unwrap_or_else(|error| panic!("join PATH: {error}"));
        let _guard = PathGuard::set(joined.as_os_str());

        let resolved = resolve_bin_on_path(Path::new("codex"))
            .unwrap_or_else(|error| panic!("resolve codex: {error}"));

        assert_eq!(resolved, candidate);
    }

    #[test]
    fn resolve_bin_on_path_errors_when_missing() {
        let _lock = test_lock();
        let empty_dir = temp_dir("missing");
        let joined =
            std::env::join_paths([empty_dir]).unwrap_or_else(|error| panic!("join PATH: {error}"));
        let _guard = PathGuard::set(joined.as_os_str());

        let error = resolve_bin_on_path(Path::new("codex"))
            .err()
            .unwrap_or_else(|| panic!("expected missing codex error"));

        assert!(error.to_string().contains("codex"));
    }

    #[test]
    fn validate_codex_version_returns_detected_version() {
        let _lock = test_lock();
        let root = temp_dir("version-ok");
        let paths = app_paths(&root);
        let fake_codex = fake_codex(&root, "0.120.0");
        let mut config = AppConfig::default();
        config.codex.bin_path = fake_codex;
        let service = LocalRuntimeEnvironmentService::new(config, paths);

        let version = service
            .validate_codex_version()
            .unwrap_or_else(|error| panic!("validate version: {error}"));

        assert_eq!(version, "0.120.0");
    }

    #[test]
    fn validate_codex_version_allows_newer_version() {
        let _lock = test_lock();
        let root = temp_dir("version-mismatch");
        let paths = app_paths(&root);
        let fake_codex = fake_codex(&root, "0.121.0");
        let mut config = AppConfig::default();
        config.codex.bin_path = fake_codex;
        let service = LocalRuntimeEnvironmentService::new(config, paths);

        let version = service
            .validate_codex_version()
            .unwrap_or_else(|error| panic!("validate newer version: {error}"));

        assert_eq!(version, "0.121.0");
    }

    #[test]
    fn prepare_codex_home_overlay_writes_runtime_contract() {
        let _lock = test_lock();
        let root = temp_dir("overlay");
        let _home = HomeGuard::set(&root.join("fake-home"));
        let paths = app_paths(&root);
        let bundled_skill_dir = paths.bundled_skills_root.join("ml-runtime-conventions");
        fs::create_dir_all(&bundled_skill_dir)
            .unwrap_or_else(|error| panic!("create bundled skill dir: {error}"));
        fs::write(
            bundled_skill_dir.join("SKILL.md"),
            "# ml-runtime-conventions\n\nRuntime conventions.\n",
        )
        .unwrap_or_else(|error| panic!("write bundled skill file: {error}"));
        let service = LocalRuntimeEnvironmentService::new(AppConfig::default(), paths.clone());

        let overlay = service
            .prepare_codex_home_overlay(&paths.cwd)
            .unwrap_or_else(|error| panic!("prepare overlay: {error}"));

        let agents = fs::read_to_string(&overlay.agents_path)
            .unwrap_or_else(|error| panic!("read agents file: {error}"));
        let skill = fs::read_to_string(
            overlay
                .generated_skills_root
                .join("runtime-artifact-contract/SKILL.md"),
        )
        .unwrap_or_else(|error| panic!("read generated skill: {error}"));

        assert!(agents.contains("ml-intern-codex runtime overlay"));
        assert!(skill.contains("write_dataset_audit"));
        assert!(skill.contains("write_paper_report"));
        assert!(skill.contains("write_job_snapshot"));
        assert_eq!(overlay.root, paths.codex_home_dir);
        assert!(paths.cwd.join(".agents/skills/ml-runtime-conventions").exists());
        assert!(
            paths.cwd
                .join(".agents/skills/runtime-artifact-contract")
                .exists()
        );
    }

    #[test]
    fn prepare_codex_home_overlay_copies_user_auth_and_config() {
        let _lock = test_lock();
        let root = temp_dir("overlay-user-state");
        let fake_home = root.join("fake-home");
        let _home = HomeGuard::set(&fake_home);
        let user_codex_home = fake_home.join(".codex");
        fs::create_dir_all(&user_codex_home)
            .unwrap_or_else(|error| panic!("create user codex home: {error}"));
        fs::write(
            user_codex_home.join("config.toml"),
            "model = \"gpt-5.4\"\nmodel_provider = \"crs\"\n",
        )
        .unwrap_or_else(|error| panic!("write user config: {error}"));
        fs::write(
            user_codex_home.join("auth.json"),
            "{\"token\":\"secret\"}\n",
        )
        .unwrap_or_else(|error| panic!("write user auth: {error}"));
        fs::write(user_codex_home.join("AGENTS.md"), "# user agents\n")
            .unwrap_or_else(|error| panic!("write user agents: {error}"));

        let paths = app_paths(&root);
        let service = LocalRuntimeEnvironmentService::new(AppConfig::default(), paths.clone());

        let overlay = service
            .prepare_codex_home_overlay(&paths.cwd)
            .unwrap_or_else(|error| panic!("prepare overlay with user state: {error}"));

        let overlay_config = fs::read_to_string(paths.codex_home_dir.join("config.toml"))
            .unwrap_or_else(|error| panic!("read overlay config: {error}"));
        let overlay_auth = fs::read_to_string(paths.codex_home_dir.join("auth.json"))
            .unwrap_or_else(|error| panic!("read overlay auth: {error}"));
        let overlay_agents = fs::read_to_string(&overlay.agents_path)
            .unwrap_or_else(|error| panic!("read overlay agents: {error}"));

        assert!(overlay_config.contains("model_provider = \"crs\""));
        assert!(overlay_config.contains("[projects."));
        assert!(overlay_auth.contains("\"token\":\"secret\""));
        assert!(overlay_agents.contains("# user agents"));
        assert!(overlay_agents.contains("ml-intern-codex runtime overlay"));
    }
}
