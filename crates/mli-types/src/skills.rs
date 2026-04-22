use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    Bundled,
    User,
    Repo,
    Generated,
}

impl SkillScope {
    pub fn priority(self) -> usize {
        match self {
            Self::Repo => 0,
            Self::User => 1,
            Self::Bundled => 2,
            Self::Generated => 3,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillDescriptor {
    pub name: String,
    pub description: String,
    pub short_description: Option<String>,
    pub path: PathBuf,
    pub scope: SkillScope,
    pub enabled: bool,
}
