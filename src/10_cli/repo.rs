//! `repo` command handlers.

use crate::cli::CommandLineHost;
use crate::cli::ConstraintCommands;
use crate::cli::Profile;
use crate::cli::RemoteCommands;
use crate::cli::RepoCommands;
use crate::cli::SECTION_SEPARATOR;
use crate::cli::arguments::DryRunArgs;
use crate::cli::arguments::RepoCredentialCommands;
use crate::cli::arguments::RepoIndexCommands;
use crate::cli::command_support::prompt_yes_no;
use crate::cli::credentials::{
    KEY_FILE_VARIABLE, NEW_PASSWORD_VARIABLE, PASSWORD_VARIABLE, environment_credential,
    password_variable,
};
use crate::cli::output;
use crate::cli::repo_remote;
use crate::cli::repo_remote::tag_references;
use crate::filesystem;
use crate::manifest::constraints::FilesystemTarget;
use crate::manifest::constraints::RepoConstraints;
use crate::progress::OperationControl;
use crate::repo::encryption::{RepoCredential, add_repo_credential};
use crate::repo::init_policy::RepoInitPathKind;
use crate::repo::init_policy::RepoInitPlan;
use crate::repo::init_policy::RepoInitRequest;
use crate::repo::init_policy::init_repo;
use crate::repo::init_policy::plan_init_repo;
use crate::repo::recovery::{RepoIndexRebuildRequest, plan_rebuild_repo_index, rebuild_repo_index};
use crate::repo::status::repo_status;
use crate::repo::updates::RepoConstraintsSetRequest;
use crate::repo::updates::RepoRenameRequest;
use crate::repo::updates::plan_rename_repo;
use crate::repo::updates::plan_set_repo_constraints;
use crate::repo::updates::rename_repo;
use crate::repo::updates::repo_constraints;
use crate::repo::updates::set_repo_constraints;
use crate::repo::verify::RepoVerifyMode;
use crate::repo::verify::RepoVerifyRequest;
use crate::repo::verify::verify_repo;
use std::path::Path;

