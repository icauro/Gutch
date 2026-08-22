use std::io::{self, Write};
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

pub fn list_remote_repos(
    endpoint: String,
    session_override: Option<String>,
) -> g::Result<Vec<g::RemoteRepoEntry>> {
    let session_id = session_id_for_cli(session_override)?;
    g::list_remote_repos(&endpoint, &session_id)
}

pub fn plan_repo_pull(
    repo: PathBuf,
    remote: Option<String>,
    endpoint: String,
    session_override: Option<String>,
    policy: g::PullConflictPolicy,
) -> g::Result<g::RepoPullPlan> {
    let session_id = session_id_for_cli(session_override)?;
    g::plan_repo_pull(&repo, remote.as_deref(), &endpoint, &session_id, policy)
}

pub fn pull_repo(
    repo: PathBuf,
    remote: Option<String>,
    endpoint: String,
    session_override: Option<String>,
    policy: g::PullConflictPolicy,
    prune: bool,
    progress: Option<Arc<dyn g::GutchProgressSink>>,
) -> g::Result<g::RepoPullStats> {
    let session_id = session_id_for_cli(session_override)?;
    let control = progress
        .map(|sink| g::GutchOperationControl::default().with_progress(sink))
        .unwrap_or_default();
    g::pull_repo(repo, remote, endpoint, session_id, policy, prune, control)
}

pub fn init_repo(path: &Path, schema: Option<&PathBuf>, force: bool) -> g::Result<()> {
    let assessment = g::assess_repo_init_path(path)?;
    let mut proceed_force = force;
    if assessment.requires_confirmation && !proceed_force {
        if !confirm_repo_init_stdio(&assessment)? {
            return Err(g::Error::manifest("repo creation cancelled"));
        }
        proceed_force = true;
    }

    let repo_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("base");
    g::initialize_tracked_gutch_repo_with_options(
        path,
        load_optional_rules(&schema.cloned())?,
        None,
        g::RepoInitOptions {
            force: proceed_force,
        },
    )?;

    println!("created gutch repo '{}' at {}", repo_name, path.display());
    Ok(())
}

pub fn rename_repo(repo: &Path, name: &str) -> g::Result<()> {
    g::update_gutch_repo_name(repo, name)?;
    println!("renamed repo '{}' to '{}'", repo.display(), name.trim());
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
    progress: Option<Arc<dyn g::GutchProgressSink>>,
) -> g::Result<g::RepoSyncUpStats> {
    let session_id = session_id_for_cli(session_override)?;
    let control = progress
        .map(|sink| g::GutchOperationControl::default().with_progress(sink))
        .unwrap_or_default();
    g::sync_up_repo(repo, endpoint, session_id, control)
}

fn confirm_repo_init_stdio(assessment: &g::RepoInitAssessment) -> g::Result<bool> {
    println!();
    for risk in &assessment.risks {
        println!("{}", risk.message);
    }
    if assessment.path_kind == g::RepoInitPathKind::ExistingRepo {
        if let Some(name) = assessment.repo_name.as_deref() {
            println!("  existing repo: {name}");
        }
        if let Some(snapshot_count) = assessment.snapshot_count {
            println!("  snapshots: {snapshot_count}");
        }
        if let Some(bundle_count) = assessment.bundle_count {
            println!("  bundles: {bundle_count}");
        }
    } else if assessment.path_kind == g::RepoInitPathKind::NonEmpty {
        println!("  entries: {}", assessment.entry_count);
    }

    prompt_yes_no(
        "Delete existing Gutch repo files and create a new repo?",
        false,
    )
}

fn prompt_yes_no(prompt: &str, default_yes: bool) -> g::Result<bool> {
    loop {
        let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
        print!("{prompt} {suffix}: ");
        io::stdout()
            .flush()
            .map_err(|err| g::Error::manifest(format!("io error: {err}")))?;
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|err| g::Error::manifest(format!("io error: {err}")))?;
        let trimmed = line.trim().to_ascii_lowercase();
        if trimmed.is_empty() {
            return Ok(default_yes);
        }
        if trimmed == "y" || trimmed == "yes" {
            return Ok(true);
        }
        if trimmed == "n" || trimmed == "no" {
            return Ok(false);
        }
        eprintln!("enter y or n");
    }
}
