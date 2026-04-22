use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use mli_types::{SkillDescriptor, SkillScope};
use walkdir::WalkDir;

#[derive(Clone, Debug)]
pub struct SkillDiscoveryRoot {
    pub path: PathBuf,
    pub scope: SkillScope,
}

pub fn discover_skills(roots: &[SkillDiscoveryRoot]) -> Result<Vec<SkillDescriptor>> {
    let mut seen = HashSet::new();
    let mut skills = Vec::new();

    for root in roots {
        if !root.path.exists() {
            continue;
        }
        for entry in WalkDir::new(&root.path)
            .follow_links(true)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file() && entry.file_name() == "SKILL.md")
        {
            let skill_path = entry.path().to_path_buf();
            let canonical = fs::canonicalize(&skill_path).unwrap_or(skill_path.clone());
            if !seen.insert(canonical) {
                continue;
            }
            let descriptor = parse_skill_file(&skill_path, root.scope)
                .with_context(|| format!("failed to parse skill {}", skill_path.display()))?;
            skills.push(descriptor);
        }
    }

    skills.sort_by(|left, right| {
        left.scope
            .priority()
            .cmp(&right.scope.priority())
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(skills)
}

pub fn parse_skill_file(path: &Path, scope: SkillScope) -> Result<SkillDescriptor> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read skill {}", path.display()))?;
    let title = parse_skill_title(&raw).unwrap_or_else(|| {
        path.parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("unnamed-skill")
            .to_owned()
    });
    let description =
        parse_skill_description(&raw).unwrap_or_else(|| "No description provided.".to_owned());
    let short_description = description
        .split('.')
        .next()
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned);

    Ok(SkillDescriptor {
        name: title,
        description,
        short_description,
        path: path.to_path_buf(),
        scope,
        enabled: true,
    })
}

fn parse_skill_title(raw: &str) -> Option<String> {
    raw.lines()
        .find_map(|line| line.strip_prefix("# ").map(|value| value.trim().to_owned()))
}

fn parse_skill_description(raw: &str) -> Option<String> {
    let mut lines = raw.lines().peekable();
    for line in lines.by_ref() {
        if line.trim_start().starts_with('#') {
            break;
        }
    }

    let mut paragraph = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !paragraph.is_empty() {
                break;
            }
            continue;
        }
        if trimmed.starts_with('#') {
            break;
        }
        paragraph.push(trimmed);
    }

    if paragraph.is_empty() {
        None
    } else {
        Some(paragraph.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("mli-skill-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create temp dir: {error}"));
        path
    }

    #[test]
    fn parse_skill_file_extracts_title_and_description() {
        let root = temp_dir("parse");
        let skill_dir = root.join("sample-skill");
        fs::create_dir_all(&skill_dir).unwrap_or_else(|error| panic!("create skill dir: {error}"));
        let skill_file = skill_dir.join("SKILL.md");
        fs::write(
            &skill_file,
            "# sample-skill

Use this skill for deterministic testing.

## Notes
- keep short
",
        )
        .unwrap_or_else(|error| panic!("write skill file: {error}"));

        let parsed = match parse_skill_file(&skill_file, SkillScope::Bundled) {
            Ok(parsed) => parsed,
            Err(error) => panic!("parse skill: {error}"),
        };
        assert_eq!(parsed.name, "sample-skill");
        assert_eq!(
            parsed.description,
            "Use this skill for deterministic testing."
        );
    }

    #[test]
    fn discover_skills_orders_by_scope_priority() {
        let root = temp_dir("roots");
        let repo_root = root.join("repo");
        let bundled_root = root.join("bundled");
        fs::create_dir_all(repo_root.join("alpha"))
            .unwrap_or_else(|error| panic!("create repo root: {error}"));
        fs::create_dir_all(bundled_root.join("beta"))
            .unwrap_or_else(|error| panic!("create bundled root: {error}"));
        fs::write(
            repo_root.join("alpha/SKILL.md"),
            "# alpha

Repo skill.
",
        )
        .unwrap_or_else(|error| panic!("write repo skill: {error}"));
        fs::write(
            bundled_root.join("beta/SKILL.md"),
            "# beta

Bundled skill.
",
        )
        .unwrap_or_else(|error| panic!("write bundled skill: {error}"));

        let skills = match discover_skills(&[
            SkillDiscoveryRoot {
                path: bundled_root,
                scope: SkillScope::Bundled,
            },
            SkillDiscoveryRoot {
                path: repo_root,
                scope: SkillScope::Repo,
            },
        ]) {
            Ok(skills) => skills,
            Err(error) => panic!("discover skills: {error}"),
        };

        assert_eq!(skills[0].scope, SkillScope::Repo);
        assert_eq!(skills[0].name, "alpha");
    }
}
