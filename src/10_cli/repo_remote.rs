//! `repo remote` command handlers: push, pull and linking.

use std::path::PathBuf;

use crate::cli::CommandLineHost;
use crate::cli::Profile;
use crate::cli::arguments::ConfirmArgs;
use crate::cli::control_with_profile;
use crate::cli::output;
use crate::cli::progress::CliProgressSink;
use crate::cli::prompt_yes_no;
use crate::sync::remote::PullConflictPolicy;
use crate::sync::remote::RemoteRepoBindRequest;
use crate::sync::remote::RemoteRepoCreateRequest;
use crate::sync::remote::RemoteRepoSummary;
use crate::sync::remote::RemoteRepoUnbindRequest;
use crate::sync::remote::RemoteTagReferencesResult;
use crate::sync::remote::RepoPullPlan;
use crate::sync::remote::RepoPullRequest;
use crate::sync::remote::RepoPushPlan;
use crate::sync::remote::RepoPushRequest;
use crate::sync::remote::bind_remote_repo;
use crate::sync::remote::create_remote_repo;
use crate::sync::remote::plan_pull_repo;
use crate::sync::remote::plan_push_repo;
use crate::sync::remote::pull_repo;
use crate::sync::remote::push_repo;
use crate::sync::remote::remote_tag_references;
use crate::sync::remote::unbind_remote_repo;

pub(crate) fn push(
    repo: PathBuf,
    confirm: ConfirmArgs,
    host: &CommandLineHost<'_>,
    profile: &Profile,
) -> crate::Result<()> {
    let provider = host.require_remote_provider()?;
    let provider = provider.as_ref();
    let repo_provider = provider.repo_provider();
    let mut request = RepoPushRequest {
        repo_root: repo,
        control: control_with_profile(profile),
    };
    let plan = plan_push_repo(&request, repo_provider)?;
    print_push_plan(&plan);
    output::record(&plan)?;
    if confirm.dry_run {
        return Ok(());
    }
    if !confirm.yes && !prompt_yes_no("Proceed?", true)? {
        return Err(crate::Error::Cancelled);
    }

    provider.prepare_push(&request.repo_root)?;
    request.control = request.control.with_progress(CliProgressSink::new());
    let stats = push_repo(&request, repo_provider)?;
    output::record(&stats)?;
    output::line(format_args!(
        "pushed repo {} to remote repo {}",
        stats.repo_root.display(),
        stats.remote_repo_id
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
    policy: PullConflictPolicy,
    prune: bool,
    confirm: ConfirmArgs,
    host: &CommandLineHost<'_>,
    profile: &Profile,
) -> crate::Result<()> {
    let provider = host.require_remote_provider()?;
    let provider = provider.as_ref();
    let repo_provider = provider.repo_provider();
    let mut request = RepoPullRequest {
        repo_root: repo,
        remote_repo_id: remote,
        on_conflict: policy,
        prune,
        control: control_with_profile(profile),
    };
    let plan = plan_pull_repo(&request, repo_provider)?;
    print_pull_plan(&plan);
    output::record(&plan)?;
    if confirm.dry_run {
        return Ok(());
    }
    if !confirm.yes && !prompt_yes_no("Proceed?", true)? {
        return Err(crate::Error::Cancelled);
    }

    request.control = request.control.with_progress(CliProgressSink::new());
    let stats = pull_repo(&request, repo_provider)?;
    output::record(&stats)?;
    output::line(format_args!(
        "pulled repo {} from remote repo {} ({})",
        stats.repo_root.display(),
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

pub(crate) fn list(host: &CommandLineHost<'_>) -> crate::Result<()> {
    let provider = host.require_remote_provider()?;
    let provider = provider.as_ref();
    let entries = provider.list_repos()?;
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
    host: &CommandLineHost<'_>,
) -> crate::Result<()> {
    let provider = host.require_remote_provider()?;
    let provider = provider.as_ref();
    let created = create_remote_repo(
        &RemoteRepoCreateRequest {
            repo_root: repo.clone(),
            scope_id: scope_id.clone(),
            ..Default::default()
        },
        provider,
    )?;
    let remote_repo_id = created.remote_repo_id;
    provider.configure_binding(&repo, &remote_repo_id)?;
    output::line(format_args!(
        "bound repo '{}' to remote repo {remote_repo_id} (scope {scope_id})",
        repo.display(),
    ));
    Ok(())
}

pub(crate) fn bind(
    repo: PathBuf,
    remote_repo_id: String,
    host: &CommandLineHost<'_>,
) -> crate::Result<()> {
    if let Some(provider) = host.optional_remote_provider()?.as_deref() {
        provider.configure_binding(&repo, &remote_repo_id)?;
    }
    let bound = bind_remote_repo(&RemoteRepoBindRequest {
        repo_root: repo.clone(),
        remote_repo_id,
        ..Default::default()
    })?;
    output::line(format_args!(
        "bound repo '{}' to remote repo {}",
        repo.display(),
        bound.remote_repo_id
    ));
    Ok(())
}

pub(crate) fn unbind(
    repo: PathBuf,
    delete_remote: bool,
    host: &CommandLineHost<'_>,
) -> crate::Result<()> {
    let provider = if delete_remote {
        Some(host.require_remote_provider()?)
    } else {
        None
    };
    let unbound = unbind_remote_repo(
        &RemoteRepoUnbindRequest {
            repo_root: repo.clone(),
            delete_remote,
            ..Default::default()
        },
        provider.as_deref(),
    )?;
    let Some(remote_repo_id) = unbound.remote_repo_id else {
        output::line(format_args!(
            "repo '{}' is not bound, nothing to do.",
            repo.display()
        ));
        return Ok(());
    };
    if unbound.remote_deleted {
        output::line(format_args!("deleted remote repo {remote_repo_id}"));
    }
    output::line(format_args!("unbound repo '{}'", repo.display()));
    Ok(())
}

pub(crate) fn tag_references(repo: PathBuf, host: &CommandLineHost<'_>) -> crate::Result<()> {
    let provider = host.require_remote_provider()?;
    let provider = provider.as_ref();
    let result: RemoteTagReferencesResult = remote_tag_references(&repo, provider)?;
    output::record(&result)?;

    output::line(format_args!("repo: {}", result.repo_root.display()));
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

fn scope_label(entry: &RemoteRepoSummary) -> &str {
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
    output::line(format_args!("repo: {}", plan.repo_root.display()));
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
    output::line(format_args!("repo: {}", plan.repo_root.display()));
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
