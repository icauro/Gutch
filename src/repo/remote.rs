//! Provider-neutral remote repo planning and synchronization.
//!
//! A caller supplies a remote repo provider for control-plane lookup and
//! artifact transfer. Gutch owns the comparison rules, local repo
//! mutations, progress, cancellation, and synchronization order.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::manifest::SnapshotTagSet;
use crate::repo::{
    GutchRepoPaths, RepoCheckStatus, load_chunk_index, load_gutch_repo_manifest,
    load_snapshot_manifest, required_bundle_ids, summarize_snapshot_tags,
    write_gutch_repo_manifest,
};
use crate::{
    Error, GutchOperation, GutchOperationControl, GutchProgressEvent, GutchProgressPhase,
    GutchProgressSink, Result, advisory_tag_references,
};

pub(crate) const MAX_REMOTE_JSON_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_REMOTE_BINARY_BYTES: u64 = 256 * 1024 * 1024;

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

/// Provider-neutral result listing tags referenced by the remote access ruleset.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteTagRequirementsResult {
    pub repo_root: String,
    pub remote_repo_id: String,
    pub referenced_tags: Vec<crate::TagReferenceStatus>,
    pub ready: bool,
}

/// HostApp or another host application supplies this context to Gutch's remote CLI.
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
    ) -> Result<Vec<crate::TagReference>>;
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
pub struct RepoSyncSnapshotChange {
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
pub struct RepoSyncPlan {
    pub repo_root: String,
    pub remote_repo_id: String,
    pub repo_manifest_changed: bool,
    pub chunk_index_changed: bool,
    pub snapshot_count: usize,
    pub remote_snapshot_count: usize,
    pub to_add: Vec<RepoSyncSnapshotChange>,
    pub to_remove: Vec<RepoSyncSnapshotChange>,
    pub to_update: Vec<RepoSyncSnapshotChange>,
    pub unchanged: Vec<RepoSyncSnapshotChange>,
}

#[derive(Debug, Serialize)]
pub struct RepoSyncUpStats {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtifactDecision {
    Identical,
    Fetch,
    KeepLocalNewer,
}

#[derive(Debug)]
struct LocalSyncSnapshot {
    snapshot_id: String,
    version: String,
    os: String,
    arch: String,
    tags: Vec<String>,
    snapshot_metadata_hash_b3: String,
    snapshot_product_metadata_hash_b3: String,
    snapshot_blueprint_hash_b3: String,
}

fn repo_file_hash(bytes: &[u8]) -> String {
    crate::hash::hash_hex(&blake3::hash(bytes))
}

fn verify_remote_artifact(bytes: &[u8], expected_hash: &str, label: &str) -> Result<()> {
    let expected_hash = expected_hash.trim();
    if expected_hash.is_empty() {
        return Ok(());
    }
    let actual_hash = repo_file_hash(bytes);
    if actual_hash != expected_hash {
        return Err(Error::HashMismatch {
            expected: format!("{label} {expected_hash}"),
            actual: format!("{label} {actual_hash}"),
        });
    }
    Ok(())
}

fn verify_remote_bundle(bundle_id: &str, bytes: &[u8]) -> Result<()> {
    let expected_hash = crate::ChunkHash::parse_hex(bundle_id)?;
    if bytes.len() < 64 {
        return Err(Error::InvalidBundle(format!(
            "bundle '{}' is missing its content hash",
            bundle_id
        )));
    }
    let actual_hash = repo_file_hash(&bytes[..bytes.len() - 64]);
    if actual_hash != expected_hash.to_hex() {
        return Err(Error::HashMismatch {
            expected: expected_hash.to_hex(),
            actual: actual_hash,
        });
    }
    let expected_hex = expected_hash.to_hex();
    if &bytes[bytes.len() - 64..] != expected_hex.as_bytes() {
        return Err(Error::InvalidBundle(format!(
            "bundle '{}' has an inconsistent trailing content hash",
            bundle_id
        )));
    }
    Ok(())
}

fn hashes_differ(local_hash: &str, remote_hash: &str) -> bool {
    let local_hash = local_hash.trim();
    let remote_hash = remote_hash.trim();
    if local_hash.is_empty() && remote_hash.is_empty() {
        return false;
    }
    remote_hash.is_empty() || local_hash != remote_hash
}

fn local_file_mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn remote_updated_at_is_newer_or_equal(
    remote_updated_at: &str,
    local_modified: Option<std::time::SystemTime>,
) -> bool {
    let Ok(remote) = chrono::DateTime::parse_from_rfc3339(remote_updated_at) else {
        return true;
    };
    let remote_utc = remote.with_timezone(&chrono::Utc);
    let Some(local_modified) = local_modified else {
        return true;
    };
    std::time::SystemTime::from(remote_utc) >= local_modified
}

fn decide_artifact(
    local_exists: bool,
    local_hash: String,
    remote_hash: &str,
    remote_updated_at: &str,
    local_modified: Option<std::time::SystemTime>,
    policy: PullConflictPolicy,
) -> ArtifactDecision {
    if !local_exists || hashes_differ(&local_hash, remote_hash) {
        match policy {
            PullConflictPolicy::KeepLocal => {
                if local_exists {
                    ArtifactDecision::KeepLocalNewer
                } else {
                    ArtifactDecision::Fetch
                }
            }
            PullConflictPolicy::KeepRemote => ArtifactDecision::Fetch,
            PullConflictPolicy::KeepNewer => {
                if remote_updated_at_is_newer_or_equal(remote_updated_at, local_modified) {
                    ArtifactDecision::Fetch
                } else {
                    ArtifactDecision::KeepLocalNewer
                }
            }
        }
    } else {
        ArtifactDecision::Identical
    }
}

fn local_repo_metadata_hash_b3(repo_paths: &GutchRepoPaths) -> Result<String> {
    if !repo_paths.repo_metadata_path.exists() {
        return Ok(String::new());
    }
    Ok(repo_file_hash(&crate::helpers::read(
        &repo_paths.repo_metadata_path,
    )?))
}

fn local_chunk_map_hash_b3(repo_paths: &GutchRepoPaths) -> Result<String> {
    if !repo_paths.chunk_map_path.exists() {
        return Ok(String::new());
    }
    let chunk_index = load_chunk_index(repo_paths)?;
    Ok(repo_file_hash(&serde_json::to_vec_pretty(&chunk_index)?))
}

fn local_snapshot_metadata_hash_b3(
    repo_paths: &GutchRepoPaths,
    snapshot_id: &str,
) -> Result<String> {
    let metadata_path = crate::repo::snapshot_metadata_path(repo_paths, snapshot_id)?;
    let blueprint_path = crate::repo::snapshot_blueprint_path(repo_paths, snapshot_id)?;
    if !metadata_path.exists() || !blueprint_path.exists() {
        return Ok(String::new());
    }
    let manifest = load_snapshot_manifest(repo_paths, snapshot_id)?;
    Ok(repo_file_hash(
        &crate::serialize_snapshot_metadata_for_remote(&manifest)?,
    ))
}

fn local_snapshot_product_metadata_hash_b3(
    repo_paths: &GutchRepoPaths,
    snapshot_id: &str,
) -> Result<String> {
    let metadata_path = crate::repo::snapshot_metadata_path(repo_paths, snapshot_id)?;
    let blueprint_path = crate::repo::snapshot_blueprint_path(repo_paths, snapshot_id)?;
    if !metadata_path.exists() || !blueprint_path.exists() {
        return Ok(String::new());
    }
    let manifest = load_snapshot_manifest(repo_paths, snapshot_id)?;
    let Some(bytes) = crate::serialize_snapshot_product_metadata_for_remote(&manifest)? else {
        return Ok(String::new());
    };
    Ok(repo_file_hash(&bytes))
}

fn local_snapshot_blueprint_hash_b3(
    repo_paths: &GutchRepoPaths,
    snapshot_id: &str,
) -> Result<String> {
    let path = crate::repo::snapshot_blueprint_path(repo_paths, snapshot_id)?;
    if !path.exists() {
        return Ok(String::new());
    }
    Ok(crate::hash::hash_hex(&crate::hash::hash_file(&path)?))
}

fn compute_pull_plan(
    repo_root: &Path,
    repo_paths: &GutchRepoPaths,
    repository: &RemoteRepository,
    snapshots: &[RemoteSnapshot],
    policy: PullConflictPolicy,
) -> Result<RepoPullPlan> {
    let repo_manifest_needed = decide_artifact(
        repo_paths.repo_metadata_path.exists(),
        local_repo_metadata_hash_b3(repo_paths)?,
        &repository.metadata_hash_b3,
        &repository.updated_at,
        local_file_mtime(&repo_paths.repo_metadata_path),
        policy,
    ) != ArtifactDecision::Identical;
    let chunk_map_needed = decide_artifact(
        repo_paths.chunk_map_path.exists(),
        local_chunk_map_hash_b3(repo_paths)?,
        &repository.chunk_map_hash_b3,
        &repository.updated_at,
        local_file_mtime(&repo_paths.chunk_map_path),
        policy,
    ) != ArtifactDecision::Identical;

    let mut snapshots_to_download = Vec::new();
    let mut snapshots_to_skip = Vec::new();
    let mut kept_newer_local_snapshots = Vec::new();
    for snapshot in snapshots {
        let updated_at = if snapshot.uploaded_at.is_empty() {
            &snapshot.created_at
        } else {
            &snapshot.uploaded_at
        };
        let metadata_path = crate::repo::snapshot_metadata_path(repo_paths, &snapshot.snapshot_id)?;
        let mut decision = decide_artifact(
            metadata_path.exists(),
            local_snapshot_metadata_hash_b3(repo_paths, &snapshot.snapshot_id)?,
            &snapshot.metadata_hash_b3,
            updated_at,
            local_file_mtime(&metadata_path),
            policy,
        );
        if decision == ArtifactDecision::Identical
            && hashes_differ(
                &local_snapshot_blueprint_hash_b3(repo_paths, &snapshot.snapshot_id)?,
                &snapshot.blueprint_hash_b3,
            )
        {
            decision = ArtifactDecision::Fetch;
        }
        let product_metadata_path =
            crate::repo::snapshot_product_metadata_path(repo_paths, &snapshot.snapshot_id)?;
        let product_metadata_decision = decide_artifact(
            product_metadata_path.exists(),
            local_snapshot_product_metadata_hash_b3(repo_paths, &snapshot.snapshot_id)?,
            &snapshot.product_metadata_hash_b3,
            updated_at,
            local_file_mtime(&product_metadata_path),
            policy,
        );
        decision = match (decision, product_metadata_decision) {
            (ArtifactDecision::Fetch, _) | (_, ArtifactDecision::Fetch) => ArtifactDecision::Fetch,
            (ArtifactDecision::KeepLocalNewer, _) | (_, ArtifactDecision::KeepLocalNewer) => {
                ArtifactDecision::KeepLocalNewer
            }
            _ => ArtifactDecision::Identical,
        };
        match decision {
            ArtifactDecision::Identical => snapshots_to_skip.push(snapshot.snapshot_id.clone()),
            ArtifactDecision::Fetch => snapshots_to_download.push(snapshot.snapshot_id.clone()),
            ArtifactDecision::KeepLocalNewer => {
                kept_newer_local_snapshots.push(snapshot.snapshot_id.clone());
                snapshots_to_skip.push(snapshot.snapshot_id.clone());
            }
        }
    }
    snapshots_to_download.sort();
    snapshots_to_skip.sort();
    kept_newer_local_snapshots.sort();

    let remote_snapshot_ids: HashSet<&str> = snapshots
        .iter()
        .map(|snapshot| snapshot.snapshot_id.as_str())
        .collect();
    let mut local_only_snapshots = Vec::new();
    if repo_paths.snapshots_root.exists() {
        for entry in walkdir::WalkDir::new(&repo_paths.snapshots_root).sort_by_file_name() {
            let entry = entry?;
            if !entry.path().is_file() || entry.file_name() != crate::repo::SNAPSHOT_METADATA_FILE {
                continue;
            }
            if let Some(snapshot_id) = entry
                .path()
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|value| value.to_str())
                && !remote_snapshot_ids.contains(snapshot_id)
            {
                local_only_snapshots.push(snapshot_id.to_string());
            }
        }
    }
    local_only_snapshots.sort();

