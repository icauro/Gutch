//! Cancels a reconstruction partway and shows the old installation is untouched.
//!
//! Installs version 1.0 of a generated folder, then starts installing 1.1
//! over it and cancels as soon as files start being written. The cancelled
//! operation returns an error for which `is_cancelled()` is true, and
//! verification shows 1.0 is still intact.
//!
//! A real application would set the flag from a Cancel button or Ctrl-C.
//!
//! Run with `cargo run --example cancel_reconstruction -- WORK_DIR`.
//! WORK_DIR must not exist yet.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gutch::api::{
    CancellationHook, InstallationReconstructRequest, InstallationVerifyRequest, OperationControl,
    ProgressEvent, ProgressPhase, ProgressSink, RepoInitRequest, Result, SnapshotCaptureRequest,
    capture_snapshot, init_repo, reconstruct_installation, verify_installation,
};

/// Gutch polls `is_cancelled` at safe points and then stops with `Error::Cancelled`.
#[derive(Debug, Default)]
struct CancelFlag(AtomicBool);

impl CancellationHook for CancelFlag {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Stands in for the user: requests cancellation once writing has started.
#[derive(Debug)]
struct CancelWhenWriting(Arc<CancelFlag>);

impl ProgressSink for CancelWhenWriting {
    fn emit(&self, event: ProgressEvent) {
        if event.phase == ProgressPhase::Reconstruct && !self.0.is_cancelled() {
            println!("cancelling at: {}", event.message);
            self.0.0.store(true, Ordering::Relaxed);
        }
    }
}

fn main() -> ExitCode {
    let arguments: Vec<PathBuf> = env::args_os().skip(1).map(PathBuf::from).collect();
    let [work_dir] = arguments.as_slice() else {
        eprintln!("usage: cancel_reconstruction WORK_DIR");
        return ExitCode::from(2);
    };
    if work_dir.exists() {
        eprintln!(
            "error: {} already exists; pick a new WORK_DIR",
            work_dir.display()
        );
        return ExitCode::from(2);
    }
    match run(work_dir) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(work_dir: &Path) -> Result<()> {
    let repo_root = work_dir.join("repo");
    let source_dir = work_dir.join("source");
    let app_dir = work_dir.join("app");
    init_repo(&RepoInitRequest {
        repo_root: repo_root.clone(),
        ..Default::default()
    })?;

    let old_id = capture_version(&repo_root, &source_dir, "1.0")?;
    let new_id = capture_version(&repo_root, &source_dir, "1.1")?;
    reconstruct_installation(&InstallationReconstructRequest {
        repo_root: repo_root.clone(),
        snapshot_id: old_id.clone(),
        destination_dir: app_dir.clone(),
        ..Default::default()
    })?;
    println!("installed 1.0");

    let flag = Arc::new(CancelFlag::default());
    let control = OperationControl::default()
        .with_cancellation(flag.clone())
        .with_progress(Arc::new(CancelWhenWriting(flag)));
    let attempt = reconstruct_installation(&InstallationReconstructRequest {
        repo_root: repo_root.clone(),
        snapshot_id: new_id,
        destination_dir: app_dir.clone(),
        control,
        ..Default::default()
    });
    match attempt {
        Err(error) if error.is_cancelled() => println!("update to 1.1 cancelled"),
        Err(error) => return Err(error),
        Ok(_) => println!("update to 1.1 finished before the cancellation was seen"),
    }

    let issues = verify_installation(&InstallationVerifyRequest {
        repo_root,
        snapshot_id: old_id,
        destination_dir: app_dir,
        ..Default::default()
    })?
    .issues;
    println!(
        "1.0 after the cancelled update: {} missing, {} corrupt, {} extra",
        issues.missing_file_count(),
        issues.corrupt_file_count(),
        issues.extra_file_count()
    );
    Ok(())
}

/// Rewrites the source folder with 64 files for `version` and captures it.
fn capture_version(repo_root: &Path, source_dir: &Path, version: &str) -> Result<String> {
    fs::create_dir_all(source_dir)?;
    for index in 0..64 {
        let line = format!("version {version}, file {index}\n");
        fs::write(
            source_dir.join(format!("file{index:02}.txt")),
            line.repeat(4000),
        )?;
    }
    let snapshot_id = capture_snapshot(&SnapshotCaptureRequest {
        repo_root: repo_root.to_path_buf(),
        source_dir: source_dir.to_path_buf(),
        ..Default::default()
    })?
    .snapshot_id;
    println!("captured {version} as {snapshot_id}");
    Ok(snapshot_id)
}
