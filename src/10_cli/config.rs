//! CLI-local tracked repositories and configuration persistence.

use std::path::Path;

use crate::filesystem;
use crate::filesystem::from_json_str;
use crate::filesystem::write_json;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CliConfig {
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

impl CliConfig {
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

// Only Windows paths are case-insensitive; elsewhere `/a/Game` and `/a/game` are distinct repos.
fn repo_paths_match(left: &str, right: &str) -> bool {
    let (left, right) = (left.trim(), right.trim());
    if cfg!(windows) {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

pub fn load_config(path: &Path) -> Result<CliConfig, crate::Error> {
    if !path.exists() {
        return Ok(CliConfig::default());
    }

    let raw = filesystem::read_to_string(path)?;
    let config: CliConfig = from_json_str(&raw)?;
    validate_config_paths(&config)?;
    Ok(config)
}

pub fn save_config(path: &Path, config: &CliConfig) -> Result<(), crate::Error> {
    validate_config_paths(config)?;
    if let Some(dir) = path.parent() {
        filesystem::create_dir_all(dir)?;
    }
    write_json(path, config)
}

fn validate_config_paths(config: &CliConfig) -> Result<(), crate::Error> {
    for repo in &config.repos {
        if !Path::new(repo.path.trim()).is_absolute() {
            return Err(crate::Error::invalid_input(
                "tracked Gutch repo paths must be absolute",
            ));
        }
        if repo
            .snapshot_sources_path
            .as_deref()
            .is_some_and(|path| !Path::new(path.trim()).is_absolute())
        {
            return Err(crate::Error::invalid_input(
                "tracked Gutch snapshot source paths must be absolute",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::CliConfig;
    use super::TrackedRepo;

    #[test]
    fn config_round_trips_at_explicit_path() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("gutch.json");
        let config = CliConfig {
            repos: vec![TrackedRepo {
                path: if cfg!(windows) {
                    "D:\\Repos\\game".to_string()
                } else {
                    "/repos/game".to_string()
                },
                snapshot_sources_path: None,
            }],
        };

        super::save_config(&path, &config).expect("save config");
        assert_eq!(
            super::load_config(&path).expect("load config").repos.len(),
            1
        );
    }

    #[cfg(windows)]
    #[test]
    fn set_repo_snapshot_sources_path_updates_existing_value() {
        let mut config = CliConfig {
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
        let mut config = CliConfig { repos: vec![] };

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
    fn repo_path_case_matters_only_on_windows() {
        let mut config = CliConfig { repos: vec![] };
        for path in ["/repos/Game", "/repos/game"] {
            config.upsert_repo(TrackedRepo {
                path: path.to_string(),
                snapshot_sources_path: None,
            });
        }

        let expected_repo_count = if cfg!(windows) { 1 } else { 2 };
        assert_eq!(config.repos.len(), expected_repo_count);
    }

    #[test]
    fn inserted_repos_remain_sorted_by_path() {
        let mut config = CliConfig { repos: vec![] };

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
