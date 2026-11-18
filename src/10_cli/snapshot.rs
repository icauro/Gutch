//! `snapshot` command handlers.

use crate::capture::contracts::SnapshotCapturePlan;
use crate::capture::contracts::SnapshotCaptureRequest;
use crate::capture::contracts::SnapshotCaptureResult;
use crate::capture::measure::SnapshotMeasureRequest;
use crate::capture::measure::measure_snapshot;
use crate::capture::sources::capture_snapshots;
use crate::capture::sources::config::init_source_config;
use crate::capture::sources::plan_capture_snapshots;
use crate::capture::sources::resolve_capture_sources;
use crate::cli::Profile;
use crate::cli::SECTION_SEPARATOR;
use crate::cli::SnapshotCommands;
use crate::cli::arguments::CaptureArgs;
use crate::cli::arguments::ConfirmArgs;
use crate::cli::arguments::SnapshotTagsCommands;
use crate::cli::command_support::SnapshotCaptureManyStats;
use crate::cli::command_support::SnapshotCaptureStats;
use crate::cli::command_support::load_snapshot_manifest_from_repo;
use crate::cli::command_support::local_snapshot_id;
use crate::cli::command_support::prompt_yes_no;
use crate::cli::control_with_profile;
use crate::cli::output;
use crate::cli::progress::CliProgressSink;
use crate::cli::reporting::build_timings_summary;
use crate::cli::reporting::mib;
use crate::cli::reporting::print_profile_summary;
use crate::cli::reporting::profile_mode;
use crate::cli::reporting::ratio_pct;
use crate::cli::reporting::write_stats_json_output;
use crate::cli::snapshot_transfer;
use crate::filesystem;
use crate::manifest::SnapshotTagSet;
use crate::manifest::repo_manifest::RepoManifest;
use crate::profiling::ProfileMode;
use crate::progress::OperationControl;
use crate::repo::paths::RepoPaths;
use crate::repo::read::load_repo_manifest;
use crate::repo::snapshot_manifests::SnapshotDiffRequest;
use crate::repo::snapshot_manifests::diff_snapshots;
use crate::repo::snapshots::SnapshotDeleteRequest;
use crate::repo::snapshots::SnapshotTagsReplaceRequest;
use crate::repo::snapshots::delete_snapshot;
use crate::repo::snapshots::plan_replace_snapshot_tags;
use crate::repo::snapshots::replace_snapshot_tags;
use crate::repo::status::repo_status;
use std::collections::BTreeMap;
use std::path::Path;

/// Maps an engine capture result to CLI stats. The generic command calls the
/// same additive engine path as every other caller instead of reimplementing
/// capture.
fn capture_stats_from_engine_result(
    repo_root: &str,
    input_path: &str,
    repository: &RepoManifest,
    storage_mode_cli: String,
    result: &SnapshotCaptureResult,
    mode: ProfileMode,
) -> SnapshotCaptureStats {
    SnapshotCaptureStats {
        command: "snapshot capture",
        repo_root: repo_root.to_string(),
        input_path: input_path.to_string(),
        snapshot_id: result.snapshot_id.clone(),
        manifest_path: result.manifest_path.display().to_string(),
        repo_name: repository.repository_name.clone(),
        remote_repo_id: repository.remote_repo_id.clone(),
        version_tag: result.version_tag.clone(),
        os: result.os.clone(),
        arch: result.arch.clone(),
        storage_mode: storage_mode_cli,
        file_count: result.file_count,
        total_logical_size: result.total_logical_size,
        total_compressed_size_estimate: result.total_compressed_size_estimate,
        required_chunk_count: result.required_chunk_count,
        required_bundle_count: result.required_bundle_count,
        generated_bundle_count: result.generated_bundle_count,
        generated_loose_chunk_count: result.generated_loose_chunk_count,
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
    }
}

