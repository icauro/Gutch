use crate::cli::RepoCommands;
use crate::cli::SECTION_SEPARATOR;
use crate::cli::actions;
use crate::cli::common::load_optional_rules;
use crate::cli::reporting::{ensure_parent_directory, print_repo_check_timings};
use gutch as g;

pub(crate) fn run(
    command: RepoCommands,
    remote_provider: Option<&dyn g::RemoteRepositoryCommandProvider>,
) -> g::Result<()> {
    match command {
        RepoCommands::Init {
            path,
            schema,
            force,
        } => {
            actions::repo::init_repo(&path, schema.as_ref(), force)?;
        }

        RepoCommands::Push { repo, yes } => {
            crate::cli::remote::push(repo, yes, remote_provider)?;
        }

        RepoCommands::Pull {
            repo,
            remote,
            on_conflict,
            prune,
            yes,
        } => {
            crate::cli::remote::pull(repo, remote, on_conflict, prune, yes, remote_provider)?;
        }

        RepoCommands::Remote { command } => match command {
            crate::cli::RemoteCommands::List => {
                crate::cli::remote::list(remote_provider)?;
            }
        },

        RepoCommands::Link { repo, scope_id } => {
            crate::cli::remote::link(repo, scope_id, remote_provider)?;
        }

        RepoCommands::Bind {
            repo,
            remote_repo_id,
        } => {
            crate::cli::remote::bind(repo, remote_repo_id)?;
        }

        RepoCommands::Unlink {
            repo,
            delete_remote,
        } => {
            crate::cli::remote::unlink(repo, delete_remote, remote_provider)?;
        }

        RepoCommands::Name {
            command: crate::cli::RepoNameCommands::Set { repo, name },
        } => {
            actions::repo::rename_repo(&repo, &name)?;
        }

        RepoCommands::Tags {
            command:
                crate::cli::RepoTagsCommands::Schema {
                    command:
                        crate::cli::RepoTagSchemaCommands::Set {
                            repo,
                            schema,
                            clear,
                        },
                },
        } => {
            if clear && schema.is_some() {
                return Err(g::Error::manifest(
                    "use either --schema <path> or --clear, not both",
                ));
            }

            let (repo_paths, mut repo_manifest) = g::load_gutch_repo_manifest(&repo)?;
            repo_manifest.snapshot_tag_schema = if clear {
                None
            } else {
                load_optional_rules(&schema)?
            };
            g::write_gutch_repo_manifest(&repo_paths, &repo_manifest)?;
            println!(
                "updated snapshot tag schema at {}",
                repo_paths.repo_metadata_path.display()
            );
        }

        RepoCommands::Tags {
            command:
                crate::cli::RepoTagsCommands::Requirements {
                    repo,
                    check,
                    json_out,
                },
        } => {
            crate::cli::remote::tag_requirements(repo, check, json_out, remote_provider)?;
        }

        RepoCommands::Status { repo, json_out } => {
            let status = g::check_gutch_repo(&repo)?;
            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                crate::helpers::write(json_out, serde_json::to_vec_pretty(&status)?)?;
            }

            if status.is_repo {
                println!("{SECTION_SEPARATOR}");
                println!("Snapshots");
                for snapshot in &status.snapshots {
                    println!(
                        "{} | version={} | os={} | arch={} | files={} | missing_bundles={}",
                        snapshot.snapshot_id,
                        snapshot.version_id,
                        snapshot.os,
                        snapshot.arch,
                        snapshot.file_count,
                        snapshot.missing_bundle_count
                    );
                }
                println!("{SECTION_SEPARATOR}");
                println!("Total");
                println!("name: {}", status.name);
                println!(
                    "remote_repo_id: {}",
                    if status.remote_repo_id.is_empty() {
                        "(not linked)"
                    } else {
                        &status.remote_repo_id
                    }
                );
                println!("repo_path: {}", status.repo_path);
                println!("snapshots: {}", status.snapshots.len());
                println!("bundles: {}", status.bundle_count);
                println!("bundle_bytes: {}", status.bundle_bytes);
                if let Some(repo_timings) = status.timings.as_ref() {
                    println!();
                    print_repo_check_timings(repo_timings);
                }
            } else if let Some(error) = status.error.as_deref() {
                println!("status: {}", error);
            } else {
                println!("status: no gutch repo found at {}", status.repo_path);
            }
        }

        RepoCommands::Verify {
            repo,
            snapshot_id,
            strict,
            json_out,
        } => {
            let mode = if strict {
                g::VerifyMode::Strict
            } else {
                g::VerifyMode::Fast
            };
            let result = g::verify_gutch_repo_artifacts(&repo, snapshot_id.as_deref(), mode)?;
            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                crate::helpers::write(json_out, serde_json::to_vec_pretty(&result)?)?;
            }

            println!("{SECTION_SEPARATOR}");
            println!("Verify ({})", result.mode);
            println!("ok: {}", result.ok);
            println!(
                "snapshots: {} ok={}",
                result.snapshot_count, result.snapshots_ok
            );
            for snapshot_result in &result.snapshots {
                println!(
                    "{} | ok={} | missing={} | corrupt={}",
                    snapshot_result.snapshot_id,
                    snapshot_result.ok,
                    snapshot_result.missing_bundle_count,
                    snapshot_result.corrupt_bundle_count
                );
                for missing in &snapshot_result.missing_bundles {
                    println!("  missing bundle: {missing}");
                }
                for corrupt in &snapshot_result.corrupt_bundles {
                    println!("  corrupt bundle: {corrupt}");
                }
            }
        }

        RepoCommands::Sources { command } => {
            crate::cli::snapshot_sources::run(command)?;
        }
    }

    Ok(())
}
