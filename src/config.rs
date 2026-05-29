use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GutchConfig {
    #[serde(default)]
    pub repos: Vec<TrackedRepo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedRepo {
    pub path: String,
    pub content_repo_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_repo_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_repo_slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_targets_path: Option<String>,
}

impl Default for GutchConfig {
    fn default() -> Self {
        Self {
            repos: Vec::new(),
        }
    }
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
            existing.content_repo_id = repo.content_repo_id;
            existing.product_id = repo.product_id.or(existing.product_id.clone());
            existing.creator_user_id = repo.creator_user_id.or(existing.creator_user_id.clone());
            existing.remote_repo_id = repo.remote_repo_id.or(existing.remote_repo_id);
            existing.remote_repo_slug = repo.remote_repo_slug.or(existing.remote_repo_slug.clone());
            if repo.source_targets_path.is_some() {
                existing.source_targets_path = repo.source_targets_path;
            }
        } else {
            self.repos.push(repo);
            self.repos
                .sort_by(|a, b| a.path.cmp(&b.path));
        }
    }

    pub fn remove_repo(&mut self, path: &str) {
        let key = path.trim().to_ascii_lowercase();
        self.repos
            .retain(|repo| repo.path.trim().to_ascii_lowercase() != key);
    }
}

pub fn gutch_config_dir() -> Result<PathBuf, crate::Error> {
    let home = dirs::home_dir()
        .ok_or_else(|| crate::Error::manifest("could not determine home directory"))?;
    Ok(home.join(".config").join("gutch"))
}

pub fn gutch_config_path() -> Result<PathBuf, crate::Error> {
    Ok(gutch_config_dir()?.join("config.json"))
}

pub fn load_config() -> Result<GutchConfig, crate::Error> {
    let path = gutch_config_path()?;
    if !path.exists() {
        return Ok(GutchConfig::default());
    }

    let raw = std::fs::read_to_string(&path)
        .map_err(|err| crate::Error::manifest(format!("failed to read gutch config: {}", err)))?;
    serde_json::from_str(&raw)
        .map_err(|err| crate::Error::manifest(format!("failed to parse gutch config: {}", err)))
}

pub fn save_config(config: &GutchConfig) -> Result<(), crate::Error> {
    let dir = gutch_config_dir()?;
    std::fs::create_dir_all(&dir)
        .map_err(|err| crate::Error::manifest(format!("failed to create gutch config dir: {}", err)))?;

    let path = gutch_config_path()?;
    let raw = serde_json::to_string_pretty(config)
        .map_err(|err| crate::Error::manifest(format!("failed to serialize gutch config: {}", err)))?;
    std::fs::write(&path, raw)
        .map_err(|err| crate::Error::manifest(format!("failed to write gutch config: {}", err)))?;
    Ok(())
}
