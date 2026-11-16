//! Verify file metadata and full-file or chunk content hashes.
use super::classify::{PreparedFileMetadata, PreparedVerifyFile};
use super::types::{InstallationIssue, InstallationVerifyMode, VerifyTimings};
use crate::Result;
use crate::artifact::hash::ChunkHash;
use crate::artifact::integrity::verify_full_file_integrity;
use crate::filesystem;
use crate::manifest;
use crate::profiling::ProfileInstant;
use crate::profiling::file_trace_args;
use crate::progress::OperationControl;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::path::Path;

pub(super) fn verify_manifest_file(
    prepared_file: &PreparedVerifyFile<'_>,
    verify_mode: InstallationVerifyMode,
    control: &OperationControl,
    concurrent_file_count: usize,
    file_span: &'static str,
) -> Result<(Vec<InstallationIssue>, VerifyTimings)> {
    let file_manifest = prepared_file.manifest;
    let _file_span = control.profile_span_with_args(file_span, || {
        file_trace_args(&file_manifest.path, file_manifest.size)
    });
    control.checkpoint()?;
    let mut timings = VerifyTimings {
        verify_mode,
        ..VerifyTimings::default()
    };
    if let Some(issue) = check_file_metadata(prepared_file, control, &mut timings)? {
        return Ok((vec![issue], timings));
    }
    if verify_mode == InstallationVerifyMode::MetadataOnly {
        return Ok((Vec::new(), timings));
    }
    let verification = if verify_mode == InstallationVerifyMode::Fast {
        verify_file_fast(prepared_file, concurrent_file_count, &mut timings, control)
    } else {
        verify_file_streaming(
            file_manifest,
            &prepared_file.path,
            verify_mode,
            &mut timings,
            control,
        )
    };
    let path = &file_manifest.path;
    let issues = match verification {
        Ok(issues) => issues,
        Err(crate::Error::Cancelled) => return Err(crate::Error::Cancelled),
        Err(crate::Error::HashMismatch { expected, actual }) => vec![InstallationIssue::corrupt(
            path,
            format!("hash mismatch for {path}: expected {expected}, got {actual}"),
        )],
        Err(error) => vec![InstallationIssue::corrupt(
            path,
            format!("cannot read {path}: {error}"),
        )],
    };
    Ok((issues, timings))
}

/// Compares the file's type, permissions and size with the manifest and
/// returns the first mismatch.
fn check_file_metadata(
    prepared_file: &PreparedVerifyFile<'_>,
    control: &OperationControl,
    timings: &mut VerifyTimings,
) -> Result<Option<InstallationIssue>> {
    let file_manifest = prepared_file.manifest;
    let path = &file_manifest.path;
    let missing = || {
        Ok(Some(InstallationIssue::missing(
            path,
            format!("missing file: {path}"),
        )))
    };
    let metadata = match &prepared_file.metadata {
        PreparedFileMetadata::Inventory(Some(metadata)) => metadata.clone(),
        PreparedFileMetadata::Inventory(None) => return missing(),
        PreparedFileMetadata::ResolveDuringVerification => {
            let metadata_read_started = ProfileInstant::detailed(control.profiler());
            let Some(metadata) = filesystem::symlink_metadata_if_exists(&prepared_file.path)?
            else {
                return missing();
            };
            timings.stat += metadata_read_started.elapsed();
            metadata
        }
    };

    let corrupt = |detail: String| Ok(Some(InstallationIssue::corrupt(path, detail)));
    if !metadata.file_type().is_file() {
        return corrupt(format!("entry type mismatch for file: {path}"));
    }
    if metadata.permissions().readonly() != file_manifest.read_only {
        return corrupt(format!("read-only mismatch for file: {path}"));
    }
    if cfg!(unix) && filesystem::is_executable(&metadata) != file_manifest.executable {
        return corrupt(format!("executable mismatch for file: {path}"));
    }
    if metadata.len() != file_manifest.size {
        return corrupt(format!(
            "size mismatch for {path}: expected {}, got {}",
            file_manifest.size,
            metadata.len()
        ));
    }
    Ok(None)
}

