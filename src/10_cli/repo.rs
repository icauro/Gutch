//! `repo` command handlers.

use crate::cli::ConstraintCommands;
use crate::cli::Profile;
use crate::cli::RemoteCommands;
use crate::cli::RepoCommands;
use crate::cli::RepoTagsCommands;
use crate::cli::SECTION_SEPARATOR;
use crate::cli::arguments::RepoIndexCommands;
use crate::cli::command_support::prompt_yes_no;
use crate::cli::output;
use crate::cli::output::dry_run;
use crate::cli::repo_remote;
use crate::cli::repo_remote::tag_references;
use crate::filesystem;
use crate::manifest::constraints::FilesystemTarget;
use crate::manifest::constraints::RepositoryConstraints;
use crate::progress::OperationControl;
use crate::repo::init_policy::RepoInitAssessment;
use crate::repo::init_policy::RepoInitOptions;
use crate::repo::init_policy::RepoInitPathKind;
use crate::repo::init_policy::assess_repo_init_path;
use crate::repo::init_policy::initialize_repo;
use crate::repo::recovery::rebuild_index;
use crate::repo::status::read_repo_status;
use crate::repo::updates::plan_repo_rename;
use crate::repo::updates::plan_repository_constraints;
use crate::repo::updates::read_repository_constraints;
use crate::repo::updates::update_repository_constraints;
use crate::repo::verify::RepoArtifactVerification;
use crate::repo::verify::verify_gutch_repo_artifacts;
use crate::sync::remote::RemoteRepositoryCommandProvider;
use std::path::Path;

pub(crate) fn run(
    command: RepoCommands,
    remote_provider: Option<&dyn RemoteRepositoryCommandProvider>,
    profile: &Profile,
) -> crate::Result<()> {
    match command {
        RepoCommands::Constraints { command } => constraints(command),
        RepoCommands::Index {
            command: RepoIndexCommands::Rebuild { repo, apply },
        } => rebuild_chunk_index(&repo, apply),
        RepoCommands::Init {
            path,
            force,
            targets,
            constraints,
        } => init_repo(
            &path,
            force,
            read_constraint_input(constraints.as_deref(), targets)?,
        ),
        RepoCommands::Push { repo, yes } => repo_remote::push(repo, yes, remote_provider, profile),
        RepoCommands::Pull {
            repo,
            remote,
            on_conflict,
            prune,
            yes,
        } => repo_remote::pull(
            repo,
            remote,
            on_conflict,
            prune,
            yes,
            remote_provider,
            profile,
        ),
        RepoCommands::Remote { command } => match command {
            RemoteCommands::List => repo_remote::list(remote_provider),
            RemoteCommands::Create { repo, scope_id } => {
                repo_remote::create(repo, scope_id, remote_provider)
            }
            RemoteCommands::Bind {
                repo,
                remote_repo_id,
            } => repo_remote::bind(repo, remote_repo_id, remote_provider),
            RemoteCommands::Unbind {
                repo,
                delete_remote,
            } => repo_remote::unbind(repo, delete_remote, remote_provider),
        },
        RepoCommands::Rename { repo, name } => {
            if dry_run() {
                let name = plan_repo_rename(&repo, &name)?;
                output::record(&serde_json::json!({"operation": "rename", "name": name}))?;
                output::line(format_args!("Would rename {} to {}", repo.display(), name));
                Ok(())
            } else {
                rename_repo(&repo, &name)
            }
        }
        RepoCommands::Tags {
            command: RepoTagsCommands::References { repo },
        } => tag_references(repo, remote_provider),
        RepoCommands::Status { repo } => status(&repo, profile),
        RepoCommands::Verify {
            repo,
            snapshot_id,
            strict,
        } => verify(&repo, snapshot_id.as_deref(), strict),
    }
}

/// Shows the repository constraints, or replaces them for `set` and `clear`.
fn constraints(command: ConstraintCommands) -> crate::Result<()> {
    let (repo, replacement) = match command {
        ConstraintCommands::Show { repo } => (repo, None),
        ConstraintCommands::Set {
            repo,
            targets,
            constraints,
        } => (
            repo,
            Some(read_constraint_input(constraints.as_deref(), targets)?),
        ),
        ConstraintCommands::Clear { repo } => (repo, Some(RepositoryConstraints::default())),
    };
    let constraints = match replacement {
        Some(replacement) => {
            let constraints = plan_repository_constraints(&repo, replacement)?;
            if !dry_run() {
                update_repository_constraints(&repo, constraints.clone())?;
            }
            constraints
        }
        None => read_repository_constraints(&repo)?,
    };
    output::record(&constraints)?;
    output::line(format_args!(
        "{}",
        serde_json::to_string_pretty(&constraints)?
    ));
    Ok(())
}

fn rebuild_chunk_index(repo: &Path, apply: bool) -> crate::Result<()> {
    let result = rebuild_index(repo, apply && !dry_run(), &OperationControl::default())?;
    output::record(&result)?;
    output::line(format_args!("{}", serde_json::to_string_pretty(&result)?));
    if !result.complete {
        return Err(crate::Error::repository(
            "chunk index rebuild is incomplete; the index was not replaced",
        ));
    }
    Ok(())
}

