use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::cli::SECTION_SEPARATOR;
use crate::cli::SnapshotCommands;
use crate::cli::args::ArtifactBuildProfileArgs;
use crate::cli::common::{
    SnapshotCreateManyStats, SnapshotCreateStats, count_required_bundles_for_build,
    count_required_bundles_from_chunk_index, count_required_chunks,
    load_snapshot_manifest_from_repo, prepare_input_path, prompt_yes_no, write_bundles,
    write_loose_chunks, write_report, write_snapshot_manifest,
};
use crate::cli::progress::{CliBuildProgressSink, CliProgressSink};
use crate::cli::reporting::{
    build_timings_summary, ensure_parent_directory, optional_output_paths, print_build_timings,
    ratio_pct,
};
use crate::cli::verbose_output_enabled;
use crate::hash::ChunkHash;
use g::core::snapshot_identity::{apply_resolved_snapshot_identity, parse_cli_tags};
use g::repo::scan_existing_store_state;
use gutch as g;

struct MutableCreateStoreState {
    bundle_ids: HashSet<String>,
    chunk_index: g::manifest::ChunkIndex,
    chunk_locations: HashMap<String, g::ExistingChunkLocation>,
    previous_file_manifests: HashMap<String, Vec<g::manifest::FileManifest>>,
}

struct SnapshotIdentityStrings {
    version_id: String,
    os: String,
    arch: String,
}

impl From<g::repo::ExistingStoreState> for MutableCreateStoreState {
    fn from(state: g::repo::ExistingStoreState) -> Self {
        Self {
            bundle_ids: state.bundle_ids,
            chunk_index: state.chunk_index,
            chunk_locations: state.chunk_locations,
            previous_file_manifests: state.previous_file_manifests,
        }
    }
}

struct CreateOneSnapshotRequest<'a> {
    input_dir: &'a Path,
    repo_paths: &'a g::GutchRepoPaths,
    repo_name: &'a str,
    rules: &'a g::manifest::SnapshotTagSchema,
    explicit_tags: &'a g::manifest::SnapshotTagSet,
    allow_name_inference: bool,
    build_profile: ArtifactBuildProfileArgs,
    runtime_hints: g::RuntimeResourceHints,
    stats_json_out: &'a Option<PathBuf>,
    command_name: &'static str,
    yes: bool,
    skip_confirmation: bool,
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

fn unchanged_snapshot_create_stats(
    command_name: &'static str,
    repo_root: &Path,
    input_path: &Path,
    manifest_path: &Path,
    repo_name: &str,
    snapshot_manifest: &g::manifest::snapshot::SnapshotManifest,
    identity: SnapshotIdentityStrings,
    storage_mode: g::manifest::ArtifactStorageMode,
    required_chunk_count: usize,
    required_bundle_count: usize,
    timings: &g::BuildTimings,
    stats_json_out: &Option<PathBuf>,
) -> SnapshotCreateStats {
    SnapshotCreateStats {
        command: command_name,
        repo_root: repo_root.display().to_string(),
        input_path: input_path.display().to_string(),
        snapshot_id: snapshot_manifest.snapshot_id.clone(),
        manifest_path: manifest_path.display().to_string(),
        product_id: repo_name.to_string(),
        remote_repo_id: repo_name.to_string(),
        version_id: identity.version_id,
        os: identity.os,
        arch: identity.arch,
        storage_mode: storage_mode.cli_name().to_string(),
        file_count: snapshot_manifest.file_count(),
        total_logical_size: snapshot_manifest.total_logical_size,
        total_compressed_size_estimate: snapshot_manifest.total_compressed_size_estimate,
        required_chunk_count,
        required_bundle_count,
        generated_bundle_count: 0,
        generated_bundle_bytes: 0,
        generated_chunk_count: 0,
        generated_chunk_bytes: 0,
        reused_chunk_count: required_chunk_count,
        new_chunk_count: 0,
        reused_chunk_percent: 100.0,
        reused_bundle_count: required_bundle_count,
        reused_bundle_percent: 100.0,
        timings: build_timings_summary(timings),
        outputs: optional_output_paths(stats_json_out),
    }
}