pub(crate) fn run(command: SnapshotCommands, profile: &Profile) -> crate::Result<()> {
    match command {
        SnapshotCommands::Diff { repo, from, to } => diff(
            &repo,
            &local_snapshot_id(&repo, &from)?,
            &local_snapshot_id(&repo, &to)?,
            profile,
        ),
        SnapshotCommands::Capture(args) => capture(args, profile),
        SnapshotCommands::List { repo } => list(&repo),
        SnapshotCommands::Delete {
            repo,
            snapshot_id,
            confirm,
        } => delete(
            &repo,
            &local_snapshot_id(&repo, &snapshot_id)?,
            confirm,
            profile,
        ),
        SnapshotCommands::Tags {
            command:
                SnapshotTagsCommands::Replace {
                    repo,
                    snapshot_id,
                    tags,
                    tags_file,
                    confirm,
                },
        } => replace_tags(
            &repo,
            snapshot_id
                .map(|id| local_snapshot_id(&repo, &id))
                .transpose()?,
            &tags,
            tags_file.as_deref(),
            confirm,
            profile,
        ),
        SnapshotCommands::Fetch(arguments) => snapshot_transfer::fetch(arguments, profile),
        SnapshotCommands::Measure { source, tags } => measure(&source, &tags),
    }
}

fn diff(repo: &Path, from: &str, to: &str, profile: &Profile) -> crate::Result<()> {
    let result = diff_snapshots(&SnapshotDiffRequest {
        repo_root: repo.to_path_buf(),
        from_snapshot_id: from.to_owned(),
        to_snapshot_id: to.to_owned(),
        control: control_with_profile(profile),
    })?;
    output::record(&result)?;
    for entry in &result.changes {
        output::line(format_args!(
            "{:<18} {:>+12} B  {:?}",
            entry.change.as_str(),
            entry.delta_bytes,
            entry.path
        ));
    }
    output::line(format_args!(
        "{} changes; total {:+} B",
        result.changes.len(),
        result.delta_bytes
    ));
    Ok(())
}

fn capture(args: CaptureArgs, profile: &Profile) -> crate::Result<()> {
    let CaptureArgs {
        sources,
        discover_from,
        repo,
        tags,
        reuse_remote_bundles,
        build_profile,
        runtime,
        confirm,
    } = args;
    let single_report = discover_from.is_none();
    let sources = resolve_capture_sources(
        discover_from.clone(),
        sources,
        SnapshotTagSet::from_compact_strings(&tags)?,
    )?;
    let repo_manifest = load_repo_manifest(&repo)?;
    let repo_paths = RepoPaths::from_repo_root(&repo);
    let requests = sources
        .iter()
        .map(|source| SnapshotCaptureRequest {
            repo_root: repo.clone(),
            source: source.path.clone(),
            tags: source.tags.clone(),
            chunk_profile: build_profile.chunk_profile,
            chunker: build_profile.chunker,
            bundle_profile: build_profile.bundle_profile,
            compression_algorithm: build_profile.compression_algorithm,
            compression_level: build_profile.compression_level,
            storage_mode: build_profile.storage_mode,
            runtime_hints: runtime.hints(),
            reuse_remote_bundles,
            control: control_with_profile(profile).with_progress(CliProgressSink::new()),
        })
        .collect::<Vec<_>>();
    if confirm.dry_run {
        let plans = plan_capture_snapshots(&requests)?;
        output::record(&plans)?;
        for plan in plans {
            output::line(format_args!("{}", describe_capture_plan(&plan)));
        }
        return Ok(());
    }
    if !confirm.yes {
        output::line(format_args!("snapshot capture plan:"));
        for plan in plan_capture_snapshots(&requests)? {
            output::line(format_args!("  {}", describe_capture_plan(&plan)));
        }
        if !prompt_yes_no("Proceed?", false)? {
            return Err(crate::Error::Cancelled);
        }
    }
    let results = capture_snapshots(&requests)?;
    if let Some(discover_from) = discover_from {
        init_source_config(&discover_from)?;
    }
    let mut snapshot_stats = Vec::with_capacity(results.len());
    for (result, source) in results.iter().zip(&sources) {
        output::line(format_args!("{}", result.outcome.message()));
        snapshot_stats.push(capture_stats_from_engine_result(
            &repo.display().to_string(),
            &source.path.display().to_string(),
            &repo_manifest,
            build_profile.storage_mode.stats_name().to_string(),
            result,
            profile_mode(profile.as_deref()),
        ));
    }
    if single_report && snapshot_stats.len() == 1 {
        let stats = snapshot_stats.into_iter().next().unwrap();
        write_stats_json_output(&stats, profile.as_deref())?;
    } else {
        let aggregate_stats = aggregate_capture_stats(
            &repo_paths.repo_root.display().to_string(),
            snapshot_stats,
            profile_mode(profile.as_deref()),
        );
        write_stats_json_output(&aggregate_stats, profile.as_deref())?;
    }
    print_profile_summary(profile.as_deref());
    Ok(())
}

