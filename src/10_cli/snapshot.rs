//! `snapshot` command handlers.

use crate::capture::contracts::SnapshotCaptureRequest;
use crate::capture::contracts::SnapshotCaptureResult;
use crate::capture::estimate::estimate_snapshot;
use crate::capture::plan::describe_snapshot_capture_plan;
use crate::capture::sources::capture_snapshots;
use crate::capture::sources::config::initialize_source_config;
use crate::capture::sources::plan_snapshot_captures;
use crate::capture::sources::resolve_capture_sources;
use crate::cli::Profile;
use crate::cli::SECTION_SEPARATOR;
use crate::cli::SnapshotCommands;
use crate::cli::arguments::CaptureArgs;
use crate::cli::arguments::SnapshotTagsCommands;
use crate::cli::command_support::SnapshotCaptureManyStats;
use crate::cli::command_support::SnapshotCaptureStats;
use crate::cli::command_support::load_snapshot_manifest_from_repo;
use crate::cli::command_support::prompt_yes_no;
use crate::cli::command_support::write_report;
use crate::cli::control_with_profile;
use crate::cli::output;
use crate::cli::output::dry_run;
use crate::cli::progress::CliProgressSink;
use crate::cli::reporting::build_timings_summary;
use crate::cli::reporting::mib;
use crate::cli::reporting::print_profile_summary;
use crate::cli::reporting::profile_mode;
use crate::cli::reporting::ratio_pct;
use crate::cli::snapshot_transfer;
use crate::cli::snapshot_transfer::FetchCommand;
use crate::cli::tags::parse_cli_tags;
use crate::filesystem;
use crate::manifest::repo_manifest::RepoManifest;
use crate::profiling::ProfileMode;
use crate::repo::read::load_gutch_repo_manifest;
use crate::repo::snapshot_manifests::diff_repo_snapshots;
use crate::repo::snapshots::delete_repo_snapshot_and_cleanup;
use crate::repo::snapshots::plan_snapshot_tag_updates;
use crate::repo::snapshots::retag_snapshots;
use crate::repo::status::read_repo_status;
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
        repo_name: repository.name.clone(),
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
        generated_chunk_count: result.generated_chunk_count,
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
        SnapshotCommands::Diff { repo, from, to } => diff(&repo, &from, &to, profile),
        SnapshotCommands::Capture(args) => capture(args, profile),
        SnapshotCommands::List { repo } => list(&repo),
        SnapshotCommands::Delete {
            repo,
            snapshot_id,
            yes,
        } => delete(&repo, &snapshot_id, yes, profile),
        SnapshotCommands::Tags {
            command:
                SnapshotTagsCommands::Replace {
                    repo,
                    snapshot_id,
                    tags,
                    tags_file,
                    yes,
                },
        } => replace_tags(
            &repo,
            snapshot_id,
            &tags,
            tags_file.as_deref(),
            yes,
            profile,
        ),
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
        } => snapshot_transfer::fetch(
            FetchCommand {
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
        ),
        SnapshotCommands::Measure { source_dir, tags } => measure(&source_dir, &tags),
    }
}

fn diff(repo: &Path, from: &str, to: &str, profile: &Profile) -> crate::Result<()> {
    let result = diff_repo_snapshots(repo, from, to, &control_with_profile(profile))?;
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
        source_dirs,
        source_root,
        repo,
        tags,
        build_profile,
        runtime,
        yes,
    } = args;
    let single_report = source_root.is_none();
    let sources =
        resolve_capture_sources(source_root.clone(), source_dirs, parse_cli_tags(&tags)?)?;
    let (repo_paths, repo_manifest) = load_gutch_repo_manifest(&repo)?;
    let requests = sources
        .iter()
        .map(|source| SnapshotCaptureRequest {
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
            control: control_with_profile(profile).with_progress(CliProgressSink::new()),
        })
        .collect::<Vec<_>>();
    if dry_run() {
        let plans = plan_snapshot_captures(&requests)?;
        output::record(&plans)?;
        for plan in plans {
            output::line(format_args!("{}", describe_snapshot_capture_plan(&plan)));
        }
        return Ok(());
    }
    if !yes {
        output::line(format_args!("snapshot capture plan:"));
        for plan in plan_snapshot_captures(&requests)? {
            output::line(format_args!("  {}", describe_snapshot_capture_plan(&plan)));
        }
        if !prompt_yes_no("Proceed?", false)? {
            return Err(crate::Error::Cancelled);
        }
    }
    let results = capture_snapshots(requests)?;
    if let Some(source_root) = source_root {
        initialize_source_config(&source_root)?;
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
        write_report(&stats, profile.as_deref())?;
    } else {
        let aggregate_stats = aggregate_capture_stats(
            &repo_paths.repo_root.display().to_string(),
            snapshot_stats,
            profile_mode(profile.as_deref()),
        );
        write_report(&aggregate_stats, profile.as_deref())?;
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
        generated_chunk_count: snapshot_stats
            .iter()
            .map(|stats| stats.generated_chunk_count)
            .sum(),
        total_build_seconds: snapshot_stats
            .iter()
            .map(|stats| stats.timings.total_seconds)
            .sum::<Option<f64>>()
            .filter(|_| mode == ProfileMode::Detailed),
        snapshots: snapshot_stats,
    }
}

