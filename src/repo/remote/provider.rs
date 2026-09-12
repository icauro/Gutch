//! Provider-neutral remote repo planning and synchronization.
//!
//! A caller supplies a remote repo provider for control-plane lookup and
//! artifact transfer. Gutch owns the comparison rules, local repo
//! mutations, progress, cancellation, and synchronization order.

use serde::Serialize;

use crate::Error;
use crate::Result;
use crate::manifest::SnapshotTagSet;
use crate::progress::GutchOperationControl;

#[derive(Debug, Clone)]
pub struct RemoteRepository {
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

#[derive(Debug, Clone)]
pub enum RemoteArtifactSource {
    Bytes(Vec<u8>),
    Url(String),
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

/// Read-side control-plane and artifact-source operations required by Gutch.
pub trait RemoteRepositoryReader {
    fn resolve_repository(
        &self,
        remote_spec: Option<&str>,
        bound_remote_repo_id: Option<&str>,
    ) -> Result<RemoteRepository>;

    fn list_snapshots(&self, repository: &RemoteRepository) -> Result<Vec<RemoteSnapshot>>;

    fn repository_metadata_source(
        &self,
        repository: &RemoteRepository,
    ) -> Result<RemoteArtifactSource>;

    fn chunk_index_source(&self, repository: &RemoteRepository) -> Result<RemoteArtifactSource>;

    fn snapshot_sources(
        &self,
        repository: &RemoteRepository,
        snapshot: &RemoteSnapshot,
    ) -> Result<RemoteSnapshotSources>;

    fn bundle_sources(
        &self,
        repository: &RemoteRepository,
        snapshot: &RemoteSnapshot,
        bundle_ids: &[String],
    ) -> Result<Vec<RemoteBundleSource>>;
}

/// Write-side control-plane and artifact-upload operations required by Gutch.
pub trait RemoteRepositoryWriter {
    fn delete_snapshot(
        &self,
        repository: &RemoteRepository,
        snapshot: &RemoteSnapshot,
    ) -> Result<()>;

    fn create_snapshot(
        &self,
        repository: &RemoteRepository,
        snapshot_id: &str,
        tags: &SnapshotTagSet,
    ) -> Result<RemoteSnapshot>;

    fn upload_repository_metadata(
        &self,
        repository: &RemoteRepository,
        bytes: Vec<u8>,
        control: &GutchOperationControl,
    ) -> Result<()>;

    fn upload_chunk_index(
        &self,
        repository: &RemoteRepository,
        bytes: Vec<u8>,
        control: &GutchOperationControl,
    ) -> Result<()>;

    fn upload_snapshot_metadata(
        &self,
        repository: &RemoteRepository,
        snapshot: &RemoteSnapshot,
        bytes: Vec<u8>,
        control: &GutchOperationControl,
    ) -> Result<()>;

    fn upload_snapshot_blueprint(
        &self,
        repository: &RemoteRepository,
        snapshot: &RemoteSnapshot,
        bytes: Vec<u8>,
        control: &GutchOperationControl,
    ) -> Result<()>;

    fn upload_snapshot_product_metadata(
        &self,
        repository: &RemoteRepository,
        snapshot: &RemoteSnapshot,
        bytes: Vec<u8>,
        control: &GutchOperationControl,
    ) -> Result<()>;

    fn delete_snapshot_product_metadata(
        &self,
        repository: &RemoteRepository,
        snapshot: &RemoteSnapshot,
    ) -> Result<()>;

    fn bundle_exists(&self, repository: &RemoteRepository, bundle_id: &str) -> Result<bool>;

    fn upload_bundle(
        &self,
        repository: &RemoteRepository,
        bundle_id: &str,
        bytes: Vec<u8>,
        control: &GutchOperationControl,
    ) -> Result<()>;

    fn reconcile_bundles(
        &self,
        repository: &RemoteRepository,
        required_bundle_ids: &[String],
    ) -> Result<usize>;
}

/// A provider that supports both repo reads and writes.
pub trait RemoteRepositoryProvider: RemoteRepositoryReader + RemoteRepositoryWriter {}

impl<T> RemoteRepositoryProvider for T where T: RemoteRepositoryReader + RemoteRepositoryWriter {}

/// Provider-neutral information used when listing remote repos.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteRepositorySummary {
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
    pub referenced_tags: Vec<crate::repo::tag_references::TagReferenceStatus>,
}

/// The host application supplies this context to Gutch's remote CLI.
///
/// The context owns backend configuration and authentication. Gutch only receives the
/// provider-neutral operations needed to synchronize repos and evaluate tags.
pub trait RemoteRepositoryCommandProvider {
    fn repository_provider(&self) -> &dyn RemoteRepositoryProvider;

    fn list_remote_repositories(&self) -> Result<Vec<RemoteRepositorySummary>>;

    fn create_remote_repository(&self, scope_id: &str, name: &str) -> Result<String>;

    fn delete_remote_repository(&self, repository: &RemoteRepository) -> Result<()>;

    fn referenced_tag_references(
        &self,
        repository: &RemoteRepository,
    ) -> Result<Vec<crate::repo::tag_references::TagReference>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullConflictPolicy {
    KeepLocal,
    KeepRemote,
    KeepNewer,
}

impl PullConflictPolicy {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim() {
            "keep-local" => Ok(Self::KeepLocal),
            "keep-remote" => Ok(Self::KeepRemote),
            "keep-newer" => Ok(Self::KeepNewer),
            other => Err(Error::manifest(format!(
                "invalid --on-conflict value '{}' (expected keep-local, keep-remote, or keep-newer)",
                other
            ))),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RepoPullStats {
    pub command: &'static str,
    pub repo_root: String,
    pub remote_repo_id: String,
    pub repo_manifest_downloaded: bool,
    pub chunk_map_downloaded: bool,
    pub snapshot_downloaded_count: usize,
    pub snapshot_skipped_count: usize,
    pub bundle_downloaded_count: usize,
    pub bundle_skipped_count: usize,
    pub kept_newer_local_count: usize,
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
    pub kept_newer_local_snapshots: Vec<String>,
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
pub struct RepoPushStats {
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
