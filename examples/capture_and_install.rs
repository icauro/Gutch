//! Captures a folder into a new repository, reconstructs it and verifies the result.
//!
//! Run with `cargo run --example capture_and_install -- SOURCE_DIR WORK_DIR`.
//! WORK_DIR must not exist yet; the example creates `repo/` and `install/` in it.

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gutch::api::{
    InstallationReconstructRequest, InstallationVerifyRequest, RepoInitRequest, Result,
    SnapshotCaptureRequest, SnapshotTagSet, capture_snapshot, init_repo, reconstruct_installation,
    verify_installation,
};

fn main() -> ExitCode {
    let arguments: Vec<PathBuf> = env::args_os().skip(1).map(PathBuf::from).collect();
    let [source_dir, work_dir] = arguments.as_slice() else {
        eprintln!("usage: capture_and_install SOURCE_DIR WORK_DIR");
        return ExitCode::from(2);
    };
    if work_dir.exists() {
        eprintln!(
            "error: {} already exists; pick a new WORK_DIR",
            work_dir.display()
        );
        return ExitCode::from(2);
    }
    match run(source_dir, work_dir) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(source_dir: &Path, work_dir: &Path) -> Result<()> {
    let repo_root = work_dir.join("repo");
    let destination_dir = work_dir.join("install");
    init_repo(&RepoInitRequest {
        repo_root: repo_root.clone(),
        ..Default::default()
    })?;

    let mut tags = SnapshotTagSet::default();
    tags.insert("version", "1.0.0")?;
    let captured = capture_snapshot(&SnapshotCaptureRequest {
        repo_root: repo_root.clone(),
        source_dir: source_dir.to_path_buf(),
        tags,
        ..Default::default()
    })?;
    println!("captured snapshot {}", captured.snapshot_id);

    reconstruct_installation(&InstallationReconstructRequest {
        repo_root: repo_root.clone(),
        snapshot_id: captured.snapshot_id.clone(),
        destination_dir: destination_dir.clone(),
        ..Default::default()
    })?;

    let verified = verify_installation(&InstallationVerifyRequest {
        repo_root,
        snapshot_id: captured.snapshot_id,
        destination_dir: destination_dir.clone(),
        ..Default::default()
    })?;
    let issues = &verified.issues;
    println!(
        "installed to {}: {} missing, {} corrupt, {} extra",
        destination_dir.display(),
        issues.missing_file_count(),
        issues.corrupt_file_count(),
        issues.extra_file_count()
    );
    Ok(())
}
