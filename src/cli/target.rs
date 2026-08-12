use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::cli::TargetCommands;
use crate::cli::args::ArtifactBuildProfileArgs;
use crate::cli::common::{
    TargetExportStats, TargetInsertManyStats, TargetInsertStats, count_required_bundles_for_build,
    count_required_bundles_from_chunk_index, count_required_chunks, load_target_manifest_from_repo,
    prepare_input_path, prompt_yes_no, write_bundles, write_loose_chunks, write_report,
    write_target_manifest,
};
use crate::cli::progress::{CliBuildProgressSink, CliProgressSink};
use crate::cli::reporting::{
    build_timings_summary, optional_output_paths, print_build_timings, print_reconstruct_timings,
    ratio_pct, reconstruct_timings_summary,
};
use crate::cli::verbose_output_enabled;
use crate::hash::ChunkHash;
use g::core::target_identity::{apply_resolved_target_identity, parse_cli_tags};
use g::repo::scan_existing_store_state;
use gutch as g;

struct MutableInsertStoreState {
    bundle_ids: HashSet<String>,
    chunk_index: g::manifest::ChunkIndex,
    chunk_locations: HashMap<String, g::ExistingChunkLocation>,
    previous_file_manifests: HashMap<String, Vec<g::manifest::FileManifest>>,
}

struct TargetIdentityStrings {
    version_id: String,
    os: String,
    arch: String,
}

impl From<g::repo::ExistingStoreState> for MutableInsertStoreState {
    fn from(state: g::repo::ExistingStoreState) -> Self {
        Self {
            bundle_ids: state.bundle_ids,
            chunk_index: state.chunk_index,
            chunk_locations: state.chunk_locations,
            previous_file_manifests: state.previous_file_manifests,
        }
    }
}

struct InsertOneTargetRequest<'a> {
    input_dir: &'a Path,
    repo_paths: &'a g::GutchRepoPaths,
    repo_name: &'a str,
    rules: &'a g::manifest::TargetTagSchema,
    explicit_tags: &'a g::manifest::TargetTagSet,
    allow_name_inference: bool,
    build_profile: ArtifactBuildProfileArgs,
    runtime_hints: g::RuntimeResourceHints,
    stats_json_out: &'a Option<PathBuf>,
    command_name: &'static str,
    yes: bool,
    skip_confirmation: bool,
}

fn resolved_identity_strings(target_tags: &g::manifest::TargetTagSet) -> TargetIdentityStrings {
    TargetIdentityStrings {
        version_id: target_tags
            .get_exclusive("version")
            .unwrap_or_default()
            .to_string(),
        os: target_tags
            .get_exclusive("os")
            .unwrap_or_default()
            .to_string(),
        arch: target_tags
            .get_exclusive("arch")
            .unwrap_or_default()
            .to_string(),
    }
}

