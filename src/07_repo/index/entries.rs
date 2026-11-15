//! Chunk index entry storage and lookup.

use super::ChunkIndexEntry;
use crate::artifact::hash::ChunkHash;

/// Sorted binary keys and contiguous values. Text is decoded only at boundaries.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChunkIndexEntries(Vec<(ChunkHash, ChunkIndexEntry)>);

impl ChunkIndexEntries {
    pub(crate) fn share_artifacts(&mut self) {
        let mut artifacts = std::collections::BTreeMap::new();
        for (_, entry) in &mut self.0 {
            let shared = artifacts
                .entry(entry.bundle_id.clone())
                .or_insert_with(|| entry.artifact.clone());
            if **shared == *entry.artifact {
                entry.artifact = shared.clone();
            }
        }
    }
    /// Keep the decoder's row allocation without sorting or dropping invalid duplicates.
    pub(super) fn from_sorted_rows(rows: Vec<(ChunkHash, ChunkIndexEntry)>) -> crate::Result<Self> {
        if rows.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(crate::Error::manifest(
                "chunk index keys must be unique and sorted",
            ));
        }
        Ok(Self(rows))
    }

    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
    pub fn get(&self, hash: &str) -> Option<&ChunkIndexEntry> {
        self.get_hash(&ChunkHash::parse_hex(hash).ok()?)
    }
    pub fn get_hash(&self, hash: &ChunkHash) -> Option<&ChunkIndexEntry> {
        self.0
            .binary_search_by_key(hash, |row| row.0)
            .ok()
            .map(|row| &self.0[row].1)
    }
    pub fn contains_key(&self, hash: &str) -> bool {
        self.get(hash).is_some()
    }
    pub fn insert(&mut self, hash: ChunkHash, entry: ChunkIndexEntry) -> Option<ChunkIndexEntry> {
        match self.0.binary_search_by_key(&hash, |row| row.0) {
            Ok(row) => Some(std::mem::replace(&mut self.0[row].1, entry)),
            Err(row) => {
                self.0.insert(row, (hash, entry));
                None
            }
        }
    }
    pub fn extend(&mut self, updates: impl IntoIterator<Item = (ChunkHash, ChunkIndexEntry)>) {
        // Later updates win for repeated hashes; merging retains sorted keys.
        let updates: std::collections::BTreeMap<_, _> = updates.into_iter().collect();
        let mut existing_entries = std::mem::take(&mut self.0).into_iter().peekable();
        let mut replacement_entries = updates.into_iter().peekable();
        let mut merged = Vec::with_capacity(existing_entries.len() + replacement_entries.len());
        while let (Some(existing_entry), Some(replacement_entry)) =
            (existing_entries.peek(), replacement_entries.peek())
        {
            match existing_entry.0.cmp(&replacement_entry.0) {
                std::cmp::Ordering::Less => merged.push(existing_entries.next().unwrap()),
                std::cmp::Ordering::Equal => {
                    existing_entries.next();
                    merged.push(replacement_entries.next().unwrap());
                }
                std::cmp::Ordering::Greater => merged.push(replacement_entries.next().unwrap()),
            }
        }
        merged.extend(existing_entries);
        merged.extend(replacement_entries);
        self.0 = merged;
        self.share_artifacts();
    }
    pub fn iter(&self) -> impl Iterator<Item = (&ChunkHash, &ChunkIndexEntry)> {
        self.0.iter().map(|(hash, value)| (hash, value))
    }
    pub fn keys(&self) -> impl Iterator<Item = &ChunkHash> {
        self.0.iter().map(|(hash, _)| hash)
    }
    pub fn values(&self) -> impl Iterator<Item = &ChunkIndexEntry> {
        self.0.iter().map(|(_, value)| value)
    }
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut ChunkIndexEntry> {
        self.0.iter_mut().map(|(_, value)| value)
    }
}
impl IntoIterator for ChunkIndexEntries {
    type Item = (ChunkHash, ChunkIndexEntry);
    type IntoIter = std::vec::IntoIter<Self::Item>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
impl<'a> IntoIterator for &'a ChunkIndexEntries {
    type Item = (&'a ChunkHash, &'a ChunkIndexEntry);
    type IntoIter = std::iter::Map<
        std::slice::Iter<'a, (ChunkHash, ChunkIndexEntry)>,
        fn(&'a (ChunkHash, ChunkIndexEntry)) -> Self::Item,
    >;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(|(hash, value)| (hash, value))
    }
}
impl FromIterator<(ChunkHash, ChunkIndexEntry)> for ChunkIndexEntries {
    fn from_iter<T: IntoIterator<Item = (ChunkHash, ChunkIndexEntry)>>(iter: T) -> Self {
        let mut rows: Vec<_> = iter.into_iter().collect();
        rows.sort_by_key(|row| row.0);
        rows.dedup_by_key(|row| row.0);
        let mut entries = Self(rows);
        entries.share_artifacts();
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_rows_require_strict_order_without_repairing_input() {
        let rows = |keys: &[u8]| {
            keys.iter()
                .map(|&key| {
                    (
                        ChunkHash::from_bytes([key; 32]),
                        ChunkIndexEntry::new(
                            128,
                            u32::from(key),
                            "ab".repeat(32).into(),
                            16,
                            crate::artifact::profiles::ArtifactStorageMode::Bundled,
                        ),
                    )
                })
                .collect::<Vec<_>>()
        };
        for keys in [vec![], vec![1], vec![0, 1, 2]] {
            let entries = ChunkIndexEntries::from_sorted_rows(rows(&keys)).unwrap();
            let expected = keys
                .into_iter()
                .map(|key| ChunkHash::from_bytes([key; 32]))
                .collect::<Vec<_>>();
            assert_eq!(entries.keys().copied().collect::<Vec<_>>(), expected);
        }
        for keys in [vec![1, 1], vec![2, 1], vec![0, 2, 1]] {
            let error = ChunkIndexEntries::from_sorted_rows(rows(&keys)).unwrap_err();
            assert!(error.to_string().contains("unique and sorted"));
        }
    }
}
