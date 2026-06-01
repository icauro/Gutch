use std::path::PathBuf;

use crate::cli::common::{
    TargetInstallApplyStats, TargetInstallVerifyStats, default_staging_dir,
    load_target_manifest_from_repo, resolve_required_bundle_paths, write_report,
};
use crate::cli::reporting::{
    optional_output_paths, print_reconstruct_timings, print_verify_timings, ratio_pct,
    reconstruct_timings_summary, verify_timings_summary,
};
use crate::cli::verbose_output_enabled;
use gutch as g;

pub(crate) struct ApplyCommand {
    pub(crate) repo: PathBuf,
    pub(crate) target: String,
    pub(crate) dir: PathBuf,
    pub(crate) staging_dir: Option<PathBuf>,
    pub(crate) source_target: Option<String>,
    pub(crate) source_dir: Option<PathBuf>,
    pub(crate) memory_mode: g::MemoryMode,
    pub(crate) max_memory_mib: Option<u64>,
    pub(crate) jobs: Option<usize>,
    pub(crate) summary_out: Option<PathBuf>,
    pub(crate) stats_json_out: Option<PathBuf>,
}

pub(crate) struct VerifyCommand {
    pub(crate) repo: PathBuf,
    pub(crate) target: String,
    pub(crate) dir: PathBuf,
    pub(crate) memory_mode: g::MemoryMode,
    pub(crate) max_memory_mib: Option<u64>,
    pub(crate) jobs: Option<usize>,
    pub(crate) summary_out: Option<PathBuf>,
    pub(crate) stats_json_out: Option<PathBuf>,
}

pub(crate) fn apply_install(command: ApplyCommand) -> g::Result<()> {
    let (repo_paths, _repo_manifest, _manifest_path, tm) =
        load_target_manifest_from_repo(&command.repo, &command.target)?;
    let bundle_paths = resolve_required_bundle_paths(&repo_paths.bundles_root)?;
    let staging_dir = command
        .staging_dir
        .unwrap_or_else(|| default_staging_dir(&command.dir, &tm.target_id));

    let source_manifest = match command.source_target.as_ref() {
        Some(source_target) => {
            let (_, _, _, source_manifest) =
                load_target_manifest_from_repo(&command.repo, source_target)?;
            Some(source_manifest)
        }
        None => None,
    };
    let source_install_dir = source_manifest.as_ref().map(|_| {
        command
            .source_dir
            .clone()
            .unwrap_or_else(|| command.dir.clone())
    });
    let source = source_manifest
        .as_ref()
        .zip(source_install_dir.as_ref())
        .map(
            |(source_manifest, source_install_dir)| g::TargetInstallApplySource {
                target_manifest: source_manifest,
                install_dir: source_install_dir,
            },
        );

    let runtime_hints = g::RuntimeResourceHints {
        memory_mode: command.memory_mode,
        max_memory_mib: command.max_memory_mib,
        jobs: command.jobs,
    };
    let result = g::apply_target_install_with_timings_and_hints(
        &tm,
        &bundle_paths,
        &command.dir,
        &staging_dir,
        source,
        &runtime_hints,
    )?;
    let reused_source_chunk_percent = ratio_pct(
        result.reused_source_chunk_count as u64,
        result.required_chunk_count as u64,
    );
    let stats = TargetInstallApplyStats {
        command: "target_install apply",
        repo_root: repo_paths.repo_root.display().to_string(),
        target_id: result.target_id.clone(),
        install_dir: result.install_dir.display().to_string(),
        staging_dir: result.staging_dir.display().to_string(),
        source_target_id: command.source_target.clone(),
        source_install_dir: source_install_dir
            .as_ref()
            .map(|path| path.display().to_string()),
        file_count: tm.files.len(),
        required_chunk_count: result.required_chunk_count,
        required_bundle_count: result.required_bundle_count,
        resolved_bundle_file_count: bundle_paths.len(),
        reused_source_chunk_count: result.reused_source_chunk_count,
        reused_source_chunk_percent,
        total_logical_size: tm.total_logical_size,
        total_compressed_size_estimate: tm.total_compressed_size_estimate,
        reconstruct_timings: reconstruct_timings_summary(&result.reconstruct_timings),
        verify_timings: verify_timings_summary(&result.verify_timings),
        outputs: optional_output_paths(&command.summary_out, &command.stats_json_out),
    };
    write_report(&command.summary_out, &command.stats_json_out, &stats)?;

    if verbose_output_enabled() {
        println!(
            "applied target {} to {}",
            stats.target_id, stats.install_dir
        );
        println!(
            "reused {} of {} chunks from source ({:.2}%)",
            stats.reused_source_chunk_count,
            stats.required_chunk_count,
            stats.reused_source_chunk_percent
        );
    }
    print_reconstruct_timings(&result.reconstruct_timings);
    print_verify_timings(&result.verify_timings);
    Ok(())
}

pub(crate) fn verify_install(command: VerifyCommand) -> g::Result<()> {
    let (repo_paths, _repo_manifest, _manifest_path, tm) =
        load_target_manifest_from_repo(&command.repo, &command.target)?;

    let runtime_hints = g::RuntimeResourceHints {
        memory_mode: command.memory_mode,
        max_memory_mib: command.max_memory_mib,
        jobs: command.jobs,
    };
    let (classification, timings) =
        g::classify_directory_with_progress(&tm, &command.dir, &runtime_hints)?;
    let missing_file_count = classification.missing_files.len();
    let corrupt_file_count = classification.corrupt_files.len();
    let extra_file_count = classification.extra_files.len();
    let errors = classification.into_messages();
    let stats = TargetInstallVerifyStats {
        command: "target_install verify",
        repo_root: repo_paths.repo_root.display().to_string(),
        target_id: tm.target_id.clone(),
        install_dir: command.dir.display().to_string(),
        expected_file_count: tm.files.len(),
        missing_file_count,
        corrupt_file_count,
        extra_file_count,
        verification_passed: errors.is_empty(),
        errors: errors.clone(),
        timings: verify_timings_summary(&timings),
        outputs: optional_output_paths(&command.summary_out, &command.stats_json_out),
    };
    write_report(&command.summary_out, &command.stats_json_out, &stats)?;

    if errors.is_empty() {
        if verbose_output_enabled() {
            println!("verification passed for {}", command.dir.display());
        }
        print_verify_timings(&timings);
    } else {
        eprintln!("{} verification errors:", errors.len());
        for err in &errors {
            eprintln!("  - {}", err);
        }
        std::process::exit(1);
    }

    Ok(())
}
