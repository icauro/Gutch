//! Captures a folder into a new repository, reconstructs it and verifies the result.
//!
//! Run with `cargo run --example capture_and_install -- SOURCE_DIR WORK_DIR`.
//! WORK_DIR must not exist yet; the example creates `repo/` and `install/` in it.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use gutch::api::{
    ArtifactStorageMode, BundleSizingPreset, ChunkSizingPreset, Chunker, CompressionKind,
    InstallationRequest, InstallationVerifyRequest, OperationControl, RepoInitOptions, Result,
    RuntimeResourceHints, SnapshotCaptureRequest, SnapshotTagSet, VerifyMode, capture_snapshot,
    initialize_repo, reconstruct_snapshot, verify_installation,
};

fn main() -> ExitCode {
    let arguments: Vec<PathBuf> = env::args_os().skip(1).map(PathBuf::from).collect();
    let [source_dir, work_dir] = arguments.as_slice() else {
        eprintln!("usage: capture_and_install SOURCE_DIR WORK_DIR");
        return ExitCode::from(2);
    };
    match run(source_dir.clone(), work_dir.clone()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(source_dir: PathBuf, work_dir: PathBuf) -> Result<()> {
    let repo_root = work_dir.join("repo");
    let destination_dir = work_dir.join("install");
    initialize_repo(&repo_root, RepoInitOptions::default())?;

    let mut tags = SnapshotTagSet::default();
    tags.insert("version", "1.0.0")?;
    let captured = capture_snapshot(&SnapshotCaptureRequest {
        repo_root: repo_root.clone(),
        source_dir,
        explicit_tags: tags,
        chunk_profile: ChunkSizingPreset::default(),
        chunker: Chunker::default(),
        bundle_profile: BundleSizingPreset::default(),
        compression_algorithm: CompressionKind::Zstd,
        compression_level: None,
        storage_mode: ArtifactStorageMode::Bundled,
        runtime_hints: RuntimeResourceHints::default(),
        control: OperationControl::default(),
    })?;
    println!("captured snapshot {}", captured.snapshot_id);

    reconstruct_snapshot(&InstallationRequest {
        repo_root: repo_root.clone(),
        snapshot_id: captured.snapshot_id.clone(),
        destination_dir: destination_dir.clone(),
        staging_dir: None,
        source_snapshot_id: None,
        source_dir: None,
        runtime_hints: RuntimeResourceHints::default(),
        control: OperationControl::default(),
    })?;

    let verified = verify_installation(&InstallationVerifyRequest {
        repo_root,
        snapshot_id: captured.snapshot_id,
        destination_dir: destination_dir.clone(),
        verify_mode: VerifyMode::Fast,
        runtime_hints: RuntimeResourceHints::default(),
        control: OperationControl::default(),
    })?;
    let issues = &verified.classification;
    println!(
        "installed to {}: {} missing, {} corrupt, {} extra",
        destination_dir.display(),
        issues.missing_files.len(),
        issues.corrupt_files.len(),
        issues.extra_files.len()
    );
    Ok(())
}