    Ok(RepoPullPlan {
        repo_root: repo_root.display().to_string(),
        remote_repo_id: repository.id.clone(),
        remote_repo_name: repository.name.clone(),
        repo_manifest_needed,
        chunk_map_needed,
        snapshots_to_download,
        snapshots_to_skip,
        local_only_snapshots,
        kept_newer_local_snapshots,
        bundle_count: 0,
    })
}

pub(crate) fn plan_pull(
    repo_root: &Path,
    repository: &RemoteRepository,
    snapshots: &[RemoteSnapshot],
    policy: PullConflictPolicy,
) -> Result<RepoPullPlan> {
    let repo_paths = GutchRepoPaths::from_repo_root(repo_root);
    compute_pull_plan(repo_root, &repo_paths, repository, snapshots, policy)
}

struct RemoteProgress {
    sink: Option<std::sync::Arc<dyn GutchProgressSink>>,
    operation: GutchOperation,
}

impl RemoteProgress {
    fn new(control: &GutchOperationControl, operation: GutchOperation) -> Self {
        Self {
            sink: control.progress().clone(),
            operation,
        }
    }

    fn emit(
        &self,
        phase: GutchProgressPhase,
        message: impl Into<String>,
        current: usize,
        total: usize,
        detail: Option<String>,
        done: bool,
    ) {
        crate::emit_progress(
            &self.sink,
            GutchProgressEvent {
                operation: self.operation,
                phase,
                message: message.into(),
                current,
                total,
                transferred_bytes: 0,
                total_bytes: 0,
                detail,
                done,
            },
        );
    }
}

fn source_bytes(
    source: &RemoteArtifactSource,
    client: &reqwest::blocking::Client,
    maximum_bytes: u64,
    control: &GutchOperationControl,
) -> Result<Vec<u8>> {
    control.checkpoint()?;
    match source {
        RemoteArtifactSource::Bytes(bytes) => {
            if bytes.len() as u64 > maximum_bytes {
                return Err(Error::manifest("remote artifact exceeded the allowed size"));
            }
            Ok(bytes.clone())
        }
        RemoteArtifactSource::Url(url) => {
            let mut bytes = Vec::new();
            crate::transport::download_url_streaming(
                client,
                url,
                control,
                Some(maximum_bytes),
                |_| Ok(()),
                |chunk| {
                    bytes.extend_from_slice(chunk);
                    Ok(())
                },
            )?;
            Ok(bytes)
        }
    }
}

fn write_remote_snapshot_artifacts(
    repo_paths: &GutchRepoPaths,
    snapshot_id: &str,
    metadata: &crate::manifest::snapshot::SnapshotManifest,
    product_metadata: Option<&crate::manifest::SnapshotProductMetadata>,
    blueprint_bytes: &[u8],
) -> Result<()> {
    crate::repo::validate_path_component(snapshot_id, "snapshot ID")?;
    let staging_dir = tempfile::Builder::new()
        .prefix(".gutch-snapshot-")
        .tempdir_in(&repo_paths.snapshots_root)?;
    let staging_path = staging_dir.path().to_path_buf();
    crate::helpers::write(
        &staging_path.join(crate::repo::SNAPSHOT_METADATA_FILE),
        &serde_json::to_vec_pretty(&metadata.metadata())?,
    )?;
    if let Some(product_metadata) = product_metadata {
        product_metadata.validate_structure()?;
        crate::helpers::write(
            &staging_path.join(crate::repo::SNAPSHOT_PRODUCT_METADATA_FILE),
            &serde_json::to_vec_pretty(product_metadata)?,
        )?;
    }
    let blueprint_path = staging_path.join(crate::repo::SNAPSHOT_BLUEPRINT_FILE);
    crate::helpers::write(&blueprint_path, blueprint_bytes)?;
    crate::manifest::binary::read_snapshot_blueprint_binary(&blueprint_path)?;

    let staging_path = staging_dir.keep();
    let destination_dir = repo_paths.snapshots_root.join(snapshot_id);
    crate::helpers::replace_directory(&staging_path, &destination_dir)
}

pub(crate) fn linked_remote_repo_id(repo_root: &Path) -> Result<String> {
    let (_paths, manifest) = load_gutch_repo_manifest(repo_root)?;
    if !manifest.is_linked() {
        return Err(Error::manifest(format!(
            "repo '{}' is not linked. Use 'gutch repo link' first.",
            repo_root.display()
        )));
    }
    if manifest.remote_repo_id.trim().is_empty() {
        return Err(Error::manifest("linked repo is missing remote_repo_id"));
    }
    Ok(manifest.remote_repo_id)
}

pub fn bind_repo_remote(repo_root: &Path, remote_repo_id: &str) -> Result<()> {
    let remote_repo_id = remote_repo_id.trim();
    if remote_repo_id.is_empty() {
        return Err(Error::manifest("remote repo ID is required"));
    }

    let (paths, mut manifest) = load_gutch_repo_manifest(repo_root)?;
    manifest.remote_repo_id = remote_repo_id.to_string();
    write_gutch_repo_manifest(&paths, &manifest)
}

pub(crate) fn list_remote_repositories(
    provider: &dyn RemoteRepositoryCommandProvider,
) -> Result<Vec<RemoteRepositorySummary>> {
    provider.list_remote_repositories()
}

pub fn link_repo(
    repo_root: &Path,
    scope_id: &str,
    provider: &dyn RemoteRepositoryCommandProvider,
) -> Result<()> {
    let (paths, mut manifest) = load_gutch_repo_manifest(repo_root)?;
    if manifest.is_linked() {
        return Err(Error::manifest(format!(
            "repo '{}' is already linked (remote_repo_id={}). Unlink first.",
            paths.repo_root().display(),
            manifest.remote_repo_id
        )));
    }

    let scope_id = scope_id.trim();
    if scope_id.is_empty() {
        return Err(Error::manifest("remote repo scope is required"));
    }
    let remote_repo_id = provider.create_remote_repository(scope_id, &manifest.name)?;
    if remote_repo_id.trim().is_empty() {
        return Err(Error::manifest("remote provider returned an empty repo ID"));
    }
    manifest.remote_repo_id = remote_repo_id;
    write_gutch_repo_manifest(&paths, &manifest)?;
    Ok(())
}

pub fn unlink_repo(
    repo_root: &Path,
    delete_remote: bool,
    provider: Option<&dyn RemoteRepositoryCommandProvider>,
) -> Result<()> {
    let (paths, mut manifest) = load_gutch_repo_manifest(repo_root)?;
    if !manifest.is_linked() {
        return Ok(());
    }

    if delete_remote {
        let remote_repo_id = manifest.remote_repo_id.trim();
        if remote_repo_id.is_empty() {
            return Err(Error::manifest(format!(
                "invalid remote_repo_id '{}' in manifest",
                manifest.remote_repo_id
            )));
        }
        let remote_repo_id = remote_repo_id.to_string();
        let provider = provider.ok_or_else(|| {
            Error::manifest("remote provider context is required when deleting the remote repo")
        })?;
        let repository = provider
            .repository_provider()
            .resolve_repository(None, Some(&remote_repo_id))?;
        provider.delete_remote_repository(&repository)?;
    }

    manifest.remote_repo_id.clear();
    write_gutch_repo_manifest(&paths, &manifest)?;
    Ok(())
}

pub(crate) fn inspect_repo_tag_requirements(
    repo_root: &Path,
    provider: &dyn RemoteRepositoryCommandProvider,
) -> Result<RemoteTagRequirementsResult> {
    let (repo_paths, repo_manifest) = load_gutch_repo_manifest(repo_root)?;
    let remote_repo_id = repo_manifest.remote_repo_id.trim();
    if remote_repo_id.is_empty() {
        return Err(Error::manifest(format!(
            "repo '{}' is not linked. Use 'gutch repo link' first.",
            repo_paths.repo_root().display()
        )));
    }
    let remote_repo_id = remote_repo_id.to_string();
    let repository = provider
        .repository_provider()
        .resolve_repository(None, Some(&remote_repo_id))?;
    let requirements = provider.referenced_tag_references(&repository)?;
    let referenced_tags = advisory_tag_references(&requirements);

    Ok(RemoteTagRequirementsResult {
        repo_root: repo_paths.repo_root().display().to_string(),
        remote_repo_id,
        ready: true,
        referenced_tags,
    })
}

fn bound_remote_repo_id(repo_root: &Path) -> Result<Option<String>> {
    let repo_paths = GutchRepoPaths::from_repo_root(repo_root);
    if !repo_paths.repo_metadata_path.exists() {
        return Ok(None);
    }
    let (_paths, manifest) = load_gutch_repo_manifest(repo_root)?;
    Ok(manifest.is_linked().then_some(manifest.remote_repo_id))
}

pub fn plan_pull_with_provider<P: RemoteRepositoryReader + ?Sized>(
    repo_root: &Path,
    provider: &P,
    remote_spec: Option<&str>,
    policy: PullConflictPolicy,
) -> Result<RepoPullPlan> {
    let bound_remote_repo_id = bound_remote_repo_id(repo_root)?;
    let repository = provider.resolve_repository(remote_spec, bound_remote_repo_id.as_deref())?;
    let snapshots = provider.list_snapshots(&repository)?;
    plan_pull(repo_root, &repository, &snapshots, policy)
}

pub fn plan_sync_up_with_provider<P: RemoteRepositoryReader + ?Sized>(
    repo_root: &Path,
    provider: &P,
) -> Result<RepoSyncPlan> {
    let bound_remote_repo_id = linked_remote_repo_id(repo_root)?;
    let repository = provider.resolve_repository(None, Some(&bound_remote_repo_id))?;
    let snapshots = provider.list_snapshots(&repository)?;
    plan_sync_up(repo_root, &repository, &snapshots)
}

fn required_bundle_ids_by_snapshot(
    repo_paths: &GutchRepoPaths,
) -> Result<Vec<(String, Vec<String>)>> {
    let chunk_index = load_chunk_index(repo_paths)?;
    let mut by_snapshot = Vec::new();
    if !repo_paths.snapshots_root.exists() {
        return Ok(by_snapshot);
    }
    for entry in walkdir::WalkDir::new(&repo_paths.snapshots_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() || entry.file_name() != crate::repo::SNAPSHOT_METADATA_FILE {
            continue;
        }
        let Some(snapshot_id) = entry
            .path()
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|value| value.to_str())
        else {
            continue;
        };
        let manifest = load_snapshot_manifest(repo_paths, snapshot_id)?;
        let mut ids: Vec<String> = required_bundle_ids(&manifest, &chunk_index)
            .into_iter()
            .collect();
        ids.sort();
        by_snapshot.push((snapshot_id.to_string(), ids));
    }
    Ok(by_snapshot)
}