fn unchanged_target_insert_stats(
    command_name: &'static str,
    repo_root: &Path,
    input_path: &Path,
    manifest_path: &Path,
    repo_name: &str,
    target_manifest: &g::manifest::target::TargetManifest,
    identity: TargetIdentityStrings,
    storage_mode: g::manifest::ArtifactStorageMode,
    required_chunk_count: usize,
    required_bundle_count: usize,
    timings: &g::BuildTimings,
    stats_json_out: &Option<PathBuf>,
) -> TargetInsertStats {
    TargetInsertStats {
        command: command_name,
        repo_root: repo_root.display().to_string(),
        input_path: input_path.display().to_string(),
        target_id: target_manifest.target_id.clone(),
        manifest_path: manifest_path.display().to_string(),
        product_id: repo_name.to_string(),
        remote_repo_id: repo_name.to_string(),
        version_id: identity.version_id,
        os: identity.os,
        arch: identity.arch,
        storage_mode: storage_mode.cli_name().to_string(),
        file_count: target_manifest.files.len(),
        total_logical_size: target_manifest.total_logical_size,
        total_compressed_size_estimate: target_manifest.total_compressed_size_estimate,
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

fn run_insert_one_target(
    request: InsertOneTargetRequest<'_>,
    store_state: &mut MutableInsertStoreState,
) -> g::Result<TargetInsertStats> {
    let insert_plan = g::plan_target_insert(
        request.repo_paths.repo_root.as_path(),
        request.input_dir,
        request.explicit_tags,
        request.allow_name_inference,
    )?;
    if !request.yes && !request.skip_confirmation {
        println!("target commit plan:");
        println!("  {}", g::describe_target_insert_plan(&insert_plan));
        if insert_plan.invalidates_existing_installs {
            println!(
                "  User installs pinned to replaced content ids become invalid until updated."
            );
        }
        if !prompt_yes_no("Proceed?", false)? {
            return Err(g::Error::manifest("target commit cancelled"));
        }
    }
    if insert_plan.kind == g::TargetInsertKind::ReplaceSameTags {
        g::delete_repo_targets(
            request.repo_paths.repo_root.as_path(),
            &insert_plan.replaced_target_ids,
        )?;
        let _ = g::cleanup_orphan_bundles_in_repo(
            request.repo_paths.repo_root.as_path(),
            &g::GutchOperationControl::default(),
        );
    }
    if insert_plan.kind == g::TargetInsertKind::UpdateTags {
        let existing_manifest =
            g::load_target_manifest(request.repo_paths, &insert_plan.content_target_id)?;
        let mut updated_manifest = existing_manifest;
        updated_manifest.target_tags = insert_plan.target_tags.clone();
        let (manifest_path, _) = write_target_manifest(request.repo_paths, &updated_manifest)?;
        if verbose_output_enabled() {
            println!(
                "updated target tags for content id '{}' (content unchanged)",
                insert_plan.content_target_id
            );
        }
        let required_chunk_count = count_required_chunks(&updated_manifest);
        let required_bundle_count =
            count_required_bundles_from_chunk_index(&updated_manifest, &store_state.chunk_index);
        return Ok(unchanged_target_insert_stats(
            request.command_name,
            &request.repo_paths.repo_root,
            request.input_dir,
            &manifest_path,
            request.repo_name,
            &updated_manifest,
            TargetIdentityStrings {
                version_id: insert_plan.version_id.clone(),
                os: insert_plan.os.clone(),
                arch: insert_plan.arch.clone(),
            },
            request.build_profile.storage_mode,
            required_chunk_count,
            required_bundle_count,
            &g::BuildTimings::default(),
            request.stats_json_out,
        ));
    }

    let prepared_input = prepare_input_path(&request.input_dir.to_path_buf())?;
    let context = format!("target commit input '{}'", request.input_dir.display());
    let resolved_identity = g::core::target_identity::resolve_target_identity_with_inference(
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
        g::BuildConfig::with_ids(request.repo_name, "", &resolved_identity.target_tags)
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
    let target_id = g::compute_target_id_from_files(&result.target_manifest.files);
    result.target_manifest.target_id = target_id.clone();
    apply_resolved_target_identity(&mut result.target_manifest, &resolved_identity);

    let existing_manifest_path = g::target_manifest_path(request.repo_paths, &target_id);
    if existing_manifest_path.exists() {
        let existing_manifest = g::load_target_manifest(request.repo_paths, &target_id)?;
        if existing_manifest.target_tags != result.target_manifest.target_tags {
            let mut updated_manifest = existing_manifest;
            updated_manifest.target_tags = result.target_manifest.target_tags.clone();
            let (manifest_path, _) = write_target_manifest(request.repo_paths, &updated_manifest)?;
            if verbose_output_enabled() {
                println!(
                    "updated target tags for content id '{}' (content unchanged)",
                    target_id
                );
            }

            let required_chunk_count = count_required_chunks(&updated_manifest);
            let required_bundle_count = count_required_bundles_from_chunk_index(
                &updated_manifest,
                &store_state.chunk_index,
            );
            return Ok(unchanged_target_insert_stats(
                request.command_name,
                &request.repo_paths.repo_root,
                request.input_dir,
                &manifest_path,
                request.repo_name,
                &updated_manifest,
                resolved_identity_strings(&resolved_identity.target_tags),
                request.build_profile.storage_mode,
                required_chunk_count,
                required_bundle_count,
                &timings,
                request.stats_json_out,
            ));
        }
    }

    let (manifest_path, _) = write_target_manifest(request.repo_paths, &result.target_manifest)?;
    write_bundles(&request.repo_paths.bundles_root, &result.bundles)?;
    write_loose_chunks(&request.repo_paths.chunks_root, &result.loose_chunks)?;
    g::add_built_bundles_to_chunk_index(&mut store_state.chunk_index, &result.bundles)?;
    g::add_built_loose_chunks_to_chunk_index(&mut store_state.chunk_index, &result.loose_chunks)?;

    for bundle in &result.bundles {
        store_state.bundle_ids.insert(bundle.bundle_id.clone());
    }
    store_state.chunk_locations =
        g::repo::existing_chunk_locations_from_index(&store_state.chunk_index, request.repo_paths);
    for file in &result.target_manifest.files {
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
            result.target_manifest.files.len(),
            result.target_manifest.total_logical_size
        );

        if let Ok(est) = g::estimate_build(&result.target_manifest, &Default::default(), 0) {
            println!(
                "estimate: {} chunks missing, {:.1} MiB logical",
                est.missing_chunk_count,
                est.missing_logical_bytes as f64 / (1024.0 * 1024.0)
            );
        }
    }

    print_build_timings(&timings);

    let required_chunk_count = count_required_chunks(&result.target_manifest);
    let required_bundle_count = count_required_bundles_for_build(
        &result.target_manifest,
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
    let reused_chunk_count = g::repo::required_chunk_hashes_typed(&result.target_manifest)?
        .iter()
        .filter(|chunk_id| existing_chunk_ids.contains(*chunk_id))
        .count();
    let new_chunk_count = required_chunk_count.saturating_sub(reused_chunk_count);
    let reused_bundle_count = required_bundle_count.saturating_sub(generated_bundle_count);
    let reused_chunk_percent = ratio_pct(reused_chunk_count as u64, required_chunk_count as u64);
    let reused_bundle_percent = ratio_pct(reused_bundle_count as u64, required_bundle_count as u64);
    let resolved_identity_strings = resolved_identity_strings(&resolved_identity.target_tags);
    let stats = TargetInsertStats {
        command: request.command_name,
        repo_root: request.repo_paths.repo_root.display().to_string(),
        input_path: request.input_dir.display().to_string(),
        target_id: result.target_manifest.target_id.clone(),
        manifest_path: manifest_path.display().to_string(),
        product_id: request.repo_name.to_string(),
        remote_repo_id: request.repo_name.to_string(),
        version_id: resolved_identity_strings.version_id,
        os: resolved_identity_strings.os,
        arch: resolved_identity_strings.arch,
        storage_mode: request.build_profile.storage_mode.cli_name().to_string(),
        file_count: result.target_manifest.files.len(),
        total_logical_size: result.target_manifest.total_logical_size,
        total_compressed_size_estimate: result.target_manifest.total_compressed_size_estimate,
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

fn collect_insert_many_sources(
    product_dir: Option<PathBuf>,
    explicit_dirs: Vec<PathBuf>,
) -> g::Result<Vec<g::source_config::ResolvedSourceTarget>> {
    let mut sources: Vec<g::source_config::ResolvedSourceTarget> = Vec::new();
    if let Some(product_dir) = product_dir {
        if !product_dir.is_dir() {
            return Err(g::Error::manifest(format!(
                "product dir is not a directory: {}",
                product_dir.display()
            )));
        }
        let config = g::source_config::SourceConfig::load_or_create(&product_dir)?;
        sources.extend(config.discover_target_sources(&product_dir)?);
    }
    for dir in explicit_dirs {
        sources.push(g::source_config::ResolvedSourceTarget {
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
            "target commit requires at least one --dir or --product-dir with target folders",
        ));
    }

    Ok(sources)
}

/// Resolve the config contribution for a single `insert --dir` target.
/// The config is read from the directory's parent when present; it is never
/// auto-created for a single insert. Returns the entry tags (explicit mode)
/// and whether name inference is allowed.
fn single_insert_source_resolution(
    dir: &Path,
) -> g::Result<(Option<g::manifest::TargetTagSet>, bool)> {
    let parent = dir.parent().ok_or_else(|| {
        g::Error::manifest(format!(
            "target input has no parent directory: {}",
            dir.display()
        ))
    })?;
    let Some(config) = g::source_config::SourceConfig::load(parent)? else {
        return Ok((None, true));
    };
    let Some(name) = dir.file_name().and_then(|value| value.to_str()) else {
        return Err(g::Error::manifest(format!(
            "target input has no usable folder name: {}",
            dir.display()
        )));
    };
    match config.ingestion_mode {
        g::source_config::IngestionMode::Auto => Ok((None, true)),
        g::source_config::IngestionMode::Explicit => {
            let entry = config.explicit_ingestion_rules.targets.iter().find(|rule| {
                g::source_config::normalize_relative_path(&rule.path)
                    .ok()
                    .as_deref()
                    == Some(name)
            });
            match entry {
                Some(rule) => {
                    let tags = g::manifest::TargetTagSet::from_compact_strings(&rule.tags)
                        .map_err(|err| {
                            g::Error::manifest(format!(
                                "invalid tags for explicit target '{}': {err}",
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

pub fn run(command: TargetCommands) -> g::Result<()> {
    match command {
        TargetCommands::LaunchSet {
            repo,
            target,
            launch,
            clear,
        } => {
            if clear == launch.is_some() {
                return Err(g::Error::manifest(
                    "provide either --launch <path> or --clear",
                ));
            }
            let configuration = if clear {
                None
            } else {
                let path = launch.as_ref().expect("launch path checked");
                let json = crate::helpers::read_to_string(path)?;
                Some(serde_json::from_str::<g::ResolvedLaunchConfiguration>(
                    &json,
                )?)
            };
            g::update_gutch_target_launch(&repo, &target, configuration, Vec::new())?;
            println!("updated target launch configuration");
        }
        TargetCommands::Commit {
            dir,
            product_dir,
            repo,
            tags,
            build_profile,
            runtime,
            stats_json_out,
            yes,
        } => {
            let sources = collect_insert_many_sources(product_dir.clone(), dir)?;
            let cli_tags = parse_cli_tags(&tags)?;
            if sources.len() == 1 && product_dir.is_none() {
                let source = &sources[0];
                let (config_tags, allow_name_inference) =
                    single_insert_source_resolution(&source.path)?;
                let mut explicit_tags = cli_tags.clone();
                if let Some(config_tags) = config_tags {
                    explicit_tags.extend_missing_from(&config_tags);
                }
                let plan = g::plan_target_insert(
                    repo.as_path(),
                    &source.path,
                    &explicit_tags,
                    allow_name_inference,
                )?;
                if !yes {
                    println!("target commit plan:");
                    println!("  {}", g::describe_target_insert_plan(&plan));
                    if plan.invalidates_existing_installs {
                        println!(
                            "  User installs pinned to replaced content ids become invalid until updated."
                        );
                    }
                    if !prompt_yes_no("Proceed?", false)? {
                        return Err(g::Error::manifest("target commit cancelled"));
                    }
                }
                let progress = CliProgressSink::new();
                let insert_result = g::insert_repo_target(g::RepoTargetInsertRequest {
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
                let stats = TargetInsertStats {
                    command: "target commit",
                    repo_root: insert_result.repo_root.display().to_string(),
                    input_path: source.path.display().to_string(),
                    target_id: insert_result.target_id,
                    manifest_path: insert_result.manifest_path.display().to_string(),
                    product_id: repo_manifest.name.clone(),
                    remote_repo_id: repo_manifest.name.clone(),
                    version_id: insert_result.version_id,
                    os: insert_result.os,
                    arch: insert_result.arch,
                    storage_mode: build_profile.storage_mode.cli_name().to_string(),
                    file_count: insert_result.file_count,
                    total_logical_size: insert_result.total_logical_size,
                    total_compressed_size_estimate: insert_result.total_compressed_size_estimate,
                    required_chunk_count: insert_result.required_chunk_count,
                    required_bundle_count: insert_result.required_bundle_count,
                    generated_bundle_count: insert_result.generated_bundle_count,
                    generated_bundle_bytes: 0,
                    generated_chunk_count: insert_result.generated_chunk_count,
                    generated_chunk_bytes: 0,
                    reused_chunk_count: insert_result.reused_chunk_count,
                    new_chunk_count: insert_result.new_chunk_count,
                    reused_chunk_percent: ratio_pct(
                        insert_result.reused_chunk_count as u64,
                        insert_result.required_chunk_count as u64,
                    ),
                    reused_bundle_count: insert_result
                        .required_bundle_count
                        .saturating_sub(insert_result.generated_bundle_count),
                    reused_bundle_percent: ratio_pct(
                        insert_result
                            .required_bundle_count
                            .saturating_sub(insert_result.generated_bundle_count)
                            as u64,
                        insert_result.required_bundle_count as u64,
                    ),
                    timings: build_timings_summary(&insert_result.build_timings),
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
                let rules = repo_manifest.target_schema.clone().unwrap_or_default();
                let repo_name = repo_manifest.name.clone();
                let insert_plans: Vec<g::TargetInsertPlan> = sources
                    .iter()
                    .map(|source| {
                        let mut effective_tags = cli_tags.clone();
                        if let Some(config_tags) = &source.config_tags {
                            effective_tags.extend_missing_from(config_tags);
                        }
                        g::plan_target_insert(
                            repo_paths.repo_root.as_path(),
                            &source.path,
                            &effective_tags,
                            source.allow_name_inference,
                        )
                    })
                    .collect::<g::Result<_>>()?;
                if !yes {
                    println!("target commit plan:");
                    for plan in &insert_plans {
                        println!("  {}", g::describe_target_insert_plan(plan));
                    }
                    if insert_plans
                        .iter()
                        .any(|plan| plan.invalidates_existing_installs)
                    {
                        println!(
                            "  User installs pinned to replaced content ids become invalid until updated."
                        );
                    }
                    if !prompt_yes_no("Proceed?", false)? {
                        return Err(g::Error::manifest("target commit cancelled"));
                    }
                }
                let mut store_state: MutableInsertStoreState =
                    scan_existing_store_state(&repo_paths)?.into();
                let empty_output = None;

                let mut target_stats = Vec::with_capacity(sources.len());
                for source in &sources {
                    let mut effective_tags = cli_tags.clone();
                    if let Some(config_tags) = &source.config_tags {
                        effective_tags.extend_missing_from(config_tags);
                    }
                    target_stats.push(run_insert_one_target(
                        InsertOneTargetRequest {
                            input_dir: &source.path,
                            repo_paths: &repo_paths,
                            repo_name: &repo_name,
                            rules: &rules,
                            explicit_tags: &effective_tags,
                            allow_name_inference: source.allow_name_inference,
                            build_profile,
                            runtime_hints: runtime.hints(),
                            stats_json_out: &empty_output,
                            command_name: "target commit",
                            yes,
                            skip_confirmation: true,
                        },
                        &mut store_state,
                    )?);
                }

                g::write_chunk_index(&repo_paths, &store_state.chunk_index)?;

                let total_logical_size = target_stats
                    .iter()
                    .map(|stats| stats.total_logical_size)
                    .sum();
                let total_compressed_size_estimate = target_stats
                    .iter()
                    .map(|stats| stats.total_compressed_size_estimate)
                    .sum();
                let generated_bundle_count = target_stats
                    .iter()
                    .map(|stats| stats.generated_bundle_count)
                    .sum();
                let generated_bundle_bytes = target_stats
                    .iter()
                    .map(|stats| stats.generated_bundle_bytes)
                    .sum();
                let generated_chunk_count = target_stats
                    .iter()
                    .map(|stats| stats.generated_chunk_count)
                    .sum();
                let generated_chunk_bytes = target_stats
                    .iter()
                    .map(|stats| stats.generated_chunk_bytes)
                    .sum();
                let total_build_seconds = target_stats
                    .iter()
                    .map(|stats| stats.timings.total_seconds)
                    .sum();
                let aggregate_stats = TargetInsertManyStats {
                    command: "target commit",
                    repo_root: repo_paths.repo_root.display().to_string(),
                    target_count: target_stats.len(),
                    total_logical_size,
                    total_compressed_size_estimate,
                    generated_bundle_count,
                    generated_bundle_bytes,
                    generated_chunk_count,
                    generated_chunk_bytes,
                    total_build_seconds,
                    targets: target_stats,
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

        TargetCommands::Delete { repo, target, yes } => {
            let (repo_paths, _repo_manifest, _, _tm) =
                load_target_manifest_from_repo(&repo, &target)?;
            if !yes {
                let manifest_path = g::target_manifest_path(&repo_paths, &target);
                eprintln!(
                    "WARNING: this will remove '{}' and related target artifacts, and may orphan bundles. Use --yes to confirm.",
                    manifest_path.display()
                );
                return Err(g::Error::manifest("deletion requires --yes flag"));
            }

            let progress = CliProgressSink::new();
            let control = g::GutchOperationControl::default().with_progress(progress);
            let orphan_count = g::delete_repo_target_and_cleanup(&repo, &target, &control)?;

            println!(
                "deleted target '{}': removed manifest, {} orphaned bundles cleaned up",
                target, orphan_count
            );
        }

        TargetCommands::Extract {
            repo,
            target,
            output,
            runtime,
            restore_verify,
            stats_json_out,
        } => {
            let (repo_paths, repo_manifest, _manifest_path, tm) =
                load_target_manifest_from_repo(&repo, &target)?;
            let chunk_index = g::load_chunk_index(&repo_paths)?;
            let required_bundle_ids = g::repo::required_bundle_ids(&tm, &chunk_index);

            let runtime_hints = runtime.hints();
            let (_, timings) = g::reconstruct_from_repo(
                &tm,
                &repo_paths,
                &repo_manifest.compression_profile(),
                &output,
                &runtime_hints,
                g::ReconstructOptions {
                    verify_chunks: restore_verify,
                    verify_files: restore_verify,
                },
                &g::GutchOperationControl::default(),
            )?;
            if verbose_output_enabled() {
                println!("reconstructed to: {}", output.display());
            }
            print_reconstruct_timings(&timings);

            let stats = TargetExportStats {
                command: "target extract",
                repo_root: repo_paths.repo_root.display().to_string(),
                target_id: tm.target_id.clone(),
                output_dir: output.display().to_string(),
                file_count: tm.files.len(),
                required_chunk_count: count_required_chunks(&tm),
                required_bundle_count: count_required_bundles_from_chunk_index(&tm, &chunk_index),
                resolved_bundle_file_count: required_bundle_ids.len(),
                restore_verify,
                total_logical_size: tm.total_logical_size,
                total_compressed_size_estimate: tm.total_compressed_size_estimate,
                timings: reconstruct_timings_summary(&timings),
                outputs: optional_output_paths(&stats_json_out),
            };
            write_report(&stats_json_out, &stats)?;
        }

        TargetCommands::Estimate { dir, repo, tags } => {
            let prepared_input = prepare_input_path(&dir)?;
            let rules = if let Some(repo) = repo.as_ref() {
                let (_, repo_manifest) = g::load_gutch_repo_manifest(repo)?;
                repo_manifest.target_schema.unwrap_or_default()
            } else {
                g::manifest::TargetTagSchema::default()
            };
            let explicit_tags = parse_cli_tags(&tags)?;
            let context = format!("target estimate input '{}'", dir.display());
            let resolved_identity = g::core::target_identity::resolve_target_identity(
                &dir,
                &explicit_tags,
                &rules,
                &context,
            )?;
            let result = g::build_from_directory(
                &prepared_input.dir,
                "estimate",
                "estimate",
                &resolved_identity.target_tags,
            )?;

            let est = g::estimate_build(
                &result.target_manifest,
                &Default::default(),
                1024 * 1024 * 1024,
            )?;

            println!("fresh install estimate:");
            println!("  files: {}", result.target_manifest.files.len());
            println!(
                "  logical size:  {:.2} MiB",
                result.target_manifest.total_logical_size as f64 / (1024.0 * 1024.0)
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