/// Sums the per-snapshot capture stats into one report.
fn aggregate_capture_stats(
    repo_root: &str,
    snapshot_stats: Vec<SnapshotCaptureStats>,
    mode: ProfileMode,
) -> SnapshotCaptureManyStats {
    let sum =
        |field: fn(&SnapshotCaptureStats) -> u64| snapshot_stats.iter().map(field).sum::<u64>();
    SnapshotCaptureManyStats {
        command: "snapshot capture",
        repo_root: repo_root.to_string(),
        snapshot_count: snapshot_stats.len(),
        total_logical_size: sum(|stats| stats.total_logical_size),
        total_compressed_size_estimate: sum(|stats| stats.total_compressed_size_estimate),
        generated_bundle_count: snapshot_stats
            .iter()
            .map(|stats| stats.generated_bundle_count)
            .sum(),
        generated_loose_chunk_count: snapshot_stats
            .iter()
            .map(|stats| stats.generated_loose_chunk_count)
            .sum(),
        total_capture_seconds: snapshot_stats
            .iter()
            .map(|stats| stats.timings.total_seconds)
            .sum::<Option<f64>>()
            .filter(|_| mode != ProfileMode::Off),
        snapshots: snapshot_stats,
    }
}

fn list(repo: &Path) -> crate::Result<()> {
    let status = repo_status(repo)?;
    output::record(&status.snapshots)?;
    if !status.is_repo {
        return Err(crate::Error::invalid_input(format!(
            "no gutch repo found at {}",
            repo.display()
        )));
    }
    output::line(format_args!("{SECTION_SEPARATOR}"));
    output::line(format_args!("Snapshots"));
    for snapshot in &status.snapshots {
        let tags: Vec<String> = (snapshot.snapshot_tags.key_value.iter())
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        output::line(format_args!("{}  {}", snapshot.snapshot_id, tags.join(" ")));
    }
    output::line(format_args!("{SECTION_SEPARATOR}"));
    output::line(format_args!("Repository"));
    output::line(format_args!("name: {}", status.name));
    output::line(format_args!("snapshots: {}", status.snapshots.len()));
    output::line(format_args!("bundles: {}", status.bundle_count));
    Ok(())
}

