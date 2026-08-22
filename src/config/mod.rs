use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GutchConfig {
    #[serde(default)]
    pub repos: Vec<TrackedRepo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedRepo {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_sources_path: Option<String>,
}

impl GutchConfig {
    pub fn find_repo(&self, path: &str) -> Option<&TrackedRepo> {
        self.repos
            .iter()
            .find(|repo| repo_paths_match(&repo.path, path))
    }

    pub fn find_repo_mut(&mut self, path: &str) -> Option<&mut TrackedRepo> {
        self.repos
            .iter_mut()
            .find(|repo| repo_paths_match(&repo.path, path))
    }

    pub fn upsert_repo(&mut self, repo: TrackedRepo) {
        if let Some(existing) = self.find_repo_mut(&repo.path) {
            if repo.snapshot_sources_path.is_some() {
                existing.snapshot_sources_path = repo.snapshot_sources_path;
            }
        } else {
            self.insert_repo(repo);
        }
    }

    pub fn set_repo_snapshot_sources_path(
        &mut self,
        path: &str,
        snapshot_sources_path: Option<String>,
    ) {
        if let Some(existing) = self.find_repo_mut(path) {
            existing.snapshot_sources_path = snapshot_sources_path;
            return;
        }

        self.insert_repo(TrackedRepo {
            path: path.to_string(),
            snapshot_sources_path,
        });
    }

    fn insert_repo(&mut self, repo: TrackedRepo) {
        self.repos.push(repo);
        self.repos.sort_by(|a, b| a.path.cmp(&b.path));
    }
}

fn repo_paths_match(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

fn gutch_config_dir() -> Result<PathBuf, crate::Error> {
    hostapp_common::paths::resolve_launcher_paths()
        .map(|paths| paths.gutch_config_dir())
        .map_err(|error| crate::Error::manifest(error.to_string()))
}

fn gutch_config_path() -> Result<PathBuf, crate::Error> {
    hostapp_common::paths::resolve_launcher_paths()
        .map(|paths| paths.gutch_config_file())
        .map_err(|error| crate::Error::manifest(error.to_string()))
}

pub fn load_config() -> Result<GutchConfig, crate::Error> {
    let path = gutch_config_path()?;
    if !path.exists() {
        return Ok(GutchConfig::default());
    }

    let raw = crate::helpers::read_to_string(&path)?;
    let config: GutchConfig = crate::helpers::from_json_str(&raw)?;
    validate_config_paths(&config)?;
    Ok(config)
}

pub fn save_config(config: &GutchConfig) -> Result<(), crate::Error> {
    validate_config_paths(config)?;
    let dir = gutch_config_dir()?;
    crate::helpers::create_dir_all(&dir)?;

    let path = gutch_config_path()?;
    crate::helpers::write_json(&path, config)
}

fn validate_config_paths(config: &GutchConfig) -> Result<(), crate::Error> {
    for repo in &config.repos {
        if !std::path::Path::new(repo.path.trim()).is_absolute() {
            return Err(crate::Error::manifest(
                "tracked Gutch repository paths must be absolute",
            ));
        }
        if repo
            .snapshot_sources_path
            .as_deref()
            .is_some_and(|path| !std::path::Path::new(path.trim()).is_absolute())
        {
            return Err(crate::Error::manifest(
                "tracked Gutch snapshot source paths must be absolute",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{GutchConfig, TrackedRepo, gutch_config_dir};

    #[test]
    fn gutch_config_dir_lives_under_shared_root() {
        let paths = hostapp_common::paths::resolve_launcher_paths().expect("launcher paths");
        assert_eq!(
            gutch_config_dir().expect("gutch config dir"),
            paths.gutch_config_dir()
        );
    }

    #[test]
    fn set_repo_snapshot_sources_path_updates_existing_value() {
        let mut config = GutchConfig {
            repos: vec![TrackedRepo {
                path: "D:\\Repos\\game".to_string(),
                snapshot_sources_path: Some("D:\\Builds\\game".to_string()),
            }],
        };

        config.set_repo_snapshot_sources_path("  d:\\repos\\GAME  ", None);

        let tracked = config.find_repo("D:\\Repos\\game").unwrap();
        assert_eq!(tracked.snapshot_sources_path, None);
    }

    #[test]
    fn upsert_repo_inserts_new_and_updates_existing() {
        let mut config = GutchConfig { repos: vec![] };

        config.upsert_repo(TrackedRepo {
            path: "D:\\Repos\\game".to_string(),
            snapshot_sources_path: Some("D:\\Builds\\game".to_string()),
        });
        assert_eq!(config.repos.len(), 1);
        assert_eq!(
            config.repos[0].snapshot_sources_path,
            Some("D:\\Builds\\game".to_string())
        );

        config.upsert_repo(TrackedRepo {
            path: "D:\\Repos\\game".to_string(),
            snapshot_sources_path: Some("D:\\Builds\\game-v2".to_string()),
        });
        assert_eq!(config.repos.len(), 1);
        assert_eq!(
            config.repos[0].snapshot_sources_path,
            Some("D:\\Builds\\game-v2".to_string())
        );
    }

    #[test]
    fn inserted_repos_remain_sorted_by_path() {
        let mut config = GutchConfig { repos: vec![] };

        for path in ["D:\\Repos\\game", "C:\\Repos\\game"] {
            config.upsert_repo(TrackedRepo {
                path: path.to_string(),
                snapshot_sources_path: None,
            });
        }

        assert_eq!(config.repos[0].path, "C:\\Repos\\game");
        assert_eq!(config.repos[1].path, "D:\\Repos\\game");
    }
}
