use std::path::PathBuf;
use std::sync::Arc;

use crate::cli::reporting::ensure_parent_directory;
use crate::{
    Error, GutchProgressEvent, GutchProgressSink, PullConflictPolicy,
    RemoteRepositoryCommandProvider, RemoteRepositorySummary, RemoteTagRequirementsResult,
};

#[derive(Debug)]
struct CliProgressSink;

impl GutchProgressSink for CliProgressSink {
    fn emit(&self, event: GutchProgressEvent) {
        if !event.done && event.total > 0 {
            eprintln!(
                "[{}] {}/{} {}",
                event.phase.as_str(),
                event.current,
                event.total,
                event.message
            );
        }
    }
}

pub(crate) fn push(
    repo: PathBuf,
    yes: bool,
    remote_provider: Option<&dyn RemoteRepositoryCommandProvider>,
    profile: &crate::cli::Profile,
) -> crate::Result<()> {
    let provider = require_provider(remote_provider)?;
    let repository_provider = provider.repository_provider();
    let plan = crate::plan_sync_up_with_provider(&repo, repository_provider)?;
    print_sync_plan(&plan);
    if !yes && !crate::cli::prompt_yes_no("Proceed?", true)? {
        println!("push cancelled");
        return Ok(());
    }

    let progress = Arc::new(CliProgressSink);
    let stats = crate::sync_up_repo(
        repo,
        repository_provider,
        crate::cli::control_with_profile(profile).with_progress(progress),
    )?;
    println!(
        "synced repo {} to remote repo {}",
        stats.repo_root, stats.remote_repo_id
    );
    println!(
        "snapshots added: {}, removed: {}, updated: {}, unchanged: {}",
        stats.snapshot_added_count,
        stats.snapshot_removed_count,
        stats.snapshot_updated_count,
        stats.snapshot_unchanged_count
    );
    println!(
        "repo manifest uploaded: {}, chunk index uploaded: {}",
        stats.repo_manifest_uploaded, stats.chunk_index_uploaded
    );
    println!(
        "snapshot manifests uploaded: {}, bundles uploaded: {}, bundles skipped: {}",
        stats.snapshot_manifest_uploaded_count,
        stats.bundle_uploaded_count,
        stats.bundle_skipped_count
    );
    Ok(())
}

pub(crate) fn pull(
    repo: PathBuf,
    remote: Option<String>,
    on_conflict: String,
    prune: bool,
    yes: bool,
    remote_provider: Option<&dyn RemoteRepositoryCommandProvider>,
    profile: &crate::cli::Profile,
) -> crate::Result<()> {
    let provider = require_provider(remote_provider)?;
    let policy = PullConflictPolicy::parse(&on_conflict)?;
    let repository_provider = provider.repository_provider();
    let plan =
        crate::plan_pull_with_provider(&repo, repository_provider, remote.as_deref(), policy)?;
    print_pull_plan(&plan);
    if !yes && !crate::cli::prompt_yes_no("Proceed?", true)? {
        println!("pull cancelled");
        return Ok(());
    }

    let progress = Arc::new(CliProgressSink);
    let stats = crate::pull_repo(
        repo,
        repository_provider,
        remote,
        policy,
        prune,
        crate::cli::control_with_profile(profile).with_progress(progress),
    )?;
    println!(
        "pulled repo {} from remote repo {} ({})",
        stats.repo_root,
        stats.remote_repo_id,
        if prune { "with prune" } else { "no prune" }
    );
    println!(
        "snapshots downloaded: {}, skipped: {}, bundles downloaded: {}, bundles skipped: {}",
        stats.snapshot_downloaded_count,
        stats.snapshot_skipped_count,
        stats.bundle_downloaded_count,
        stats.bundle_skipped_count
    );
    Ok(())
}

pub(crate) fn list(
    remote_provider: Option<&dyn RemoteRepositoryCommandProvider>,
) -> crate::Result<()> {
    let provider = require_provider(remote_provider)?;
    let entries = crate::list_remote_repositories(provider)?;
    for entry in &entries {
        println!("{}/{}", scope_label(entry), entry.repository_name);
    }
    if entries.is_empty() {
        println!("no remote repos found");
    }
    Ok(())
}

