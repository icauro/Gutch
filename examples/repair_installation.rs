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

use gutch::api::{
    InstallationRepairRequest, OperationControl, Repo, Result, plan_repair_installation,
    repair_installation,
};

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
    let source_dir = work_dir.join("source");
    let app_dir = work_dir.join("app");
    for index in 0..8 {
        let path = source_dir.join(format!("data/part{index}.txt"));
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, format!("part {index}\n").repeat(1000))?;
    }
    let repo = Repo::init(work_dir.join("repo"))?;
    let snapshot_id = repo.capture(&source_dir, &[])?.snapshot_id;
    repo.reconstruct(&snapshot_id, &app_dir)?;

    fs::remove_file(app_dir.join("data/part1.txt"))?;
    fs::write(app_dir.join("data/part2.txt"), "damaged\n")?;
    fs::write(app_dir.join("stray.log"), "not part of the snapshot\n")?;

    let issues = repo.verify(&snapshot_id, &app_dir)?.issues;
    for issue in issues.missing_files.iter().chain(&issues.corrupt_files) {
        println!("found {:?}: {}", issue.kind, issue.path);
    }

    // `repo.repair` keeps extra files; the full request can remove them and
    // report progress. The plan and the repair take the same request.
    // Gutch calls the progress closure from worker threads, so keep it quick.
    let request = InstallationRepairRequest {
        repo_root: repo.root().to_path_buf(),
        snapshot_id: snapshot_id.clone(),
        destination_dir: app_dir.clone(),
        remove_extra_files: true,
        control: OperationControl::default()
            .on_progress(|event| println!("  [{:?}] {}", event.phase, event.message)),
        ..Default::default()
    };
    let plan = plan_repair_installation(&request)?;
    println!(
        "repair plan: {} missing, {} corrupt, {} extra",
        plan.missing_file_count, plan.corrupt_file_count, plan.extra_file_count
    );
    println!("repairing:");
    let repaired = repair_installation(&request)?;
    println!(
        "repaired {} files, removed {} extra",
        repaired.missing_file_count + repaired.corrupt_file_count,
        repaired.removed_extra_file_count
    );

    let issues = repo.verify(&snapshot_id, &app_dir)?.issues;
    println!(
        "after repair: {issues}; matches snapshot: {}",
        issues.matches_snapshot()
    );
    Ok(())
}
