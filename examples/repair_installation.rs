//! Finds and repairs damage in an installation, reporting progress as it goes.
//!
//! Installs a small generated folder, deletes one file, changes another and
//! adds a stray one, then verifies, plans the repair, repairs while printing
//! progress events, and verifies again.
//!
//! Run with `cargo run --example repair_installation -- WORK_DIR`.
//! WORK_DIR must not exist yet.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use gutch::api::{
    InstallationReconstructRequest, InstallationRepairRequest, InstallationVerifyRequest,
    OperationControl, ProgressEvent, ProgressSink, RepoInitRequest, Result, SnapshotCaptureRequest,
    capture_snapshot, init_repo, plan_repair_installation, reconstruct_installation,
    repair_installation, verify_installation,
};

/// Prints each progress event. Gutch calls `emit` from worker threads, so it must be quick.
#[derive(Debug)]
struct PrintProgress;

impl ProgressSink for PrintProgress {
    fn emit(&self, event: ProgressEvent) {
        println!("  [{:?}] {}", event.phase, event.message);
    }
}

fn main() -> ExitCode {
    let arguments: Vec<PathBuf> = env::args_os().skip(1).map(PathBuf::from).collect();
    let [work_dir] = arguments.as_slice() else {
        eprintln!("usage: repair_installation WORK_DIR");
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
    for index in 0..8 {
        let path = source_dir.join(format!("data/part{index}.txt"));
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, format!("part {index}\n").repeat(1000))?;
    }

    init_repo(&RepoInitRequest {
        repo_root: repo_root.clone(),
        ..Default::default()
    })?;
    let snapshot_id = capture_snapshot(&SnapshotCaptureRequest {
        repo_root: repo_root.clone(),
        source_dir,
        ..Default::default()
    })?
    .snapshot_id;
    reconstruct_installation(&InstallationReconstructRequest {
        repo_root: repo_root.clone(),
        snapshot_id: snapshot_id.clone(),
        destination_dir: app_dir.clone(),
        ..Default::default()
    })?;

    fs::remove_file(app_dir.join("data/part1.txt"))?;
    fs::write(app_dir.join("data/part2.txt"), "damaged\n")?;
    fs::write(app_dir.join("stray.log"), "not part of the snapshot\n")?;

    let verify = InstallationVerifyRequest {
        repo_root: repo_root.clone(),
        snapshot_id: snapshot_id.clone(),
        destination_dir: app_dir.clone(),
        ..Default::default()
    };
    let issues = verify_installation(&verify)?.issues;
    for issue in issues.missing_files.iter().chain(&issues.corrupt_files) {
        println!("found {:?}: {}", issue.kind, issue.path);
    }

    // The plan and the repair take the same request.
    let request = InstallationRepairRequest {
        repo_root,
        snapshot_id,
        destination_dir: app_dir,
        remove_extra_files: true,
        control: OperationControl::default().with_progress(Arc::new(PrintProgress)),
        ..Default::default()
    };
    let plan = plan_repair_installation(&request)?;
    println!(
        "repair plan: {} missing, {} corrupt, {} extra",
        plan.missing_file_count, plan.corrupt_file_count, plan.extra_file_count
    );
    println!("repairing:");
    let repaired = repair_installation(&request)?.repair_result;
    println!(
        "repaired {} files, removed {} extra",
        repaired.missing_file_count + repaired.corrupt_file_count,
        repaired.removed_extra_file_count
    );

    let issues = verify_installation(&verify)?.issues;
    println!(
        "after repair: {} missing, {} corrupt, {} extra",
        issues.missing_files.len(),
        issues.corrupt_files.len(),
        issues.extra_files.len()
    );
    Ok(())
}
