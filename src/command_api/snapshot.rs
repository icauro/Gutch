use std::path::{Path, PathBuf};

use crate::command_api::SECTION_SEPARATOR;
use crate::command_api::SnapshotCommands;
use crate::command_api::common::{
    SnapshotCaptureManyStats, SnapshotCaptureStats, load_snapshot_manifest_from_repo,
    prepare_input_path, prompt_yes_no, write_report,
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

fn collect_capture_many_sources(
    product_dir: Option<PathBuf>,
    explicit_dirs: Vec<PathBuf>,
) -> g::Result<Vec<g::source_config::ResolvedSourceSnapshot>> {
    let mut sources: Vec<g::source_config::ResolvedSourceSnapshot> = Vec::new();
    if let Some(product_dir) = product_dir {
        if !product_dir.is_dir() {
            return Err(g::Error::manifest(format!(
                "product dir is not a directory: {}",
                product_dir.display()
            )));
        }
        let config = g::source_config::SourceConfig::load_or_create(&product_dir)?;
        sources.extend(config.discover_snapshot_sources(&product_dir)?);
    }
    for dir in explicit_dirs {
        sources.push(g::source_config::ResolvedSourceSnapshot {
            path: dir.clone(),
            relative_path: dir,
            config_tags: None,
        });
    }

    sources.sort_by(|left, right| left.path.cmp(&right.path));
    sources.dedup_by(|left, right| left.path == right.path);
    if sources.is_empty() {
        return Err(g::Error::manifest(
            "snapshot capture requires at least one --source-dir or --source-root with snapshot folders",
        ));
    }

    Ok(sources)
}

/// Resolve the config contribution for a single `snapshot capture --source-dir` operation.
/// The config is read from the directory's parent when present; it is never
/// generated automatically for a single capture. Returns the entry's explicit
/// tags when the source is listed in explicit mode.
fn single_capture_source_resolution(dir: &Path) -> g::Result<Option<g::manifest::SnapshotTagSet>> {
    let parent = dir.parent().ok_or_else(|| {
        g::Error::manifest(format!(
            "snapshot input has no parent directory: {}",
            dir.display()
        ))
    })?;
    let Some(config) = g::source_config::SourceConfig::load(parent)? else {
        return Ok(None);
    };
    let Some(name) = dir.file_name().and_then(|value| value.to_str()) else {
        return Err(g::Error::manifest(format!(
            "snapshot input has no usable folder name: {}",
            dir.display()
        )));
    };
    match config.ingestion_mode {
        g::source_config::IngestionMode::Auto => Ok(None),
        g::source_config::IngestionMode::Explicit => {
            let entry = config
                .explicit_ingestion_rules
                .snapshots
                .iter()
                .find(|rule| {
                    g::source_config::normalize_relative_path(&rule.path)
                        .ok()
                        .as_deref()
                        == Some(name)
                });
            match entry {
                Some(rule) => {
                    let tags = g::manifest::SnapshotTagSet::from_compact_strings(&rule.tags)
                        .map_err(|err| {
                            g::Error::manifest(format!(
                                "invalid tags for explicit snapshot '{}': {err}",
                                rule.path
                            ))
                        })?;
                    Ok(Some(tags))
                }
                None => Ok(None),
            }
        }
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
            let sources = collect_capture_many_sources(source_root.clone(), source_dirs)?;
            let cli_tags = parse_cli_tags(&tags)?;
            if sources.len() == 1 && source_root.is_none() {
                let source = &sources[0];
                let config_tags = single_capture_source_resolution(&source.path)?;
                let mut explicit_tags = cli_tags.clone();
                if let Some(config_tags) = config_tags {
                    explicit_tags.extend_missing_from(&config_tags);
                }
                let progress = CliProgressSink::new();
                let capture_control =
                    crate::command_api::control_with_profile(profile).with_progress(progress);
                let plan = g::plan_snapshot_capture_with_control(
                    repo.as_path(),
                    &source.path,
                    &explicit_tags,
                    &capture_control,
                )?;
                if !yes {
                    println!("snapshot capture plan:");
                    println!("  {}", g::describe_snapshot_capture_plan(&plan));
                    if !prompt_yes_no("Proceed?", false)? {
                        return Err(g::Error::manifest("snapshot capture cancelled"));
                    }
                }
                let capture_result = g::capture_repo_snapshot(g::RepoSnapshotCaptureRequest {
                    repo_root: repo.clone(),
                    source_dir: source.path.clone(),
                    explicit_tags: explicit_tags.clone(),
                    chunk_profile: build_profile.chunk_profile,
                    bundle_profile: build_profile.bundle_profile,
                    compression_algorithm: build_profile.compression_algorithm,
                    compression_level: build_profile.compression_level,
                    storage_mode: build_profile.storage_mode,
                    runtime_hints: runtime.hints(),
                    control: capture_control,
                })?;
                println!("{}", capture_result.outcome.message());
                let (_, repo_manifest) = g::load_gutch_repo_manifest(&repo)?;
                let stats = capture_stats_from_engine_result(
                    &capture_result.repo_root.display().to_string(),
                    &source.path.display().to_string(),
                    &repo_manifest.name,
                    &explicit_tags,
                    build_profile.storage_mode.cli_name().to_string(),
                    &capture_result,
                );
                let stats = SnapshotCaptureStats {
                    outputs: optional_output_paths(&stats_json_out),
                    ..stats
                };
                write_report(&stats_json_out, &stats)?;

                let (repo_paths, mut repo_manifest) = g::load_gutch_repo_manifest(&repo)?;
                repo_manifest.set_artifact_profile(
                    build_profile.chunk_profile,
                    build_profile.compression_algorithm,
                    build_profile.compression_level,
                    build_profile.bundle_profile,
                    build_profile.storage_mode,
                );
                g::write_gutch_repo_manifest(&repo_paths, &repo_manifest)?;
            } else {
                let (repo_paths, mut repo_manifest) = g::load_gutch_repo_manifest(&repo)?;
                let repo_name = repo_manifest.name.clone();
                let prepare_control = crate::command_api::control_with_profile(profile);
                let _plan_all_span = prepare_control.profile_span_with_args(
                    "capture.prepare_source.plan_all_snapshots",
                    std::collections::BTreeMap::from([(
                        "source_count".to_string(),
                        serde_json::json!(sources.len()),
                    )]),
                );
                let capture_plans: Vec<g::SnapshotCapturePlan> = sources
                    .iter()
                    .map(|source| {
                        let mut effective_tags = cli_tags.clone();
                        if let Some(config_tags) = &source.config_tags {
                            effective_tags.extend_missing_from(config_tags);
                        }
                        g::plan_snapshot_capture_with_control(
                            repo_paths.repo_root.as_path(),
                            &source.path,
                            &effective_tags,
                            &prepare_control,
                        )
                    })
                    .collect::<g::Result<_>>()?;
                drop(_plan_all_span);
                if !yes {
                    println!("snapshot capture plan:");
                    for plan in &capture_plans {
                        println!("  {}", g::describe_snapshot_capture_plan(plan));
                    }
                    if !prompt_yes_no("Proceed?", false)? {
                        return Err(g::Error::manifest("snapshot capture cancelled"));
                    }
                }

                let mut snapshot_stats = Vec::with_capacity(sources.len());
                for source in sources.iter() {
                    let mut effective_tags = cli_tags.clone();
                    if let Some(config_tags) = &source.config_tags {
                        effective_tags.extend_missing_from(config_tags);
                    }
                    let progress = CliProgressSink::new();
                    let capture_control =
                        crate::command_api::control_with_profile(profile).with_progress(progress);
                    let capture_result = g::capture_repo_snapshot(g::RepoSnapshotCaptureRequest {
                        repo_root: repo_paths.repo_root.clone(),
                        source_dir: source.path.clone(),
                        explicit_tags: effective_tags.clone(),
                        chunk_profile: build_profile.chunk_profile,
                        bundle_profile: build_profile.bundle_profile,
                        compression_algorithm: build_profile.compression_algorithm,
                        compression_level: build_profile.compression_level,
                        storage_mode: build_profile.storage_mode,
                        runtime_hints: runtime.hints(),
                        control: capture_control,
                    })?;
                    println!("{}", capture_result.outcome.message());
                    snapshot_stats.push(capture_stats_from_engine_result(
                        &repo_paths.repo_root.display().to_string(),
                        &source.path.display().to_string(),
                        &repo_name,
                        &effective_tags,
                        build_profile.storage_mode.cli_name().to_string(),
                        &capture_result,
                    ));
                }

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

                repo_manifest.set_artifact_profile(
                    build_profile.chunk_profile,
                    build_profile.compression_algorithm,
                    build_profile.compression_level,
                    build_profile.bundle_profile,
                    build_profile.storage_mode,
                );
                g::write_gutch_repo_manifest(&repo_paths, &repo_manifest)?;
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
            let prepared_input = prepare_input_path(&source_dir)?;
            let explicit_tags = parse_cli_tags(&tags)?;
            let context = format!("snapshot estimate input '{}'", source_dir.display());
            let resolved_tags = g::snapshot::identity::resolve_snapshot_tags(
                &source_dir,
                &explicit_tags,
                &context,
            )?;
            let result = g::build_from_directory(
                &prepared_input.dir,
                "estimate",
                &resolved_tags.snapshot_tags,
            )?;

            let est = g::estimate_build(
                &result.snapshot_manifest,
                &Default::default(),
                1024 * 1024 * 1024,
            )?;

            println!("fresh reconstruction estimate:");
            println!("  files: {}", result.snapshot_manifest.file_count());
            println!(
                "  logical size:  {:.2} MiB",
                result.snapshot_manifest.total_logical_size as f64 / (1024.0 * 1024.0)
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
