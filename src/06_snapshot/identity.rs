//! Content-derived snapshot identity.

use crate::artifact::hash::hash_hex;
use crate::manifest::SnapshotEntry;

/// Hashes entries in path order, so the ID ignores input order. Paths must be
/// unique, as manifest validation guarantees; capture already sorts entries,
/// so that path neither allocates nor sorts again.
pub(crate) fn snapshot_id_from_entries(entries: &[SnapshotEntry]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"gutch-installation-id-v2");
    hasher.update(&(entries.len() as u64).to_le_bytes());
    if entries
        .windows(2)
        .all(|pair| pair[0].path() <= pair[1].path())
    {
        for entry in entries {
            hash_identity_entry(&mut hasher, entry);
        }
    } else {
        let mut sorted_entries: Vec<_> = entries.iter().collect();
        sorted_entries.sort_unstable_by(|left, right| left.path().cmp(right.path()));
        for entry in sorted_entries {
            hash_identity_entry(&mut hasher, entry);
        }
    }
    hash_hex(&hasher.finalize())
}

fn hash_identity_field(hasher: &mut blake3::Hasher, value: &str) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn hash_identity_entry(hasher: &mut blake3::Hasher, entry: &SnapshotEntry) {
    hash_identity_field(hasher, entry.path());
    match entry {
        SnapshotEntry::File(file) => {
            hasher.update(b"f");
            hash_identity_field(hasher, &file.full_file_hash.to_hex());
            hasher.update(&[u8::from(file.executable), u8::from(file.read_only)]);
        }
        SnapshotEntry::Directory(directory) => {
            hasher.update(b"d");
            hasher.update(&[u8::from(directory.read_only)]);
        }
        SnapshotEntry::Symlink(link) => {
            hasher.update(b"l");
            hash_identity_field(hasher, &link.target_path);
        }
    }
}

#[cfg(test)]
#[path = "identity_tests.rs"]
mod tests;
