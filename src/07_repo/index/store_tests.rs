use super::*;
use crate::artifact::hash::ChunkHash;
use crate::artifact::profiles::ArtifactStorageMode;
use crate::repo::index::ChunkIndexEntry;
use crate::repo::paths::RepoPaths;

#[test]
fn incoming_indexes_keep_cached_and_local_only_locations_for_both_storage_modes() {
    for storage_mode in [
        ArtifactStorageMode::Bundled,
        ArtifactStorageMode::LooseChunks,
    ] {
        for (cached, shared) in [(true, true), (false, true), (false, false)] {
            let directory = tempfile::tempdir().unwrap();
            let paths = RepoPaths::from_repo_root(directory.path());
            crate::repo::paths::ensure_repo_layout(&paths, "merge").unwrap();
            let chunk_hash = ChunkHash::of(b"shared chunk");
            let (local_bundle_id, remote_bundle_id) = match storage_mode {
                ArtifactStorageMode::Bundled => (
                    blake3::hash(b"local bundle").to_hex().to_string(),
                    blake3::hash(b"remote bundle").to_hex().to_string(),
                ),
                ArtifactStorageMode::LooseChunks => (chunk_hash.to_hex(), chunk_hash.to_hex()),
            };
            let local_entry =
                ChunkIndexEntry::new(128, 0, local_bundle_id.into(), 16, storage_mode);
            let remote_entry =
                ChunkIndexEntry::new(256, 0, remote_bundle_id.into(), 16, storage_mode);
            let mut local = empty_chunk_index();
            local.entries.insert(chunk_hash, local_entry.clone());
            write_chunk_index(&paths, &local).unwrap();
            if cached {
                let artifact = crate::artifact::reference::ArtifactRef::from_chunk_entry(
                    &chunk_hash,
                    &local_entry,
                );
                let path = artifact.local_path(&paths).unwrap();
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, [0; 128]).unwrap();
            }
            let mut incoming = empty_chunk_index();
            if shared {
                incoming.entries.insert(chunk_hash, remote_entry.clone());
            }
            let remote_only_hash = ChunkHash::of(b"remote only chunk");
            incoming
                .entries
                .insert(remote_only_hash, remote_entry.clone());

            preserve_local_chunk_locations(&paths, &mut incoming).unwrap();

            let expected = if cached || !shared {
                &local_entry
            } else {
                &remote_entry
            };
            assert_eq!(incoming.entries.get(&chunk_hash), Some(expected));
            assert_eq!(incoming.entries.get(&remote_only_hash), Some(&remote_entry));
            // Planning merges only in memory; the persisted local index remains usable.
            assert_eq!(
                load_chunk_index(&paths).unwrap().entries.get(&chunk_hash),
                Some(&local_entry)
            );
        }
    }
}
