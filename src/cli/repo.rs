use crate::cli::RepoCommands;
use crate::cli::SECTION_SEPARATOR;
use crate::cli::actions;
use crate::cli::reporting::ensure_parent_directory;
use crate::cli::reporting::print_repo_check_timings;

pub(crate) fn run(
    command: RepoCommands,
    remote_provider: Option<&dyn crate::api::RemoteRepositoryCommandProvider>,
    profile: &crate::cli::Profile,
) -> crate::api::Result<()> {
    match command {
        RepoCommands::RegenerateChunkMap {
            repo,
            publish,
            json_out,
        } => {
            let result = crate::api::regenerate_chunk_map(
                &repo,
                publish,
                &crate::progress::GutchOperationControl::default(),
            )?;
            let json = serde_json::to_vec_pretty(&result)?;
            if let Some(path) = json_out {
                ensure_parent_directory(&path)?;
                crate::filesystem::write(&path, &json)?;
            }
            println!("{}", String::from_utf8_lossy(&json));
            if !result.complete {
                return Err(crate::Error::manifest(
                    "chunk map regeneration is incomplete; no index was published",
                ));
            }
        }
        RepoCommands::Init { path, force } => {
            actions::repo::init_repo(&path, force)?;
        }

        RepoCommands::Push { repo, yes } => {
            crate::cli::remote::push(repo, yes, remote_provider, profile)?;
        }

        RepoCommands::Pull {
            repo,
            remote,
            on_conflict,
            prune,
            yes,
        } => {
            crate::cli::remote::pull(
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
            command: crate::cli::RepoTagsCommands::References { repo, json_out },
        } => {
            crate::cli::remote::tag_references(repo, json_out, remote_provider)?;
        }

        RepoCommands::Status { repo, json_out } => {
            let status = crate::api::check_gutch_repo(&repo)?;
            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                crate::filesystem::write(json_out, serde_json::to_vec_pretty(&status)?)?;
            }

            if status.is_repo {
                println!("{SECTION_SEPARATOR}");
                println!("Snapshots");
                for snapshot in &status.snapshots {
                    println!(
                        "{} | version={} | os={} | arch={} | files={} | missing_artifacts={}",
                        snapshot.snapshot_id,
                        snapshot.version_id,
                        snapshot.os,
                        snapshot.arch,
                        snapshot.file_count,
                        snapshot.missing_artifact_count
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
                println!("artifacts: {}", status.artifact_count);
                println!("artifact_bytes: {}", status.artifact_bytes);
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
                crate::api::VerifyMode::Strict
            } else {
                crate::api::VerifyMode::Fast
            };
            let result =
                crate::api::verify_gutch_repo_artifacts(&repo, snapshot_id.as_deref(), mode)?;
            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                crate::filesystem::write(json_out, serde_json::to_vec_pretty(&result)?)?;
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
                    "{} | ok={} | missing_artifacts={} | corrupt_artifacts={}",
                    snapshot_result.snapshot_id,
                    snapshot_result.ok,
                    snapshot_result.missing_artifact_count,
                    snapshot_result.corrupt_artifact_count
                );
                for missing in &snapshot_result.missing_artifacts {
                    println!("  missing artifact: {missing}");
                }
                for corrupt in &snapshot_result.corrupt_artifacts {
                    println!("  corrupt artifact: {corrupt}");
                }
            }
            if !result.ok {
                return Err(crate::api::Error::verify("repository verification failed"));
            }
        }

        RepoCommands::Sources { command } => {
            crate::cli::snapshot_sources::run(command, profile)?;
        }
    }

    Ok(())
}
