//! Updates an installation from one version to the next.
//!
//! Captures versions 1.0 and 1.1 of a small generated folder, lists what
//! changed between them, installs 1.0, previews the update to 1.1, and then
//! updates the installation in place, reusing the content already on disk.
//!
//! Run with `cargo run --example update_installation -- WORK_DIR`.
//! WORK_DIR must not exist yet.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gutch::api::{
    InstallationPreviewEvent, InstallationPreviewRequest, InstallationPublication,
    InstallationReconstructRequest, RepoInitRequest, Result, SnapshotCaptureRequest,
    SnapshotDiffRequest, SnapshotTagSet, capture_snapshot, diff_snapshots, init_repo,
    preview_installation, reconstruct_installation,
};

fn main() -> ExitCode {
    let arguments: Vec<PathBuf> = env::args_os().skip(1).map(PathBuf::from).collect();
    let [work_dir] = arguments.as_slice() else {
        eprintln!("usage: update_installation WORK_DIR");
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
    let app_dir = work_dir.join("app");
    init_repo(&RepoInitRequest {
        repo_root: repo_root.clone(),
        ..Default::default()
    })?;

    // Version 1.0, then 1.1: one file changes, one is removed and one is added.
    let source_dir = work_dir.join("source");
    write_file(
        &source_dir.join("assets/textures.bin"),
        &sample_bytes(1, 2 << 20),
    )?;
    write_file(&source_dir.join("config.toml"), b"level = 1\n")?;
    write_file(&source_dir.join("notes.txt"), b"first release\n")?;
    let old_id = capture(&repo_root, &source_dir, "1.0")?;

    write_file(&source_dir.join("config.toml"), b"level = 2\n")?;
    fs::remove_file(source_dir.join("notes.txt"))?;
    write_file(
        &source_dir.join("assets/sounds.bin"),
        &sample_bytes(2, 512 << 10),
    )?;
    let new_id = capture(&repo_root, &source_dir, "1.1")?;

    let diff = diff_snapshots(&SnapshotDiffRequest {
        repo_root: repo_root.clone(),
        from_snapshot_id: old_id.clone(),
        to_snapshot_id: new_id.clone(),
        ..Default::default()
    })?;
    println!("changes from 1.0 to 1.1:");
    for change in &diff.changes {
        println!(
            "  {:?} {} ({:+} bytes)",
            change.change, change.path, change.delta_bytes
        );
    }

    reconstruct_installation(&InstallationReconstructRequest {
        repo_root: repo_root.clone(),
        snapshot_id: old_id.clone(),
        destination_dir: app_dir.clone(),
        ..Default::default()
    })?;
    println!("installed 1.0 to {}", app_dir.display());

    // The preview streams one event per entry; nothing on disk changes.
    // `ReplaceDirectory` previews what `reconstruct_installation` does.
    let mut actions = BTreeMap::new();
    let summary = preview_installation(
        &InstallationPreviewRequest {
            repo_root: repo_root.clone(),
            snapshot_id: new_id.clone(),
            destination_dir: app_dir.clone(),
            reuse_snapshot_id: Some(old_id.clone()),
            publication: InstallationPublication::ReplaceDirectory,
            ..Default::default()
        },
        &mut |event| {
            if let InstallationPreviewEvent::Entry { action, .. } = event {
                *actions.entry(format!("{action:?}")).or_insert(0) += 1;
            }
            Ok(())
        },
    )?;
    println!(
        "update preview: {actions:?}, {} bytes reusable from the installed 1.0",
        summary.reused_bytes
    );

    // Without `reuse_from`, content is reused from the destination itself.
    let updated = reconstruct_installation(&InstallationReconstructRequest {
        repo_root,
        snapshot_id: new_id,
        destination_dir: app_dir.clone(),
        reuse_snapshot_id: Some(old_id),
        ..Default::default()
    })?;
    println!(
        "updated {} to 1.1: {} of {} chunks reused from 1.0",
        app_dir.display(),
        updated.reused_chunk_count,
        updated.required_chunk_count
    );
    Ok(())
}

fn capture(repo_root: &Path, source_dir: &Path, version: &str) -> Result<String> {
    let captured = capture_snapshot(&SnapshotCaptureRequest {
        repo_root: repo_root.to_path_buf(),
        source_dir: source_dir.to_path_buf(),
        tags: SnapshotTagSet::from_pairs([("version", version)])?,
        ..Default::default()
    })?;
    println!("captured {version} as {}", captured.snapshot_id);
    Ok(captured.snapshot_id)
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)?;
    Ok(())
}

/// Deterministic incompressible bytes, so the example behaves like real content.
fn sample_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}
