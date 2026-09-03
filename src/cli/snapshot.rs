use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::cli::SECTION_SEPARATOR;
use crate::cli::SnapshotCommands;
use crate::cli::args::ArtifactBuildProfileArgs;
use crate::cli::common::{
    SnapshotCaptureManyStats, SnapshotCaptureStats, count_required_bundles_for_build,
    count_required_bundles_from_chunk_index, count_required_chunks,
    load_snapshot_manifest_from_repo, prepare_input_path, prompt_yes_no, write_bundles,
    write_loose_chunks, write_report, write_snapshot_manifest,
};
use crate::cli::progress::CliProgressSink;
use crate::cli::reporting::{
    build_timings_summary, ensure_parent_directory, optional_output_paths, print_build_timings,
    print_profile_summary, ratio_pct,
};
use crate::cli::verbose_output_enabled;
use crate::hash::ChunkHash;
use g::repo::{ExistingStoreState, scan_existing_store_state_with_pool};
use g::snapshot::identity::{apply_resolved_snapshot_identity, parse_cli_tags};
use gutch as g;

struct SnapshotIdentityStrings {
    version_id: String,
    os: String,
    arch: String,
}

struct CaptureOneSnapshotRequest<'a> {
    input_dir: &'a Path,
    repo_paths: &'a g::GutchRepoPaths,
    repo_name: &'a str,
    rules: &'a g::manifest::SnapshotTagSchema,
    explicit_tags: &'a g::manifest::SnapshotTagSet,
    allow_name_inference: bool,
    build_profile: ArtifactBuildProfileArgs,
    runtime_hints: g::RuntimeResourceHints,
    stats_json_out: &'a Option<PathBuf>,
    profile: crate::cli::Profile,
    command_name: &'static str,
    snapshot_sequence: usize,
    yes: bool,
    skip_confirmation: bool,
}

struct UnchangedSnapshotCaptureRequest<'a> {
    command_name: &'static str,
    repo_root: &'a Path,
    input_path: &'a Path,
    manifest_path: &'a Path,
    repo_name: &'a str,
    snapshot_manifest: &'a g::manifest::snapshot::SnapshotManifest,
    identity: SnapshotIdentityStrings,
    storage_mode: g::manifest::ArtifactStorageMode,
    required_chunk_count: usize,
    required_bundle_count: usize,
    timings: &'a g::BuildTimings,
    stats_json_out: &'a Option<PathBuf>,
}

fn resolved_identity_strings(
    snapshot_tags: &g::manifest::SnapshotTagSet,
) -> SnapshotIdentityStrings {
    SnapshotIdentityStrings {
        version_id: snapshot_tags
            .get_exclusive("version")
            .unwrap_or_default()
            .to_string(),
        os: snapshot_tags
            .get_exclusive("os")
            .unwrap_or_default()
            .to_string(),
        arch: snapshot_tags
            .get_exclusive("arch")
            .unwrap_or_default()
            .to_string(),
    }
}

fn unchanged_snapshot_capture_stats(
    request: UnchangedSnapshotCaptureRequest<'_>,
) -> SnapshotCaptureStats {
    SnapshotCaptureStats {
        command: request.command_name,
        repo_root: request.repo_root.display().to_string(),
        input_path: request.input_path.display().to_string(),
        snapshot_id: request.snapshot_manifest.snapshot_id.clone(),
        manifest_path: request.manifest_path.display().to_string(),
        product_id: request.repo_name.to_string(),
        remote_repo_id: request.repo_name.to_string(),
        version_id: request.identity.version_id,
        os: request.identity.os,
        arch: request.identity.arch,
        storage_mode: request.storage_mode.cli_name().to_string(),
        file_count: request.snapshot_manifest.file_count(),
        total_logical_size: request.snapshot_manifest.total_logical_size,
        total_compressed_size_estimate: request.snapshot_manifest.total_compressed_size_estimate,
        required_chunk_count: request.required_chunk_count,
        required_bundle_count: request.required_bundle_count,
        generated_bundle_count: 0,
        generated_bundle_bytes: 0,
        generated_chunk_count: 0,
        generated_chunk_bytes: 0,
        reused_chunk_count: request.required_chunk_count,
        new_chunk_count: 0,
        reused_chunk_percent: 100.0,
        reused_bundle_count: request.required_bundle_count,
        reused_bundle_percent: 100.0,
        timings: build_timings_summary(request.timings),
        outputs: optional_output_paths(request.stats_json_out),
    }
}