fn local_bundle_paths(bundles_root: &Path) -> Result<Vec<PathBuf>> {
    if !bundles_root.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in walkdir::WalkDir::new(bundles_root).sort_by_file_name() {
        let entry = entry?;
        if entry.path().is_file() && crate::bundle_id_from_bundle_path(entry.path()).is_some() {
            paths.push(entry.path().to_path_buf());
        }
    }
    Ok(paths)
}

fn record_bundle_sources(
    bundle_sources: &mut HashMap<String, RemoteArtifactSource>,
    expected_bundle_ids: &HashSet<&str>,
    sources: Vec<RemoteBundleSource>,
) -> Result<()> {
    for source in sources {
        if !expected_bundle_ids.contains(source.bundle_id.as_str()) {
            return Err(Error::manifest(format!(
                "remote provider returned an unexpected bundle '{}'",
                source.bundle_id
            )));
        }
        if bundle_sources
            .insert(source.bundle_id.clone(), source.source)
            .is_some()
        {
            return Err(Error::manifest(format!(
                "remote provider returned duplicate bundle '{}'",
                source.bundle_id
            )));
        }
    }
    Ok(())
}

fn bundle_source_requests(
    bundles_by_snapshot: &[(String, Vec<String>)],
    missing_bundles: &[String],
) -> Vec<(String, Vec<String>)> {
    let mut unresolved: HashSet<&str> = missing_bundles.iter().map(String::as_str).collect();
    let mut requests = Vec::new();
    for (snapshot_id, bundle_ids) in bundles_by_snapshot {
        let missing_for_snapshot: Vec<String> = bundle_ids
            .iter()
            .filter(|bundle_id| unresolved.contains(bundle_id.as_str()))
            .cloned()
            .collect();
        if missing_for_snapshot.is_empty() {
            continue;
        }
        for bundle_id in &missing_for_snapshot {
            unresolved.remove(bundle_id.as_str());
        }
        requests.push((snapshot_id.clone(), missing_for_snapshot));
    }
    requests
}