fn status(repo: &Path, profile: &Profile) -> crate::Result<()> {
    let _status_span = profile.as_ref().and_then(|p| p.span("repo.status"));
    let status = read_repo_status(repo)?;
    output::record(&status)?;

    if !status.is_repo {
        match status.error.as_deref() {
            Some(error) => output::line(format_args!("status: {error}")),
            None => output::line(format_args!(
                "status: no gutch repo found at {}",
                status.repo_path
            )),
        }
        return Ok(());
    }
    output::line(format_args!("{SECTION_SEPARATOR}"));
    output::line(format_args!("Snapshots"));
    for snapshot in &status.snapshots {
        output::line(format_args!(
            "{} | version={} | os={} | arch={} | files={} | missing_artifacts={}",
            snapshot.snapshot_id,
            snapshot.version_tag,
            snapshot.os,
            snapshot.arch,
            snapshot.file_count,
            snapshot.missing_artifact_count
        ));
    }
    let remote_repo_id = status.remote_repo_id.as_deref().unwrap_or("(not linked)");
    output::line(format_args!("{SECTION_SEPARATOR}"));
    output::line(format_args!("Total"));
    output::line(format_args!("name: {}", status.name));
    output::line(format_args!("remote_repo_id: {remote_repo_id}"));
    output::line(format_args!("repo_path: {}", status.repo_path));
    output::line(format_args!("snapshots: {}", status.snapshots.len()));
    output::line(format_args!("bundles: {}", status.bundle_count));
    output::line(format_args!("bundle_bytes: {}", status.bundle_bytes));
    output::line(format_args!("artifacts: {}", status.artifact_count));
    output::line(format_args!("artifact_bytes: {}", status.artifact_bytes));
    Ok(())
}

fn verify(repo: &Path, snapshot_id: Option<&str>, strict: bool) -> crate::Result<()> {
    let mode = if strict {
        RepoArtifactVerification::Strict
    } else {
        RepoArtifactVerification::Fast
    };
    let result = verify_gutch_repo_artifacts(repo, snapshot_id, mode)?;
    output::record(&result)?;

    output::line(format_args!("{SECTION_SEPARATOR}"));
    output::line(format_args!("Verify ({})", result.mode.as_str()));
    output::line(format_args!("ok: {}", result.ok));
    output::line(format_args!(
        "snapshots: {} ok={}",
        result.snapshot_count, result.snapshots_ok
    ));
    for snapshot in &result.snapshots {
        output::line(format_args!(
            "{} | ok={} | missing_artifacts={} | corrupt_artifacts={}",
            snapshot.snapshot_id,
            snapshot.ok,
            snapshot.missing_artifact_count,
            snapshot.corrupt_artifact_count
        ));
        for missing in &snapshot.missing_artifacts {
            output::line(format_args!("  missing artifact: {missing}"));
        }
        for corrupt in &snapshot.corrupt_artifacts {
            output::line(format_args!("  corrupt artifact: {corrupt}"));
        }
    }
    if !result.ok {
        return Err(crate::Error::verify("repository verification failed"));
    }
    Ok(())
}

pub(crate) fn init_repo(
    path: &Path,
    force: bool,
    constraints: RepositoryConstraints,
) -> crate::Result<()> {
    let assessment = assess_repo_init_path(path)?;
    if dry_run() {
        output::record(&assessment)?;
        output::line(format_args!(
            "Would initialize repository at {}",
            path.display()
        ));
        for risk in &assessment.risks {
            output::line(format_args!("{}", risk.message));
        }
        return Ok(());
    }
    let mut proceed_force = force;
    if assessment.requires_confirmation && !proceed_force {
        if !confirm_repo_init_stdio(&assessment)? {
            return Err(crate::Error::Cancelled);
        }
        proceed_force = true;
    }

    let repository = initialize_repo(
        path,
        RepoInitOptions {
            force: proceed_force,
            constraints,
        },
    )?;

    output::line(format_args!(
        "created gutch repo '{}' at {}",
        repository.repository_name,
        path.display()
    ));
    Ok(())
}

pub(crate) fn rename_repo(repo: &Path, name: &str) -> crate::Result<()> {
    crate::repo::updates::rename_repo(repo, name)?;
    output::line(format_args!(
        "renamed repo '{}' to '{}'",
        repo.display(),
        name.trim()
    ));
    Ok(())
}

fn confirm_repo_init_stdio(assessment: &RepoInitAssessment) -> crate::Result<bool> {
    output::line(format_args!(""));
    for risk in &assessment.risks {
        output::line(format_args!("{}", risk.message));
    }
    if assessment.path_kind == RepoInitPathKind::ExistingRepo {
        if let Some(name) = assessment.repo_name.as_deref() {
            output::line(format_args!("  existing repo: {name}"));
        }
        if let Some(snapshot_count) = assessment.snapshot_count {
            output::line(format_args!("  snapshots: {snapshot_count}"));
        }
        if let Some(bundle_count) = assessment.bundle_count {
            output::line(format_args!("  bundles: {bundle_count}"));
        }
    } else if assessment.path_kind == RepoInitPathKind::NonEmpty {
        output::line(format_args!("  entries: {}", assessment.entry_count));
    }

    prompt_yes_no(
        "Delete existing Gutch repo files and create a new repo?",
        false,
    )
}

fn read_constraint_input(
    path: Option<&Path>,
    targets: Vec<FilesystemTarget>,
) -> crate::Result<RepositoryConstraints> {
    let constraints = if let Some(path) = path {
        filesystem::read_json(path)?
    } else {
        RepositoryConstraints {
            targets,
            ..Default::default()
        }
    };
    constraints.validate_definition()?;
    Ok(constraints)
}
