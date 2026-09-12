use crate::cli::SECTION_SEPARATOR;
use crate::cli::SnapshotCommands;
use crate::cli::common::SnapshotCaptureManyStats;
use crate::cli::common::SnapshotCaptureStats;
use crate::cli::common::load_snapshot_manifest_from_repo;
use crate::cli::common::prompt_yes_no;
use crate::cli::common::write_report;
use crate::cli::progress::CliProgressSink;
use crate::cli::reporting::build_timings_summary;
use crate::cli::reporting::ensure_parent_directory;
use crate::cli::reporting::optional_output_paths;
use crate::cli::reporting::print_profile_summary;
use crate::cli::reporting::ratio_pct;
use crate::cli::tags::parse_cli_tags;

/// Maps an engine capture result to CLI stats. The generic command calls the
/// same additive engine path as every other caller instead of reimplementing
/// capture.
fn capture_stats_from_engine_result(
    repo_root: &str,
    input_path: &str,
    repo_name: &str,
    explicit_tags: &crate::manifest::SnapshotTagSet,
    storage_mode_cli: String,
    result: &crate::api::RepoSnapshotCaptureResult,
) -> SnapshotCaptureStats {
    SnapshotCaptureStats {
        command: "snapshot capture",
        repo_root: repo_root.to_string(),
        input_path: input_path.to_string(),
        snapshot_id: result.snapshot_id.clone(),
        manifest_path: result.manifest_path.display().to_string(),
        product_id: repo_name.to_string(),
        remote_repo_id: repo_name.to_string(),
        version_id: explicit_tags
            .get_exclusive("version")
            .unwrap_or_default()
            .to_string(),
        os: explicit_tags
            .get_exclusive("os")
            .unwrap_or_default()
            .to_string(),
        arch: explicit_tags
            .get_exclusive("arch")
            .unwrap_or_default()
            .to_string(),
        storage_mode: storage_mode_cli,
        file_count: result.file_count,
        total_logical_size: result.total_logical_size,
        total_compressed_size_estimate: result.total_compressed_size_estimate,
        required_chunk_count: result.required_chunk_count,
        required_bundle_count: result.required_bundle_count,
        generated_bundle_count: result.generated_bundle_count,
        generated_bundle_bytes: 0,
        generated_chunk_count: result.generated_chunk_count,
        generated_chunk_bytes: 0,
        reused_chunk_count: result.reused_chunk_count,
        new_chunk_count: result.new_chunk_count,
        reused_chunk_percent: ratio_pct(
            result.reused_chunk_count as u64,
            result.required_chunk_count as u64,
        ),
        reused_bundle_count: result
            .required_bundle_count
            .saturating_sub(result.generated_bundle_count),
        reused_bundle_percent: ratio_pct(
            result
                .required_bundle_count
                .saturating_sub(result.generated_bundle_count) as u64,
            result.required_bundle_count as u64,
        ),
        timings: build_timings_summary(&result.build_timings),
        outputs: optional_output_paths(&None),
    }
}

