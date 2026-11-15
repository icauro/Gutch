//! Entry-level comparison of two snapshots.

use std::cmp::Ordering;
use std::iter::Peekable;

use crate::Result;
use crate::manifest::SnapshotEntry;
use crate::progress::OperationControl;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotChangeKind {
    Added,
    Removed,
    /// File content or symlink target changed.
    Modified,
    /// Only executable or read-only flags changed.
    PropertiesChanged,
    /// The path changed between file, directory and symlink.
    TypeChanged,
}

impl SnapshotChangeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::Modified => "modified",
            Self::PropertiesChanged => "properties_changed",
            Self::TypeChanged => "type_changed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SnapshotEntryChange {
    pub path: String,
    pub change: SnapshotChangeKind,
    /// Signed logical file-size difference, not compressed or transferred bytes.
    pub delta_bytes: i128,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SnapshotDiff {
    pub changes: Vec<SnapshotEntryChange>,
    pub delta_bytes: i128,
}

pub(crate) fn compare_snapshot_entries(
    before: &[SnapshotEntry],
    after: &[SnapshotEntry],
    control: &OperationControl,
) -> Result<SnapshotDiff> {
    let mut before = sorted_by_path(before).into_iter().peekable();
    let mut after = sorted_by_path(after).into_iter().peekable();
    let mut result = SnapshotDiff::default();
    while let Some((before, after)) = next_pair(&mut before, &mut after) {
        control.checkpoint()?;
        let (path, change) = match (before, after) {
            (None, Some(after)) => (after.path(), SnapshotChangeKind::Added),
            (Some(before), None) => (before.path(), SnapshotChangeKind::Removed),
            (Some(before), Some(after)) => match changed_entry_kind(before, after) {
                Some(change) => (after.path(), change),
                None => continue,
            },
            (None, None) => unreachable!("next_pair yields at least one entry"),
        };
        let size = |entry: Option<&SnapshotEntry>| {
            i128::from(
                entry
                    .and_then(SnapshotEntry::as_file)
                    .map_or_default(|file| file.size),
            )
        };
        let delta_bytes = size(after) - size(before);
        result.delta_bytes += delta_bytes;
        result.changes.push(SnapshotEntryChange {
            path: path.to_owned(),
            change,
            delta_bytes,
        });
    }
    Ok(result)
}

/// Snapshot manifests have unique paths; capture stores them sorted, so the
/// sort is usually a cheap already-sorted pass.
fn sorted_by_path(entries: &[SnapshotEntry]) -> Vec<&SnapshotEntry> {
    let mut sorted = entries.iter().collect::<Vec<_>>();
    sorted.sort_unstable_by(|left, right| left.path().cmp(right.path()));
    sorted
}

type EntryIter<'a> = Peekable<std::vec::IntoIter<&'a SnapshotEntry>>;
type EntryPair<'a> = (Option<&'a SnapshotEntry>, Option<&'a SnapshotEntry>);

/// Takes the next path from two path-sorted streams, pairing equal paths.
fn next_pair<'a>(before: &mut EntryIter<'a>, after: &mut EntryIter<'a>) -> Option<EntryPair<'a>> {
    let order = match (before.peek(), after.peek()) {
        (None, None) => return None,
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (Some(left), Some(right)) => left.path().cmp(right.path()),
    };
    Some(match order {
        Ordering::Less => (before.next(), None),
        Ordering::Greater => (None, after.next()),
        Ordering::Equal => (before.next(), after.next()),
    })
}

fn changed_entry_kind(before: &SnapshotEntry, after: &SnapshotEntry) -> Option<SnapshotChangeKind> {
    use SnapshotChangeKind::{Modified, PropertiesChanged, TypeChanged};
    match (before, after) {
        (SnapshotEntry::File(before), SnapshotEntry::File(after)) => {
            if before.size != after.size || before.full_file_hash != after.full_file_hash {
                Some(Modified)
            } else if before.executable != after.executable || before.read_only != after.read_only {
                Some(PropertiesChanged)
            } else {
                None
            }
        }
        (SnapshotEntry::Directory(before), SnapshotEntry::Directory(after)) => {
            (before.read_only != after.read_only).then_some(PropertiesChanged)
        }
        (SnapshotEntry::Symlink(before), SnapshotEntry::Symlink(after)) => {
            (before.target_path != after.target_path).then_some(Modified)
        }
        _ => Some(TypeChanged),
    }
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