pub fn pull_repo<P: RemoteRepositoryReader + ?Sized>(
    repo: PathBuf,
    provider: &P,
    remote_spec: Option<String>,
    policy: PullConflictPolicy,
    prune: bool,
    control: GutchOperationControl,
) -> Result<RepoPullStats> {
    control.checkpoint()?;
    let progress = RemoteProgress::new(&control, GutchOperation::Pull);
    let bound_remote_repo_id = bound_remote_repo_id(&repo)?;
    let repository =
        provider.resolve_repository(remote_spec.as_deref(), bound_remote_repo_id.as_deref())?;
    let repo_paths = GutchRepoPaths::from_repo_root(&repo);
    crate::ensure_gutch_repo_layout(&repo_paths, &repository.name)?;
    progress.emit(
        GutchProgressPhase::Planning,
        "Planning pull changes",
        0,
        1,
        None,
        false,
    );
    let snapshots = provider.list_snapshots(&repository)?;
    let plan = compute_pull_plan(&repo, &repo_paths, &repository, &snapshots, policy)?;
    control.checkpoint()?;
    let download_client = crate::transport::build_download_client()?;

    let mut stats = RepoPullStats {
        command: "repo pull",
        repo_root: repo_paths.repo_root.display().to_string(),
        remote_repo_id: repository.id.clone(),
        repo_manifest_downloaded: false,
        chunk_map_downloaded: false,
        snapshot_downloaded_count: 0,
        snapshot_skipped_count: plan.snapshots_to_skip.len(),
        bundle_downloaded_count: 0,
        bundle_skipped_count: 0,
        kept_newer_local_count: plan.kept_newer_local_snapshots.len(),
        local_only_snapshots_kept: 0,
        pruned_snapshot_count: 0,
        pruned_bundle_count: 0,
    };
    let total_steps = 1
        + usize::from(plan.repo_manifest_needed)
        + usize::from(plan.chunk_map_needed)
        + plan.snapshots_to_download.len()
        + 2;
    let mut step = 0usize;

    if plan.repo_manifest_needed {
        control.checkpoint()?;
        step += 1;
        progress.emit(
            GutchProgressPhase::RepoMetadata,
            "Downloading repo metadata",
            step,
            total_steps,
            None,
            false,
        );
        let bytes = source_bytes(
            &provider.repository_metadata_source(&repository)?,
            &download_client,
            MAX_REMOTE_JSON_BYTES,
            &control,
        )?;
        verify_remote_artifact(&bytes, &repository.metadata_hash_b3, "repo metadata")?;
        let mut manifest: crate::manifest::gutch_repo::GutchRepoManifest =
            crate::helpers::from_json_bytes(&bytes)?;
        manifest.remote_repo_id = repository.id.clone();
        crate::write_gutch_repo_manifest(&repo_paths, &manifest)?;
        stats.repo_manifest_downloaded = true;
    }

    if plan.chunk_map_needed {
        control.checkpoint()?;
        step += 1;
        progress.emit(
            GutchProgressPhase::ChunkIndex,
            "Downloading chunk map",
            step,
            total_steps,
            None,
            false,
        );
        let bytes = source_bytes(
            &provider.chunk_index_source(&repository)?,
            &download_client,
            MAX_REMOTE_JSON_BYTES,
            &control,
        )?;
        verify_remote_artifact(&bytes, &repository.chunk_map_hash_b3, "chunk index")?;
        let chunk_index: crate::manifest::ChunkIndex = crate::helpers::from_json_bytes(&bytes)
            .map_err(|error| {
                Error::manifest(format!("downloaded chunk map is invalid JSON: {error}"))
            })?;
        crate::repo::write_chunk_index(&repo_paths, &chunk_index)?;
        stats.chunk_map_downloaded = true;
    }

    let snapshot_start = step;
    for snapshot_id in &plan.snapshots_to_download {
        control.checkpoint()?;
        step += 1;
        progress.emit(
            GutchProgressPhase::SnapshotMetadata,
            format!(
                "Downloading snapshot metadata ({}/{})",
                step - snapshot_start,
                plan.snapshots_to_download.len().max(1)
            ),
            step,
            total_steps,
            Some(snapshot_id.clone()),
            false,
        );
        let snapshot = snapshots
            .iter()
            .find(|snapshot| snapshot.snapshot_id == *snapshot_id)
            .ok_or_else(|| {
                Error::manifest(format!(
                    "remote snapshot '{}' disappeared during pull",
                    snapshot_id
                ))
            })?;
        let sources = provider.snapshot_sources(&repository, snapshot)?;
        let metadata_bytes = source_bytes(
            &sources.metadata,
            &download_client,
            MAX_REMOTE_JSON_BYTES,
            &control,
        )?;
        verify_remote_artifact(
            &metadata_bytes,
            &snapshot.metadata_hash_b3,
            "snapshot metadata",
        )?;
        let metadata = crate::snapshot_manifest_from_download(snapshot_id, &metadata_bytes)?;
        let product_metadata = if let Some(source) = sources.product_metadata {
            let product_metadata_bytes =
                source_bytes(&source, &download_client, MAX_REMOTE_JSON_BYTES, &control)?;
            verify_remote_artifact(
                &product_metadata_bytes,
                &snapshot.product_metadata_hash_b3,
                "snapshot product metadata",
            )?;
            let product_metadata: crate::manifest::SnapshotProductMetadata =
                crate::helpers::from_json_bytes(&product_metadata_bytes)?;
            product_metadata.validate_structure()?;
            Some(product_metadata)
        } else {
            None
        };
        let blueprint_bytes = source_bytes(
            &sources.blueprint,
            &download_client,
            MAX_REMOTE_BINARY_BYTES,
            &control,
        )?;
        verify_remote_artifact(
            &blueprint_bytes,
            &snapshot.blueprint_hash_b3,
            "snapshot blueprint",
        )?;
        write_remote_snapshot_artifacts(
            &repo_paths,
            snapshot_id,
            &metadata,
            product_metadata.as_ref(),
            &blueprint_bytes,
        )?;
        stats.snapshot_downloaded_count += 1;
    }

    step += 1;
    progress.emit(
        GutchProgressPhase::Bundle,
        "Resolving bundles",
        step,
        total_steps,
        None,
        false,
    );
    let bundles_by_snapshot = required_bundle_ids_by_snapshot(&repo_paths)?;
    let required_set: HashSet<String> = bundles_by_snapshot
        .iter()
        .flat_map(|(_, ids)| ids.iter())
        .cloned()
        .collect();
    let mut missing_bundles = Vec::new();
    for id in &required_set {
        if !crate::bundle_path(&repo_paths, id)?.is_file() {
            missing_bundles.push(id.clone());
        }
    }
    missing_bundles.sort();
    let mut bundle_sources = HashMap::new();
    for (snapshot_id, missing_for_snapshot) in
        bundle_source_requests(&bundles_by_snapshot, &missing_bundles)
    {
        let snapshot = snapshots
            .iter()
            .find(|snapshot| snapshot.snapshot_id == snapshot_id)
            .ok_or_else(|| {
                Error::manifest(format!(
                    "remote snapshot '{}' disappeared during bundle resolution",
                    snapshot_id
                ))
            })?;
        let expected_for_request: HashSet<&str> =
            missing_for_snapshot.iter().map(String::as_str).collect();
        record_bundle_sources(
            &mut bundle_sources,
            &expected_for_request,
            provider.bundle_sources(&repository, snapshot, &missing_for_snapshot)?,
        )?;
    }
    let mut bundle_downloaded = 0usize;
    for bundle_id in &missing_bundles {
        control.checkpoint()?;
        let source = bundle_sources.get(bundle_id).ok_or_else(|| {
            Error::manifest(format!(
                "remote provider did not return a source for required bundle '{}'",
                bundle_id
            ))
        })?;
        progress.emit(
            GutchProgressPhase::Bundle,
            format!(
                "Downloading bundles ({}/{})",
                bundle_downloaded + 1,
                missing_bundles.len().max(1)
            ),
            step,
            total_steps,
            Some(bundle_id.clone()),
            false,
        );
        let bytes = source_bytes(source, &download_client, MAX_REMOTE_BINARY_BYTES, &control)?;
        verify_remote_bundle(bundle_id, &bytes)?;
        let bundle_path = crate::bundle_path(&repo_paths, bundle_id)?;
        crate::helpers::write(&bundle_path, &bytes)?;
        bundle_downloaded += 1;
    }
    stats.bundle_downloaded_count = bundle_downloaded;
    stats.bundle_skipped_count = required_set.len().saturating_sub(bundle_downloaded);

    if prune {
        control.checkpoint()?;
        step += 1;
        progress.emit(
            GutchProgressPhase::Prune,
            "Removing local-only content",
            step,
            total_steps,
            None,
            false,
        );
        for snapshot_id in &plan.local_only_snapshots {
            control.checkpoint()?;
            crate::repo::remove_snapshot_from_repo(&repo, snapshot_id)?;
            stats.pruned_snapshot_count += 1;
        }
        for path in local_bundle_paths(&repo_paths.bundles_root)? {
            control.checkpoint()?;
            if let Some(bundle_id) = crate::bundle_id_from_bundle_path(&path)
                && !required_set.contains(&bundle_id)
            {
                std::fs::remove_file(&path)?;
                stats.pruned_bundle_count += 1;
            }
        }
    } else {
        stats.local_only_snapshots_kept = plan.local_only_snapshots.len();
    }

    progress.emit(
        GutchProgressPhase::Done,
        "Pull complete",
        total_steps,
        total_steps,
        None,
        true,
    );
    Ok(stats)
}