pub(crate) fn run(
    command: RepoCommands,
    host: &CommandLineHost<'_>,
    profile: &Profile,
) -> crate::Result<()> {
    match command {
        RepoCommands::Constraints { command } => constraints(command),
        RepoCommands::Index {
            command:
                RepoIndexCommands::Rebuild {
                    repo,
                    dry_run: DryRunArgs { dry_run },
                },
        } => rebuild_chunk_index(&repo, dry_run),
        RepoCommands::Init {
            path,
            force,
            targets,
            constraints,
            encrypt,
            new_key_file,
            dry_run: DryRunArgs { dry_run },
        } => init(
            &path,
            force,
            read_constraint_input(constraints.as_deref(), targets)?,
            init_credential(encrypt, new_key_file.as_deref(), dry_run)?,
            dry_run,
        ),
        RepoCommands::Credential {
            command:
                RepoCredentialCommands::Add {
                    repo,
                    new_password,
                    new_key_file,
                },
        } => add_credential(&repo, new_password, new_key_file.as_deref()),
        RepoCommands::Push { repo, confirm } => repo_remote::push(repo, confirm, host, profile),
        RepoCommands::Pull {
            repo,
            remote_repo_id,
            on_conflict,
            prune,
            snapshot_ids,
            metadata_only,
            confirm,
        } => repo_remote::pull(
            crate::sync::remote::RepoPullRequest {
                repo_root: repo,
                remote_repo_id,
                on_conflict,
                prune,
                snapshot_ids,
                bundles: if metadata_only {
                    crate::sync::remote::PullBundles::None
                } else {
                    crate::sync::remote::PullBundles::All
                },
                ..Default::default()
            },
            confirm,
            host,
            profile,
        ),
        RepoCommands::Remote { command } => match command {
            RemoteCommands::List => repo_remote::list(host),
            RemoteCommands::TagReferences { repo } => tag_references(repo, host),
            RemoteCommands::Create { repo, scope_id } => repo_remote::create(repo, scope_id, host),
            RemoteCommands::Bind {
                repo,
                remote_repo_id,
            } => repo_remote::bind(repo, remote_repo_id, host),
            RemoteCommands::Unbind {
                repo,
                delete_remote,
            } => repo_remote::unbind(repo, delete_remote, host),
        },
        RepoCommands::Rename {
            repo,
            name,
            dry_run: DryRunArgs { dry_run },
        } => {
            let request = RepoRenameRequest {
                repo_root: repo,
                name,
                control: OperationControl::default(),
            };
            if dry_run {
                let plan = plan_rename_repo(&request)?;
                output::record(&plan)?;
                output::line(format_args!(
                    "Would rename repo '{}' to '{}'",
                    plan.previous_name, plan.name
                ));
                Ok(())
            } else {
                let result = rename_repo(&request)?;
                output::record(&result)?;
                output::line(format_args!(
                    "renamed repo '{}' to '{}'",
                    result.previous_name, result.name
                ));
                Ok(())
            }
        }
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
    let (repo, replacement, dry_run) = match command {
        ConstraintCommands::Show { repo } => (repo, None, false),
        ConstraintCommands::Set {
            repo,
            targets,
            constraints,
            dry_run: DryRunArgs { dry_run },
        } => (
            repo,
            Some(read_constraint_input(constraints.as_deref(), targets)?),
            dry_run,
        ),
        ConstraintCommands::Clear {
            repo,
            dry_run: DryRunArgs { dry_run },
        } => (repo, Some(RepoConstraints::default()), dry_run),
    };
    let constraints = match replacement {
        Some(constraints) => {
            let request = RepoConstraintsSetRequest {
                repo_root: repo,
                constraints,
                control: OperationControl::default(),
            };
            if dry_run {
                plan_set_repo_constraints(&request)?.constraints
            } else {
                set_repo_constraints(&request)?.constraints
            }
        }
        None => repo_constraints(&repo)?,
    };
    output::record(&constraints)?;
    output::line(format_args!(
        "{}",
        serde_json::to_string_pretty(&constraints)?
    ));
    Ok(())
}

fn rebuild_chunk_index(repo: &Path, dry_run: bool) -> crate::Result<()> {
    let request = RepoIndexRebuildRequest {
        repo_root: repo.to_path_buf(),
        control: OperationControl::default(),
    };
    let (record, complete) = if dry_run {
        let plan = plan_rebuild_repo_index(&request)?;
        (serde_json::to_value(&plan)?, plan.complete)
    } else {
        let result = rebuild_repo_index(&request)?;
        (serde_json::to_value(&result)?, result.plan.complete)
    };
    output::record(&record)?;
    output::line(format_args!("{}", serde_json::to_string_pretty(&record)?));
    if !complete {
        return Err(crate::Error::repo(
            "chunk index rebuild is incomplete; the index was not replaced",
        ));
    }
    Ok(())
}

fn status(repo: &Path, profile: &Profile) -> crate::Result<()> {
    let _status_span = profile.as_ref().and_then(|p| p.span("repo.status"));
    let status = repo_status(repo)?;
    output::record(&status)?;

    if !status.is_repo {
        match status.error.as_deref() {
            Some(error) => output::line(format_args!("status: {error}")),
            None => output::line(format_args!(
                "status: no gutch repo found at {}",
                status.repo_root.display()
            )),
        }
        return Ok(());
    }
    let remote_repo_id = status.remote_repo_id.as_deref().unwrap_or("(not linked)");
    output::line(format_args!("{SECTION_SEPARATOR}"));
    output::line(format_args!("Repository"));
    output::line(format_args!("name: {}", status.name));
    output::line(format_args!("remote_repo_id: {remote_repo_id}"));
    output::line(format_args!("repo_root: {}", status.repo_root.display()));
    output::line(format_args!("snapshots: {}", status.snapshots.len()));
    output::line(format_args!("bundles: {}", status.bundle_count));
    output::line(format_args!("bundle_bytes: {}", status.bundle_bytes));
    output::line(format_args!("artifacts: {}", status.artifact_count));
    output::line(format_args!("artifact_bytes: {}", status.artifact_bytes));
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
    Ok(())
}

fn verify(repo: &Path, snapshot_id: Option<&str>, strict: bool) -> crate::Result<()> {
    let mode = if strict {
        RepoVerifyMode::Strict
    } else {
        RepoVerifyMode::Fast
    };
    let result = verify_repo(&RepoVerifyRequest {
        repo_root: repo.to_path_buf(),
        snapshot_id: snapshot_id.map(str::to_owned),
        mode,
        control: OperationControl::default(),
    })?;
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

/// The credential a new repository is encrypted with, if any. A dry run
/// writes no key file.
fn init_credential(
    encrypt: bool,
    new_key_file: Option<&Path>,
    dry_run: bool,
) -> crate::Result<Option<RepoCredential>> {
    if encrypt {
        return environment_credential()?.map(Some).ok_or_else(|| {
            crate::Error::invalid_input(format!(
                "--encrypt needs {PASSWORD_VARIABLE} or {KEY_FILE_VARIABLE}"
            ))
        });
    }
    match new_key_file {
        Some(_) if dry_run => Ok(None),
        Some(path) => RepoCredential::create_key_file(path).map(Some),
        None => Ok(None),
    }
}

fn add_credential(
    repo: &Path,
    new_password: bool,
    new_key_file: Option<&Path>,
) -> crate::Result<()> {
    let credential = match new_key_file {
        Some(path) => RepoCredential::create_key_file(path)?,
        None => {
            debug_assert!(new_password);
            password_variable(NEW_PASSWORD_VARIABLE)?.ok_or_else(|| {
                crate::Error::invalid_input(format!("--new-password needs {NEW_PASSWORD_VARIABLE}"))
            })?
        }
    };
    add_repo_credential(repo, &credential)?;
    output::line(format_args!("added a credential to {}", repo.display()));
    Ok(())
}

pub(crate) fn init(
    path: &Path,
    force: bool,
    constraints: RepoConstraints,
    encryption: Option<RepoCredential>,
    dry_run: bool,
) -> crate::Result<()> {
    let mut request = RepoInitRequest {
        repo_root: path.to_path_buf(),
        force,
        constraints,
        encryption,
        control: OperationControl::default(),
    };
    let assessment = plan_init_repo(&request)?;
    if dry_run {
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
    if assessment.requires_confirmation && !request.force {
        if !confirm_repo_init_stdio(&assessment)? {
            return Err(crate::Error::Cancelled);
        }
        request.force = true;
    }

    let repository = init_repo(&request)?;

    output::line(format_args!(
        "created gutch repo '{}' at {}",
        repository.repository_name,
        path.display()
    ));
    Ok(())
}

fn confirm_repo_init_stdio(assessment: &RepoInitPlan) -> crate::Result<bool> {
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
) -> crate::Result<RepoConstraints> {
    let constraints = if let Some(path) = path {
        filesystem::read_json(path)?
    } else {
        RepoConstraints {
            targets,
            ..Default::default()
        }
    };
    constraints.validate_definition()?;
    Ok(constraints)
}
