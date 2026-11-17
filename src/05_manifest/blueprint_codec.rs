//! Binary encoding of the snapshot blueprint (the stored entry list).

use crate::artifact::seal::{SealKeys, SealPurpose, open_bytes, seal_bytes};
use crate::artifact::{format::SNAPSHOT_BLUEPRINT_MAGIC, metadata};
use crate::filesystem::write_atomic_with;
use crate::manifest::snapshot::SnapshotBlueprint;
use crate::{Result, manifest};
use std::io::Write;
use std::path::Path;

/// Writes a blueprint, sealed with `keys` in an encrypted repository.
pub(crate) fn write_snapshot_blueprint_binary(
    path: &Path,
    blueprint: &SnapshotBlueprint,
    keys: Option<&SealKeys>,
) -> Result<()> {
    let groups = super::blueprint_columns::encode(&blueprint.entries)?;
    let bytes = metadata::encode(SNAPSHOT_BLUEPRINT_MAGIC, &groups)?;
    let bytes = seal_bytes(keys, SealPurpose::SnapshotBlueprint, bytes)?;
    write_atomic_with(path, |file| {
        file.write_all(&bytes)?;
        Ok(())
    })
}
/// Decodes blueprint bytes as stored in the repository.
pub(crate) fn read_stored_snapshot_blueprint(
    stored: Vec<u8>,
    keys: Option<&SealKeys>,
) -> Result<Vec<manifest::SnapshotEntry>> {
    read_snapshot_blueprint_bytes(&open_bytes(keys, SealPurpose::SnapshotBlueprint, stored)?)
}

pub(crate) fn read_snapshot_blueprint_bytes(bytes: &[u8]) -> Result<Vec<manifest::SnapshotEntry>> {
    super::blueprint_columns::decode(&metadata::decode(bytes, SNAPSHOT_BLUEPRINT_MAGIC, 3)?)
}

#[cfg(test)]
#[path = "blueprint_codec_tests.rs"]
mod tests;