fn delete(
    repo: &Path,
    snapshot_id: &str,
    confirm: ConfirmArgs,
    profile: &Profile,
) -> crate::Result<()> {
    load_snapshot_manifest_from_repo(repo, snapshot_id)?;
    if confirm.dry_run {
        output::record(&serde_json::json!({"operation": "delete", "snapshot": snapshot_id}))?;
        output::line(format_args!(
            "Would delete snapshot {snapshot_id} and clean up unreferenced artifacts"
        ));
        return Ok(());
    }
    if !confirm.yes
        && !prompt_yes_no(
            "Delete this snapshot and clean up unreferenced artifacts?",
            false,
        )?
    {
        return Err(crate::Error::Cancelled);
    }

    let control = control_with_profile(profile).with_progress(CliProgressSink::new());
    let result = delete_snapshot(&SnapshotDeleteRequest {
        repo_root: repo.to_path_buf(),
        snapshot_id: snapshot_id.to_owned(),
        control,
    })?;
    output::record(&result)?;
    output::line(format_args!(
        "deleted snapshot '{snapshot_id}': removed manifest, {} orphaned artifacts cleaned up",
        result.removed_artifact_count
    ));
    Ok(())
}

fn replace_tags(
    repo: &Path,
    snapshot_id: Option<String>,
    tags: &[String],
    tags_file: Option<&Path>,
    confirm: ConfirmArgs,
    profile: &Profile,
) -> crate::Result<()> {
    let single_snapshot = snapshot_id.is_some();
    let replacements = if let Some(path) = tags_file {
        filesystem::read_json(path)?
    } else {
        BTreeMap::from([(
            snapshot_id
                .ok_or_else(|| crate::Error::invalid_input("select a snapshot or tags file"))?,
            SnapshotTagSet::from_compact_strings(tags)?,
        )])
    };
    if replacements.is_empty() {
        return Err(crate::Error::invalid_input("select at least one snapshot"));
    }
    let request = SnapshotTagsReplaceRequest {
        repo_root: repo.to_path_buf(),
        replacements,
        control: control_with_profile(profile),
    };
    let preview = plan_replace_snapshot_tags(&request)?.changes;
    for change in &preview {
        output::line(format_args!(
            "{}: {} -> {}",
            change.snapshot_id,
            change.previous_tags.to_canonical_strings().join(" "),
            change.new_tags.to_canonical_strings().join(" ")
        ));
    }
    if confirm.dry_run {
        output::record(&preview)?;
        return Ok(());
    }
    if !confirm.yes && !prompt_yes_no("Apply these tag changes?", false)? {
        return Err(crate::Error::Cancelled);
    }
    let results = replace_snapshot_tags(&request)?.changes;
    if single_snapshot {
        output::record(&results[0])
    } else {
        output::record(&results)
    }
}

fn measure(source: &Path, tags: &[String]) -> crate::Result<()> {
    let result = measure_snapshot(&SnapshotMeasureRequest {
        source: source.to_path_buf(),
        tags: SnapshotTagSet::from_compact_strings(tags)?,
        control: OperationControl::default(),
    })?;
    output::record(&serde_json::json!({
        "file_count": result.file_count,
        "logical_bytes": result.logical_bytes,
        "unique_chunks": result.unique_chunk_count,
        "unique_chunk_bytes": result.unique_chunk_bytes,
        "encoded_artifact_bytes": result.encoded_artifact_bytes,
        "bundle_count": result.bundle_count,
    }))?;
    output::line(format_args!(
        "fresh capture measurement (default packaging presets):"
    ));
    output::line(format_args!("  files: {}", result.file_count));
    output::line(format_args!(
        "  logical size:  {:.2} MiB",
        mib(result.logical_bytes)
    ));
    output::line(format_args!(
        "  unique chunks: {}",
        result.unique_chunk_count
    ));
    output::line(format_args!(
        "  unique chunk bytes: {:.2} MiB",
        mib(result.unique_chunk_bytes)
    ));
    output::line(format_args!(
        "  encoded artifacts: {:.2} MiB",
        mib(result.encoded_artifact_bytes)
    ));
    output::line(format_args!("  bundles: {}", result.bundle_count));
    Ok(())
}

fn describe_capture_plan(plan: &SnapshotCapturePlan) -> String {
    format!(
        "Capture new snapshot from {} ({}) with tags {}",
        plan.folder_name,
        plan.version_tag,
        plan.snapshot_tags.to_canonical_strings().join(" ")
    )
}