fn run_capture_one_snapshot(
    request: CaptureOneSnapshotRequest<'_>,
    store_state: &mut ExistingStoreState,
    build_pool: &rayon::ThreadPool,
) -> g::Result<SnapshotCaptureStats> {
    let capture_control =
        crate::cli::control_with_profile(&request.profile).with_progress(CliProgressSink::new());
    let _prepare_span = capture_control.profile_span_with_args(
        "capture.prepare_source",
        std::collections::BTreeMap::from([
            (
                "source_path".to_string(),
                serde_json::json!(request.input_dir.display().to_string()),
            ),
            (
                "snapshot_sequence".to_string(),
                serde_json::json!(request.snapshot_sequence),
            ),
            (
                "is_first_snapshot".to_string(),
                serde_json::json!(request.snapshot_sequence == 1),
            ),
        ]),
    );
    let _plan_span = capture_control.profile_span("capture.prepare_source.plan_snapshot");
    let capture_plan = g::plan_snapshot_capture(
        request.repo_paths.repo_root.as_path(),
        request.input_dir,
        request.explicit_tags,
        request.allow_name_inference,
    )?;
    drop(_plan_span);
    if !request.yes && !request.skip_confirmation {
        println!("snapshot capture plan:");
        println!("  {}", g::describe_snapshot_capture_plan(&capture_plan));
        if capture_plan.replaces_existing_slots {
            println!("  Existing snapshot references to replaced snapshots must be updated.");
        }
        if !prompt_yes_no("Proceed?", false)? {
            return Err(g::Error::manifest("snapshot capture cancelled"));
        }
    }
    if capture_plan.kind == g::SnapshotCaptureKind::ReplaceSlot {
        let _replace_span = capture_control.profile_span_with_args(
            "capture.prepare_source.replace_snapshot_slot",
            std::collections::BTreeMap::from([(
                "replaced_snapshot_count".to_string(),
                serde_json::json!(capture_plan.replaced_snapshot_ids.len()),
            )]),
        );
        g::delete_repo_snapshots(
            request.repo_paths.repo_root.as_path(),
            &capture_plan.replaced_snapshot_ids,
        )?;
        let _ = g::cleanup_orphan_bundles_in_repo(
            request.repo_paths.repo_root.as_path(),
            &crate::cli::control_with_profile(&request.profile),
        );
        drop(_replace_span);
    }
    if capture_plan.kind == g::SnapshotCaptureKind::Relabel {
        let _relabel_span =
            capture_control.profile_span("capture.prepare_source.relabel_existing_snapshot");
        let existing_manifest =
            g::load_snapshot_manifest(request.repo_paths, &capture_plan.snapshot_id)?;
        let mut updated_manifest = existing_manifest;
        updated_manifest.snapshot_tags = capture_plan.snapshot_tags.clone();
        let (manifest_path, _) = write_snapshot_manifest(request.repo_paths, &updated_manifest)?;
        if verbose_output_enabled() {
            println!(
                "updated snapshot tags for snapshot id '{}' (snapshot unchanged)",
                capture_plan.snapshot_id
            );
        }
        let required_chunk_count = count_required_chunks(&updated_manifest);
        let required_bundle_count =
            count_required_bundles_from_chunk_index(&updated_manifest, &store_state.chunk_index);
        let stats = unchanged_snapshot_capture_stats(UnchangedSnapshotCaptureRequest {
            command_name: request.command_name,
            repo_root: &request.repo_paths.repo_root,
            input_path: request.input_dir,
            manifest_path: &manifest_path,
            repo_name: request.repo_name,
            snapshot_manifest: &updated_manifest,
            identity: SnapshotIdentityStrings {
                version_id: capture_plan.version_id.clone(),
                os: capture_plan.os.clone(),
                arch: capture_plan.arch.clone(),
            },
            storage_mode: request.build_profile.storage_mode,
            required_chunk_count,
            required_bundle_count,
            timings: &g::BuildTimings::default(),
            stats_json_out: request.stats_json_out,
        });
        drop(_relabel_span);
        return Ok(stats);
    }

    let _input_span = capture_control.profile_span("capture.prepare_source.prepare_input");
    let prepared_input = prepare_input_path(&request.input_dir.to_path_buf())?;
    drop(_input_span);

    let _identity_span = capture_control.profile_span("capture.prepare_source.resolve_identity");
    let context = format!("snapshot capture input '{}'", request.input_dir.display());
    let resolved_identity = g::snapshot::identity::resolve_snapshot_identity_with_inference(
        request.input_dir,
        request.explicit_tags,
        request.rules,
        request.allow_name_inference,
        &context,
    )?;
    drop(_identity_span);

    let _existing_chunks_span = capture_control.profile_span_with_args(
        "capture.prepare_source.collect_existing_chunk_ids",
        std::collections::BTreeMap::from([
            (
                "existing_chunk_location_count".to_string(),
                serde_json::json!(store_state.chunk_locations.len()),
            ),
            (
                "previous_file_manifest_path_count".to_string(),
                serde_json::json!(store_state.previous_file_manifests.len()),
            ),
        ]),
    );
    let existing_chunk_ids = store_state
        .chunk_locations
        .keys()
        .map(|chunk_hash| ChunkHash::parse_hex(chunk_hash))
        .collect::<g::Result<HashSet<_>>>()?;
    drop(_existing_chunks_span);

    let _config_span = capture_control.profile_span_with_args(
        "capture.prepare_source.configure_build",
        std::collections::BTreeMap::from([
            (
                "capture_kind".to_string(),
                serde_json::json!(format!("{:?}", capture_plan.kind)),
            ),
            (
                "existing_bundle_count".to_string(),
                serde_json::json!(store_state.bundle_ids.len()),
            ),
            (
                "existing_chunk_location_count".to_string(),
                serde_json::json!(store_state.chunk_locations.len()),
            ),
            (
                "previous_file_manifest_path_count".to_string(),
                serde_json::json!(store_state.previous_file_manifests.len()),
            ),
        ]),
    );
    let mut build_config = g::BuildConfig::for_snapshot("", &resolved_identity.snapshot_tags)
        .with_existing_chunks(&store_state.chunk_locations)
        .with_existing_bundles(&store_state.bundle_ids)
        .with_previous_manifests(&store_state.previous_file_manifests)
        .with_chunk_profile(request.build_profile.chunk_profile)
        .with_bundle_profile(request.build_profile.bundle_profile)
        .with_compression_profile(request.build_profile.compression_profile)
        .with_storage_mode(request.build_profile.storage_mode)
        .with_runtime_hints(request.runtime_hints.clone())
        .with_artifact_staging_root(request.repo_paths.bundles_root.clone());
    build_config.control = capture_control.clone();
    drop(_config_span);

    drop(_prepare_span);
    let (result, timings) =
        g::build_with_config_timed_on_pool(&prepared_input.dir, &build_config, build_pool)?;
    let _publish_span = build_config
        .control
        .profile_span("capture.publish_snapshot");
    let mut result = result;
    let snapshot_id = g::compute_snapshot_id_from_entries(&result.snapshot_manifest.entries);
    result.snapshot_manifest.snapshot_id = snapshot_id.clone();
    apply_resolved_snapshot_identity(&mut result.snapshot_manifest, &resolved_identity);

    let existing_manifest_path = g::snapshot_metadata_path(request.repo_paths, &snapshot_id)?;
    if existing_manifest_path.exists() {
        let existing_manifest = g::load_snapshot_manifest(request.repo_paths, &snapshot_id)?;
        if existing_manifest.snapshot_tags != result.snapshot_manifest.snapshot_tags {
            let mut updated_manifest = existing_manifest;
            updated_manifest.snapshot_tags = result.snapshot_manifest.snapshot_tags.clone();
            let (manifest_path, _) =
                write_snapshot_manifest(request.repo_paths, &updated_manifest)?;
            if verbose_output_enabled() {
                println!(
                    "updated snapshot tags for snapshot id '{}' (snapshot unchanged)",
                    snapshot_id
                );
            }

            let required_chunk_count = count_required_chunks(&updated_manifest);
            let required_bundle_count = count_required_bundles_from_chunk_index(
                &updated_manifest,
                &store_state.chunk_index,
            );
            return Ok(unchanged_snapshot_capture_stats(
                UnchangedSnapshotCaptureRequest {
                    command_name: request.command_name,
                    repo_root: &request.repo_paths.repo_root,
                    input_path: request.input_dir,
                    manifest_path: &manifest_path,
                    repo_name: request.repo_name,
                    snapshot_manifest: &updated_manifest,
                    identity: resolved_identity_strings(&resolved_identity.snapshot_tags),
                    storage_mode: request.build_profile.storage_mode,
                    required_chunk_count,
                    required_bundle_count,
                    timings: &timings,
                    stats_json_out: request.stats_json_out,
                },
            ));
        }
    }

    let _manifest_write_span =
        capture_control.profile_span("capture.publish_snapshot.write_snapshot_manifest");
    let (manifest_path, _) =
        write_snapshot_manifest(request.repo_paths, &result.snapshot_manifest)?;
    drop(_manifest_write_span);
    let _artifact_write_span =
        capture_control.profile_span("capture.publish_snapshot.write_artifacts");
    write_bundles(
        &request.repo_paths.bundles_root,
        &result.bundles,
        build_pool,
        &capture_control,
    )?;
    write_loose_chunks(
        &request.repo_paths.chunks_root,
        &result.loose_chunks,
        build_pool,
        &capture_control,
    )?;
    drop(_artifact_write_span);
    let _index_span = capture_control.profile_span("capture.publish_snapshot.update_chunk_index");
    let compression_profile =
        g::manifest::CompressionProfile::for_preset(request.build_profile.compression_profile);
    {
        let _bundle_index_span =
            capture_control.profile_span("capture.publish_snapshot.index_bundle_entries");
        g::add_built_bundles_to_chunk_index(
            &mut store_state.chunk_index,
            &result.bundles,
            &compression_profile,
        )?;
    }
    {
        let _chunk_index_span =
            capture_control.profile_span("capture.publish_snapshot.index_loose_chunk_entries");
        g::add_built_loose_chunks_to_chunk_index(
            &mut store_state.chunk_index,
            &result.loose_chunks,
        )?;
    }
    drop(_index_span);

    let _state_span = capture_control.profile_span("capture.publish_snapshot.update_capture_state");
    for bundle in &result.bundles {
        store_state.bundle_ids.insert(bundle.bundle_id.clone());
    }
    store_state.chunk_locations = g::repo::existing_chunk_locations_from_index_with_pool(
        &store_state.chunk_index,
        request.repo_paths,
        build_pool,
    )?;
    for file in result.snapshot_manifest.files() {
        let candidates = store_state
            .previous_file_manifests
            .entry(file.path.clone())
            .or_default();
        if !candidates.iter().any(|candidate| {
            candidate.size == file.size && candidate.full_file_hash == file.full_file_hash
        }) {
            candidates.push(file.clone());
        }
    }

    if verbose_output_enabled() {
        println!(
            "done. {} files in manifest, total logical: {} bytes",
            result.snapshot_manifest.file_count(),
            result.snapshot_manifest.total_logical_size
        );

        if let Ok(est) = g::estimate_build(&result.snapshot_manifest, &Default::default(), 0) {
            println!(
                "estimate: {} chunks missing, {:.1} MiB logical",
                est.missing_chunk_count,
                est.missing_logical_bytes as f64 / (1024.0 * 1024.0)
            );
        }
    }

    if request.profile.is_none() {
        print_build_timings(&timings);
    }

    let required_chunk_count = count_required_chunks(&result.snapshot_manifest);
    let required_bundle_count = count_required_bundles_for_build(
        &result.snapshot_manifest,
        &store_state.chunk_locations,
        &result.bundles,
        &result.loose_chunks,
    );
    let generated_bundle_count = result.bundles.len();
    let generated_bundle_bytes: u64 = result.bundles.iter().map(|bundle| bundle.size).sum::<u64>();
    let generated_chunk_count = result.loose_chunks.len();
    let generated_chunk_bytes = result
        .loose_chunks
        .iter()
        .map(|chunk| chunk.data.len() as u64)
        .sum::<u64>();
    let reused_chunk_count = g::repo::required_chunk_hashes_typed(&result.snapshot_manifest)?
        .iter()
        .filter(|chunk_id| existing_chunk_ids.contains(*chunk_id))
        .count();
    let new_chunk_count = required_chunk_count.saturating_sub(reused_chunk_count);
    let reused_bundle_count = required_bundle_count.saturating_sub(generated_bundle_count);
    let reused_chunk_percent = ratio_pct(reused_chunk_count as u64, required_chunk_count as u64);
    let reused_bundle_percent = ratio_pct(reused_bundle_count as u64, required_bundle_count as u64);
    let resolved_identity_strings = resolved_identity_strings(&resolved_identity.snapshot_tags);
    let stats = SnapshotCaptureStats {
        command: request.command_name,
        repo_root: request.repo_paths.repo_root.display().to_string(),
        input_path: request.input_dir.display().to_string(),
        snapshot_id: result.snapshot_manifest.snapshot_id.clone(),
        manifest_path: manifest_path.display().to_string(),
        product_id: request.repo_name.to_string(),
        remote_repo_id: request.repo_name.to_string(),
        version_id: resolved_identity_strings.version_id,
        os: resolved_identity_strings.os,
        arch: resolved_identity_strings.arch,
        storage_mode: request.build_profile.storage_mode.cli_name().to_string(),
        file_count: result.snapshot_manifest.file_count(),
        total_logical_size: result.snapshot_manifest.total_logical_size,
        total_compressed_size_estimate: result.snapshot_manifest.total_compressed_size_estimate,
        required_chunk_count,
        required_bundle_count,
        generated_bundle_count,
        generated_bundle_bytes,
        generated_chunk_count,
        generated_chunk_bytes,
        reused_chunk_count,
        new_chunk_count,
        reused_chunk_percent,
        reused_bundle_count,
        reused_bundle_percent,
        timings: build_timings_summary(&timings),
        outputs: optional_output_paths(request.stats_json_out),
    };

    Ok(stats)
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
            allow_name_inference: true,
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
/// generated automatically for a single capture. Returns the entry tags (explicit mode)
/// and whether name inference is allowed.
fn single_capture_source_resolution(
    dir: &Path,
) -> g::Result<(Option<g::manifest::SnapshotTagSet>, bool)> {
    let parent = dir.parent().ok_or_else(|| {
        g::Error::manifest(format!(
            "snapshot input has no parent directory: {}",
            dir.display()
        ))
    })?;
    let Some(config) = g::source_config::SourceConfig::load(parent)? else {
        return Ok((None, true));
    };
    let Some(name) = dir.file_name().and_then(|value| value.to_str()) else {
        return Err(g::Error::manifest(format!(
            "snapshot input has no usable folder name: {}",
            dir.display()
        )));
    };
    match config.ingestion_mode {
        g::source_config::IngestionMode::Auto => Ok((None, true)),
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
                    Ok((Some(tags), false))
                }
                None => Ok((None, false)),
            }
        }
    }
}

