use crate::command_api::SECTION_SEPARATOR;
use crate::command_api::SnapshotCommands;
use crate::command_api::common::{
    SnapshotCaptureManyStats, SnapshotCaptureStats, load_snapshot_manifest_from_repo,
    prompt_yes_no, write_report,
};
use crate::command_api::progress::CliProgressSink;
use crate::command_api::reporting::{
    build_timings_summary, ensure_parent_directory, optional_output_paths, print_profile_summary,
    ratio_pct,
};
use g::snapshot::identity::parse_cli_tags;
use gutch as g;

/// Maps an engine capture result to CLI stats. The generic command calls the
/// same additive engine path as every other caller instead of reimplementing
/// capture.
fn capture_stats_from_engine_result(
    repo_root: &str,
    input_path: &str,
    repo_name: &str,
    explicit_tags: &g::manifest::SnapshotTagSet,
    storage_mode_cli: String,
    result: &g::RepoSnapshotCaptureResult,
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
    profile: &crate::command_api::Profile,
) -> g::Result<()> {
    match command {
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
            let sources =
                g::api::resolve_capture_sources(source_root, source_dirs, parse_cli_tags(&tags)?)?;
            let (repo_paths, repo_manifest) = g::load_gutch_repo_manifest(&repo)?;
            let requests = sources
                .iter()
                .map(|source| g::RepoSnapshotCaptureRequest {
                    repo_root: repo.clone(),
                    source_dir: source.path.clone(),
                    explicit_tags: source.tags.clone(),
                    chunk_profile: build_profile.chunk_profile,
                    bundle_profile: build_profile.bundle_profile,
                    compression_algorithm: build_profile.compression_algorithm,
                    compression_level: build_profile.compression_level,
                    storage_mode: build_profile.storage_mode,
                    runtime_hints: runtime.hints(),
                    control: crate::command_api::control_with_profile(profile)
                        .with_progress(CliProgressSink::new()),
                })
                .collect::<Vec<_>>();
            if !yes {
                println!("snapshot capture plan:");
                for plan in g::api::plan_repo_snapshot_captures(&requests)? {
                    println!("  {}", g::describe_snapshot_capture_plan(&plan));
                }
                if !prompt_yes_no("Proceed?", false)? {
                    return Err(g::Error::manifest("snapshot capture cancelled"));
                }
            }
            let results = g::api::capture_repo_snapshots(requests)?;
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
                write_report(&stats_json_out, &stats)?;
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

                write_report(&stats_json_out, &aggregate_stats)?;
            }
            print_profile_summary(profile.as_deref());
        }

        SnapshotCommands::List { repo, json_out } => {
            let status = g::check_gutch_repo(&repo)?;
            if !status.is_repo {
                return Err(g::Error::manifest(format!(
                    "no gutch repo found at {}",
                    repo.display()
                )));
            }
            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                crate::helpers::write(json_out, serde_json::to_vec_pretty(&status.snapshots)?)?;
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
                let manifest_path = g::snapshot_metadata_path(&repo_paths, &snapshot_id)?;
                eprintln!(
                    "WARNING: this will remove '{}' and related snapshot artifacts, and may orphan bundles. Use --yes to confirm.",
                    manifest_path.display()
                );
                return Err(g::Error::manifest("deletion requires --yes flag"));
            }

            let progress = CliProgressSink::new();
            let control = crate::command_api::control_with_profile(profile).with_progress(progress);
            let orphan_count = g::delete_repo_snapshot_and_cleanup(&repo, &snapshot_id, &control)?;

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
                    return Err(g::Error::manifest("snapshot retag cancelled"));
                }
            }
            let result = g::retag_snapshot_by_id(&repo, &snapshot_id, new_tags)?;
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
        } => crate::command_api::snapshot_ops::reconstruct(
            crate::command_api::snapshot_ops::ReconstructCommand {
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
        } => crate::command_api::snapshot_ops::fetch_and_reconstruct(
            crate::command_api::snapshot_ops::FetchAndReconstructCommand {
                fetch: crate::command_api::snapshot_ops::FetchCommand {
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
        } => crate::command_api::snapshot_ops::fetch(
            crate::command_api::snapshot_ops::FetchCommand {
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
            let result = g::api::estimate_snapshot(&source_dir, &parse_cli_tags(&tags)?)?;
            let est = &result.estimate;

            println!("fresh reconstruction estimate:");
            println!("  files: {}", result.file_count);
            println!(
                "  logical size:  {:.2} MiB",
                result.total_logical_size as f64 / (1024.0 * 1024.0)
            );
            println!("  missing chunks: {}", est.missing_chunk_count);
            println!(
                "  missing logical: {:.2} MiB",
                est.missing_logical_bytes as f64 / (1024.0 * 1024.0)
            );
            println!(
                "  missing compressed: {:.2} MiB",
                est.missing_compressed_bytes as f64 / (1024.0 * 1024.0)
            );
            println!("  estimated bundles: {}", est.required_bundle_count);
            println!("  fragmentation score: {:.1}", est.fragmentation_score);
        }
    }

    Ok(())
}
