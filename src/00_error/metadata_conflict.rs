//! Metadata differences carried by [`super::Error::SnapshotMetadataConflict`].
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

/// One differing metadata field between a capture candidate and the stored
/// snapshot with the same content-derived ID.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MetadataFieldDifference {
    pub field: String,
    pub existing: String,
    pub requested: String,
}

/// Renders the differences of a metadata conflict, one field per line.
pub(super) struct MetadataDifferences<'a>(pub(super) &'a [MetadataFieldDifference]);

impl Display for MetadataDifferences<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for difference in self.0 {
            write!(
                f,
                "\nfield '{}': existing '{}', requested '{}'",
                difference.field, difference.existing, difference.requested
            )?;
        }
        Ok(())
    }
}
