//! Binary encoding of the snapshot blueprint (the stored entry list).

use crate::artifact::{format::SNAPSHOT_BLUEPRINT_MAGIC, metadata};
use crate::filesystem::write_atomic_with;
use crate::manifest::snapshot::SnapshotBlueprint;
use crate::{Result, manifest};
use std::io::Write;
use std::path::Path;

pub(crate) fn write_snapshot_blueprint_binary(
    path: &Path,
    blueprint: &SnapshotBlueprint,
) -> Result<()> {
    let groups = super::blueprint_columns::encode(&blueprint.entries)?;
    let bytes = metadata::encode(SNAPSHOT_BLUEPRINT_MAGIC, &groups)?;
    write_atomic_with(path, |file| {
        file.write_all(&bytes)?;
        Ok(())
    })
}
pub(crate) fn read_snapshot_blueprint_bytes(bytes: &[u8]) -> Result<Vec<manifest::SnapshotEntry>> {
    super::blueprint_columns::decode(&metadata::decode(bytes, SNAPSHOT_BLUEPRINT_MAGIC, 3)?)
}

#[cfg(test)]
#[path = "blueprint_codec_tests.rs"]
mod tests;
