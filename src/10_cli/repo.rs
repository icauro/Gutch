use crate::cli::RepoCommands;
use crate::cli::SECTION_SEPARATOR;
use crate::cli::command_support::prompt_yes_no;
use crate::cli::reporting::ensure_parent_directory;
use std::path::Path;

pub(crate) fn run(
    command: RepoCommands,
    remote_provider: Option<&dyn crate::sync::remote::RemoteRepositoryCommandProvider>,
    profile: &crate::cli::Profile,
) -> crate::Result<()> {
    match command {
        RepoCommands::Constraints { command } => {
            let (repo, replacement) = match command {
                crate::cli::ConstraintCommands::Show { repo } => (repo, None),
                crate::cli::ConstraintCommands::Set {
                    repo,
                    targets,
                    constraints,
                } => (
                    repo,
                    Some(read_constraint_input(constraints.as_deref(), targets)?),
                ),
                crate::cli::ConstraintCommands::Clear { repo } => (
                    repo,
                    Some(crate::manifest::constraints::RepositoryConstraints::default()),
                ),
            };
            if let Some(constraints) = replacement {
                let constraints =
                    crate::repo::updates::plan_repository_constraints(&repo, constraints)?;
                if !crate::cli::output::dry_run() {
                    crate::repo::updates::update_repository_constraints(
                        &repo,
                        constraints.clone(),
                    )?;
                }
                crate::cli::output::record(&constraints)?;
                crate::cli::output::line(format_args!(
                    "{}",
                    serde_json::to_string_pretty(&constraints)?
                ));
            } else {
                let constraints = crate::repo::updates::read_repository_constraints(&repo)?;
                crate::cli::output::record(&constraints)?;
                crate::cli::output::line(format_args!(
                    "{}",
                    serde_json::to_string_pretty(&constraints)?
                ));
            }
        }
        RepoCommands::Index {
            command:
                crate::cli::arguments::RepoIndexCommands::Rebuild {
                    repo,
                    apply,
                    json_out,
                },
        } => {
            let result = crate::repo::recovery::rebuild_index(
                &repo,
                apply && !crate::cli::output::dry_run(),
                &crate::progress::OperationControl::default(),
            )?;
            crate::cli::output::record(&result)?;
            let json = serde_json::to_vec_pretty(&result)?;
            if let Some(path) = json_out {
                ensure_parent_directory(&path)?;
                crate::filesystem::write(&path, &json)?;
            }
            crate::cli::output::line(format_args!("{}", String::from_utf8_lossy(&json)));
            if !result.complete {
                return Err(crate::Error::manifest(
                    "chunk index rebuild is incomplete; the index was not replaced",
                ));
            }
        }
        RepoCommands::Init {
            path,
            force,
            targets,
            constraints,
        } => {
            let constraints = read_constraint_input(constraints.as_deref(), targets)?;
            crate::cli::repo::init_repo(&path, force, constraints)?;
        }

        RepoCommands::Push { repo, yes } => {
            crate::cli::repo_remote::push(repo, yes, remote_provider, profile)?;
        }

        RepoCommands::Pull {
            repo,
            remote,
            on_conflict,
            prune,
            yes,
        } => {
            crate::cli::repo_remote::pull(
                repo,
                remote,
                on_conflict,
                prune,
                yes,
                remote_provider,
                profile,
            )?;
        }

        RepoCommands::Remote { command } => match command {
            crate::cli::RemoteCommands::List => crate::cli::repo_remote::list(remote_provider)?,
            crate::cli::RemoteCommands::Create { repo, scope_id } => {
                crate::cli::repo_remote::create(repo, scope_id, remote_provider)?
            }
            crate::cli::RemoteCommands::Bind {
                repo,
                remote_repo_id,
            } => crate::cli::repo_remote::bind(repo, remote_repo_id, remote_provider)?,
            crate::cli::RemoteCommands::Unbind {
                repo,
                delete_remote,
            } => crate::cli::repo_remote::unbind(repo, delete_remote, remote_provider)?,
        },
        RepoCommands::Rename { repo, name } => {
            if crate::cli::output::dry_run() {
                let name = crate::repo::updates::plan_repo_rename(&repo, &name)?;
                crate::cli::output::record(
                    &serde_json::json!({"operation": "rename", "name": name}),
                )?;
                crate::cli::output::line(format_args!(
                    "Would rename {} to {}",
                    repo.display(),
                    name
                ));
            } else {
                crate::cli::repo::rename_repo(&repo, &name)?;
            }
        }
        RepoCommands::Tags {
            command: crate::cli::RepoTagsCommands::References { repo, json_out },
        } => {
            crate::cli::repo_remote::tag_references(repo, json_out, remote_provider)?;
        }

        RepoCommands::Status { repo, json_out } => {
            let _check_span = profile.as_ref().and_then(|p| p.span("repo.status"));
            let status = crate::repo::status::read_repo_status(&repo)?;
            crate::cli::output::record(&status)?;
            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                crate::filesystem::write(json_out, serde_json::to_vec_pretty(&status)?)?;
            }

            if status.is_repo {
                crate::cli::output::line(format_args!("{SECTION_SEPARATOR}"));
                crate::cli::output::line(format_args!("Snapshots"));
                for snapshot in &status.snapshots {
                    crate::cli::output::line(format_args!(
                        "{} | version={} | os={} | arch={} | files={} | missing_artifacts={}",
                        snapshot.snapshot_id,
                        snapshot.version_tag,
                        snapshot.os,
                        snapshot.arch,
                        snapshot.file_count,
                        snapshot.missing_artifact_count
                    ));
                }
                crate::cli::output::line(format_args!("{SECTION_SEPARATOR}"));
                crate::cli::output::line(format_args!("Total"));
                crate::cli::output::line(format_args!("name: {}", status.name));
                crate::cli::output::line(format_args!(
                    "remote_repo_id: {}",
                    if status.remote_repo_id.is_empty() {
                        "(not linked)"
                    } else {
                        &status.remote_repo_id
                    }
                ));
                crate::cli::output::line(format_args!("repo_path: {}", status.repo_path));
                crate::cli::output::line(format_args!("snapshots: {}", status.snapshots.len()));
                crate::cli::output::line(format_args!("bundles: {}", status.bundle_count));
                crate::cli::output::line(format_args!("bundle_bytes: {}", status.bundle_bytes));
                crate::cli::output::line(format_args!("artifacts: {}", status.artifact_count));
                crate::cli::output::line(format_args!("artifact_bytes: {}", status.artifact_bytes));
            } else if let Some(error) = status.error.as_deref() {
                crate::cli::output::line(format_args!("status: {}", error));
            } else {
                crate::cli::output::line(format_args!(
                    "status: no gutch repo found at {}",
                    status.repo_path
                ));
            }
        }

        RepoCommands::Verify {
            repo,
            snapshot_id,
            strict,
            json_out,
        } => {
            let mode = if strict {
                crate::repo::verify::RepoArtifactVerification::Identity
            } else {
                crate::repo::verify::RepoArtifactVerification::Structure
            };
            let result = crate::repo::verify::verify_gutch_repo_artifacts(
                &repo,
                snapshot_id.as_deref(),
                mode,
            )?;
            crate::cli::output::record(&result)?;
            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                crate::filesystem::write(json_out, serde_json::to_vec_pretty(&result)?)?;
            }

            crate::cli::output::line(format_args!("{SECTION_SEPARATOR}"));
            crate::cli::output::line(format_args!("Verify ({})", result.mode));
            crate::cli::output::line(format_args!("ok: {}", result.ok));
            crate::cli::output::line(format_args!(
                "snapshots: {} ok={}",
                result.snapshot_count, result.snapshots_ok
            ));
            for snapshot_result in &result.snapshots {
                crate::cli::output::line(format_args!(
                    "{} | ok={} | missing_artifacts={} | corrupt_artifacts={}",
                    snapshot_result.snapshot_id,
                    snapshot_result.ok,
                    snapshot_result.missing_artifact_count,
                    snapshot_result.corrupt_artifact_count
                ));
                for missing in &snapshot_result.missing_artifacts {
                    crate::cli::output::line(format_args!("  missing artifact: {missing}"));
                }
                for corrupt in &snapshot_result.corrupt_artifacts {
                    crate::cli::output::line(format_args!("  corrupt artifact: {corrupt}"));
                }
            }
            if !result.ok {
                return Err(crate::Error::verify("repository verification failed"));
            }
        }
    }

    Ok(())
}