pub(crate) fn run(
    command: SnapshotCommands,
    profile: &crate::cli::Profile,
) -> crate::api::Result<()> {
    match command {
        SnapshotCommands::Diff { repo, from, to, json } => {
            let result = crate::api::diff_repo_snapshots(
                &repo, &from, &to, &crate::cli::control_with_profile(profile),
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                for entry in &result.changes {
                    let label = match entry.change {
                        crate::api::SnapshotChangeKind::Added => "Added",
                        crate::api::SnapshotChangeKind::Removed => "Removed",
                        crate::api::SnapshotChangeKind::Modified => "Modified",
                        crate::api::SnapshotChangeKind::Metadata => "Metadata",
                    };
                    println!("{label:<8} {:>+12} B  {:?}", entry.delta_bytes, entry.path);
                }
                println!("{} changes; total {:+} B", result.changes.len(), result.delta_bytes);
            }
        }
        SnapshotCommands::Capture {
            source_dirs,
            source_root,
            repo,
            tags,
            build_profile,
            runtime,
            stats_json_out,
            yes,
        } => {
            let single_report = source_root.is_none();
            let sources = crate::api::resolve_capture_sources(
                source_root,
                source_dirs,
                parse_cli_tags(&tags)?,
            )?;
            let (repo_paths, repo_manifest) = crate::api::load_gutch_repo_manifest(&repo)?;
            let requests = sources
                .iter()
                .map(|source| crate::api::RepoSnapshotCaptureRequest {
                    repo_root: repo.clone(),
                    source_dir: source.path.clone(),
                    explicit_tags: source.tags.clone(),
                    chunk_profile: build_profile.chunk_profile,
                    bundle_profile: build_profile.bundle_profile,
                    compression_algorithm: build_profile.compression_algorithm,
                    compression_level: build_profile.compression_level,
                    storage_mode: build_profile.storage_mode,
                    runtime_hints: runtime.hints(),
                    control: crate::cli::control_with_profile(profile)
                        .with_progress(CliProgressSink::new()),
                })
                .collect::<Vec<_>>();
            if !yes {
                println!("snapshot capture plan:");
                for plan in crate::api::plan_repo_snapshot_captures(&requests)? {
                    println!("  {}", crate::api::describe_snapshot_capture_plan(&plan));
                }
                if !prompt_yes_no("Proceed?", false)? {
                    return Err(crate::api::Error::manifest("snapshot capture cancelled"));
                }
            }
            let results = crate::api::capture_repo_snapshots(requests)?;
            let snapshot_stats = results
                .iter()
                .zip(&sources)
                .map(|(result, source)| {
                    println!("{}", result.outcome.message());
                    capture_stats_from_engine_result(
                        &repo.display().to_string(),
                        &source.path.display().to_string(),
                        &repo_manifest.name,
                        &source.tags,
                        build_profile.storage_mode.cli_name().to_string(),
                        result,
                    )
                })
                .collect::<Vec<_>>();
            if single_report && snapshot_stats.len() == 1 {
                let stats = SnapshotCaptureStats {
                    outputs: optional_output_paths(&stats_json_out),
                    ..snapshot_stats.into_iter().next().unwrap()
                };
                write_report(&stats_json_out, &stats, profile.as_deref())?;
            } else {
                let total_logical_size = snapshot_stats
                    .iter()
                    .map(|stats| stats.total_logical_size)
                    .sum();
                let total_compressed_size_estimate = snapshot_stats
                    .iter()
                    .map(|stats| stats.total_compressed_size_estimate)
                    .sum();
                let generated_bundle_count = snapshot_stats
                    .iter()
                    .map(|stats| stats.generated_bundle_count)
                    .sum();
                let generated_bundle_bytes = snapshot_stats
                    .iter()
                    .map(|stats| stats.generated_bundle_bytes)
                    .sum();
                let generated_chunk_count = snapshot_stats
                    .iter()
                    .map(|stats| stats.generated_chunk_count)
                    .sum();
                let generated_chunk_bytes = snapshot_stats
                    .iter()
                    .map(|stats| stats.generated_chunk_bytes)
                    .sum();
                let total_build_seconds = snapshot_stats
                    .iter()
                    .map(|stats| stats.timings.total_seconds)
                    .sum();
                let aggregate_stats = SnapshotCaptureManyStats {
                    command: "snapshot capture",
                    repo_root: repo_paths.repo_root.display().to_string(),
                    snapshot_count: snapshot_stats.len(),
                    total_logical_size,
                    total_compressed_size_estimate,
                    generated_bundle_count,
                    generated_bundle_bytes,
                    generated_chunk_count,
                    generated_chunk_bytes,
                    total_build_seconds,
                    snapshots: snapshot_stats,
                    outputs: optional_output_paths(&stats_json_out),
                };

                write_report(&stats_json_out, &aggregate_stats, profile.as_deref())?;
            }
            print_profile_summary(profile.as_deref());
        }

        SnapshotCommands::List { repo, json_out } => {
            let status = crate::api::check_gutch_repo(&repo)?;
            if !status.is_repo {
                return Err(crate::api::Error::manifest(format!(
                    "no gutch repo found at {}",
                    repo.display()
                )));
            }
            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                crate::filesystem::write(json_out, serde_json::to_vec_pretty(&status.snapshots)?)?;
            }
            println!("{SECTION_SEPARATOR}");
            println!("Snapshots");
            for snapshot in &status.snapshots {
                println!("{}", snapshot.snapshot_id);
            }
            println!("{SECTION_SEPARATOR}");
            println!("Total");
            println!("name: {}", status.name);
            println!("snapshots: {}", status.snapshots.len());
            println!("bundles: {}", status.bundle_count);
        }

        SnapshotCommands::Delete {
            repo,
            snapshot_id,
            yes,
        } => {
            let (repo_paths, _repo_manifest, _, _tm) =
                load_snapshot_manifest_from_repo(&repo, &snapshot_id)?;
            if !yes {
                let manifest_path = crate::api::snapshot_metadata_path(&repo_paths, &snapshot_id)?;
                eprintln!(
                    "WARNING: this will remove '{}' and related snapshot artifacts, and may orphan bundles. Use --yes to confirm.",
                    manifest_path.display()
                );
                return Err(crate::api::Error::manifest("deletion requires --yes flag"));
            }

            let progress = CliProgressSink::new();
            let control = crate::cli::control_with_profile(profile).with_progress(progress);
            let orphan_count =
                crate::api::delete_repo_snapshot_and_cleanup(&repo, &snapshot_id, &control)?;

            println!(
                "deleted snapshot '{}': removed manifest, {} orphaned bundles cleaned up",
                snapshot_id, orphan_count
            );
        }

        SnapshotCommands::Retag {
            repo,
            snapshot_id,
            tags,
            yes,
        } => {
            let new_tags = parse_cli_tags(&tags)?;
            if !yes {
                println!("retag snapshot '{snapshot_id}':");
                println!("  tags: {}", new_tags.to_canonical_strings().join(" "));
                if !prompt_yes_no("Proceed?", false)? {
                    return Err(crate::api::Error::manifest("snapshot retag cancelled"));
                }
            }
            let result = crate::api::retag_snapshot_by_id(&repo, &snapshot_id, new_tags)?;
            println!(
                "retagged snapshot '{}': previous tags '{}', new tags '{}'",
                result.snapshot_id,
                result.previous_tags.to_canonical_strings().join(" "),
                result.new_tags.to_canonical_strings().join(" ")
            );
        }

        SnapshotCommands::Reconstruct {
            repo,
            snapshot_id,
            destination,
            staging_dir,
            source_snapshot_id,
            source_dir,
            runtime,
            stats_json_out,
        } => crate::cli::installation::reconstruct(
            crate::cli::installation::ReconstructCommand {
                repo,
                snapshot_id,
                destination,
                staging_dir,
                source_snapshot_id,
                source_dir,
                runtime,
                stats_json_out,
            },
            profile,
        )?,

        SnapshotCommands::FetchReconstruct {
            repo,
            snapshot_id,
            destination,
            staging_dir,
            source_snapshot_id,
            source_dir,
            metadata_url,
            product_metadata_url,
            blueprint_url,
            repo_metadata_url,
            chunk_index_url,
            bundle_downloads_endpoint,
            session_id,
            bundles,
            artifact_url_manifest,
            runtime,
            transfer,
            stats_json_out,
        } => crate::cli::installation::fetch_and_reconstruct(
            crate::cli::installation::FetchAndReconstructCommand {
                fetch: crate::cli::installation::FetchCommand {
                    repo,
                    snapshot_id,
                    metadata_url,
                    product_metadata_url,
                    blueprint_url,
                    repo_metadata_url,
                    chunk_index_url,
                    bundle_downloads_endpoint,
                    session_id,
                    bundles,
                    artifact_url_manifest,
                    transfer_policy: transfer.policy(),
                },
                destination,
                staging_dir,
                source_snapshot_id,
                source_dir,
                runtime_hints: runtime.hints(),
                stats_json_out,
            },
            profile,
        )?,

        SnapshotCommands::Fetch {
            repo,
            snapshot_id,
            metadata_url,
            product_metadata_url,
            blueprint_url,
            repo_metadata_url,
            chunk_index_url,
            bundle_downloads_endpoint,
            session_id,
            bundles,
            artifact_url_manifest,
            transfer,
        } => crate::cli::installation::fetch(
            crate::cli::installation::FetchCommand {
                repo,
                snapshot_id,
                metadata_url,
                product_metadata_url,
                blueprint_url,
                repo_metadata_url,
                chunk_index_url,
                bundle_downloads_endpoint,
                session_id,
                bundles,
                artifact_url_manifest,
                transfer_policy: transfer.policy(),
            },
            profile,
        )?,

        SnapshotCommands::Estimate {
            source_dir,
            repo: _,
            tags,
        } => {
            let result = crate::api::estimate_snapshot(&source_dir, &parse_cli_tags(&tags)?)?;
            let est = &result.estimate;

            println!("fresh capture measurement (default profile):");
            println!("  files: {}", result.file_count);
            println!(
                "  logical size:  {:.2} MiB",
                result.total_logical_size as f64 / (1024.0 * 1024.0)
            );
            println!("  unique chunks: {}", est.unique_chunk_count);
            println!(
                "  unique chunk bytes: {:.2} MiB",
                est.unique_chunk_bytes as f64 / (1024.0 * 1024.0)
            );
            println!(
                "  encoded artifacts: {:.2} MiB",
                est.encoded_artifact_bytes as f64 / (1024.0 * 1024.0)
            );
            println!("  bundles: {}", est.bundle_count);
        }
    }

    Ok(())
}
