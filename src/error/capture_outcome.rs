//! Typed capture outcome carried by [`super::Error`] on conflicts.
use serde::{Deserialize, Serialize};

/// One differing metadata field between a capture candidate and the stored
/// snapshot with the same content-derived ID.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MetadataFieldDifference {
    pub field: String,
    pub existing: String,
    pub requested: String,
}

/// Typed outcome when capture computes a content-derived snapshot ID that
/// already exists in the repo.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotCaptureOutcome {
    Created {
        snapshot_id: String,
    },
    Unchanged {
        snapshot_id: String,
    },
    MetadataConflict {
        snapshot_id: String,
        differences: Vec<MetadataFieldDifference>,
    },
    IntegrityConflict {
        snapshot_id: String,
    },
}

impl SnapshotCaptureOutcome {
    /// Caller-facing wording for the outcome. Adapters render this message
    /// without inferring their own tag-merging or overwrite policy.
    pub fn message(&self) -> String {
        match self {
            SnapshotCaptureOutcome::Created { snapshot_id } => {
                format!("Captured snapshot {snapshot_id}.")
            }
            SnapshotCaptureOutcome::Unchanged { .. } => {
                String::from("Snapshot content already exists; no changes were made.")
            }
            SnapshotCaptureOutcome::MetadataConflict {
                snapshot_id,
                differences,
            } => {
                let mut message = format!("snapshot_id: {snapshot_id}");
                for difference in differences {
                    message.push_str(&format!(
                        "\nfield '{}': existing '{}', requested '{}'",
                        difference.field, difference.existing, difference.requested
                    ));
                }
                message.push_str(
                    "\nCapture did not change the existing snapshot. Use an explicit metadata update for this snapshot ID.",
                );
                message
            }
            SnapshotCaptureOutcome::IntegrityConflict { snapshot_id } => {
                format!(
                    "Existing snapshot data does not match its content-derived ID. The repository may be corrupt.\nsnapshot_id: {snapshot_id}"
                )
            }
        }
    }
}

impl std::fmt::Display for SnapshotCaptureOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message())
    }
}
