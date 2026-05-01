pub mod bundle;
pub mod chunk;
pub mod compress;
pub mod error;
pub mod hash;
pub mod manifest;
#[cfg(test)]
mod test_util;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use bundle::BundleBuilder;
use manifest::{ChunkRef, FileManifest, VersionManifest};

pub use error::{Error, Result};

use crate::compress::compress_chunk;
use crate::hash::hash_hex;

fn file_mode(metadata: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return metadata.permissions().mode();
    }
    #[cfg(not(unix))]
    {
        if metadata.permissions().readonly() {
            0o444
        } else {
            0o644
        }
    }
}

pub struct BuildResult {
    pub version_manifest: VersionManifest,
    pub file_manifests: Vec<FileManifest>,
    pub bundle_data: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Default)]
pub struct BuildTimings {
    pub scan_read: Duration,
    pub scan_read_bytes: u64,
    pub file_hash: Duration,
    pub file_hash_bytes: u64,
    pub chunking: Duration,
    pub chunking_bytes: u64,
    pub metadata: Duration,
    pub dedupe_compress: Duration,
    pub dedupe_input_bytes: u64,
    pub dedupe_output_bytes: u64,
    pub bundle_build: Duration,
    pub bundle_build_bytes: u64,
    pub manifest_finalize: Duration,
    pub total: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct ReconstructTimings {
    pub bundle_read: Duration,
    pub bundle_read_compressed_bytes: u64,
    pub bundle_read_decompressed_bytes: u64,
    pub file_rebuild: Duration,
    pub file_rebuild_bytes: u64,
    pub file_verify: Duration,
    pub file_verify_bytes: u64,
    pub file_write: Duration,
    pub file_write_bytes: u64,
    pub total: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct VerifyTimings {
    pub stat: Duration,
    pub file_read: Duration,
    pub file_read_bytes: u64,
    pub file_hash: Duration,
    pub file_hash_bytes: u64,
    pub chunk_hash: Duration,
    pub chunk_hash_bytes: u64,
    pub total: Duration,
}

pub fn build_from_directory(
    dir: &Path,
    product_id: &str,
    version_id: &str,
    platform: &str,
    architecture: &str,
    tier_id: &str,
) -> Result<BuildResult> {
    let (result, _) = build_from_directory_with_timings(
        dir,
        product_id,
        version_id,
        platform,
        architecture,
        tier_id,
    )?;
    Ok(result)
}

pub fn build_from_directory_with_timings(
    dir: &Path,
    product_id: &str,
    version_id: &str,
    platform: &str,
    architecture: &str,
    tier_id: &str,
) -> Result<(BuildResult, BuildTimings)> {
    let total_start = Instant::now();
    let chunking_profile = manifest::ChunkingProfile::default();
    let compression_profile = manifest::CompressionProfile::default();
    let mut timings = BuildTimings::default();

    let mut file_manifests: Vec<FileManifest> = Vec::new();
    let mut all_chunks: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total_logical_size: u64 = 0;

    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        let relative = path.strip_prefix(dir).map_err(|e| {
            Error::PathError { path: path.to_path_buf(), reason: e.to_string() }
        })?;

        let t = Instant::now();
        let file_data = std::fs::read(path)?;
        timings.scan_read += t.elapsed();
        timings.scan_read_bytes += file_data.len() as u64;
        let file_size = file_data.len() as u64;
        total_logical_size += file_size;

        let t = Instant::now();
        let full_file_hash = hash_hex(&hash::hash_chunk(&file_data));
        timings.file_hash += t.elapsed();
        timings.file_hash_bytes += file_data.len() as u64;

        let t = Instant::now();
        let chunk_results = chunk::chunk_bytes(&file_data, &chunking_profile);
        timings.chunking += t.elapsed();
        timings.chunking_bytes += file_data.len() as u64;
        let mut chunk_refs: Vec<ChunkRef> = Vec::new();

        for cr in &chunk_results {
            let ch = hash_hex(&cr.hash);
            chunk_refs.push(ChunkRef {
                chunk_hash: ch.clone(),
                raw_size: cr.data.len() as u64,
                file_offset: cr.offset,
            });
            all_chunks.push((ch, cr.data.clone()));
        }

        let t = Instant::now();
        let metadata = std::fs::metadata(path)?;
        let mode = file_mode(&metadata);
        timings.metadata += t.elapsed();

        file_manifests.push(FileManifest {
            path: relative.to_string_lossy().replace('\\', "/"),
            size: file_size,
            mode,
            executable: false,
            symlink_target: None,
            full_file_hash,
            chunks: chunk_refs,
        });
    }

    let mut builder = BundleBuilder::new(compression_profile);
    let mut total_compressed: u64 = 0;
    let mut all_chunk_hashes: HashSet<String> = HashSet::new();

    let t = Instant::now();
    for (hash, data) in &all_chunks {
        if !all_chunk_hashes.contains(hash.as_str()) {
            let compressed = compress_chunk(data, &builder.profile)?;
            timings.dedupe_input_bytes += data.len() as u64;
            timings.dedupe_output_bytes += compressed.len() as u64;
            if builder.estimated_compressed_size() + compressed.len() as u64 > 64 * 1024 * 1024 {
                if !builder.is_empty() {
                    break;
                }
            }
            builder.add_chunk(hash.clone(), data.clone());
            total_compressed += compressed.len() as u64;
            all_chunk_hashes.insert(hash.clone());
        }
    }
    timings.dedupe_compress += t.elapsed();

    let t = Instant::now();
    let bundles = if builder.is_empty() {
        vec![]
    } else {
        vec![builder.build()?]
    };
    timings.bundle_build += t.elapsed();
    timings.bundle_build_bytes = bundles.iter().map(|b| b.len() as u64).sum();

    let t = Instant::now();
    let mut vm = VersionManifest::new(
        product_id.to_string(),
        version_id.to_string(),
        platform.to_string(),
        architecture.to_string(),
        tier_id.to_string(),
    );
    vm.total_logical_size = total_logical_size;
    vm.total_compressed_size_estimate = total_compressed;
    vm.required_chunk_set = all_chunk_hashes.into_iter().collect();
    vm.required_chunk_set.sort();
    timings.manifest_finalize += t.elapsed();
    timings.total = total_start.elapsed();

    Ok((BuildResult {
        version_manifest: vm,
        file_manifests,
        bundle_data: bundles,
    }, timings))
}

pub fn reconstruct_from_manifest(
    version_manifest: &VersionManifest,
    file_manifests: &[FileManifest],
    bundles: &[Vec<u8>],
    output_dir: &Path,
) -> Result<()> {
    let (_, _) = reconstruct_from_manifest_with_timings(version_manifest, file_manifests, bundles, output_dir)?;
    Ok(())
}

pub fn reconstruct_from_manifest_with_timings(
    version_manifest: &VersionManifest,
    file_manifests: &[FileManifest],
    bundles: &[Vec<u8>],
    output_dir: &Path,
) -> Result<((), ReconstructTimings)> {
    let total_start = Instant::now();
    let mut timings = ReconstructTimings::default();
    std::fs::create_dir_all(output_dir)?;

    let mut chunk_map: HashMap<String, Vec<u8>> = HashMap::new();

    for bundle_data in bundles {
        let t = Instant::now();
        let mut reader = bundle::read_bundle(bundle_data)?;
        timings.bundle_read_compressed_bytes += bundle_data.len() as u64;
        let hashes: Vec<String> = reader.entries().iter().map(|e| e.chunk_hash.clone()).collect();
        for hash in &hashes {
            let data = reader.read_chunk_by_hash(hash)?;
            timings.bundle_read_decompressed_bytes += data.len() as u64;
            chunk_map.insert(hash.clone(), data);
        }
        timings.bundle_read += t.elapsed();
    }

    for fm in file_manifests {
        let t = Instant::now();
        let mut file_data = Vec::with_capacity(fm.size as usize);

        for chunk_ref in &fm.chunks {
            let chunk_data = chunk_map
                .get(&chunk_ref.chunk_hash)
                .ok_or_else(|| Error::MissingChunk(chunk_ref.chunk_hash.clone()))?;

            let target_offset = chunk_ref.file_offset as usize;
            if target_offset > file_data.len() {
                file_data.resize(target_offset, 0);
            }

            file_data.extend_from_slice(chunk_data);
        }
        timings.file_rebuild += t.elapsed();
        timings.file_rebuild_bytes += file_data.len() as u64;

        if file_data.len() != fm.size as usize {
            return Err(Error::Reconstruct(format!(
                "reconstructed size {} does not match expected {} for {}",
                file_data.len(),
                fm.size,
                fm.path
            )));
        }

        let t = Instant::now();
        let actual_hash = hash_hex(&hash::hash_chunk(&file_data));
        timings.file_verify += t.elapsed();
        timings.file_verify_bytes += file_data.len() as u64;
        if actual_hash != fm.full_file_hash {
            return Err(Error::HashMismatch {
                expected: fm.full_file_hash.clone(),
                actual: actual_hash,
            });
        }

        let output_path = output_dir.join(&fm.path);
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let t = Instant::now();
        std::fs::write(&output_path, &file_data)?;
        timings.file_write += t.elapsed();
        timings.file_write_bytes += file_data.len() as u64;
    }

    let _ = version_manifest;
    timings.total = total_start.elapsed();
    Ok(((), timings))
}

pub fn verify_directory(
    version_manifest: &VersionManifest,
    file_manifests: &[FileManifest],
    dir: &Path,
) -> Result<Vec<String>> {
    let (errors, _) = verify_directory_with_timings(version_manifest, file_manifests, dir)?;
    Ok(errors)
}

pub fn verify_directory_with_timings(
    version_manifest: &VersionManifest,
    file_manifests: &[FileManifest],
    dir: &Path,
) -> Result<(Vec<String>, VerifyTimings)> {
    let total_start = Instant::now();
    let mut errors = Vec::new();
    let mut timings = VerifyTimings::default();

    for fm in file_manifests {
        let file_path = dir.join(&fm.path);

        if !file_path.exists() {
            errors.push(format!("missing file: {}", fm.path));
            continue;
        }

        let t = Instant::now();
        let metadata = match std::fs::metadata(&file_path) {
            Ok(m) => m,
            Err(e) => {
                errors.push(format!("cannot stat {}: {}", fm.path, e));
                continue;
            }
        };
        timings.stat += t.elapsed();

        if metadata.len() != fm.size {
            errors.push(format!(
                "size mismatch for {}: expected {}, got {}",
                fm.path,
                fm.size,
                metadata.len()
            ));
            continue;
        }

        let t = Instant::now();
        let file_data = match std::fs::read(&file_path) {
            Ok(d) => d,
            Err(e) => {
                errors.push(format!("cannot read {}: {}", fm.path, e));
                continue;
            }
        };
        timings.file_read += t.elapsed();
        timings.file_read_bytes += file_data.len() as u64;

        let t = Instant::now();
        let actual_hash = hash_hex(&hash::hash_chunk(&file_data));
        timings.file_hash += t.elapsed();
        timings.file_hash_bytes += file_data.len() as u64;
        if actual_hash != fm.full_file_hash {
            errors.push(format!(
                "hash mismatch for {}: expected {}, got {}",
                fm.path,
                fm.full_file_hash,
                actual_hash
            ));
        }

        for chunk_ref in &fm.chunks {
            let start = chunk_ref.file_offset as usize;
            let end = start + chunk_ref.raw_size as usize;
            if end > file_data.len() {
                errors.push(format!(
                    "chunk reference out of bounds in {}: offset {}, size {}",
                    fm.path,
                    chunk_ref.file_offset,
                    chunk_ref.raw_size
                ));
                continue;
            }

            let t = Instant::now();
            let chunk_bytes = &file_data[start..end];
            let actual_chunk_hash = hash_hex(&hash::hash_chunk(chunk_bytes));
            timings.chunk_hash += t.elapsed();
            timings.chunk_hash_bytes += chunk_bytes.len() as u64;
            if actual_chunk_hash != chunk_ref.chunk_hash {
                errors.push(format!(
                    "chunk hash mismatch in {} at offset {}: expected {}, got {}",
                    fm.path,
                    chunk_ref.file_offset,
                    chunk_ref.chunk_hash,
                    actual_chunk_hash
                ));
            }
        }
    }

    let _ = version_manifest;
    timings.total = total_start.elapsed();
    Ok((errors, timings))
}

pub fn estimate_build(
    version_manifest: &VersionManifest,
    file_manifests: &[FileManifest],
    available_chunks: &HashSet<String>,
    available_disk_space: u64,
) -> Result<manifest::Estimate> {
    let mut missing_chunks: HashSet<String> = HashSet::new();
    let mut missing_logical: u64 = 0;

    for fm in file_manifests {
        for cr in &fm.chunks {
            if !available_chunks.contains(&cr.chunk_hash) {
                missing_chunks.insert(cr.chunk_hash.clone());
                missing_logical += cr.raw_size;
            }
        }
    }

    let estimate_ratio = 0.5;
    let missing_compressed = (missing_logical as f64 * estimate_ratio) as u64;

    let bundle_estimate = if missing_chunks.is_empty() {
        0
    } else {
        1 + (missing_chunks.len() as u64 / 2000)
    };

    let _ = version_manifest;

    Ok(manifest::Estimate {
        missing_logical_bytes: missing_logical,
        missing_compressed_bytes: missing_compressed,
        missing_chunk_count: missing_chunks.len() as u64,
        required_bundle_count: bundle_estimate,
        range_request_count: missing_chunks.len() as u64,
        local_cache_hits: 0,
        available_disk_space,
        fragmentation_score: bundle_estimate as f64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util;
    use std::time::Instant;

    #[test]
    fn test_build_and_reconstruct_roundtrip() {
        let dir = std::env::temp_dir().join("gutch_test_roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let sub = dir.join("subdir");
        std::fs::create_dir_all(&sub).unwrap();

        std::fs::write(dir.join("file1.txt"), b"hello world, this is file one ".repeat(50)).unwrap();
        std::fs::write(dir.join("file2.txt"), b"goodbye world, this is file two ".repeat(80)).unwrap();
        std::fs::write(sub.join("nested.txt"), b"nested content here ".repeat(30)).unwrap();

        let result = build_from_directory(
            &dir, "test-product", "1.0.0", "windows", "x86_64", "base",
        )
        .unwrap();

        assert!(!result.file_manifests.is_empty());
        assert!(!result.version_manifest.required_chunk_set.is_empty());

        let out_dir = std::env::temp_dir().join("gutch_test_output");
        let _ = std::fs::remove_dir_all(&out_dir);

        reconstruct_from_manifest(
            &result.version_manifest,
            &result.file_manifests,
            &result.bundle_data,
            &out_dir,
        )
        .unwrap();

        let verify_errors = verify_directory(
            &result.version_manifest,
            &result.file_manifests,
            &out_dir,
        )
        .unwrap();

        assert!(verify_errors.is_empty(), "verification errors: {:?}", verify_errors);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn test_verify_directory_missing_file() {
        let dir = std::env::temp_dir().join("gutch_test_verify");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let fm = FileManifest {
            path: "nonexistent.txt".to_string(),
            size: 10,
            mode: 0o644,
            executable: false,
            symlink_target: None,
            full_file_hash: "abc".to_string(),
            chunks: vec![],
        };

        let vm = VersionManifest::new("p".into(), "v".into(), "win".into(), "x64".into(), "base".into());
        let errors = verify_directory(&vm, &[fm], &dir).unwrap();
        assert!(!errors.is_empty());
        assert!(errors[0].contains("missing"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_large_game_build_200mb() {
        let root = std::env::temp_dir().join("gutch_test_200mb");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        for sub in &["audio", "textures", "data"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }

        let t0 = Instant::now();

        test_util::make_file(root.join("game.exe"), 42 * 1024 * 1024, test_util::Seed::Binary);
        test_util::make_file(root.join("audio/eng.pak"), 48 * 1024 * 1024, test_util::Seed::Audio);
        test_util::make_file(root.join("audio/fr.pak"), 32 * 1024 * 1024, test_util::Seed::Audio);
        test_util::make_file(root.join("textures/characters.tex"), 22 * 1024 * 1024, test_util::Seed::Texture);
        test_util::make_file(root.join("textures/environment.tex"), 22 * 1024 * 1024, test_util::Seed::Texture);
        test_util::make_file(root.join("data/assets.pak"), 38 * 1024 * 1024, test_util::Seed::Archive);
        test_util::make_file(root.join("data/config.json"), 2 * 1024 * 1024, test_util::Seed::Config);
        test_util::make_file(root.join("readme.txt"), 500 * 1024, test_util::Seed::Text);

        let gen_ms = t0.elapsed().as_millis();

        let total_original = test_util::dir_size(&root);
        assert!(total_original >= 190 * 1024 * 1024, "expected >=190 MB, got {} MB", total_original / (1024 * 1024));

        let t1 = Instant::now();
        let result = build_from_directory(&root, "mock-game", "1.0.0", "windows", "x86_64", "base").unwrap();
        let build_ms = t1.elapsed().as_millis();

        let bundle_total: u64 = result.bundle_data.iter().map(|b| b.len() as u64).sum();
        let compression_pct = if result.version_manifest.total_logical_size > 0 {
            100.0 * bundle_total as f64 / result.version_manifest.total_logical_size as f64
        } else {
            0.0
        };

        let t2 = Instant::now();
        let out_dir = std::env::temp_dir().join("gutch_test_200mb_reconstructed");
        let _ = std::fs::remove_dir_all(&out_dir);
        reconstruct_from_manifest(&result.version_manifest, &result.file_manifests, &result.bundle_data, &out_dir).unwrap();
        let recon_ms = t2.elapsed().as_millis();

        let t3 = Instant::now();
        let errors = verify_directory(&result.version_manifest, &result.file_manifests, &out_dir).unwrap();
        let verify_ms = t3.elapsed().as_millis();

        assert!(errors.is_empty(), "verification errors: {:?}", errors);

        let est = estimate_build(&result.version_manifest, &result.file_manifests, &Default::default(), 1024 * 1024 * 1024).unwrap();

        println!();
        println!("=== 200 MB mock game build ===");
        println!("  files:           {}", result.file_manifests.len());
        println!("  total logical:   {:.1} MB", result.version_manifest.total_logical_size as f64 / (1024.0 * 1024.0));
        println!("  total bundled:   {:.1} MB ({:.1}% of original)", bundle_total as f64 / (1024.0 * 1024.0), compression_pct);
        println!("  unique chunks:   {}", result.version_manifest.required_chunk_set.len());
        println!("  bundles:         {}", result.bundle_data.len());
        println!();
        println!("  gen files:       {} ms", gen_ms);
        println!("  build:           {} ms", build_ms);
        println!("  reconstruct:     {} ms", recon_ms);
        println!("  verify:          {} ms", verify_ms);
        println!("  total pipeline:  {} ms", gen_ms + build_ms + recon_ms + verify_ms);
        println!();
        println!("  fresh install estimate:");
        println!("    missing chunks:      {}", est.missing_chunk_count);
        println!("    missing logical:     {:.1} MB", est.missing_logical_bytes as f64 / (1024.0 * 1024.0));
        println!("    missing compressed:  {:.1} MB", est.missing_compressed_bytes as f64 / (1024.0 * 1024.0));
        println!("    estimated bundles:   {}", est.required_bundle_count);

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&out_dir);
    }
}
