//! Captures a folder into a new repository, reconstructs it and verifies the result.
//!
//! Run with `cargo run --example capture_and_install -- SOURCE_DIR WORK_DIR`.
//! WORK_DIR must not exist yet; the example creates `repo/` and `install/` in it.

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gutch::api::{Repo, Result};

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
    let repo = Repo::init(work_dir.join("repo"))?;
    let snapshot_id = repo
        .capture(source_dir, &[("version", "1.0.0")])?
        .snapshot_id;
    println!("captured snapshot {snapshot_id}");

    let destination_dir = work_dir.join("install");
    repo.reconstruct(&snapshot_id, &destination_dir)?;
    let issues = repo.verify(&snapshot_id, &destination_dir)?.issues;
    println!("installed to {}: {issues}", destination_dir.display());
    Ok(())
}
