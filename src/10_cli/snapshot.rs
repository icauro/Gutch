use crate::cli::SECTION_SEPARATOR;
use crate::cli::SnapshotCommands;
use crate::cli::command_support::SnapshotCaptureManyStats;
use crate::cli::command_support::SnapshotCaptureStats;
use crate::cli::command_support::load_snapshot_manifest_from_repo;
use crate::cli::command_support::prompt_yes_no;
use crate::cli::command_support::write_report;
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
    repository: &crate::manifest::gutch_repo::GutchRepoManifest,
    explicit_tags: &crate::manifest::SnapshotTagSet,
    storage_mode_cli: String,
    result: &crate::capture::contracts::SnapshotCaptureResult,
    mode: crate::profile::GutchProfileMode,
) -> SnapshotCaptureStats {
    SnapshotCaptureStats {
        command: "snapshot capture",
        repo_root: repo_root.to_string(),
        input_path: input_path.to_string(),
        snapshot_id: result.snapshot_id.clone(),
        manifest_path: result.manifest_path.display().to_string(),
        repo_name: repository.name.clone(),
        remote_repo_id: (!repository.remote_repo_id.is_empty())
            .then(|| repository.remote_repo_id.clone()),
        version_tag: explicit_tags.get("version").unwrap_or_default().to_string(),
        os: explicit_tags.get("os").unwrap_or_default().to_string(),
        arch: explicit_tags.get("arch").unwrap_or_default().to_string(),
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
        timings: build_timings_summary(&result.build_timings, mode),
        outputs: optional_output_paths(&None),
    }
}