pub(crate) fn init_repo(
    path: &Path,
    force: bool,
    constraints: crate::manifest::constraints::RepositoryConstraints,
) -> crate::Result<()> {
    constraints.validate_definition()?;
    let assessment = crate::repo::init_policy::assess_repo_init_path(path)?;
    if crate::cli::output::dry_run() {
        crate::cli::output::record(&assessment)?;
        crate::cli::output::line(format_args!(
            "Would initialize repository at {}",
            path.display()
        ));
        for risk in &assessment.risks {
            crate::cli::output::line(format_args!("{}", risk.message));
        }
        return Ok(());
    }
    let mut proceed_force = force;
    if assessment.requires_confirmation && !proceed_force {
        if !confirm_repo_init_stdio(&assessment)? {
            return Err(crate::Error::manifest("repo creation cancelled"));
        }
        proceed_force = true;
    }

    let repo_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("base");
    crate::repo::init_policy::initialize_repo(
        path,
        crate::repo::init_policy::RepoInitOptions {
            force: proceed_force,
            constraints,
        },
    )?;

    crate::cli::output::line(format_args!(
        "created gutch repo '{}' at {}",
        repo_name,
        path.display()
    ));
    Ok(())
}

pub(crate) fn rename_repo(repo: &Path, name: &str) -> crate::Result<()> {
    crate::repo::updates::rename_repo(repo, name)?;
    crate::cli::output::line(format_args!(
        "renamed repo '{}' to '{}'",
        repo.display(),
        name.trim()
    ));
    Ok(())
}

