//! Repository-independent identity of one stored artifact.

/// A persisted artifact that can satisfy one or more logical chunks.
///
/// Bundles and loose chunks differ only at this boundary. Snapshot planning,
/// reconstruction, and verification operate on this type; the remote transfer
/// layer resolves bundle URLs at its provider-specific boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ArtifactRef {
    // Many chunks share a bundle, so retain the index's shared identifier storage.
    Bundle { id: smol_str::SmolStr },
    LooseChunk { hash: String },
}

impl ArtifactRef {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Bundle { id } => id,
            Self::LooseChunk { hash } => hash,
        }
    }
}
