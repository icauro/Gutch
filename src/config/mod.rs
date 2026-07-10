use std::env;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const LAUNCHER_CONFIG_DIR_ENV: &str = "LAUNCHER_CONFIG_DIR";

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
    pub target_sources_path: Option<String>,
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
            if repo.target_sources_path.is_some() {
                existing.target_sources_path = repo.target_sources_path;
            }
        } else {
            self.repos.push(repo);
            self.repos.sort_by(|a, b| a.path.cmp(&b.path));
        }
    }

    pub fn set_repo_target_sources_path(
        &mut self,
        path: &str,
        target_sources_path: Option<String>,
    ) {
        if let Some(existing) = self.find_repo_mut(path) {
            existing.target_sources_path = target_sources_path;
            return;
        }

        self.repos.push(TrackedRepo {
            path: path.to_string(),
            target_sources_path,
        });
        self.repos.sort_by(|a, b| a.path.cmp(&b.path));
    }

    pub fn remove_repo(&mut self, path: &str) {
        let key = path.trim().to_ascii_lowercase();
        self.repos
            .retain(|repo| repo.path.trim().to_ascii_lowercase() != key);
    }
}

/// Shared Launcher desktop data root used by the app, updater, and Gutch CLI.
///
/// Layout:
/// - `{root}/config.json` — launcher session and settings
/// - `{root}/logs/` — launcher logs
/// - `{root}/gutch/` — Gutch engine state (`config.json`, etc.)
pub fn patron_launcher_config_root() -> Result<PathBuf, crate::Error> {
    if let Some(override_dir) = env::var(LAUNCHER_CONFIG_DIR_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return Ok(override_dir);
    }

    let home = dirs::home_dir()
        .ok_or_else(|| crate::Error::manifest("could not determine home directory"))?;
    Ok(home.join(".config").join("Launcher"))
}

fn gutch_config_dir() -> Result<PathBuf, crate::Error> {
    Ok(patron_launcher_config_root()?.join("gutch"))
}

fn gutch_config_path() -> Result<PathBuf, crate::Error> {
    Ok(gutch_config_dir()?.join("config.json"))
}

pub fn load_config() -> Result<GutchConfig, crate::Error> {
    let path = gutch_config_path()?;
    if !path.exists() {
        return Ok(GutchConfig::default());
    }

    let raw = crate::helpers::read_to_string(&path)?;
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
    use std::sync::{Mutex, OnceLock};

    use super::{GutchConfig, TrackedRepo, gutch_config_dir, patron_launcher_config_root};

    fn env_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn lock_env_test() -> std::sync::MutexGuard<'static, ()> {
        env_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn patron_launcher_config_root_honors_override_env() {
        let _guard = lock_env_test();
        let temp_dir =
            std::env::temp_dir().join(format!("launcher-config-root-{}", std::process::id()));
        unsafe {
            std::env::set_var(super::LAUNCHER_CONFIG_DIR_ENV, &temp_dir);
        }
        assert_eq!(
            patron_launcher_config_root().expect("config root"),
            temp_dir
        );
        unsafe {
            std::env::remove_var(super::LAUNCHER_CONFIG_DIR_ENV);
        }
    }

    #[test]
    fn gutch_config_dir_lives_under_shared_root() {
        let _guard = lock_env_test();
        let temp_dir = std::env::temp_dir().join(format!(
            "launcher-gutch-config-{}",
            std::process::id()
        ));
        unsafe {
            std::env::set_var(super::LAUNCHER_CONFIG_DIR_ENV, &temp_dir);
        }
        assert_eq!(
            gutch_config_dir().expect("gutch config dir"),
            temp_dir.join("gutch")
        );
        unsafe {
            std::env::remove_var(super::LAUNCHER_CONFIG_DIR_ENV);
        }
    }

    #[test]
    fn set_repo_target_sources_path_updates_existing_value() {
        let mut config = GutchConfig {
            repos: vec![TrackedRepo {
                path: "D:\\Repos\\game".to_string(),
                target_sources_path: Some("D:\\Builds\\game".to_string()),
            }],
        };

        config.set_repo_target_sources_path("D:\\Repos\\game", None);

        let tracked = config.find_repo("D:\\Repos\\game").unwrap();
        assert_eq!(tracked.target_sources_path, None);
    }

    #[test]
    fn upsert_repo_inserts_new_and_updates_existing() {
        let mut config = GutchConfig { repos: vec![] };

        config.upsert_repo(TrackedRepo {
            path: "D:\\Repos\\game".to_string(),
            target_sources_path: Some("D:\\Builds\\game".to_string()),
        });
        assert_eq!(config.repos.len(), 1);
        assert_eq!(
            config.repos[0].target_sources_path,
            Some("D:\\Builds\\game".to_string())
        );

        config.upsert_repo(TrackedRepo {
            path: "D:\\Repos\\game".to_string(),
            target_sources_path: Some("D:\\Builds\\game-v2".to_string()),
        });
        assert_eq!(config.repos.len(), 1);
        assert_eq!(
            config.repos[0].target_sources_path,
            Some("D:\\Builds\\game-v2".to_string())
        );
    }
}