pub(crate) fn run(command: SnapshotCommands, profile: &crate::cli::Profile) -> g::Result<()> {
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
                let (config_tags, allow_name_inference) =
                    single_capture_source_resolution(&source.path)?;
                let mut explicit_tags = cli_tags.clone();
                if let Some(config_tags) = config_tags {
                    explicit_tags.extend_missing_from(&config_tags);
                }
                let plan = g::plan_snapshot_capture(
                    repo.as_path(),
                    &source.path,
                    &explicit_tags,
                    allow_name_inference,
                )?;
                if !yes {
                    println!("snapshot capture plan:");
                    println!("  {}", g::describe_snapshot_capture_plan(&plan));
                    if plan.replaces_existing_slots {
                        println!(
                            "  Existing snapshot references to replaced snapshots must be updated."
                        );
                    }
                    if !prompt_yes_no("Proceed?", false)? {
                        return Err(g::Error::manifest("snapshot capture cancelled"));
                    }
                }
                let progress = CliProgressSink::new();
                let capture_result = g::capture_repo_snapshot(g::RepoSnapshotCaptureRequest {
                    repo_root: repo.clone(),
                    source_dir: source.path.clone(),
                    explicit_tags,
                    allow_name_inference,
                    chunk_profile: build_profile.chunk_profile,
                    bundle_profile: build_profile.bundle_profile,
                    compression_profile: build_profile.compression_profile,
                    storage_mode: build_profile.storage_mode,
                    runtime_hints: runtime.hints(),
                    control: crate::cli::control_with_profile(profile).with_progress(progress),
                })?;
                let (_, repo_manifest) = g::load_gutch_repo_manifest(&repo)?;
                let stats = SnapshotCaptureStats {
                    command: "snapshot capture",
                    repo_root: capture_result.repo_root.display().to_string(),
                    input_path: source.path.display().to_string(),
                    snapshot_id: capture_result.snapshot_id,
                    manifest_path: capture_result.manifest_path.display().to_string(),
                    product_id: repo_manifest.name.clone(),
                    remote_repo_id: repo_manifest.name.clone(),
                    version_id: capture_result.version_id,
                    os: capture_result.os,
                    arch: capture_result.arch,
                    storage_mode: build_profile.storage_mode.cli_name().to_string(),
                    file_count: capture_result.file_count,
                    total_logical_size: capture_result.total_logical_size,
                    total_compressed_size_estimate: capture_result.total_compressed_size_estimate,
                    required_chunk_count: capture_result.required_chunk_count,
                    required_bundle_count: capture_result.required_bundle_count,
                    generated_bundle_count: capture_result.generated_bundle_count,
                    generated_bundle_bytes: 0,
                    generated_chunk_count: capture_result.generated_chunk_count,
                    generated_chunk_bytes: 0,
                    reused_chunk_count: capture_result.reused_chunk_count,
                    new_chunk_count: capture_result.new_chunk_count,
                    reused_chunk_percent: ratio_pct(
                        capture_result.reused_chunk_count as u64,
                        capture_result.required_chunk_count as u64,
                    ),
                    reused_bundle_count: capture_result
                        .required_bundle_count
                        .saturating_sub(capture_result.generated_bundle_count),
                    reused_bundle_percent: ratio_pct(
                        capture_result
                            .required_bundle_count
                            .saturating_sub(capture_result.generated_bundle_count)
                            as u64,
                        capture_result.required_bundle_count as u64,
                    ),
                    timings: build_timings_summary(&capture_result.build_timings),
                    outputs: optional_output_paths(&stats_json_out),
                };
                write_report(&stats_json_out, &stats)?;

                let (repo_paths, mut repo_manifest) = g::load_gutch_repo_manifest(&repo)?;
                repo_manifest.set_artifact_profile(
                    build_profile.chunk_profile,
                    build_profile.compression_profile,
                    build_profile.bundle_profile,
                    build_profile.storage_mode,
                );
                g::write_gutch_repo_manifest(&repo_paths, &repo_manifest)?;
            } else {
                let (repo_paths, mut repo_manifest) = g::load_gutch_repo_manifest(&repo)?;
                let rules = repo_manifest
                    .snapshot_tag_schema
                    .clone()
                    .unwrap_or_default();
                let repo_name = repo_manifest.name.clone();
                let prepare_control = crate::cli::control_with_profile(profile);
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
                        g::plan_snapshot_capture(
                            repo_paths.repo_root.as_path(),
                            &source.path,
                            &effective_tags,
                            source.allow_name_inference,
                        )
                    })
                    .collect::<g::Result<_>>()?;
                drop(_plan_all_span);
                if !yes {
                    println!("snapshot capture plan:");
                    for plan in &capture_plans {
                        println!("  {}", g::describe_snapshot_capture_plan(plan));
                    }
                    if capture_plans
                        .iter()
                        .any(|plan| plan.replaces_existing_slots)
                    {
                        println!(
                            "  Existing snapshot references to replaced snapshots must be updated."
                        );
                    }
                    if !prompt_yes_no("Proceed?", false)? {
                        return Err(g::Error::manifest("snapshot capture cancelled"));
                    }
                }
                let runtime_plan = g::plan_build_runtime(&runtime.hints());
                let _pool_span = prepare_control.profile_span_with_args(
                    "capture.create_worker_pool",
                    std::collections::BTreeMap::from([(
                        "worker_count".to_string(),
                        serde_json::json!(runtime_plan.worker_count),
                    )]),
                );
                let build_pool = g::build_thread_pool(runtime_plan.worker_count, "gutch-capture")?;
                drop(_pool_span);
                let _existing_state_span = prepare_control.profile_span_with_args(
                    "capture.prepare_source.load_existing_store_state",
                    std::collections::BTreeMap::from([(
                        "source_count".to_string(),
                        serde_json::json!(sources.len()),
                    )]),
                );
                let mut store_state: ExistingStoreState = scan_existing_store_state_with_pool(
                    &repo_paths,
                    &build_pool,
                    &prepare_control,
                )?;
                drop(_existing_state_span);
                let empty_output = None;

                let mut snapshot_stats = Vec::with_capacity(sources.len());
                for (source_index, source) in sources.iter().enumerate() {
                    let mut effective_tags = cli_tags.clone();
                    if let Some(config_tags) = &source.config_tags {
                        effective_tags.extend_missing_from(config_tags);
                    }
                    snapshot_stats.push(run_capture_one_snapshot(
                        CaptureOneSnapshotRequest {
                            input_dir: &source.path,
                            repo_paths: &repo_paths,
                            repo_name: &repo_name,
                            rules: &rules,
                            explicit_tags: &effective_tags,
                            allow_name_inference: source.allow_name_inference,
                            build_profile,
                            runtime_hints: runtime.hints(),
                            stats_json_out: &empty_output,
                            profile: profile.clone(),
                            command_name: "snapshot capture",
                            snapshot_sequence: source_index + 1,
                            yes,
                            skip_confirmation: true,
                        },
                        &mut store_state,
                        &build_pool,
                    )?);
                }

                g::write_chunk_index(&repo_paths, &store_state.chunk_index)?;

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
                    build_profile.compression_profile,
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
            let control = crate::cli::control_with_profile(profile).with_progress(progress);
            let orphan_count = g::delete_repo_snapshot_and_cleanup(&repo, &snapshot_id, &control)?;

            println!(
                "deleted snapshot '{}': removed manifest, {} orphaned bundles cleaned up",
                snapshot_id, orphan_count
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
        } => crate::cli::snapshot_ops::reconstruct(
            crate::cli::snapshot_ops::ReconstructCommand {
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
        } => crate::cli::snapshot_ops::fetch_and_reconstruct(
            crate::cli::snapshot_ops::FetchAndReconstructCommand {
                fetch: crate::cli::snapshot_ops::FetchCommand {
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
        } => crate::cli::snapshot_ops::fetch(
            crate::cli::snapshot_ops::FetchCommand {
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
            repo,
            tags,
        } => {
            let prepared_input = prepare_input_path(&source_dir)?;
            let rules = if let Some(repo) = repo.as_ref() {
                let (_, repo_manifest) = g::load_gutch_repo_manifest(repo)?;
                repo_manifest.snapshot_tag_schema.unwrap_or_default()
            } else {
                g::manifest::SnapshotTagSchema::default()
            };
            let explicit_tags = parse_cli_tags(&tags)?;
            let context = format!("snapshot estimate input '{}'", source_dir.display());
            let resolved_identity = g::snapshot::identity::resolve_snapshot_identity(
                &source_dir,
                &explicit_tags,
                &rules,
                &context,
            )?;
            let result = g::build_from_directory(
                &prepared_input.dir,
                "estimate",
                &resolved_identity.snapshot_tags,
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