fn collect_local_sync_snapshots(
    repo_root: &Path,
) -> Result<(RepoCheckStatus, GutchRepoPaths, Vec<LocalSyncSnapshot>)> {
    let status = crate::check_gutch_repo(repo_root)?;
    if !status.is_repo {
        return Err(Error::manifest(format!(
            "no gutch repo found at {}",
            repo_root.display()
        )));
    }
    let (repo_paths, _) = load_gutch_repo_manifest(repo_root)?;
    let mut local_snapshots = Vec::with_capacity(status.snapshots.len());
    for snapshot in &status.snapshots {
        let manifest = load_snapshot_manifest(&repo_paths, &snapshot.snapshot_id)?;
        let summary = summarize_snapshot_tags(&manifest.snapshot_tags);
        local_snapshots.push(LocalSyncSnapshot {
            snapshot_id: manifest.snapshot_id.clone(),
            version: summary.version_id,
            os: summary.os,
            arch: summary.arch,
            tags: summary.canonical_tags,
            snapshot_metadata_hash_b3: local_snapshot_metadata_hash_b3(
                &repo_paths,
                &manifest.snapshot_id,
            )?,
            snapshot_product_metadata_hash_b3: local_snapshot_product_metadata_hash_b3(
                &repo_paths,
                &manifest.snapshot_id,
            )?,
            snapshot_blueprint_hash_b3: local_snapshot_blueprint_hash_b3(
                &repo_paths,
                &manifest.snapshot_id,
            )?,
        });
    }
    Ok((status, repo_paths, local_snapshots))
}

fn sync_snapshot_change(
    snapshot_id: &str,
    version: String,
    os: String,
    arch: String,
    tags: Vec<String>,
    artifact_state: Option<String>,
    update_reason: Option<String>,
) -> RepoSyncSnapshotChange {
    RepoSyncSnapshotChange {
        snapshot_id: snapshot_id.to_string(),
        version,
        os,
        arch,
        artifact_state,
        tags,
        update_reason,
    }
}

fn remote_snapshot_change(snapshot: &RemoteSnapshot) -> RepoSyncSnapshotChange {
    let summary = summarize_snapshot_tags_from_values(&snapshot.tags);
    sync_snapshot_change(
        &snapshot.snapshot_id,
        summary.0,
        summary.1,
        summary.2,
        snapshot.tags.clone(),
        snapshot.artifact_state.clone(),
        None,
    )
}

fn summarize_snapshot_tags_from_values(tags: &[String]) -> (String, String, String) {
    let mut tag_set = SnapshotTagSet::default();
    for tag in tags {
        if let Some((key, value)) = tag.split_once("::") {
            tag_set.exclusive.insert(key.to_string(), value.to_string());
        } else if let Some((key, value)) = tag.split_once(':') {
            tag_set
                .scoped
                .entry(key.to_string())
                .or_default()
                .insert(value.to_string());
        }
    }
    let summary = summarize_snapshot_tags(&tag_set);
    (summary.version_id, summary.os, summary.arch)
}

