//! Verify file metadata and full-file or chunk content hashes.
use super::types::{RepairIssue, RepairIssueKind, VerifyMode, VerifyTimings};
use super::{PreparedFileMetadata, PreparedVerifyFile};
use crate::Result;
use crate::artifact::hash::ChunkHash;
use crate::artifact::integrity::verify_full_file_integrity;
use crate::manifest;
use crate::manifest::typed::typed_file_manifest;
use crate::profiling::ProfileInstant;
use crate::profiling::file_trace_args;
use crate::progress::OperationControl;
use std::fs;
use std::fs::File;
use std::io;
use std::io::BufReader;
use std::io::Read;
use std::path::Path;

pub(super) fn verify_manifest_file(
    prepared_file: &PreparedVerifyFile<'_>,
    verify_mode: VerifyMode,
    control: &OperationControl,
    concurrent_file_count: usize,
    file_span: &'static str,
) -> Result<(Vec<RepairIssue>, VerifyTimings)> {
    let file_manifest = prepared_file.manifest;
    let _file_span = control.profile_span_with_args(file_span, || {
        file_trace_args(&file_manifest.path, file_manifest.size)
    });
    control.checkpoint()?;
    let mut timings = VerifyTimings {
        verify_mode,
        ..VerifyTimings::default()
    };
    let metadata = match &prepared_file.metadata {
        PreparedFileMetadata::Inventory(Some(metadata)) => metadata.clone(),
        PreparedFileMetadata::Inventory(None) => {
            return Ok((
                vec![RepairIssue {
                    kind: RepairIssueKind::Missing,
                    path: file_manifest.path.clone(),
                    detail: format!("missing file: {}", file_manifest.path),
                }],
                timings,
            ));
        }
        PreparedFileMetadata::ResolveDuringVerification => {
            let metadata_read_started = ProfileInstant::detailed(control.profiler());
            let metadata = match fs::symlink_metadata(&prepared_file.path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Ok((
                        vec![RepairIssue {
                            kind: RepairIssueKind::Missing,
                            path: file_manifest.path.clone(),
                            detail: format!("missing file: {}", file_manifest.path),
                        }],
                        timings,
                    ));
                }
                Err(error) => return Err(error.into()),
            };
            timings.stat += metadata_read_started.elapsed();
            metadata
        }
    };
    if !metadata.file_type().is_file() {
        return Ok((
            vec![RepairIssue {
                kind: RepairIssueKind::Corrupt,
                path: file_manifest.path.clone(),
                detail: format!("entry type mismatch for file: {}", file_manifest.path),
            }],
            timings,
        ));
    }

    if metadata.permissions().readonly() != file_manifest.read_only {
        return Ok((
            vec![RepairIssue {
                kind: RepairIssueKind::Corrupt,
                path: file_manifest.path.clone(),
                detail: format!("read-only mismatch for file: {}", file_manifest.path),
            }],
            timings,
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if (metadata.permissions().mode() & 0o111 != 0) != file_manifest.executable {
            return Ok((
                vec![RepairIssue {
                    kind: RepairIssueKind::Corrupt,
                    path: file_manifest.path.clone(),
                    detail: format!("executable mismatch for file: {}", file_manifest.path),
                }],
                timings,
            ));
        }
    }

    if metadata.len() != file_manifest.size {
        return Ok((
            vec![RepairIssue {
                kind: RepairIssueKind::Corrupt,
                path: file_manifest.path.clone(),
                detail: format!(
                    "size mismatch for {}: expected {}, got {}",
                    file_manifest.path,
                    file_manifest.size,
                    metadata.len()
                ),
            }],
            timings,
        ));
    }

    if verify_mode == VerifyMode::MetadataOnly {
        return Ok((Vec::new(), timings));
    }
    let verification = if verify_mode == VerifyMode::Fast {
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
    match verification {
        Ok(issues) => Ok((issues, timings)),
        Err(crate::Error::Cancelled) => Err(crate::Error::Cancelled),
        Err(crate::Error::HashMismatch { expected, actual }) => Ok((
            vec![RepairIssue {
                kind: RepairIssueKind::Corrupt,
                path: file_manifest.path.clone(),
                detail: format!(
                    "hash mismatch for {}: expected {}, got {}",
                    file_manifest.path, expected, actual
                ),
            }],
            timings,
        )),
        Err(e) => Ok((
            vec![RepairIssue {
                kind: RepairIssueKind::Corrupt,
                path: file_manifest.path.clone(),
                detail: format!("cannot read {}: {}", file_manifest.path, e),
            }],
            timings,
        )),
    }
}

fn verify_file_fast(
    prepared_file: &PreparedVerifyFile<'_>,
    concurrent_file_count: usize,
    timings: &mut VerifyTimings,
    control: &OperationControl,
) -> Result<Vec<RepairIssue>> {
    let expected_full_hash = ChunkHash::parse_hex(&prepared_file.manifest.full_file_hash)?;
    let integrity = verify_full_file_integrity(
        &prepared_file.path,
        prepared_file.manifest.size,
        expected_full_hash,
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
    verify_mode: VerifyMode,
    timings: &mut VerifyTimings,
    control: &OperationControl,
) -> Result<Vec<RepairIssue>> {
    let typed_file =
        typed_file_manifest(file_manifest).map_err(|err| io::Error::other(err.to_string()))?;
    let file = File::open(file_path)?;
    let mut reader = BufReader::new(file);
    let mut issues = Vec::new();
    let mut full_file_hasher = blake3::Hasher::new();
    let mut chunk_buffer = Vec::new();
    let mut chunk_offset = 0u64;

    for chunk_ref in typed_file.chunks {
        let raw_size = u64::from(chunk_ref.raw_size);
        control.checkpoint()?;
        let chunk_size = chunk_ref.raw_size as usize;
        let start = chunk_offset;
        let end = start + raw_size;
        chunk_offset = end;

        if end > file_manifest.size {
            issues.push(RepairIssue {
                kind: RepairIssueKind::Corrupt,
                path: file_manifest.path.clone(),
                detail: format!(
                    "chunk reference out of bounds in {}: offset {}, size {}",
                    file_manifest.path, start, chunk_ref.raw_size
                ),
            });
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
                issues.push(RepairIssue {
                    kind: RepairIssueKind::Corrupt,
                    path: file_manifest.path.clone(),
                    detail: format!(
                        "chunk hash mismatch in {} at offset {}: expected {}, got {}",
                        file_manifest.path,
                        start,
                        chunk_ref.chunk_hash,
                        chunk_hash_actual.to_hex()
                    ),
                });
            }
        }
    }

    if chunk_offset != file_manifest.size {
        issues.push(RepairIssue {
            kind: RepairIssueKind::Corrupt,
            path: file_manifest.path.clone(),
            detail: format!(
                "chunk coverage mismatch for {}: expected {} bytes, got {}",
                file_manifest.path, file_manifest.size, chunk_offset
            ),
        });
    }

    let file_hash_finalize_started = ProfileInstant::detailed(control.profiler());
    let actual_hash = ChunkHash::from_blake3(full_file_hasher.finalize());
    timings.file_hash += file_hash_finalize_started.elapsed();
    if actual_hash != typed_file.full_file_hash {
        issues.push(RepairIssue {
            kind: RepairIssueKind::Corrupt,
            path: file_manifest.path.clone(),
            detail: format!(
                "hash mismatch for {}: expected {}, got {}",
                file_manifest.path,
                file_manifest.full_file_hash,
                actual_hash.to_hex()
            ),
        });
    }

    Ok(issues)
}