fn list(repo: &Path) -> crate::Result<()> {
    let status = read_repo_status(repo)?;
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
        output::line(format_args!("{}", snapshot.snapshot_id));
    }
    output::line(format_args!("{SECTION_SEPARATOR}"));
    output::line(format_args!("Total"));
    output::line(format_args!("name: {}", status.name));
    output::line(format_args!("snapshots: {}", status.snapshots.len()));
    output::line(format_args!("bundles: {}", status.bundle_count));
    Ok(())
}

fn delete(repo: &Path, snapshot_id: &str, yes: bool, profile: &Profile) -> crate::Result<()> {
    load_snapshot_manifest_from_repo(repo, snapshot_id)?;
    if dry_run() {
        output::record(&serde_json::json!({"operation": "delete", "snapshot": snapshot_id}))?;
        output::line(format_args!(
            "Would delete snapshot {snapshot_id} and clean up unreferenced artifacts"
        ));
        return Ok(());
    }
    if !yes
        && !prompt_yes_no(
            "Delete this snapshot and clean up unreferenced artifacts?",
            false,
        )?
    {
        return Err(crate::Error::Cancelled);
    }

    let control = control_with_profile(profile).with_progress(CliProgressSink::new());
    let orphan_count = delete_repo_snapshot_and_cleanup(repo, snapshot_id, &control)?;
    output::line(format_args!(
        "deleted snapshot '{snapshot_id}': removed manifest, {orphan_count} orphaned artifacts cleaned up"
    ));
    Ok(())
}

fn replace_tags(
    repo: &Path,
    snapshot_id: Option<String>,
    tags: &[String],
    tags_file: Option<&Path>,
    yes: bool,
    profile: &Profile,
) -> crate::Result<()> {
    let single_snapshot = snapshot_id.is_some();
    let replacements = if let Some(path) = tags_file {
        filesystem::read_json(path)?
    } else {
        BTreeMap::from([(
            snapshot_id
                .ok_or_else(|| crate::Error::invalid_input("select a snapshot or tags file"))?,
            parse_cli_tags(tags)?,
        )])
    };
    if replacements.is_empty() {
        return Err(crate::Error::invalid_input("select at least one snapshot"));
    }
    let preview = plan_snapshot_tag_updates(repo, &replacements)?;
    for change in &preview {
        output::line(format_args!(
            "{}: {} -> {}",
            change.snapshot_id,
            change.previous_tags.to_canonical_strings().join(" "),
            change.new_tags.to_canonical_strings().join(" ")
        ));
    }
    if dry_run() {
        output::record(&preview)?;
        return Ok(());
    }
    if !yes && !prompt_yes_no("Apply these tag changes?", false)? {
        return Err(crate::Error::Cancelled);
    }
    let results = retag_snapshots(repo, replacements, &control_with_profile(profile))?;
    if single_snapshot {
        output::record(&results[0])
    } else {
        output::record(&results)
    }
}

fn measure(source_dir: &Path, tags: &[String]) -> crate::Result<()> {
    let result = estimate_snapshot(source_dir, &parse_cli_tags(tags)?)?;
    let estimate = &result.estimate;
    output::record(&serde_json::json!({
        "file_count": result.file_count,
        "logical_bytes": result.total_logical_size,
        "unique_chunks": estimate.unique_chunk_count,
        "unique_chunk_bytes": estimate.unique_chunk_bytes,
        "encoded_artifact_bytes": estimate.encoded_artifact_bytes,
        "bundle_count": estimate.bundle_count,
    }))?;
    output::line(format_args!(
        "fresh capture measurement (default packaging presets):"
    ));
    output::line(format_args!("  files: {}", result.file_count));
    output::line(format_args!(
        "  logical size:  {:.2} MiB",
        mib(result.total_logical_size)
    ));
    output::line(format_args!(
        "  unique chunks: {}",
        estimate.unique_chunk_count
    ));
    output::line(format_args!(
        "  unique chunk bytes: {:.2} MiB",
        mib(estimate.unique_chunk_bytes)
    ));
    output::line(format_args!(
        "  encoded artifacts: {:.2} MiB",
        mib(estimate.encoded_artifact_bytes)
    ));
    output::line(format_args!("  bundles: {}", estimate.bundle_count));
    Ok(())
}