pub(crate) fn plan_sync_up(
    repo_root: &Path,
    repository: &RemoteRepository,
    remote_snapshots: &[RemoteSnapshot],
) -> Result<RepoSyncPlan> {
    let (status, repo_paths, local_snapshots) = collect_local_sync_snapshots(repo_root)?;
    let repo_manifest_changed = hashes_differ(
        &local_repo_metadata_hash_b3(&repo_paths)?,
        &repository.metadata_hash_b3,
    );
    let chunk_index_changed = hashes_differ(
        &local_chunk_map_hash_b3(&repo_paths)?,
        &repository.chunk_map_hash_b3,
    );
    let remote_by_snapshot_id: HashMap<&str, &RemoteSnapshot> = remote_snapshots
        .iter()
        .map(|snapshot| (snapshot.snapshot_id.as_str(), snapshot))
        .collect();
    let mut to_add = Vec::new();
    let mut to_update = Vec::new();
    let mut unchanged = Vec::new();
    for local in &local_snapshots {
        match remote_by_snapshot_id.get(local.snapshot_id.as_str()) {
            None => to_add.push(sync_snapshot_change(
                &local.snapshot_id,
                local.version.clone(),
                local.os.clone(),
                local.arch.clone(),
                local.tags.clone(),
                None,
                None,
            )),
            Some(remote) => {
                let tag_mismatch = local.tags != remote.tags;
                let metadata_hash_changed =
                    hashes_differ(&local.snapshot_metadata_hash_b3, &remote.metadata_hash_b3);
                let blueprint_hash_changed =
                    hashes_differ(&local.snapshot_blueprint_hash_b3, &remote.blueprint_hash_b3);
                let product_metadata_hash_changed = hashes_differ(
                    &local.snapshot_product_metadata_hash_b3,
                    &remote.product_metadata_hash_b3,
                );
                if tag_mismatch
                    || !remote.artifact_ready
                    || metadata_hash_changed
                    || blueprint_hash_changed
                    || product_metadata_hash_changed
                {
                    let mut reasons = Vec::new();
                    if tag_mismatch {
                        reasons.push("snapshot tags differ from remote");
                    }
                    if !remote.artifact_ready {
                        reasons.push("remote artifact is not ready");
                    }
                    if metadata_hash_changed {
                        reasons.push("snapshot metadata hash differs");
                    }
                    if blueprint_hash_changed {
                        reasons.push("snapshot blueprint hash differs");
                    }
                    if product_metadata_hash_changed {
                        reasons.push("snapshot product metadata hash differs");
                    }
                    to_update.push(sync_snapshot_change(
                        &local.snapshot_id,
                        local.version.clone(),
                        local.os.clone(),
                        local.arch.clone(),
                        local.tags.clone(),
                        remote.artifact_state.clone(),
                        Some(format!("{}.", reasons.join("; "))),
                    ));
                } else {
                    unchanged.push(sync_snapshot_change(
                        &local.snapshot_id,
                        local.version.clone(),
                        local.os.clone(),
                        local.arch.clone(),
                        local.tags.clone(),
                        remote.artifact_state.clone(),
                        None,
                    ));
                }
            }
        }
    }
    let local_ids: HashSet<&str> = local_snapshots
        .iter()
        .map(|snapshot| snapshot.snapshot_id.as_str())
        .collect();
    let mut to_remove: Vec<_> = remote_snapshots
        .iter()
        .filter(|snapshot| !local_ids.contains(snapshot.snapshot_id.as_str()))
        .map(remote_snapshot_change)
        .collect();
    to_add.sort_by(|left, right| left.snapshot_id.cmp(&right.snapshot_id));
    to_update.sort_by(|left, right| left.snapshot_id.cmp(&right.snapshot_id));
    unchanged.sort_by(|left, right| left.snapshot_id.cmp(&right.snapshot_id));
    to_remove.sort_by(|left, right| left.snapshot_id.cmp(&right.snapshot_id));
    Ok(RepoSyncPlan {
        repo_root: repo_paths.repo_root.display().to_string(),
        remote_repo_id: repository.id.clone(),
        repo_manifest_changed,
        chunk_index_changed,
        snapshot_count: status.snapshots.len(),
        remote_snapshot_count: remote_snapshots.len(),
        to_add,
        to_remove,
        to_update,
        unchanged,
    })
}

struct SnapshotUploadContext<'a, P: RemoteRepositoryWriter + ?Sized> {
    provider: &'a P,
    repository: &'a RemoteRepository,
    repo_paths: &'a GutchRepoPaths,
    control: &'a GutchOperationControl,
    stats: &'a mut RepoSyncUpStats,
    progress: &'a RemoteProgress,
    step: &'a mut usize,
    total_steps: usize,
}

fn write_snapshot_uploads<P: RemoteRepositoryWriter + ?Sized>(
    context: &mut SnapshotUploadContext<'_, P>,
    snapshot: &crate::RepoCheckSnapshotStatus,
    remote_snapshot: &RemoteSnapshot,
) -> Result<()> {
    context.control.checkpoint()?;
    *context.step += 1;
    context.progress.emit(
        GutchProgressPhase::UploadSnapshotMetadata,
        "Uploading snapshot metadata",
        *context.step,
        context.total_steps,
        Some(snapshot.snapshot_id.clone()),
        false,
    );
    let manifest = load_snapshot_manifest(context.repo_paths, &snapshot.snapshot_id)?;
    context.provider.upload_snapshot_metadata(
        context.repository,
        remote_snapshot,
        crate::serialize_snapshot_metadata_for_remote(&manifest)?,
        context.control,
    )?;
    context.stats.snapshot_manifest_uploaded_count += 1;

    context.control.checkpoint()?;
    *context.step += 1;
    context.progress.emit(
        GutchProgressPhase::UploadSnapshotBlueprint,
        "Uploading snapshot blueprints",
        *context.step,
        context.total_steps,
        Some(snapshot.snapshot_id.clone()),
        false,
    );
    let blueprint_path =
        crate::repo::snapshot_blueprint_path(context.repo_paths, &snapshot.snapshot_id)?;
    context.provider.upload_snapshot_blueprint(
        context.repository,
        remote_snapshot,
        crate::helpers::read(&blueprint_path)?,
        context.control,
    )?;
    context.stats.snapshot_blueprint_uploaded_count += 1;

    context.control.checkpoint()?;
    *context.step += 1;
    context.progress.emit(
        GutchProgressPhase::UploadSnapshotMetadata,
        "Uploading snapshot product metadata",
        *context.step,
        context.total_steps,
        Some(snapshot.snapshot_id.clone()),
        false,
    );
    if let Some(bytes) = crate::serialize_snapshot_product_metadata_for_remote(&manifest)? {
        context.provider.upload_snapshot_product_metadata(
            context.repository,
            remote_snapshot,
            bytes,
            context.control,
        )?;
        context.stats.product_metadata_uploaded_count += 1;
    } else if !remote_snapshot.product_metadata_hash_b3.trim().is_empty() {
        context
            .provider
            .delete_snapshot_product_metadata(context.repository, remote_snapshot)?;
    }
    Ok(())
}

