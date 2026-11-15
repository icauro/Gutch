//! Publish verified staged bundles and loose chunks.
use crate::Result;
use rayon::prelude::*;
use std::path::Path;

fn write_bundles(
    bundles_root: &Path,
    bundles: &[crate::capture::build::types::BuiltBundle],
    pool: &rayon::ThreadPool,
    control: &crate::progress::OperationControl,
) -> Result<()> {
    let _phase_span =
        control.profile_span_with_args("capture.publish_snapshot.write_bundle_artifacts", || {
            capture_count_bytes_trace_args(
                bundles.len(),
                bundles.iter().map(|bundle| bundle.size).sum(),
            )
        });
    let control_for_work = control.clone();
    pool.install(|| {
        bundles
            .par_iter()
            .map(|bundle| {
                control_for_work.checkpoint()?;
                let bundle_path =
                    crate::repo::paths::bundle_path_in_root(bundles_root, &bundle.bundle_id)?;
                let _bundle_span =
                    control_for_work.profile_span_with_args("capture.bundle.publish", || {
                        capture_artifact_write_trace_args(
                            "bundle",
                            &bundle.bundle_id,
                            &bundle_path,
                            bundle.size,
                        )
                    });
                if let Some(parent) = bundle_path.parent() {
                    crate::filesystem::create_dir_all(parent)?;
                }
                bundle.staged_file.promote_unless_present(&bundle_path)
            })
            .collect::<Result<Vec<_>>>()
    })?;
    Ok(())
}

fn write_loose_chunks(
    chunks_root: &Path,
    chunks: &[crate::capture::build::types::BuiltLooseChunk],
    pool: &rayon::ThreadPool,
    control: &crate::progress::OperationControl,
) -> Result<()> {
    let total_bytes = chunks
        .iter()
        .map(|chunk| chunk.compressed_size)
        .sum::<u64>();
    let _phase_span = control.profile_span_with_args(
        "capture.publish_snapshot.write_loose_chunk_artifacts",
        || capture_count_bytes_trace_args(chunks.len(), total_bytes),
    );
    let control_for_work = control.clone();
    pool.install(|| {
        chunks
            .par_iter()
            .map(|chunk| {
                control_for_work.checkpoint()?;
                let chunk_hash = chunk.chunk_hash.to_string();
                let chunk_path = crate::repo::paths::chunk_path_in_root(chunks_root, &chunk_hash)?;
                if let Some(parent) = chunk_path.parent() {
                    crate::filesystem::create_dir_all(parent)?;
                }
                chunk.staged_file.promote_unless_present(&chunk_path)
            })
            .collect::<Result<Vec<_>>>()
    })?;
    Ok(())
}

fn capture_count_bytes_trace_args(
    count: usize,
    bytes: u64,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    std::collections::BTreeMap::from([
        ("count".to_string(), serde_json::json!(count)),
        ("size_bytes".to_string(), serde_json::json!(bytes)),
    ])
}

fn capture_artifact_write_trace_args(
    kind: &str,
    artifact_id: &str,
    path: &Path,
    bytes: u64,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut args = crate::profiling::file_trace_args(path.display().to_string(), bytes);
    args.insert("artifact_kind".to_string(), serde_json::json!(kind));
    args.insert("artifact_id".to_string(), serde_json::json!(artifact_id));
    args
}

/// The caller holds the repository write lock. Publish immutable artifacts,
/// the additive index, then snapshot metadata as the commit marker.
/// Returns the manifest path and the distinct bundle count from the validated artifact set.
pub(super) fn publish_snapshot_artifacts(
    control: &crate::progress::OperationControl,
    repo_paths: &crate::repo::paths::RepoPaths,
    result: &crate::capture::build::types::BuildResult,
    reuse_state: &mut crate::capture::reuse::CaptureReuseState,
    build_pool: &rayon::ThreadPool,
) -> Result<(std::path::PathBuf, usize)> {
    // Snapshot publication below persists the captured snapshot. The snapshot
    // manifest is published last as the commit marker: immutable bundles and
    // loose chunks go first, then the additive chunk index, then the snapshot
    // directory. A failure before the manifest write leaves only unreferenced
    // immutable artifacts, never a visible snapshot that cannot be
    // reconstructed. Check once before the write set.
    control.checkpoint()?;
    let _artifact_write_span = control.profile_span("capture.publish_snapshot.write_artifacts");
    write_bundles(
        &repo_paths.bundles_root,
        &result.bundles,
        build_pool,
        control,
    )?;
    write_loose_chunks(
        &repo_paths.chunks_root,
        &result.loose_chunks,
        build_pool,
        control,
    )?;
    drop(_artifact_write_span);
    let _index_span = control.profile_span("capture.publish_snapshot.update_chunk_index");
    {
        let _bundle_index_span =
            control.profile_span("capture.publish_snapshot.index_bundle_entries");
        crate::capture::reuse::add_built_bundles_to_chunk_index(
            &mut reuse_state.chunk_index,
            &result.bundles,
        )?;
    }
    {
        let _chunk_index_span =
            control.profile_span("capture.publish_snapshot.index_loose_chunk_entries");
        crate::capture::reuse::add_built_loose_chunks_to_chunk_index(
            &mut reuse_state.chunk_index,
            &result.loose_chunks,
        )?;
    }
    drop(_index_span);

    let _state_span = control.profile_span("capture.publish_snapshot.update_capture_state");
    for bundle in &result.bundles {
        reuse_state.bundle_ids.insert(bundle.bundle_id.clone());
        for chunk in &bundle.entries {
            reuse_state
                .available_chunk_sizes
                .insert(chunk.chunk_hash, u64::from(chunk.compressed_size));
        }
    }
    for chunk in &result.loose_chunks {
        reuse_state
            .available_chunk_sizes
            .insert(chunk.chunk_hash, chunk.compressed_size);
    }
    for file in result.snapshot_manifest.files() {
        let candidates = reuse_state
            .previous_file_manifests
            .entry(file.path.clone())
            .or_default();
        if !candidates.iter().any(|candidate| {
            candidate.size == file.size && candidate.full_file_hash == file.full_file_hash
        }) {
            candidates.push(file.clone());
        }
    }
    let _chunk_index_write_span =
        control.profile_span("capture.publish_snapshot.write_chunk_index");
    crate::repo::index::write_chunk_index(repo_paths, &reuse_state.chunk_index)?;
    drop(_chunk_index_write_span);
    let mut required_bundle_count = 0;
    for artifact in crate::repo::artifacts::required_artifacts(
        &result.snapshot_manifest,
        &reuse_state.chunk_index,
    )? {
        if !artifact.local_path(repo_paths)?.is_file() {
            return Err(crate::Error::manifest(format!(
                "capture is missing required artifact '{}'",
                artifact.id()
            )));
        }
        if matches!(
            &artifact,
            crate::artifact::reference::ArtifactRef::Bundle { .. }
        ) {
            required_bundle_count += 1;
        }
    }
    let _manifest_write_span =
        control.profile_span("capture.publish_snapshot.write_snapshot_manifest");
    let (manifest_path, _) = crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts(
        repo_paths,
        &result.snapshot_manifest,
    )?;
    drop(_manifest_write_span);
    Ok((manifest_path, required_bundle_count))
}
