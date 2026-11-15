use std::collections::BTreeMap;

use crate::Result;
use crate::manifest::SnapshotEntry;
use crate::progress::GutchOperationControl;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotChangeKind {
    Added,
    Removed,
    Modified,
    Metadata,
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
    control: &GutchOperationControl,
) -> Result<SnapshotDiff> {
    let mut entries = BTreeMap::new();
    for entry in before {
        control.checkpoint()?;
        entries.insert(entry.path(), (Some(entry), None));
    }
    for entry in after {
        control.checkpoint()?;
        entries.entry(entry.path()).or_insert((None, None)).1 = Some(entry);
    }
    let mut result = SnapshotDiff::default();
    for (path, (before, after)) in entries {
        control.checkpoint()?;
        let change = match (before, after) {
            (None, Some(_)) => SnapshotChangeKind::Added,
            (Some(_), None) => SnapshotChangeKind::Removed,
            (Some(before), Some(after)) => match changed_entry_kind(before, after) {
                Some(change) => change,
                None => continue,
            },
            (None, None) => unreachable!("each path came from at least one snapshot"),
        };
        let size = |entry: Option<&SnapshotEntry>| {
            i128::from(
                entry
                    .and_then(SnapshotEntry::as_file)
                    .map_or(0, |file| file.size),
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

fn changed_entry_kind(before: &SnapshotEntry, after: &SnapshotEntry) -> Option<SnapshotChangeKind> {
    use SnapshotChangeKind::{Metadata, Modified};
    match (before, after) {
        (SnapshotEntry::File(before), SnapshotEntry::File(after)) => {
            if before.size != after.size || before.full_file_hash != after.full_file_hash {
                Some(Modified)
            } else if before.executable != after.executable || before.read_only != after.read_only {
                Some(Metadata)
            } else {
                None
            }
        }
        (SnapshotEntry::Directory(before), SnapshotEntry::Directory(after)) => {
            (before.read_only != after.read_only).then_some(Metadata)
        }
        (SnapshotEntry::Symlink(before), SnapshotEntry::Symlink(after)) => {
            (before.target_path != after.target_path).then_some(Modified)
        }
        _ => Some(Modified),
    }
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
