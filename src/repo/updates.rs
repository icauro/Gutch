use std::path::Path;

use super::{load_gutch_repo_manifest, write_gutch_repo_manifest};
use crate::Result;

pub fn update_gutch_repo_name(repo_root: &Path, name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(crate::Error::manifest("repo name cannot be empty"));
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err(crate::Error::manifest(
            "repo name cannot contain path separators",
        ));
    }

    let (repo_paths, mut repo_manifest) = load_gutch_repo_manifest(repo_root)?;
    repo_manifest.name = trimmed.to_string();
    write_gutch_repo_manifest(&repo_paths, &repo_manifest)
}