pub(crate) fn run(command: SnapshotCommands, profile: &crate::cli::Profile) -> crate::Result<()> {
    match command {
        SnapshotCommands::Diff { repo, from, to } => {
            let result = crate::repo::snapshot_manifests::diff_repo_snapshots(
                &repo,
                &from,
                &to,
                &crate::cli::control_with_profile(profile),
            )?;
            crate::cli::output::record(&result)?;
            {
                for entry in &result.changes {
                    let label = match entry.change {
                        crate::snapshot::diff::SnapshotChangeKind::Added => "Added",
                        crate::snapshot::diff::SnapshotChangeKind::Removed => "Removed",
                        crate::snapshot::diff::SnapshotChangeKind::Modified => "Modified",
                        crate::snapshot::diff::SnapshotChangeKind::Metadata => "Metadata",
                    };
                    crate::cli::output::line(format_args!(
                        "{label:<8} {:>+12} B  {:?}",
                        entry.delta_bytes, entry.path
                    ));
                }
                crate::cli::output::line(format_args!(
                    "{} changes; total {:+} B",
                    result.changes.len(),
                    result.delta_bytes
                ));
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
            let sources = crate::capture::sources::resolve_capture_sources(
                source_root.clone(),
                source_dirs,
                parse_cli_tags(&tags)?,
            )?;
            let (repo_paths, repo_manifest) = crate::repo::read::load_gutch_repo_manifest(&repo)?;
            let requests = sources
                .iter()
                .map(|source| crate::capture::contracts::SnapshotCaptureRequest {
                    repo_root: repo.clone(),
                    source_dir: source.path.clone(),
                    explicit_tags: source.tags.clone(),
                    chunk_profile: build_profile.chunk_profile,
                    chunker: build_profile.chunker,
                    bundle_profile: build_profile.bundle_profile,
                    compression_algorithm: build_profile.compression_algorithm,
                    compression_level: build_profile.compression_level,
                    storage_mode: build_profile.storage_mode,
                    runtime_hints: runtime.hints(),
                    control: crate::cli::control_with_profile(profile)
                        .with_progress(CliProgressSink::new()),
                })
                .collect::<Vec<_>>();
            if crate::cli::output::dry_run() {
                let plans = crate::capture::sources::plan_snapshot_captures(&requests)?;
                crate::cli::output::record(&plans)?;
                for plan in plans {
                    crate::cli::output::line(format_args!(
                        "{}",
                        crate::capture::plan::describe_snapshot_capture_plan(&plan)
                    ));
                }
                return Ok(());
            }
            if !yes {
                crate::cli::output::line(format_args!("snapshot capture plan:"));
                for plan in crate::capture::sources::plan_snapshot_captures(&requests)? {
                    crate::cli::output::line(format_args!(
                        "  {}",
                        crate::capture::plan::describe_snapshot_capture_plan(&plan)
                    ));
                }
                if !prompt_yes_no("Proceed?", false)? {
                    return Err(crate::Error::manifest("snapshot capture cancelled"));
                }
            }
            let results = crate::capture::sources::capture_snapshots(requests)?;
            if let Some(source_root) = source_root {
                crate::capture::source_config::initialize_source_config(&source_root)?;
            }
            let mut snapshot_stats = Vec::with_capacity(results.len());
            for (result, source) in results.iter().zip(&sources) {
                crate::cli::output::line(format_args!("{}", result.outcome.message()));
                snapshot_stats.push(capture_stats_from_engine_result(
                    &repo.display().to_string(),
                    &source.path.display().to_string(),
                    &repo_manifest,
                    &source.tags,
                    build_profile.storage_mode.cli_name().to_string(),
                    result,
                    crate::cli::reporting::profile_mode(profile.as_deref()),
                ));
            }
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
                    .sum::<Option<f64>>()
                    .filter(|_| {
                        crate::cli::reporting::profile_mode(profile.as_deref())
                            == crate::profile::GutchProfileMode::Detailed
                    });
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
            let status = crate::repo::status::read_repo_status(&repo)?;
            crate::cli::output::record(&status.snapshots)?;
            if !status.is_repo {
                return Err(crate::Error::manifest(format!(
                    "no gutch repo found at {}",
                    repo.display()
                )));
            }
            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                crate::filesystem::write(json_out, serde_json::to_vec_pretty(&status.snapshots)?)?;
            }
            crate::cli::output::line(format_args!("{SECTION_SEPARATOR}"));
            crate::cli::output::line(format_args!("Snapshots"));
            for snapshot in &status.snapshots {
                crate::cli::output::line(format_args!("{}", snapshot.snapshot_id));
            }
            crate::cli::output::line(format_args!("{SECTION_SEPARATOR}"));
            crate::cli::output::line(format_args!("Total"));
            crate::cli::output::line(format_args!("name: {}", status.name));
            crate::cli::output::line(format_args!("snapshots: {}", status.snapshots.len()));
            crate::cli::output::line(format_args!("bundles: {}", status.bundle_count));
        }

        SnapshotCommands::Delete {
            repo,
            snapshot_id,
            yes,
        } => {
            load_snapshot_manifest_from_repo(&repo, &snapshot_id)?;
            if crate::cli::output::dry_run() {
                crate::cli::output::record(
                    &serde_json::json!({"operation": "delete", "snapshot": snapshot_id}),
                )?;
                crate::cli::output::line(format_args!(
                    "Would delete snapshot {} and clean up unreferenced artifacts",
                    snapshot_id
                ));
                return Ok(());
            }
            if !yes
                && !prompt_yes_no(
                    "Delete this snapshot and clean up unreferenced artifacts?",
                    false,
                )?
            {
                return Err(crate::Error::manifest("snapshot deletion cancelled"));
            }

            let progress = CliProgressSink::new();
            let control = crate::cli::control_with_profile(profile).with_progress(progress);
            let orphan_count = crate::repo::snapshots::delete_repo_snapshot_and_cleanup(
                &repo,
                &snapshot_id,
                &control,
            )?;

            crate::cli::output::line(format_args!(
                "deleted snapshot '{}': removed manifest, {} orphaned artifacts cleaned up",
                snapshot_id, orphan_count
            ));
        }

        SnapshotCommands::Tags {
            command:
                crate::cli::arguments::SnapshotTagsCommands::Replace {
                    repo,
                    snapshot_id,
                    tags,
                    tags_file,
                    yes,
                },
        } => {
            let single_snapshot = snapshot_id.is_some();
            let replacements = if let Some(path) = tags_file {
                serde_json::from_str(&crate::filesystem::read_to_string(&path)?)?
            } else {
                std::collections::BTreeMap::from([(
                    snapshot_id
                        .ok_or_else(|| crate::Error::manifest("select a snapshot or tags file"))?,
                    parse_cli_tags(&tags)?,
                )])
            };
            if replacements.is_empty() {
                return Err(crate::Error::manifest("select at least one snapshot"));
            }
            let preview = crate::repo::snapshots::plan_snapshot_tag_updates(&repo, &replacements)?;
            for change in &preview {
                crate::cli::output::line(format_args!(
                    "{}: {} -> {}",
                    change.snapshot_id,
                    change.previous_tags.to_canonical_strings().join(" "),
                    change.new_tags.to_canonical_strings().join(" ")
                ));
            }
            if crate::cli::output::dry_run() {
                crate::cli::output::record(&preview)?;
                return Ok(());
            }
            if !yes && !prompt_yes_no("Apply these tag changes?", false)? {
                return Err(crate::Error::manifest("snapshot tag replacement cancelled"));
            }
            let results = crate::repo::snapshots::retag_snapshots(&repo, replacements)?;
            if single_snapshot {
                crate::cli::output::record(&results[0])?;
            } else {
                crate::cli::output::record(&results)?;
            }
        }

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
            artifacts,
            artifact_url_manifest,
            transfer,
        } => crate::cli::snapshot_transfer::fetch(
            crate::cli::snapshot_transfer::FetchCommand {
                repo,
                snapshot_id,
                metadata_url,
                product_metadata_url,
                blueprint_url,
                repo_metadata_url,
                chunk_index_url,
                bundle_downloads_endpoint,
                session_id,
                artifacts,
                artifact_url_manifest,
                transfer_policy: transfer.policy(),
            },
            profile,
        )?,

        SnapshotCommands::Measure { source_dir, tags } => {
            let result =
                crate::capture::estimate::estimate_snapshot(&source_dir, &parse_cli_tags(&tags)?)?;
            crate::cli::output::record(
                &serde_json::json!({"file_count": result.file_count, "logical_bytes": result.total_logical_size, "unique_chunks": result.estimate.unique_chunk_count, "encoded_artifact_bytes": result.estimate.encoded_artifact_bytes}),
            )?;
            let est = &result.estimate;

            crate::cli::output::line(format_args!(
                "fresh capture measurement (default packaging presets):"
            ));
            crate::cli::output::line(format_args!("  files: {}", result.file_count));
            crate::cli::output::line(format_args!(
                "  logical size:  {:.2} MiB",
                result.total_logical_size as f64 / (1024.0 * 1024.0)
            ));
            crate::cli::output::line(format_args!("  unique chunks: {}", est.unique_chunk_count));
            crate::cli::output::line(format_args!(
                "  unique chunk bytes: {:.2} MiB",
                est.unique_chunk_bytes as f64 / (1024.0 * 1024.0)
            ));
            crate::cli::output::line(format_args!(
                "  encoded artifacts: {:.2} MiB",
                est.encoded_artifact_bytes as f64 / (1024.0 * 1024.0)
            ));
            crate::cli::output::line(format_args!("  artifacts: {}", est.bundle_count));
        }
    }

    Ok(())
}
