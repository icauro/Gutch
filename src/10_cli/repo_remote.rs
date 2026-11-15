//! `repo remote` command handlers: push, pull and linking.

use std::path::PathBuf;

use crate::Error;
use crate::cli::Profile;
use crate::cli::control_with_profile;
use crate::cli::output;
use crate::cli::output::dry_run;
use crate::cli::progress::CliProgressSink;
use crate::cli::prompt_yes_no;
use crate::cli::reporting::write_json_file;
use crate::repo::read::load_gutch_repo_manifest;
use crate::sync::remote::PullConflictPolicy;
use crate::sync::remote::RemoteRepositoryCommandProvider;
use crate::sync::remote::RemoteRepositorySummary;
use crate::sync::remote::RemoteTagReferencesResult;
use crate::sync::remote::RepoPullPlan;
use crate::sync::remote::RepoPushPlan;
use crate::sync::remote::bind_remote_repo;
use crate::sync::remote::create_remote_repo;
use crate::sync::remote::inspect_repo_tag_references;
use crate::sync::remote::list_remote_repositories;
use crate::sync::remote::plan_pull_with_provider;
use crate::sync::remote::plan_push_with_provider;
use crate::sync::remote::pull_repo;
use crate::sync::remote::push_repo;
use crate::sync::remote::unbind_remote_repo;

pub(crate) fn push(
    repo: PathBuf,
    yes: bool,
    remote_provider: Option<&dyn RemoteRepositoryCommandProvider>,
    profile: &Profile,
) -> crate::Result<()> {
    let provider = require_provider(remote_provider)?;
    let repository_provider = provider.repository_provider();
    let plan = plan_push_with_provider(&repo, repository_provider)?;
    print_push_plan(&plan);
    output::record(&plan)?;
    if dry_run() {
        return Ok(());
    }
    if !yes && !prompt_yes_no("Proceed?", true)? {
        return Err(crate::Error::Cancelled);
    }

    provider.prepare_repository_push(&repo)?;
    let progress = CliProgressSink::new();
    let stats = push_repo(
        repo,
        repository_provider,
        control_with_profile(profile).with_progress(progress),
    )?;
    output::record(&stats)?;
    output::line(format_args!(
        "pushed repo {} to remote repo {}",
        stats.repo_root, stats.remote_repo_id
    ));
    output::line(format_args!(
        "snapshots added: {}, removed: {}, updated: {}, unchanged: {}",
        stats.snapshot_added_count,
        stats.snapshot_removed_count,
        stats.snapshot_updated_count,
        stats.snapshot_unchanged_count
    ));
    output::line(format_args!(
        "repository metadata uploaded: {}, chunk index uploaded: {}",
        stats.repo_manifest_uploaded, stats.chunk_index_uploaded
    ));
    output::line(format_args!(
        "snapshot metadata uploaded: {}, bundles uploaded: {}, bundles skipped: {}",
        stats.snapshot_manifest_uploaded_count,
        stats.bundle_uploaded_count,
        stats.bundle_skipped_count
    ));
    Ok(())
}

pub(crate) fn pull(
    repo: PathBuf,
    remote: Option<String>,
    on_conflict: String,
    prune: bool,
    yes: bool,
    remote_provider: Option<&dyn RemoteRepositoryCommandProvider>,
    profile: &Profile,
) -> crate::Result<()> {
    let provider = require_provider(remote_provider)?;
    let policy = PullConflictPolicy::parse(&on_conflict)?;
    let repository_provider = provider.repository_provider();
    let plan = plan_pull_with_provider(&repo, repository_provider, remote.as_deref(), policy)?;
    print_pull_plan(&plan);
    output::record(&plan)?;
    if dry_run() {
        return Ok(());
    }
    if !yes && !prompt_yes_no("Proceed?", true)? {
        return Err(crate::Error::Cancelled);
    }

    let progress = CliProgressSink::new();
    let stats = pull_repo(
        repo,
        repository_provider,
        remote,
        policy,
        prune,
        control_with_profile(profile).with_progress(progress),
    )?;
    output::record(&stats)?;
    output::line(format_args!(
        "pulled repo {} from remote repo {} ({})",
        stats.repo_root,
        stats.remote_repo_id,
        if prune { "with prune" } else { "no prune" }
    ));
    output::line(format_args!(
        "snapshots downloaded: {}, skipped: {}, bundles downloaded: {}, bundles skipped: {}",
        stats.snapshot_downloaded_count,
        stats.snapshot_skipped_count,
        stats.bundle_downloaded_count,
        stats.bundle_skipped_count
    ));
    Ok(())
}

pub(crate) fn list(
    remote_provider: Option<&dyn RemoteRepositoryCommandProvider>,
) -> crate::Result<()> {
    let provider = require_provider(remote_provider)?;
    let entries = list_remote_repositories(provider)?;
    output::record(&entries)?;
    for entry in &entries {
        output::line(format_args!(
            "{}/{}",
            scope_label(entry),
            entry.repository_name
        ));
    }
    if entries.is_empty() {
        output::line(format_args!("no remote repos found"));
    }
    Ok(())
}

