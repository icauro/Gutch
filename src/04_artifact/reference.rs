//! Repository-independent identity of one stored artifact.

use crate::artifact::hash::ChunkHash;
use std::borrow::Cow;

/// A persisted artifact that can satisfy one or more logical chunks.
///
/// Bundles and loose chunks differ only at this boundary. Snapshot planning,
/// reconstruction, and verification operate on this type; the remote transfer
/// layer resolves bundle URLs at its provider-specific boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ArtifactRef {
    // Many chunks share a bundle, so retain the index's shared identifier storage.
    Bundle { id: smol_str::SmolStr },
    LooseChunk { hash: ChunkHash },
}

impl ArtifactRef {
    /// File name of the artifact: the bundle id or the chunk hash in hex.
    pub(crate) fn id(&self) -> Cow<'_, str> {
        match self {
            Self::Bundle { id } => Cow::Borrowed(id),
            Self::LooseChunk { hash } => Cow::Owned(hash.to_hex()),
        }
    }
    pub(crate) fn is_bundle(&self) -> bool {
        matches!(self, Self::Bundle { .. })
    }
}