fn confirm_repo_init_stdio(
    assessment: &crate::repo::init_policy::RepoInitAssessment,
) -> crate::Result<bool> {
    crate::cli::output::line(format_args!(""));
    for risk in &assessment.risks {
        crate::cli::output::line(format_args!("{}", risk.message));
    }
    if assessment.path_kind == crate::repo::init_policy::RepoInitPathKind::ExistingRepo {
        if let Some(name) = assessment.repo_name.as_deref() {
            crate::cli::output::line(format_args!("  existing repo: {name}"));
        }
        if let Some(snapshot_count) = assessment.snapshot_count {
            crate::cli::output::line(format_args!("  snapshots: {snapshot_count}"));
        }
        if let Some(bundle_count) = assessment.bundle_count {
            crate::cli::output::line(format_args!("  bundles: {bundle_count}"));
        }
    } else if assessment.path_kind == crate::repo::init_policy::RepoInitPathKind::NonEmpty {
        crate::cli::output::line(format_args!("  entries: {}", assessment.entry_count));
    }

    prompt_yes_no(
        "Delete existing Gutch repo files and create a new repo?",
        false,
    )
}

fn read_constraint_input(
    path: Option<&Path>,
    targets: Vec<crate::manifest::constraints::FilesystemTarget>,
) -> crate::Result<crate::manifest::constraints::RepositoryConstraints> {
    let constraints = if let Some(path) = path {
        serde_json::from_str(&crate::filesystem::read_to_string(path)?)?
    } else {
        crate::manifest::constraints::RepositoryConstraints {
            targets,
            ..Default::default()
        }
    };
    constraints.validate_definition()?;
    Ok(constraints)
}
