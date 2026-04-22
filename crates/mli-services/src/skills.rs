use std::path::Path;

use anyhow::Result;
use dirs::home_dir;
use mli_config::{AppConfig, AppPaths};
use mli_skills::{SkillDiscoveryRoot, discover_skills};
use mli_types::{SkillDescriptor, SkillScope};

pub trait SkillService {
    fn list_skills(&self, cwd: Option<&Path>, force_reload: bool) -> Result<Vec<SkillDescriptor>>;
}

#[derive(Clone)]
pub struct LocalSkillService {
    config: AppConfig,
    paths: AppPaths,
}

impl LocalSkillService {
    pub fn new(config: AppConfig, paths: AppPaths) -> Self {
        Self { config, paths }
    }

    fn skill_roots(&self, cwd: &Path) -> Vec<SkillDiscoveryRoot> {
        let mut roots = Vec::new();
        roots.push(SkillDiscoveryRoot {
            path: cwd.join(".agents/skills"),
            scope: SkillScope::Repo,
        });
        if let Some(home) = home_dir() {
            roots.push(SkillDiscoveryRoot {
                path: home.join(".agents/skills"),
                scope: SkillScope::User,
            });
        }
        for extra_root in &self.config.skills.extra_user_roots {
            roots.push(SkillDiscoveryRoot {
                path: extra_root.clone(),
                scope: SkillScope::User,
            });
        }
        if self.config.skills.bundled_enabled {
            roots.push(SkillDiscoveryRoot {
                path: self.paths.bundled_skills_root.clone(),
                scope: SkillScope::Bundled,
            });
        }
        roots.push(SkillDiscoveryRoot {
            path: self.paths.generated_skills_dir.clone(),
            scope: SkillScope::Generated,
        });
        roots
    }
}

impl SkillService for LocalSkillService {
    fn list_skills(&self, cwd: Option<&Path>, _force_reload: bool) -> Result<Vec<SkillDescriptor>> {
        let cwd = cwd.unwrap_or(&self.paths.cwd);
        discover_skills(&self.skill_roots(cwd))
    }
}
