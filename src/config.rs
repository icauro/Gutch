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
    pub target_lookup_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<LinkedRepo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedRepo {
    pub product_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_user_id: Option<String>,
    pub remote_repo_id: i64,
    pub remote_repo_slug: String,
}

impl GutchConfig {
    pub fn find_repo(&self, path: &str) -> Option<&TrackedRepo> {
        let key = path.trim().to_ascii_lowercase();
        self.repos
            .iter()
            .find(|repo| repo.path.trim().to_ascii_lowercase() == key)
    }

    pub fn find_repo_mut(&mut self, path: &str) -> Option<&mut TrackedRepo> {
        let key = path.trim().to_ascii_lowercase();
        self.repos
            .iter_mut()
            .find(|repo| repo.path.trim().to_ascii_lowercase() == key)
    }

    pub fn upsert_repo(&mut self, repo: TrackedRepo) {
        let key = repo.path.trim().to_ascii_lowercase();
        if let Some(existing) = self
            .repos
            .iter_mut()
            .find(|r| r.path.trim().to_ascii_lowercase() == key)
        {
            if repo.target_lookup_path.is_some() {
                existing.target_lookup_path = repo.target_lookup_path;
            }
            if repo.linked.is_some() {
                existing.linked = repo.linked;
            }
        } else {
            self.repos.push(repo);
            self.repos.sort_by(|a, b| a.path.cmp(&b.path));
        }
    }

    pub fn set_repo_target_lookup_path(&mut self, path: &str, target_lookup_path: Option<String>) {
        if let Some(existing) = self.find_repo_mut(path) {
            existing.target_lookup_path = target_lookup_path;
            return;
        }

        self.repos.push(TrackedRepo {
            path: path.to_string(),
            target_lookup_path,
            linked: None,
        });
        self.repos.sort_by(|a, b| a.path.cmp(&b.path));
    }

    pub fn clear_repo_link(&mut self, path: &str) -> bool {
        if let Some(existing) = self.find_repo_mut(path) {
            existing.linked = None;
            return true;
        }
        false
    }

    pub fn remove_repo(&mut self, path: &str) {
        let key = path.trim().to_ascii_lowercase();
        self.repos
            .retain(|repo| repo.path.trim().to_ascii_lowercase() != key);
    }
}

fn config_root_dir() -> Result<PathBuf, crate::Error> {
    if let Some(config_root) = dirs::config_dir() {
        return Ok(config_root.join("Launcher"));
    }

    let home = dirs::home_dir()
        .ok_or_else(|| crate::Error::manifest("could not determine home directory"))?;
    Ok(home.join(".config").join("Launcher"))
}

fn legacy_gutch_config_dir() -> Result<PathBuf, crate::Error> {
    let home = dirs::home_dir()
        .ok_or_else(|| crate::Error::manifest("could not determine home directory"))?;
    Ok(home.join(".config").join("gutch"))
}

fn legacy_gutch_config_path() -> Result<PathBuf, crate::Error> {
    Ok(legacy_gutch_config_dir()?.join("config.json"))
}

pub fn gutch_config_dir() -> Result<PathBuf, crate::Error> {
    Ok(config_root_dir()?.join("gutch"))
}

pub fn gutch_config_path() -> Result<PathBuf, crate::Error> {
    Ok(gutch_config_dir()?.join("config.json"))
}

pub fn load_config() -> Result<GutchConfig, crate::Error> {
    let path = gutch_config_path()?;
    if path.exists() {
        let raw = crate::helpers::read_to_string(&path)?;
        return crate::helpers::from_json_str(&raw);
    }

    let legacy_path = legacy_gutch_config_path()?;
    if !legacy_path.exists() {
        return Ok(GutchConfig::default());
    }

    let raw = crate::helpers::read_to_string(&legacy_path)?;
    crate::helpers::from_json_str(&raw)
}

pub fn save_config(config: &GutchConfig) -> Result<(), crate::Error> {
    let dir = gutch_config_dir()?;
    crate::helpers::create_dir_all(&dir)?;

    let path = gutch_config_path()?;
    crate::helpers::write_json(&path, config)
}

#[cfg(test)]
mod tests {
    use super::{GutchConfig, LinkedRepo, TrackedRepo};

    #[test]
    fn set_repo_target_lookup_path_updates_and_clears_existing_value() {
        let mut config = GutchConfig {
            repos: vec![TrackedRepo {
                path: "D:\\Repos\\game".to_string(),
                target_lookup_path: Some("D:\\Builds\\game".to_string()),
                linked: Some(LinkedRepo {
                    product_id: "42".to_string(),
                    creator_user_id: None,
                    remote_repo_id: 7,
                    remote_repo_slug: "base".to_string(),
                }),
            }],
        };

        config.set_repo_target_lookup_path("D:\\Repos\\game", None);

        let tracked = config.find_repo("D:\\Repos\\game").unwrap();
        assert_eq!(tracked.target_lookup_path, None);
        assert_eq!(tracked.linked.as_ref().unwrap().remote_repo_id, 7);
    }

    #[test]
    fn clear_repo_link_preserves_other_repo_metadata() {
        let mut config = GutchConfig {
            repos: vec![TrackedRepo {
                path: "D:\\Repos\\game".to_string(),
                target_lookup_path: Some("D:\\Builds\\game".to_string()),
                linked: Some(LinkedRepo {
                    product_id: "42".to_string(),
                    creator_user_id: Some("9".to_string()),
                    remote_repo_id: 7,
                    remote_repo_slug: "base".to_string(),
                }),
            }],
        };

        assert!(config.clear_repo_link("D:\\Repos\\game"));

        let tracked = config.find_repo("D:\\Repos\\game").unwrap();
        assert_eq!(
            tracked.target_lookup_path.as_deref(),
            Some("D:\\Builds\\game")
        );
        assert!(tracked.linked.is_none());
    }
}
