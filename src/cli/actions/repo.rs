use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cli::common::{load_optional_rules, session_id_for_cli};
use gutch as g;

pub fn plan_sync_up_repo(
    repo: PathBuf,
    endpoint: String,
    session_override: Option<String>,
) -> g::Result<g::RepoSyncPlan> {
    let session_id = session_id_for_cli(session_override)?;
    g::plan_sync_up_repo(repo, endpoint, session_id)
}

pub fn init_repo(path: &Path, schema: Option<&PathBuf>) -> g::Result<()> {
    let repo_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("base");
    g::initialize_tracked_gutch_repo(path, load_optional_rules(&schema.cloned())?, None)?;

    println!("created gutch repo '{}' at {}", repo_name, path.display());
    Ok(())
}

pub fn link_repo(
    repo: &Path,
    product_id: i64,
    endpoint: &str,
    session_override: Option<String>,
) -> g::Result<()> {
    let session_id = session_id_for_cli(session_override)?;
    g::link_repo(repo, product_id, endpoint, session_id)?;

    let (_, manifest) = g::load_gutch_repo_manifest(repo)?;
    println!(
        "linked repo '{}' to remote repo {} (product {})",
        repo.display(),
        manifest.remote_repo_id,
        product_id
    );
    Ok(())
}

pub fn unlink_repo(
    repo: &Path,
    delete_remote: bool,
    endpoint: Option<&str>,
    session_override: Option<String>,
) -> g::Result<()> {
    let (paths, manifest) = g::load_gutch_repo_manifest(repo)?;
    if !manifest.is_linked() {
        println!(
            "repo '{}' is not linked, nothing to do.",
            paths.repo_root.display()
        );
        return Ok(());
    }

    let remote_repo_id = manifest.remote_repo_id.clone();
    let product_id = manifest.product_id.clone();
    let session_id = if delete_remote {
        Some(session_id_for_cli(session_override)?)
    } else {
        None
    };
    g::unlink_repo(repo, delete_remote, endpoint, session_id)?;
    if delete_remote {
        println!(
            "deleted remote repo {} (product {})",
            remote_repo_id, product_id
        );
    }

    println!("unlinked repo '{}'", paths.repo_root.display());
    Ok(())
}

pub fn sync_up_repo(
    repo: PathBuf,
    endpoint: String,
    session_override: Option<String>,
) -> g::Result<g::RepoSyncUpStats> {
    let session_id = session_id_for_cli(session_override)?;
    g::sync_up_repo(repo, endpoint, session_id)
}

pub fn sync_up_repo_with_progress(
    repo: PathBuf,
    endpoint: String,
    session_override: Option<String>,
    progress: Option<Arc<dyn g::SyncUpProgressSink>>,
) -> g::Result<g::RepoSyncUpStats> {
    let session_id = session_id_for_cli(session_override)?;
    g::sync_up_repo_with_progress(repo, endpoint, session_id, progress)
}
