use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{
    Result, SourceTargetInstallCandidate, discover_source_target_installs, inspect_gutch_repo,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupTargetChangePlan {
    pub lookup_ready: bool,
    pub candidate_count: usize,
    pub insertable_count: usize,
    pub removable_count: usize,
    #[serde(default)]
    pub to_insert: Vec<SourceTargetInstallCandidate>,
    #[serde(default)]
    pub to_remove: Vec<String>,
}

impl LookupTargetChangePlan {
    fn empty() -> Self {
        Self {
            lookup_ready: false,
            candidate_count: 0,
            insertable_count: 0,
            removable_count: 0,
            to_insert: Vec::new(),
            to_remove: Vec::new(),
        }
    }
}

pub fn plan_lookup_target_changes(
    repo_root: &Path,
    lookup_path: Option<&Path>,
) -> Result<LookupTargetChangePlan> {
    let Some(lookup_path) = normalized_lookup_path(lookup_path) else {
        return Ok(LookupTargetChangePlan::empty());
    };

    let mut plan = LookupTargetChangePlan {
        lookup_ready: true,
        ..LookupTargetChangePlan::empty()
    };

    let candidates = match discover_source_target_installs(repo_root, &lookup_path) {
        Ok(candidates) => candidates,
        Err(_) => return Ok(plan),
    };

    let status = inspect_gutch_repo(repo_root)?;
    let existing_ids: HashSet<String> = status
        .targets
        .iter()
        .map(|target| target.target_id.clone())
        .collect();
    let available_ids: HashSet<String> = candidates
        .iter()
        .map(|candidate| candidate.target_id.clone())
        .collect();

    plan.candidate_count = candidates.len();
    plan.to_insert = candidates
        .into_iter()
        .filter(|candidate| !existing_ids.contains(&candidate.target_id))
        .collect();
    plan.insertable_count = plan.to_insert.len();
    plan.to_remove = status
        .targets
        .into_iter()
        .filter(|target| !available_ids.contains(&target.target_id))
        .map(|target| target.target_id)
        .collect();
    plan.removable_count = plan.to_remove.len();

    Ok(plan)
}

fn normalized_lookup_path(lookup_path: Option<&Path>) -> Option<std::path::PathBuf> {
    let lookup_path = lookup_path?;
    let trimmed = lookup_path.to_string_lossy().trim().to_string();
    if trimmed.is_empty() {
        return None;
    }
    Some(std::path::PathBuf::from(trimmed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GutchRepoPaths;
    use crate::repo::ensure_gutch_repo_layout;

    #[test]
    fn plan_lookup_target_changes_returns_empty_plan_without_lookup_path() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

        let plan = plan_lookup_target_changes(&repo_root, None).unwrap();
        assert!(!plan.lookup_ready);
        assert_eq!(plan.insertable_count, 0);
        assert_eq!(plan.removable_count, 0);
    }
}