fn verify_file_fast(
    prepared_file: &PreparedVerifyFile<'_>,
    concurrent_file_count: usize,
    timings: &mut VerifyTimings,
    control: &OperationControl,
) -> Result<Vec<InstallationIssue>> {
    let integrity = verify_full_file_integrity(
        &prepared_file.path,
        prepared_file.manifest.size,
        prepared_file.manifest.full_file_hash,
        concurrent_file_count,
        control,
    )?;
    timings.file_read += integrity.read_elapsed;
    timings.file_read_bytes += integrity.bytes;
    timings.file_hash += integrity.hash_elapsed;
    timings.file_hash_bytes += integrity.bytes;
    Ok(Vec::new())
}

fn verify_file_streaming(
    file_manifest: &manifest::FileManifest,
    file_path: &Path,
    verify_mode: InstallationVerifyMode,
    timings: &mut VerifyTimings,
    control: &OperationControl,
) -> Result<Vec<InstallationIssue>> {
    let file = File::open(file_path)?;
    let mut reader = BufReader::new(file);
    let mut issues = Vec::new();
    let mut full_file_hasher = blake3::Hasher::new();
    let mut chunk_buffer = Vec::new();
    let mut chunk_offset = 0u64;

    for chunk_ref in &file_manifest.chunks {
        let raw_size = u64::from(chunk_ref.raw_size);
        control.checkpoint()?;
        let chunk_size = chunk_ref.raw_size as usize;
        let start = chunk_offset;
        let end = start + raw_size;
        chunk_offset = end;

        if end > file_manifest.size {
            issues.push(InstallationIssue::corrupt(
                &file_manifest.path,
                format!(
                    "chunk reference out of bounds in {}: offset {start}, size {raw_size}",
                    file_manifest.path
                ),
            ));
            break;
        }

        chunk_buffer.resize(chunk_size, 0);

        let chunk_read_started = ProfileInstant::detailed(control.profiler());
        reader.read_exact(&mut chunk_buffer)?;
        timings.file_read += chunk_read_started.elapsed();
        timings.file_read_bytes += raw_size;

        let file_hash_update_started = ProfileInstant::detailed(control.profiler());
        full_file_hasher.update(&chunk_buffer);
        timings.file_hash += file_hash_update_started.elapsed();
        timings.file_hash_bytes += raw_size;

        if verify_mode.checks_chunk_hashes() {
            let chunk_hash_started = ProfileInstant::detailed(control.profiler());
            let chunk_hash_actual = ChunkHash::of(&chunk_buffer);
            timings.chunk_hash += chunk_hash_started.elapsed();
            timings.chunk_hash_bytes += raw_size;
            if chunk_hash_actual != chunk_ref.chunk_hash {
                issues.push(InstallationIssue::corrupt(
                    &file_manifest.path,
                    format!(
                        "chunk hash mismatch in {} at offset {start}: expected {}, got {}",
                        file_manifest.path,
                        chunk_ref.chunk_hash,
                        chunk_hash_actual.to_hex()
                    ),
                ));
            }
        }
    }

    if chunk_offset != file_manifest.size {
        issues.push(InstallationIssue::corrupt(
            &file_manifest.path,
            format!(
                "chunk coverage mismatch for {}: expected {} bytes, got {chunk_offset}",
                file_manifest.path, file_manifest.size
            ),
        ));
    }

    let file_hash_finalize_started = ProfileInstant::detailed(control.profiler());
    let actual_hash = ChunkHash::from_blake3(full_file_hasher.finalize());
    timings.file_hash += file_hash_finalize_started.elapsed();
    if actual_hash != file_manifest.full_file_hash {
        issues.push(InstallationIssue::corrupt(
            &file_manifest.path,
            format!(
                "hash mismatch for {}: expected {}, got {}",
                file_manifest.path, file_manifest.full_file_hash, actual_hash
            ),
        ));
    }

    Ok(issues)
}