fn run_create_one_snapshot(
    request: CreateOneSnapshotRequest<'_>,
    store_state: &mut MutableCreateStoreState,
) -> g::Result<SnapshotCreateStats> {
    let create_plan = g::plan_snapshot_create(
        request.repo_paths.repo_root.as_path(),
        request.input_dir,
        request.explicit_tags,
        request.allow_name_inference,
    )?;
    if !request.yes && !request.skip_confirmation {
        println!("snapshot create plan:");
        println!("  {}", g::describe_snapshot_create_plan(&create_plan));
        if create_plan.replaces_existing_slots {
            println!("  Existing snapshot references to replaced snapshots must be updated.");
        }
        if !prompt_yes_no("Proceed?", false)? {
            return Err(g::Error::manifest("snapshot create cancelled"));
        }
    }
    if create_plan.kind == g::SnapshotCreateKind::ReplaceSlot {
        g::delete_repo_snapshots(
            request.repo_paths.repo_root.as_path(),
            &create_plan.replaced_snapshot_ids,
        )?;
        let _ = g::cleanup_orphan_bundles_in_repo(
            request.repo_paths.repo_root.as_path(),
            &g::GutchOperationControl::default(),
        );
    }
    if create_plan.kind == g::SnapshotCreateKind::Relabel {
        let existing_manifest =
            g::load_snapshot_manifest(request.repo_paths, &create_plan.snapshot_id)?;
        let mut updated_manifest = existing_manifest;
        updated_manifest.snapshot_tags = create_plan.snapshot_tags.clone();
        let (manifest_path, _) = write_snapshot_manifest(request.repo_paths, &updated_manifest)?;
        if verbose_output_enabled() {
            println!(
                "updated snapshot tags for snapshot id '{}' (snapshot unchanged)",
                create_plan.snapshot_id
            );
        }
        let required_chunk_count = count_required_chunks(&updated_manifest);
        let required_bundle_count =
            count_required_bundles_from_chunk_index(&updated_manifest, &store_state.chunk_index);
        return Ok(unchanged_snapshot_create_stats(
            request.command_name,
            &request.repo_paths.repo_root,
            request.input_dir,
            &manifest_path,
            request.repo_name,
            &updated_manifest,
            SnapshotIdentityStrings {
                version_id: create_plan.version_id.clone(),
                os: create_plan.os.clone(),
                arch: create_plan.arch.clone(),
            },
            request.build_profile.storage_mode,
            required_chunk_count,
            required_bundle_count,
            &g::BuildTimings::default(),
            request.stats_json_out,
        ));
    }

    let prepared_input = prepare_input_path(&request.input_dir.to_path_buf())?;
    let context = format!("snapshot create input '{}'", request.input_dir.display());
    let resolved_identity = g::core::snapshot_identity::resolve_snapshot_identity_with_inference(
        request.input_dir,
        request.explicit_tags,
        request.rules,
        request.allow_name_inference,
        &context,
    )?;
    let existing_chunk_ids = store_state
        .chunk_locations
        .keys()
        .map(|chunk_hash| ChunkHash::parse_hex(chunk_hash))
        .collect::<g::Result<HashSet<_>>>()?;
    let chunk_sizing_profile =
        g::manifest::ChunkSizingProfile::for_preset(request.build_profile.chunk_profile);
    let bundle_sizing_profile =
        g::manifest::BundleSizingProfile::for_preset(request.build_profile.bundle_profile);
    let compression_profile =
        g::manifest::CompressionProfile::for_preset(request.build_profile.compression_profile);

    let mut build_config =
        g::BuildConfig::with_ids(request.repo_name, "", &resolved_identity.snapshot_tags)
            .with_existing_chunks(&store_state.chunk_locations)
            .with_existing_bundles(&store_state.bundle_ids)
            .with_previous_manifests(&store_state.previous_file_manifests)
            .with_chunk_profile(&chunk_sizing_profile)
            .with_bundle_profile(&bundle_sizing_profile)
            .with_compression_profile(&compression_profile);
    build_config.storage_mode = request.build_profile.storage_mode;
    build_config.runtime_hints = request.runtime_hints.clone();
    let build_progress = CliBuildProgressSink::new();
    build_config.progress = Some(build_progress.as_build_sink());

    let (result, timings) = g::build_with_config(&prepared_input.dir, &build_config)?;
    let mut result = result;
    let snapshot_id = g::compute_snapshot_id_from_entries(&result.snapshot_manifest.entries);
    result.snapshot_manifest.snapshot_id = snapshot_id.clone();
    apply_resolved_snapshot_identity(&mut result.snapshot_manifest, &resolved_identity);

    let existing_manifest_path = g::snapshot_metadata_path(request.repo_paths, &snapshot_id);
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
            return Ok(unchanged_snapshot_create_stats(
                request.command_name,
                &request.repo_paths.repo_root,
                request.input_dir,
                &manifest_path,
                request.repo_name,
                &updated_manifest,
                resolved_identity_strings(&resolved_identity.snapshot_tags),
                request.build_profile.storage_mode,
                required_chunk_count,
                required_bundle_count,
                &timings,
                request.stats_json_out,
            ));
        }
    }

    let (manifest_path, _) =
        write_snapshot_manifest(request.repo_paths, &result.snapshot_manifest)?;
    write_bundles(&request.repo_paths.bundles_root, &result.bundles)?;
    write_loose_chunks(&request.repo_paths.chunks_root, &result.loose_chunks)?;
    let compression_profile =
        g::manifest::CompressionProfile::for_preset(request.build_profile.compression_profile);
    g::add_built_bundles_to_chunk_index(
        &mut store_state.chunk_index,
        &result.bundles,
        &compression_profile,
    )?;
    g::add_built_loose_chunks_to_chunk_index(&mut store_state.chunk_index, &result.loose_chunks)?;

    for bundle in &result.bundles {
        store_state.bundle_ids.insert(bundle.bundle_id.clone());
    }
    store_state.chunk_locations =
        g::repo::existing_chunk_locations_from_index(&store_state.chunk_index, request.repo_paths);
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

    print_build_timings(&timings);

    let required_chunk_count = count_required_chunks(&result.snapshot_manifest);
    let required_bundle_count = count_required_bundles_for_build(
        &result.snapshot_manifest,
        &store_state.chunk_locations,
        &result.bundles,
        &result.loose_chunks,
    );
    let generated_bundle_count = result.bundles.len();
    let generated_bundle_bytes: u64 = result
        .bundles
        .iter()
        .map(|bundle| bundle.data.len() as u64)
        .sum::<u64>();
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
    let stats = SnapshotCreateStats {
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

fn collect_create_many_sources(
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
            "snapshot create requires at least one --source-dir or --source-root with snapshot folders",
        ));
    }

    Ok(sources)
}