pub(crate) fn link(
    repo: PathBuf,
    scope_id: String,
    remote_provider: Option<&dyn RemoteRepositoryCommandProvider>,
) -> crate::Result<()> {
    let provider = require_provider(remote_provider)?;
    crate::link_repo(&repo, &scope_id, provider)?;
    let (_, manifest) = crate::load_gutch_repo_manifest(&repo)?;
    println!(
        "linked repo '{}' to remote repo {} (scope {})",
        repo.display(),
        manifest.remote_repo_id,
        scope_id
    );
    Ok(())
}

pub(crate) fn bind(repo: PathBuf, remote_repo_id: String) -> crate::Result<()> {
    crate::bind_repo_remote(&repo, &remote_repo_id)?;
    println!(
        "bound repo '{}' to remote repo {}",
        repo.display(),
        remote_repo_id.trim()
    );
    Ok(())
}

pub(crate) fn unlink(
    repo: PathBuf,
    delete_remote: bool,
    remote_provider: Option<&dyn RemoteRepositoryCommandProvider>,
) -> crate::Result<()> {
    let (paths, manifest) = crate::load_gutch_repo_manifest(&repo)?;
    if !manifest.is_linked() {
        println!(
            "repo '{}' is not linked, nothing to do.",
            paths.repo_root().display()
        );
        return Ok(());
    }

    let remote_repo_id = manifest.remote_repo_id.clone();
    let provider = if delete_remote {
        Some(require_provider(remote_provider)?)
    } else {
        None
    };
    crate::unlink_repo(&repo, delete_remote, provider)?;
    if delete_remote {
        println!("deleted remote repo {remote_repo_id}");
    }
    println!("unlinked repo '{}'", paths.repo_root().display());
    Ok(())
}

pub(crate) fn tag_requirements(
    repo: PathBuf,
    check: bool,
    json_out: Option<PathBuf>,
    remote_provider: Option<&dyn RemoteRepositoryCommandProvider>,
) -> crate::Result<()> {
    let provider = require_provider(remote_provider)?;
    let result: RemoteTagRequirementsResult =
        crate::inspect_repo_tag_requirements(&repo, provider)?;
    if let Some(json_out) = json_out.as_ref() {
        ensure_parent_directory(json_out)?;
        crate::helpers::write(json_out, serde_json::to_vec_pretty(&result)?)?;
    }

    println!("repo: {}", result.repo_root);
    println!("remote_repo_id: {}", result.remote_repo_id);
    for tag in &result.referenced_tags {
        let values = if tag.allowed_values.is_empty() {
            "any non-empty value".to_string()
        } else {
            tag.allowed_values.join(", ")
        };
        println!(
            "{} | {} | {} | {}{}",
            tag.status,
            tag.tag_name,
            tag.tag_kind,
            values,
            tag.issue
                .as_deref()
                .map(|issue| format!(" | {issue}"))
                .unwrap_or_default()
        );
    }
    println!("advisory_only: true");
    let _ = (check, result.ready);
    Ok(())
}

fn require_provider(
    provider: Option<&dyn RemoteRepositoryCommandProvider>,
) -> crate::Result<&dyn RemoteRepositoryCommandProvider> {
    provider.ok_or_else(|| {
        Error::manifest(
            "remote provider context is required; configure a backend endpoint and session",
        )
    })
}

fn scope_label(entry: &RemoteRepositorySummary) -> &str {
    if !entry.scope_name.trim().is_empty() {
        &entry.scope_name
    } else if !entry.scope_slug.trim().is_empty() {
        &entry.scope_slug
    } else {
        &entry.scope_id
    }
}

fn print_sync_plan(plan: &crate::RepoSyncPlan) {
    println!("Sync plan");
    println!("repo: {}", plan.repo_root);
    println!(
        "snapshots: local={} remote={}",
        plan.snapshot_count, plan.remote_snapshot_count
    );
    println!(
        "changes: add={} remove={} update={} unchanged={}",
        plan.to_add.len(),
        plan.to_remove.len(),
        plan.to_update.len(),
        plan.unchanged.len()
    );
}

fn print_pull_plan(plan: &crate::RepoPullPlan) {
    println!("Pull plan");
    println!("repo: {}", plan.repo_root);
    println!(
        "remote: repo {} ({})",
        plan.remote_repo_id, plan.remote_repo_name
    );
    println!(
        "changes: repo_manifest={} chunk_map={} snapshots_download={} snapshots_skip={} local_only={} kept_newer_local={}",
        plan.repo_manifest_needed,
        plan.chunk_map_needed,
        plan.snapshots_to_download.len(),
        plan.snapshots_to_skip.len(),
        plan.local_only_snapshots.len(),
        plan.kept_newer_local_snapshots.len()
    );
}