pub fn sync_up_repo<P: RemoteRepositoryProvider + ?Sized>(
    repo: PathBuf,
    provider: &P,
    control: GutchOperationControl,
) -> Result<RepoSyncUpStats> {
    control.checkpoint()?;
    let progress = RemoteProgress::new(&control, GutchOperation::SyncUp);
    let bound_remote_repo_id = linked_remote_repo_id(&repo)?;
    let repository = provider.resolve_repository(None, Some(&bound_remote_repo_id))?;
    progress.emit(
        GutchProgressPhase::Planning,
        "Planning sync changes",
        0,
        1,
        None,
        false,
    );
    let remote_snapshots = provider.list_snapshots(&repository)?;
    let plan = plan_sync_up(&repo, &repository, &remote_snapshots)?;
    control.checkpoint()?;
    let (status, repo_paths, _) = collect_local_sync_snapshots(&repo)?;
    let bundle_paths = local_bundle_paths(&repo_paths.bundles_root)?;
    let required_bundle_ids = {
        let chunk_index = load_chunk_index(&repo_paths)?;
        let mut ids = BTreeSet::new();
        for snapshot in &status.snapshots {
            let manifest = load_snapshot_manifest(&repo_paths, &snapshot.snapshot_id)?;
            ids.extend(required_bundle_ids(&manifest, &chunk_index));
        }
        ids.into_iter().collect::<Vec<_>>()
    };
    let snapshot_upload_count = plan.to_add.len() + plan.to_update.len();
    let total_steps = plan.to_remove.len()
        + 1
        + usize::from(repo_paths.chunk_map_path.exists())
        + snapshot_upload_count * 3
        + bundle_paths.len()
        + 1;
    let mut step = 0usize;
    let mut stats = RepoSyncUpStats {
        command: "repo push",
        repo_root: repo_paths.repo_root.display().to_string(),
        remote_repo_id: repository.id.clone(),
        repo_manifest_uploaded: false,
        chunk_index_uploaded: false,
        snapshot_manifest_uploaded_count: 0,
        product_metadata_uploaded_count: 0,
        snapshot_blueprint_uploaded_count: 0,
        bundle_uploaded_count: 0,
        bundle_skipped_count: 0,
        remote_bundle_deleted_count: 0,
        snapshot_count: status.snapshots.len(),
        bundle_count: bundle_paths.len(),
        snapshot_added_count: plan.to_add.len(),
        snapshot_removed_count: plan.to_remove.len(),
        snapshot_updated_count: plan.to_update.len(),
        snapshot_unchanged_count: plan.unchanged.len(),
    };

    let mut remote_by_snapshot_id: HashMap<String, RemoteSnapshot> = remote_snapshots
        .into_iter()
        .map(|snapshot| (snapshot.snapshot_id.clone(), snapshot))
        .collect();
    for change in &plan.to_remove {
        control.checkpoint()?;
        step += 1;
        progress.emit(
            GutchProgressPhase::RemoveSnapshots,
            "Removing remote snapshots",
            step,
            total_steps,
            Some(change.snapshot_id.clone()),
            false,
        );
        if let Some(snapshot) = remote_by_snapshot_id.remove(&change.snapshot_id) {
            provider.delete_snapshot(&repository, &snapshot)?;
        }
    }

    control.checkpoint()?;
    step += 1;
    progress.emit(
        GutchProgressPhase::UploadRepoMetadata,
        "Uploading repo metadata",
        step,
        total_steps,
        None,
        false,
    );
    if plan.repo_manifest_changed {
        provider.upload_repository_metadata(
            &repository,
            crate::helpers::read(&repo_paths.repo_metadata_path)?,
            &control,
        )?;
        stats.repo_manifest_uploaded = true;
    }
    if repo_paths.chunk_map_path.exists() {
        control.checkpoint()?;
        step += 1;
        progress.emit(
            GutchProgressPhase::UploadChunkIndex,
            "Uploading chunk index",
            step,
            total_steps,
            None,
            false,
        );
        if plan.chunk_index_changed {
            provider.upload_chunk_index(
                &repository,
                serde_json::to_vec_pretty(&load_chunk_index(&repo_paths)?)?,
                &control,
            )?;
            stats.chunk_index_uploaded = true;
        }
    }

    let snapshot_ids_to_upload: HashSet<&str> = plan
        .to_add
        .iter()
        .chain(plan.to_update.iter())
        .map(|change| change.snapshot_id.as_str())
        .collect();
    for snapshot in status
        .snapshots
        .iter()
        .filter(|snapshot| snapshot_ids_to_upload.contains(snapshot.snapshot_id.as_str()))
    {
        let manifest = load_snapshot_manifest(&repo_paths, &snapshot.snapshot_id)?;
        let remote_snapshot = match remote_by_snapshot_id.get(&manifest.snapshot_id) {
            Some(snapshot) => snapshot.clone(),
            None => {
                let created = provider.create_snapshot(
                    &repository,
                    &manifest.snapshot_id,
                    &manifest.snapshot_tags,
                )?;
                remote_by_snapshot_id.insert(manifest.snapshot_id.clone(), created.clone());
                created
            }
        };
        let mut snapshot_upload_context = SnapshotUploadContext {
            provider,
            repository: &repository,
            repo_paths: &repo_paths,
            control: &control,
            stats: &mut stats,
            progress: &progress,
            step: &mut step,
            total_steps,
        };
        write_snapshot_uploads(&mut snapshot_upload_context, snapshot, &remote_snapshot)?;
    }

    for bundle_path in bundle_paths {
        control.checkpoint()?;
        step += 1;
        let Some(bundle_id) = crate::bundle_id_from_bundle_path(&bundle_path) else {
            continue;
        };
        if provider.bundle_exists(&repository, &bundle_id)? {
            stats.bundle_skipped_count += 1;
            progress.emit(
                GutchProgressPhase::UploadBundle,
                "Skipping bundles already present remotely",
                step,
                total_steps,
                Some(bundle_id),
                false,
            );
            continue;
        }
        let bundle_bytes = crate::helpers::read(&bundle_path)?;
        let bundle_size = bundle_bytes.len() as u64;
        progress.emit(
            GutchProgressPhase::UploadBundle,
            "Uploading bundles",
            step,
            total_steps,
            Some(bundle_id.clone()),
            false,
        );
        provider.upload_bundle(&repository, &bundle_id, bundle_bytes, &control)?;
        stats.bundle_uploaded_count += 1;
        progress.emit(
            GutchProgressPhase::UploadBundle,
            "Uploaded bundle",
            step,
            total_steps,
            Some(format!("{bundle_id} ({bundle_size} bytes)")),
            false,
        );
    }

    control.checkpoint()?;
    step += 1;
    progress.emit(
        GutchProgressPhase::Prune,
        "Removing unreferenced remote bundles",
        step,
        total_steps,
        None,
        false,
    );
    stats.remote_bundle_deleted_count =
        provider.reconcile_bundles(&repository, &required_bundle_ids)?;
    progress.emit(
        GutchProgressPhase::Done,
        "Sync complete",
        total_steps,
        total_steps,
        None,
        true,
    );
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EmptyReader;

    impl RemoteRepositoryReader for EmptyReader {
        fn resolve_repository(
            &self,
            _remote_spec: Option<&str>,
            _bound_remote_repo_id: Option<&str>,
        ) -> Result<RemoteRepository> {
            Ok(RemoteRepository {
                id: "remote-1".to_string(),
                scope_id: String::new(),
                name: "fixture".to_string(),
                metadata_hash_b3: String::new(),
                chunk_map_hash_b3: String::new(),
                updated_at: String::new(),
            })
        }

        fn list_snapshots(&self, _repository: &RemoteRepository) -> Result<Vec<RemoteSnapshot>> {
            Ok(Vec::new())
        }

        fn repository_metadata_source(
            &self,
            _repository: &RemoteRepository,
        ) -> Result<RemoteArtifactSource> {
            Ok(RemoteArtifactSource::Bytes(Vec::new()))
        }

        fn chunk_index_source(
            &self,
            _repository: &RemoteRepository,
        ) -> Result<RemoteArtifactSource> {
            Ok(RemoteArtifactSource::Bytes(Vec::new()))
        }

        fn snapshot_sources(
            &self,
            _repository: &RemoteRepository,
            _snapshot: &RemoteSnapshot,
        ) -> Result<RemoteSnapshotSources> {
            Ok(RemoteSnapshotSources {
                metadata: RemoteArtifactSource::Bytes(Vec::new()),
                blueprint: RemoteArtifactSource::Bytes(Vec::new()),
                product_metadata: None,
            })
        }

        fn bundle_sources(
            &self,
            _repository: &RemoteRepository,
            _snapshot: &RemoteSnapshot,
            _bundle_ids: &[String],
        ) -> Result<Vec<RemoteBundleSource>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn conflict_policy_parser_is_provider_neutral() {
        assert_eq!(
            PullConflictPolicy::parse("keep-local").unwrap(),
            PullConflictPolicy::KeepLocal
        );
        assert_eq!(
            PullConflictPolicy::parse("keep-remote").unwrap(),
            PullConflictPolicy::KeepRemote
        );
        assert_eq!(
            PullConflictPolicy::parse("keep-newer").unwrap(),
            PullConflictPolicy::KeepNewer
        );
        assert!(PullConflictPolicy::parse("overwrite").is_err());
    }

    #[test]
    fn tag_values_are_summarized_without_provider_types() {
        assert_eq!(
            summarize_snapshot_tags_from_values(&[
                "version::1.2.3".into(),
                "os::linux".into(),
                "-loader-steam".into()
            ]),
            ("1.2.3".into(), "linux".into(), "".into())
        );
    }

    #[test]
    fn remote_artifact_hashes_are_verified_before_persistence() {
        let bytes = b"remote artifact";
        let expected = repo_file_hash(bytes);
        assert!(verify_remote_artifact(bytes, &expected, "metadata").is_ok());
        assert!(verify_remote_artifact(b"tampered", &expected, "metadata").is_err());
    }

    #[test]
    fn remote_bundle_content_hash_is_verified() {
        let payload = b"bundle payload";
        let bundle_hash = repo_file_hash(payload);
        let mut bundle_bytes = payload.to_vec();
        bundle_bytes.extend_from_slice(bundle_hash.as_bytes());

        assert!(verify_remote_bundle(&bundle_hash, &bundle_bytes).is_ok());
        bundle_bytes[0] ^= 1;
        assert!(verify_remote_bundle(&bundle_hash, &bundle_bytes).is_err());
    }

    #[test]
    fn empty_remote_hashes_do_not_force_a_change() {
        assert!(!hashes_differ("", ""));
        assert!(hashes_differ("abc", ""));
        assert!(hashes_differ("", "abc"));
        assert!(!hashes_differ("abc", "abc"));
    }

    fn mtime_ago(seconds: u64) -> std::time::SystemTime {
        std::time::SystemTime::now() - std::time::Duration::from_secs(seconds)
    }

    #[test]
    fn identical_hashes_are_never_fetched() {
        for policy in [
            PullConflictPolicy::KeepLocal,
            PullConflictPolicy::KeepRemote,
            PullConflictPolicy::KeepNewer,
        ] {
            let decision = decide_artifact(
                true,
                "same".to_string(),
                "same",
                "2026-08-08T10:00:00Z",
                Some(mtime_ago(100)),
                policy,
            );
            assert_eq!(decision, ArtifactDecision::Identical);
        }
    }

    #[test]
    fn keep_local_preserves_existing_content_but_fetches_missing_content() {
        let existing = decide_artifact(
            true,
            "local".to_string(),
            "remote",
            "2026-08-08T10:00:00Z",
            Some(mtime_ago(100)),
            PullConflictPolicy::KeepLocal,
        );
        assert_eq!(existing, ArtifactDecision::KeepLocalNewer);

        let missing = decide_artifact(
            false,
            String::new(),
            "remote",
            "2026-08-08T10:00:00Z",
            None,
            PullConflictPolicy::KeepLocal,
        );
        assert_eq!(missing, ArtifactDecision::Fetch);
    }

    #[test]
    fn keep_remote_fetches_even_when_local_is_newer() {
        let decision = decide_artifact(
            true,
            "local".to_string(),
            "remote",
            "2026-08-08T10:00:00Z",
            Some(mtime_ago(100)),
            PullConflictPolicy::KeepRemote,
        );
        assert_eq!(decision, ArtifactDecision::Fetch);
    }

    #[test]
    fn keep_newer_uses_remote_updated_at() {
        let local_newer = decide_artifact(
            true,
            "local".to_string(),
            "remote",
            "2026-08-01T10:00:00Z",
            Some(mtime_ago(100)),
            PullConflictPolicy::KeepNewer,
        );
        assert_eq!(local_newer, ArtifactDecision::KeepLocalNewer);

        let remote_newer = decide_artifact(
            true,
            "local".to_string(),
            "remote",
            "2030-08-01T10:00:00Z",
            Some(mtime_ago(100)),
            PullConflictPolicy::KeepNewer,
        );
        assert_eq!(remote_newer, ArtifactDecision::Fetch);
    }

    #[test]
    fn unparseable_remote_timestamp_fetches() {
        let decision = decide_artifact(
            true,
            "local".to_string(),
            "remote",
            "not-a-timestamp",
            Some(mtime_ago(100)),
            PullConflictPolicy::KeepNewer,
        );
        assert_eq!(decision, ArtifactDecision::Fetch);
    }

    #[test]
    fn pull_planning_accepts_a_reader_without_hostapp_or_http() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let plan = plan_pull_with_provider(
            directory.path(),
            &EmptyReader,
            None,
            PullConflictPolicy::KeepRemote,
        )
        .expect("pull plan");

        assert_eq!(plan.remote_repo_id, "remote-1");
        assert!(plan.repo_manifest_needed);
        assert!(plan.chunk_map_needed);
        assert!(plan.snapshots_to_download.is_empty());
    }

    #[test]
    fn binding_a_remote_repo_keeps_the_id_opaque() {
        let directory = tempfile::tempdir().expect("temporary directory");
        crate::ensure_gutch_repo_layout(
            &GutchRepoPaths::from_repo_root(directory.path()),
            "fixture",
        )
        .expect("repo layout");

        bind_repo_remote(directory.path(), "provider-specific-id").expect("bind repo");
        assert_eq!(
            linked_remote_repo_id(directory.path()).expect("linked id"),
            "provider-specific-id"
        );
    }

    #[test]
    fn binding_rejects_an_empty_remote_repo_id() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let error = bind_repo_remote(directory.path(), " ").expect_err("empty id must fail");
        assert!(error.to_string().contains("remote repo ID is required"));
    }

    #[test]
    fn bundle_source_validation_rejects_unknown_and_duplicate_ids() {
        let expected = HashSet::from(["bundle-a"]);
        let mut sources = HashMap::new();
        let error = record_bundle_sources(
            &mut sources,
            &expected,
            vec![RemoteBundleSource {
                bundle_id: "bundle-b".to_string(),
                source: RemoteArtifactSource::Bytes(vec![]),
            }],
        )
        .expect_err("unknown bundle should be rejected");
        assert!(error.to_string().contains("unexpected bundle"));

        record_bundle_sources(
            &mut sources,
            &expected,
            vec![RemoteBundleSource {
                bundle_id: "bundle-a".to_string(),
                source: RemoteArtifactSource::Bytes(vec![]),
            }],
        )
        .expect("first bundle should be accepted");
        let error = record_bundle_sources(
            &mut sources,
            &expected,
            vec![RemoteBundleSource {
                bundle_id: "bundle-a".to_string(),
                source: RemoteArtifactSource::Bytes(vec![]),
            }],
        )
        .expect_err("duplicate bundle should be rejected");
        assert!(error.to_string().contains("duplicate bundle"));
    }

    #[test]
    fn bundle_source_requests_deduplicate_shared_bundles() {
        let requests = bundle_source_requests(
            &[
                ("snapshot-a".to_string(), vec!["shared".to_string()]),
                (
                    "snapshot-b".to_string(),
                    vec!["shared".to_string(), "unique".to_string()],
                ),
            ],
            &["shared".to_string(), "unique".to_string()],
        );
        assert_eq!(
            requests,
            vec![
                ("snapshot-a".to_string(), vec!["shared".to_string()]),
                ("snapshot-b".to_string(), vec!["unique".to_string()]),
            ]
        );
    }
}
