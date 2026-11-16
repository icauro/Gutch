//! Provider-neutral remote repo planning and synchronization.
//!
//! A caller supplies a remote repo provider for control-plane lookup and
//! artifact transfer. Gutch owns the comparison rules, local repo
//! mutations, progress, cancellation, and synchronization order.

use serde::Serialize;

use crate::Result;
use crate::manifest::SnapshotTagSet;
use crate::progress::OperationControl;
use crate::repo::tag_references::TagReference;
use crate::repo::tag_references::TagReferenceStatus;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct RemoteRepo {
    pub id: String,
    /// Opaque provider-owned grouping value used only by the provider.
    pub scope_id: String,
    pub name: String,
    pub metadata_hash_b3: String,
    pub chunk_map_hash_b3: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct RemoteSnapshot {
    pub remote_id: String,
    pub snapshot_id: String,
    pub metadata_hash_b3: String,
    pub product_metadata_hash_b3: String,
    pub blueprint_hash_b3: String,
    pub uploaded_at: String,
    pub created_at: String,
    pub tags: Vec<String>,
    pub artifact_state: Option<String>,
    pub artifact_ready: bool,
}

/// Where Gutch reads one remote artifact from.
#[derive(Clone)]
pub enum RemoteArtifactSource {
    Bytes(Vec<u8>),
    /// Downloaded over HTTP(S), for example from a presigned link.
    Url(String),
    /// Read when Gutch needs it, so a provider can hand out many sources
    /// without holding every artifact in memory.
    Fetch(Arc<dyn Fn() -> Result<Vec<u8>> + Send + Sync>),
}

/// URLs may be presigned, so `Debug` never prints them or the bytes.
impl std::fmt::Debug for RemoteArtifactSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bytes(bytes) => write!(formatter, "Bytes({} bytes)", bytes.len()),
            Self::Url(_) => formatter.write_str("Url(<redacted>)"),
            Self::Fetch(_) => formatter.write_str("Fetch"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RemoteSnapshotSources {
    pub metadata: RemoteArtifactSource,
    pub blueprint: RemoteArtifactSource,
    pub product_metadata: Option<RemoteArtifactSource>,
}

#[derive(Debug, Clone)]
pub struct RemoteBundleSource {
    pub bundle_id: String,
    pub source: RemoteArtifactSource,
}

/// A stored remote object that [`RemoteRepoWriter::upload_artifact`]
/// replaces.
#[derive(Debug, Clone, Copy)]
pub enum RemoteArtifactTarget<'a> {
    RepositoryMetadata,
    ChunkIndex,
    SnapshotMetadata(&'a RemoteSnapshot),
    SnapshotBlueprint(&'a RemoteSnapshot),
    SnapshotProductMetadata(&'a RemoteSnapshot),
    Bundle(&'a str),
}

/// Read-side control-plane and artifact-source operations required by Gutch.
pub trait RemoteRepoReader {
    fn resolve_repo(
        &self,
        remote_repo_id: Option<&str>,
        bound_remote_repo_id: Option<&str>,
    ) -> Result<RemoteRepo>;

    fn list_snapshots(&self, repository: &RemoteRepo) -> Result<Vec<RemoteSnapshot>>;

    fn repo_metadata_source(&self, repository: &RemoteRepo) -> Result<RemoteArtifactSource>;

    fn chunk_index_source(&self, repository: &RemoteRepo) -> Result<RemoteArtifactSource>;

    fn snapshot_sources(
        &self,
        repository: &RemoteRepo,
        snapshot: &RemoteSnapshot,
    ) -> Result<RemoteSnapshotSources>;

    fn bundle_sources(
        &self,
        repository: &RemoteRepo,
        snapshot: &RemoteSnapshot,
        bundle_ids: &[String],
    ) -> Result<Vec<RemoteBundleSource>>;
}

/// Write-side control-plane and artifact-upload operations required by Gutch.
pub trait RemoteRepoWriter {
    /// Called by `push_repo` before it reads or changes the remote. A provider
    /// that allows only one push at a time takes its lock here and returns an
    /// error when another push holds it. The default does nothing.
    fn begin_push(&self, _repository: &RemoteRepo) -> Result<()> {
        Ok(())
    }

    /// Called once after a successful `begin_push`, when the push has ended;
    /// `succeeded` says whether it completed. The default does nothing.
    fn finish_push(&self, _repository: &RemoteRepo, _succeeded: bool) -> Result<()> {
        Ok(())
    }

    fn delete_snapshot(&self, repository: &RemoteRepo, snapshot: &RemoteSnapshot) -> Result<()>;

    fn create_snapshot(
        &self,
        repository: &RemoteRepo,
        snapshot_id: &str,
        tags: &SnapshotTagSet,
    ) -> Result<RemoteSnapshot>;

    fn upload_artifact(
        &self,
        repository: &RemoteRepo,
        target: RemoteArtifactTarget<'_>,
        bytes: Vec<u8>,
        control: &OperationControl,
    ) -> Result<()>;

    fn delete_snapshot_product_metadata(
        &self,
        repository: &RemoteRepo,
        snapshot: &RemoteSnapshot,
    ) -> Result<()>;

    fn bundle_exists(&self, repository: &RemoteRepo, bundle_id: &str) -> Result<bool>;

    fn reconcile_bundles(
        &self,
        repository: &RemoteRepo,
        required_bundle_ids: &[String],
    ) -> Result<usize>;
}

/// A provider that supports both repo reads and writes.
pub trait RemoteRepoProvider: RemoteRepoReader + RemoteRepoWriter {}

impl<T> RemoteRepoProvider for T where T: RemoteRepoReader + RemoteRepoWriter {}

/// Provider-neutral information used when listing remote repos.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteRepoSummary {
    pub scope_id: String,
    pub scope_name: String,
    pub scope_slug: String,
    pub repository_id: String,
    pub repository_name: String,
}

/// Provider-neutral result listing tags referenced by the remote provider ruleset.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteTagReferencesResult {
    pub repo_root: String,
    pub remote_repo_id: String,
    pub referenced_tags: Vec<TagReferenceStatus>,
}

/// The host application supplies this context to Gutch's remote CLI.
///
/// The context owns backend configuration and authentication. Gutch only receives the
/// provider-neutral operations needed to synchronize repos and evaluate tags.
pub trait RemoteRepoService {
    /// Let the host enforce its publication policy before CLI synchronization.
    fn prepare_push(&self, _repo: &Path) -> Result<()> {
        Ok(())
    }

    /// Apply host requirements when a local repository is linked.
    fn configure_binding(&self, _repo: &Path, _remote_repo_id: &str) -> Result<()> {
        Ok(())
    }

    fn repo_provider(&self) -> &dyn RemoteRepoProvider;

    fn list_repos(&self) -> Result<Vec<RemoteRepoSummary>>;

    fn create_repo(&self, scope_id: &str, name: &str) -> Result<String>;

    fn delete_repo(&self, repository: &RemoteRepo) -> Result<()>;

    fn tag_references(&self, repository: &RemoteRepo) -> Result<Vec<TagReference>>;
}

/// Which copy pull keeps when a local file differs from the remote one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum PullConflictPolicy {
    KeepLocal,
    KeepRemote,
    #[default]
    KeepNewer,
}

#[derive(Debug, Serialize)]
pub struct RepoPullResult {
    pub command: &'static str,
    pub repo_root: String,
    pub remote_repo_id: String,
    pub repo_manifest_downloaded: bool,
    pub chunk_map_downloaded: bool,
    pub snapshot_downloaded_count: usize,
    pub snapshot_skipped_count: usize,
    pub bundle_downloaded_count: usize,
    pub bundle_skipped_count: usize,
    pub kept_local_count: usize,
    pub local_only_snapshots_kept: usize,
    pub pruned_snapshot_count: usize,
    pub pruned_bundle_count: usize,
}

#[derive(Debug, Serialize)]
pub struct RepoPullPlan {
    pub repo_root: String,
    pub remote_repo_id: String,
    pub remote_repo_name: String,
    pub repo_manifest_needed: bool,
    pub chunk_map_needed: bool,
    pub snapshots_to_download: Vec<String>,
    pub snapshots_to_skip: Vec<String>,
    pub local_only_snapshots: Vec<String>,
    pub kept_local_snapshots: Vec<String>,
    pub bundle_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoPushSnapshotChange {
    pub snapshot_id: String,
    pub version: String,
    pub os: String,
    pub arch: String,
    pub artifact_state: Option<String>,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoPushPlan {
    pub repo_root: String,
    pub remote_repo_id: String,
    pub repo_manifest_changed: bool,
    pub chunk_index_changed: bool,
    pub snapshot_count: usize,
    pub remote_snapshot_count: usize,
    pub to_add: Vec<RepoPushSnapshotChange>,
    pub to_remove: Vec<RepoPushSnapshotChange>,
    pub to_update: Vec<RepoPushSnapshotChange>,
    pub unchanged: Vec<RepoPushSnapshotChange>,
}

#[derive(Debug, Serialize)]
pub struct RepoPushResult {
    pub command: &'static str,
    pub repo_root: String,
    pub remote_repo_id: String,
    pub repo_manifest_uploaded: bool,
    pub chunk_index_uploaded: bool,
    pub snapshot_manifest_uploaded_count: usize,
    pub product_metadata_uploaded_count: usize,
    pub snapshot_blueprint_uploaded_count: usize,
    pub bundle_uploaded_count: usize,
    pub bundle_skipped_count: usize,
    pub remote_bundle_deleted_count: usize,
    pub snapshot_count: usize,
    pub bundle_count: usize,
    pub snapshot_added_count: usize,
    pub snapshot_removed_count: usize,
    pub snapshot_updated_count: usize,
    pub snapshot_unchanged_count: usize,
}

#[cfg(test)]
mod tests {
    use super::RemoteArtifactSource;

    #[test]
    fn debug_output_hides_urls_and_bytes() {
        let url = RemoteArtifactSource::Url("https://example.test/a?signature=secret".into());
        let bytes = RemoteArtifactSource::Bytes(vec![1, 2, 3]);

        assert_eq!(format!("{url:?}"), "Url(<redacted>)");
        assert_eq!(format!("{bytes:?}"), "Bytes(3 bytes)");
    }
}