/// Resolve the config contribution for a single `snapshot create --source-dir` operation.
/// The config is read from the directory's parent when present; it is never
/// auto-created for a single create. Returns the entry tags (explicit mode)
/// and whether name inference is allowed.
fn single_create_source_resolution(
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

pub fn run(command: SnapshotCommands) -> g::Result<()> {
    match command {
        SnapshotCommands::Create {
            source_dirs,
            source_root,
            repo,
            tags,
            build_profile,
            runtime,
            stats_json_out,
            yes,
        } => {
            let sources = collect_create_many_sources(source_root.clone(), source_dirs)?;
            let cli_tags = parse_cli_tags(&tags)?;
            if sources.len() == 1 && source_root.is_none() {
                let source = &sources[0];
                let (config_tags, allow_name_inference) =
                    single_create_source_resolution(&source.path)?;
                let mut explicit_tags = cli_tags.clone();
                if let Some(config_tags) = config_tags {
                    explicit_tags.extend_missing_from(&config_tags);
                }
                let plan = g::plan_snapshot_create(
                    repo.as_path(),
                    &source.path,
                    &explicit_tags,
                    allow_name_inference,
                )?;
                if !yes {
                    println!("snapshot create plan:");
                    println!("  {}", g::describe_snapshot_create_plan(&plan));
                    if plan.replaces_existing_slots {
                        println!(
                            "  Existing snapshot references to replaced snapshots must be updated."
                        );
                    }
                    if !prompt_yes_no("Proceed?", false)? {
                        return Err(g::Error::manifest("snapshot create cancelled"));
                    }
                }
                let progress = CliProgressSink::new();
                let create_result = g::create_repo_snapshot(g::RepoSnapshotCreateRequest {
                    repo_root: repo.clone(),
                    source_dir: source.path.clone(),
                    explicit_tags,
                    allow_name_inference,
                    chunk_profile: build_profile.chunk_profile,
                    bundle_profile: build_profile.bundle_profile,
                    compression_profile: build_profile.compression_profile,
                    storage_mode: build_profile.storage_mode,
                    runtime_hints: runtime.hints(),
                    control: g::GutchOperationControl::default().with_progress(progress),
                })?;
                let (_, repo_manifest) = g::load_gutch_repo_manifest(&repo)?;
                let stats = SnapshotCreateStats {
                    command: "snapshot create",
                    repo_root: create_result.repo_root.display().to_string(),
                    input_path: source.path.display().to_string(),
                    snapshot_id: create_result.snapshot_id,
                    manifest_path: create_result.manifest_path.display().to_string(),
                    product_id: repo_manifest.name.clone(),
                    remote_repo_id: repo_manifest.name.clone(),
                    version_id: create_result.version_id,
                    os: create_result.os,
                    arch: create_result.arch,
                    storage_mode: build_profile.storage_mode.cli_name().to_string(),
                    file_count: create_result.file_count,
                    total_logical_size: create_result.total_logical_size,
                    total_compressed_size_estimate: create_result.total_compressed_size_estimate,
                    required_chunk_count: create_result.required_chunk_count,
                    required_bundle_count: create_result.required_bundle_count,
                    generated_bundle_count: create_result.generated_bundle_count,
                    generated_bundle_bytes: 0,
                    generated_chunk_count: create_result.generated_chunk_count,
                    generated_chunk_bytes: 0,
                    reused_chunk_count: create_result.reused_chunk_count,
                    new_chunk_count: create_result.new_chunk_count,
                    reused_chunk_percent: ratio_pct(
                        create_result.reused_chunk_count as u64,
                        create_result.required_chunk_count as u64,
                    ),
                    reused_bundle_count: create_result
                        .required_bundle_count
                        .saturating_sub(create_result.generated_bundle_count),
                    reused_bundle_percent: ratio_pct(
                        create_result
                            .required_bundle_count
                            .saturating_sub(create_result.generated_bundle_count)
                            as u64,
                        create_result.required_bundle_count as u64,
                    ),
                    timings: build_timings_summary(&create_result.build_timings),
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
                let create_plans: Vec<g::SnapshotCreatePlan> = sources
                    .iter()
                    .map(|source| {
                        let mut effective_tags = cli_tags.clone();
                        if let Some(config_tags) = &source.config_tags {
                            effective_tags.extend_missing_from(config_tags);
                        }
                        g::plan_snapshot_create(
                            repo_paths.repo_root.as_path(),
                            &source.path,
                            &effective_tags,
                            source.allow_name_inference,
                        )
                    })
                    .collect::<g::Result<_>>()?;
                if !yes {
                    println!("snapshot create plan:");
                    for plan in &create_plans {
                        println!("  {}", g::describe_snapshot_create_plan(plan));
                    }
                    if create_plans.iter().any(|plan| plan.replaces_existing_slots) {
                        println!(
                            "  Existing snapshot references to replaced snapshots must be updated."
                        );
                    }
                    if !prompt_yes_no("Proceed?", false)? {
                        return Err(g::Error::manifest("snapshot create cancelled"));
                    }
                }
                let mut store_state: MutableCreateStoreState =
                    scan_existing_store_state(&repo_paths)?.into();
                let empty_output = None;

                let mut snapshot_stats = Vec::with_capacity(sources.len());
                for source in &sources {
                    let mut effective_tags = cli_tags.clone();
                    if let Some(config_tags) = &source.config_tags {
                        effective_tags.extend_missing_from(config_tags);
                    }
                    snapshot_stats.push(run_create_one_snapshot(
                        CreateOneSnapshotRequest {
                            input_dir: &source.path,
                            repo_paths: &repo_paths,
                            repo_name: &repo_name,
                            rules: &rules,
                            explicit_tags: &effective_tags,
                            allow_name_inference: source.allow_name_inference,
                            build_profile,
                            runtime_hints: runtime.hints(),
                            stats_json_out: &empty_output,
                            command_name: "snapshot create",
                            yes,
                            skip_confirmation: true,
                        },
                        &mut store_state,
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
                let aggregate_stats = SnapshotCreateManyStats {
                    command: "snapshot create",
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
                let manifest_path = g::snapshot_metadata_path(&repo_paths, &snapshot_id);
                eprintln!(
                    "WARNING: this will remove '{}' and related snapshot artifacts, and may orphan bundles. Use --yes to confirm.",
                    manifest_path.display()
                );
                return Err(g::Error::manifest("deletion requires --yes flag"));
            }

            let progress = CliProgressSink::new();
            let control = g::GutchOperationControl::default().with_progress(progress);
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
            repo,
            snapshot_id,
            destination,
            staging_dir,
            source_snapshot_id,
            source_dir,
            runtime,
            stats_json_out,
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
        } => crate::cli::snapshot_ops::fetch(
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
        )?,

        SnapshotCommands::Verify {
            repo,
            snapshot_id,
            destination,
            strict,
            runtime,
            stats_json_out,
        } => crate::cli::snapshot_ops::verify(
            repo,
            snapshot_id,
            destination,
            strict,
            runtime,
            stats_json_out,
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
            let resolved_identity = g::core::snapshot_identity::resolve_snapshot_identity(
                &source_dir,
                &explicit_tags,
                &rules,
                &context,
            )?;
            let result = g::build_from_directory(
                &prepared_input.dir,
                "estimate",
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
