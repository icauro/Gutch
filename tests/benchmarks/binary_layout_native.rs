//! Native codec measurements invoked by binary_layout_experiment.py.
use crate::artifact::{binary_io::*, hash::ChunkHash};
use crate::manifest::blueprint_columns::{decode, encode};

#[test]
fn native_corpus_layout_comparison() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let repetitions = std::env::var("GUTCH_LAYOUT_REPETITIONS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(7)
        .clamp(1, 100);
    fn median_us(repetitions: usize, mut operation: impl FnMut()) -> u128 {
        let mut samples = Vec::new();
        for _ in 0..repetitions {
            let start = std::time::Instant::now();
            operation();
            samples.push(start.elapsed().as_micros());
        }
        samples.sort_unstable();
        samples[samples.len() / 2]
    }
    let mut report = Vec::new();
    let mut index_report = Vec::new();
    for item in walkdir::WalkDir::new(root.join("tests/datasets/corpus")) {
        let item = item.unwrap();
        if item.file_type().is_file() && item.file_name() == "chunk_map.gtcbin" {
            let bytes = std::fs::read(item.path()).unwrap();
            let index =
                crate::repo::index::binary_codec::load_chunk_index_binary_from_bytes(&bytes)
                    .unwrap();
            let decode_us = median_us(repetitions, || {
                std::hint::black_box(
                    crate::repo::index::binary_codec::load_chunk_index_binary_from_bytes(&bytes)
                        .unwrap(),
                );
            });
            let lookup_us = median_us(repetitions, || {
                for key in index.entries.keys() {
                    assert!(std::hint::black_box(index.entries.get_hash(key)).is_some());
                }
            });
            let artifacts: std::collections::HashSet<_> = index
                .entries
                .values()
                .map(|entry| std::sync::Arc::as_ptr(&entry.artifact))
                .collect();
            index_report.push(serde_json::json!({"path": item.path().strip_prefix(root).unwrap(), "encoded_bytes": bytes.len(), "chunks": index.entries.len(), "shared_artifacts": artifacts.len(), "inline_bytes_per_chunk": std::mem::size_of::<(ChunkHash, crate::repo::index::ChunkIndexEntry)>(), "decode_median_us": decode_us, "all_key_lookups_median_us": lookup_us}));
            continue;
        }
        if !item.file_type().is_file() || item.file_name() != "snapshot_blueprint.gtcbin" {
            continue;
        }
        let original = std::fs::read(item.path()).unwrap();
        let entries =
            crate::manifest::blueprint_codec::read_snapshot_blueprint_bytes(&original).unwrap();
        let columns = encode(&entries).unwrap();
        let magic = crate::artifact::format::SNAPSHOT_BLUEPRINT_MAGIC;
        let grouped = crate::artifact::metadata::encode(magic, &columns).unwrap();
        assert_eq!(
            grouped, original,
            "canonical writer must reproduce frozen metadata"
        );
        let mut merged = Vec::new();
        for column in &columns {
            write_u32(&mut merged, column.len() as u32).unwrap();
        }
        merged.extend_from_slice(&columns.concat());
        let whole = crate::artifact::metadata::encode(magic, &[merged]).unwrap();
        let decode_whole = || {
            let groups = crate::artifact::metadata::decode(&whole, magic, 1).unwrap();
            let mut merged = groups[0].as_slice();
            let lengths: Vec<_> = (0..3)
                .map(|_| read_u32(&mut merged).unwrap() as usize)
                .collect();
            let mut restored = Vec::new();
            for length in lengths {
                let (column, rest) = merged.split_at(length);
                restored.push(column.to_vec());
                merged = rest;
            }
            assert!(merged.is_empty());
            decode(&restored).unwrap()
        };
        let restored =
            decode(&crate::artifact::metadata::decode(&grouped, magic, 3).unwrap()).unwrap();
        let mut sorted = entries.clone();
        sorted.sort_by(|a, b| a.path().cmp(b.path()));
        assert_eq!(
            serde_json::to_value(&restored).unwrap(),
            serde_json::to_value(&sorted).unwrap()
        );
        assert_eq!(
            serde_json::to_value(decode_whole()).unwrap(),
            serde_json::to_value(&sorted).unwrap()
        );
        let full_decode_us = median_us(repetitions, || {
            std::hint::black_box(
                decode(&crate::artifact::metadata::decode(&grouped, magic, 3).unwrap()).unwrap(),
            );
        });
        let whole_decode_us = median_us(repetitions, || {
            std::hint::black_box(decode_whole());
        });
        let columns_decode_us = median_us(repetitions, || {
            std::hint::black_box(decode(&columns).unwrap());
        });
        let encode_us = median_us(repetitions, || {
            std::hint::black_box(
                crate::artifact::metadata::encode(magic, &encode(&entries).unwrap()).unwrap(),
            );
        });
        report.push(serde_json::json!({"path": item.path().strip_prefix(root).unwrap(), "current_bytes": original.len(), "whole_bytes": whole.len(), "group_bytes": grouped.len(), "full_decode_median_us": full_decode_us, "whole_decode_median_us": whole_decode_us, "column_validation_materialization_median_us": columns_decode_us, "encode_median_us": encode_us, "repetitions": repetitions}));
    }
    assert!(!report.is_empty(), "corpus blueprint files must be present");
    let directory = root.join("tests/.artifacts/binary-layouts");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("native.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    std::fs::write(
        directory.join("index-native.json"),
        serde_json::to_vec_pretty(&index_report).unwrap(),
    )
    .unwrap();
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
