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

use gutch::api::{
    CancelFlag, InstallationReconstructRequest, OperationControl, ProgressPhase, Repo, Result,
    reconstruct_installation,
};

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
    let repo = Repo::init(work_dir.join("repo"))?;
    let source_dir = work_dir.join("source");
    let app_dir = work_dir.join("app");
    let old_id = capture_version(&repo, &source_dir, "1.0")?;
    let new_id = capture_version(&repo, &source_dir, "1.1")?;
    repo.reconstruct(&old_id, &app_dir)?;
    println!("installed 1.0");

    // Stands in for the user: cancel once writing has started. Gutch checks
    // the flag at safe points and then stops with an error.
    let flag = CancelFlag::default();
    let cancel = flag.clone();
    let control = OperationControl::default()
        .with_cancel_flag(&flag)
        .on_progress(move |event| {
            if event.phase == ProgressPhase::Reconstruct && !cancel.is_cancelled() {
                println!("cancelling at: {}", event.message);
                cancel.cancel();
            }
        });
    let attempt = reconstruct_installation(&InstallationReconstructRequest {
        repo_root: repo.root().to_path_buf(),
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

    let issues = repo.verify(&old_id, &app_dir)?.issues;
    println!("1.0 after the cancelled update: {issues}");
    Ok(())
}

/// Rewrites the source folder with 64 files for `version` and captures it.
fn capture_version(repo: &Repo, source_dir: &Path, version: &str) -> Result<String> {
    fs::create_dir_all(source_dir)?;
    for index in 0..64 {
        let line = format!(
            "version {version}, file {index}
"
        );
        fs::write(
            source_dir.join(format!("file{index:02}.txt")),
            line.repeat(4000),
        )?;
    }
    let snapshot_id = repo.capture(source_dir, &[])?.snapshot_id;
    println!("captured {version} as {snapshot_id}");
    Ok(snapshot_id)
}
