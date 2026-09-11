use super::ChunkIndexEntry;
use crate::ChunkHash;

/// Sorted binary keys and contiguous values. Text is decoded only at boundaries.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChunkIndexEntries(Vec<(ChunkHash, ChunkIndexEntry)>);

impl ChunkIndexEntries {
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
        let updates: std::collections::BTreeMap<_, _> = updates.into_iter().collect();
        let mut old = std::mem::take(&mut self.0).into_iter().peekable();
        let mut new = updates.into_iter().peekable();
        let mut merged = Vec::with_capacity(old.len() + new.len());
        while let (Some(left), Some(right)) = (old.peek(), new.peek()) {
            match left.0.cmp(&right.0) {
                std::cmp::Ordering::Less => merged.push(old.next().unwrap()),
                std::cmp::Ordering::Equal => {
                    old.next();
                    merged.push(new.next().unwrap());
                }
                std::cmp::Ordering::Greater => merged.push(new.next().unwrap()),
            }
        }
        merged.extend(old);
        merged.extend(new);
        self.0 = merged;
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
        Self(rows)
    }
}