pub(crate) fn create(
    repo: PathBuf,
    scope_id: String,
    remote_provider: Option<&dyn RemoteRepositoryCommandProvider>,
) -> crate::Result<()> {
    let provider = require_provider(remote_provider)?;
    create_remote_repo(&repo, &scope_id, provider)?;
    let (_, manifest) = load_gutch_repo_manifest(&repo)?;
    let remote_repo_id = manifest
        .remote_repo_id
        .ok_or_else(|| Error::repository("created remote repo was not linked"))?;
    provider.configure_repository_binding(&repo, &remote_repo_id)?;
    output::line(format_args!(
        "bound repo '{}' to remote repo {remote_repo_id} (scope {scope_id})",
        repo.display(),
    ));
    Ok(())
}

pub(crate) fn bind(
    repo: PathBuf,
    remote_repo_id: String,
    provider: Option<&dyn RemoteRepositoryCommandProvider>,
) -> crate::Result<()> {
    if let Some(provider) = provider {
        provider.configure_repository_binding(&repo, &remote_repo_id)?;
    }
    bind_remote_repo(&repo, &remote_repo_id)?;
    output::line(format_args!(
        "bound repo '{}' to remote repo {}",
        repo.display(),
        remote_repo_id.trim()
    ));
    Ok(())
}

pub(crate) fn unbind(
    repo: PathBuf,
    delete_remote: bool,
    remote_provider: Option<&dyn RemoteRepositoryCommandProvider>,
) -> crate::Result<()> {
    let (paths, manifest) = load_gutch_repo_manifest(&repo)?;
    let Some(remote_repo_id) = manifest.remote_repo_id else {
        output::line(format_args!(
            "repo '{}' is not bound, nothing to do.",
            paths.repo_root().display()
        ));
        return Ok(());
    };

    let provider = if delete_remote {
        Some(require_provider(remote_provider)?)
    } else {
        None
    };
    unbind_remote_repo(&repo, delete_remote, provider)?;
    if delete_remote {
        output::line(format_args!("deleted remote repo {remote_repo_id}"));
    }
    output::line(format_args!(
        "unbound repo '{}'",
        paths.repo_root().display()
    ));
    Ok(())
}

pub(crate) fn tag_references(
    repo: PathBuf,
    json_out: Option<PathBuf>,
    remote_provider: Option<&dyn RemoteRepositoryCommandProvider>,
) -> crate::Result<()> {
    let provider = require_provider(remote_provider)?;
    let result: RemoteTagReferencesResult = inspect_repo_tag_references(&repo, provider)?;
    write_json_file(json_out.as_deref(), &result)?;

    output::line(format_args!("repo: {}", result.repo_root));
    output::line(format_args!("remote_repo_id: {}", result.remote_repo_id));
    for tag in &result.referenced_tags {
        let values = if tag.referenced_values.is_empty() {
            "any non-empty value".to_string()
        } else {
            tag.referenced_values.join(", ")
        };
        output::line(format_args!(
            "{} | {} | {} | {}",
            tag.status, tag.tag_name, tag.tag_kind, values
        ));
    }
    output::line(format_args!("advisory_only: true"));
    Ok(())
}

fn require_provider(
    provider: Option<&dyn RemoteRepositoryCommandProvider>,
) -> crate::Result<&dyn RemoteRepositoryCommandProvider> {
    provider.ok_or_else(|| {
        Error::invalid_input(
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

fn print_push_plan(plan: &RepoPushPlan) {
    output::line(format_args!("Push plan"));
    output::line(format_args!("repo: {}", plan.repo_root));
    output::line(format_args!(
        "snapshots: local={} remote={}",
        plan.snapshot_count, plan.remote_snapshot_count
    ));
    output::line(format_args!(
        "changes: add={} remove={} update={} unchanged={}",
        plan.to_add.len(),
        plan.to_remove.len(),
        plan.to_update.len(),
        plan.unchanged.len()
    ));
}

fn print_pull_plan(plan: &RepoPullPlan) {
    output::line(format_args!("Pull plan"));
    output::line(format_args!("repo: {}", plan.repo_root));
    output::line(format_args!(
        "remote: repo {} ({})",
        plan.remote_repo_id, plan.remote_repo_name
    ));
    output::line(format_args!(
        "changes: repo_metadata={} chunk_index={} snapshots_download={} snapshots_skip={} local_only={} kept_local={}",
        plan.repo_manifest_needed,
        plan.chunk_map_needed,
        plan.snapshots_to_download.len(),
        plan.snapshots_to_skip.len(),
        plan.local_only_snapshots.len(),
        plan.kept_local_snapshots.len()
    ));
}
