//! Push and pull through an in-memory provider that records artifact hashes
//! the way a real remote does.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

use super::link::bind_remote_repo;
use super::pull::local_bundle_paths;
use super::*;
use crate::manifest::SnapshotTagSet;

#[derive(Default)]
struct MemoryState {
    metadata: Vec<u8>,
    chunk_index: Vec<u8>,
    snapshots: BTreeMap<String, MemorySnapshot>,
    bundles: BTreeMap<String, Vec<u8>>,
    requested_bundles: Vec<String>,
}

#[derive(Default, Clone)]
struct MemorySnapshot {
    metadata: Vec<u8>,
    blueprint: Vec<u8>,
    product_metadata: Option<Vec<u8>>,
    artifact_ready: bool,
}

#[derive(Default)]
struct MemoryProvider(Mutex<MemoryState>);

fn hash(bytes: &[u8]) -> String {
    repo_file_hash(bytes)
}

impl MemoryProvider {
    fn set_ready(&self, snapshot_id: &str, ready: bool) {
        let mut state = self.0.lock().unwrap();
        state.snapshots.get_mut(snapshot_id).unwrap().artifact_ready = ready;
    }

    fn take_requested_bundles(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap().requested_bundles)
    }

    fn remote_snapshot(snapshot_id: &str, snapshot: &MemorySnapshot) -> RemoteSnapshot {
        RemoteSnapshot {
            remote_id: format!("remote-{snapshot_id}"),
            snapshot_id: snapshot_id.to_string(),
            metadata_hash_b3: hash(&snapshot.metadata),
            product_metadata_hash_b3: snapshot
                .product_metadata
                .as_deref()
                .map(hash)
                .unwrap_or_default(),
            blueprint_hash_b3: hash(&snapshot.blueprint),
            uploaded_at: String::new(),
            created_at: String::new(),
            tags: Vec::new(),
            artifact_state: None,
            artifact_ready: snapshot.artifact_ready,
        }
    }

    fn update(&self, snapshot: &RemoteSnapshot, edit: impl FnOnce(&mut MemorySnapshot)) {
        let mut state = self.0.lock().unwrap();
        edit(state.snapshots.get_mut(&snapshot.snapshot_id).unwrap());
    }
}

impl RemoteRepositoryReader for MemoryProvider {
    fn resolve_repository(
        &self,
        _remote_spec: Option<&str>,
        _bound_remote_repo_id: Option<&str>,
    ) -> Result<RemoteRepository> {
        let state = self.0.lock().unwrap();
        Ok(RemoteRepository {
            id: "memory".into(),
            scope_id: "scope".into(),
            name: "memory".into(),
            metadata_hash_b3: hash(&state.metadata),
            chunk_map_hash_b3: hash(&state.chunk_index),
            updated_at: String::new(),
        })
    }

    fn list_snapshots(&self, _repository: &RemoteRepository) -> Result<Vec<RemoteSnapshot>> {
        let state = self.0.lock().unwrap();
        Ok(state
            .snapshots
            .iter()
            .map(|(id, snapshot)| Self::remote_snapshot(id, snapshot))
            .collect())
    }

    fn repository_metadata_source(
        &self,
        _repository: &RemoteRepository,
    ) -> Result<RemoteArtifactSource> {
        Ok(RemoteArtifactSource::Bytes(
            self.0.lock().unwrap().metadata.clone(),
        ))
    }

    fn chunk_index_source(&self, _repository: &RemoteRepository) -> Result<RemoteArtifactSource> {
        Ok(RemoteArtifactSource::Bytes(
            self.0.lock().unwrap().chunk_index.clone(),
        ))
    }

    fn snapshot_sources(
        &self,
        _repository: &RemoteRepository,
        snapshot: &RemoteSnapshot,
    ) -> Result<RemoteSnapshotSources> {
        let state = self.0.lock().unwrap();
        let stored = &state.snapshots[&snapshot.snapshot_id];
        Ok(RemoteSnapshotSources {
            metadata: RemoteArtifactSource::Bytes(stored.metadata.clone()),
            blueprint: RemoteArtifactSource::Bytes(stored.blueprint.clone()),
            product_metadata: stored
                .product_metadata
                .clone()
                .map(RemoteArtifactSource::Bytes),
        })
    }

    fn bundle_sources(
        &self,
        _repository: &RemoteRepository,
        _snapshot: &RemoteSnapshot,
        bundle_ids: &[String],
    ) -> Result<Vec<RemoteBundleSource>> {
        let mut state = self.0.lock().unwrap();
        state.requested_bundles.extend(bundle_ids.iter().cloned());
        Ok(bundle_ids
            .iter()
            .map(|bundle_id| RemoteBundleSource {
                bundle_id: bundle_id.clone(),
                source: RemoteArtifactSource::Bytes(state.bundles[bundle_id].clone()),
            })
            .collect())
    }
}

impl RemoteRepositoryWriter for MemoryProvider {
    fn delete_snapshot(
        &self,
        _repository: &RemoteRepository,
        snapshot: &RemoteSnapshot,
    ) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .snapshots
            .remove(&snapshot.snapshot_id);
        Ok(())
    }

    fn create_snapshot(
        &self,
        _repository: &RemoteRepository,
        snapshot_id: &str,
        _tags: &SnapshotTagSet,
    ) -> Result<RemoteSnapshot> {
        let snapshot = MemorySnapshot {
            artifact_ready: true,
            ..Default::default()
        };
        let remote = Self::remote_snapshot(snapshot_id, &snapshot);
        self.0
            .lock()
            .unwrap()
            .snapshots
            .insert(snapshot_id.to_string(), snapshot);
        Ok(remote)
    }

    fn upload_repository_metadata(
        &self,
        _repository: &RemoteRepository,
        bytes: Vec<u8>,
        _control: &OperationControl,
    ) -> Result<()> {
        self.0.lock().unwrap().metadata = bytes;
        Ok(())
    }

    fn upload_chunk_index(
        &self,
        _repository: &RemoteRepository,
        bytes: Vec<u8>,
        _control: &OperationControl,
    ) -> Result<()> {
        self.0.lock().unwrap().chunk_index = bytes;
        Ok(())
    }

    fn upload_snapshot_metadata(
        &self,
        _repository: &RemoteRepository,
        snapshot: &RemoteSnapshot,
        bytes: Vec<u8>,
        _control: &OperationControl,
    ) -> Result<()> {
        self.update(snapshot, |stored| stored.metadata = bytes);
        Ok(())
    }

    fn upload_snapshot_blueprint(
        &self,
        _repository: &RemoteRepository,
        snapshot: &RemoteSnapshot,
        bytes: Vec<u8>,
        _control: &OperationControl,
    ) -> Result<()> {
        self.update(snapshot, |stored| stored.blueprint = bytes);
        Ok(())
    }

    fn upload_snapshot_product_metadata(
        &self,
        _repository: &RemoteRepository,
        snapshot: &RemoteSnapshot,
        bytes: Vec<u8>,
        _control: &OperationControl,
    ) -> Result<()> {
        self.update(snapshot, |stored| stored.product_metadata = Some(bytes));
        Ok(())
    }

    fn delete_snapshot_product_metadata(
        &self,
        _repository: &RemoteRepository,
        snapshot: &RemoteSnapshot,
    ) -> Result<()> {
        self.update(snapshot, |stored| stored.product_metadata = None);
        Ok(())
    }

    fn bundle_exists(&self, _repository: &RemoteRepository, bundle_id: &str) -> Result<bool> {
        Ok(self.0.lock().unwrap().bundles.contains_key(bundle_id))
    }

    fn upload_bundle(
        &self,
        _repository: &RemoteRepository,
        bundle_id: &str,
        bytes: Vec<u8>,
        _control: &OperationControl,
    ) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .bundles
            .insert(bundle_id.to_string(), bytes);
        Ok(())
    }

    fn reconcile_bundles(
        &self,
        _repository: &RemoteRepository,
        required_bundle_ids: &[String],
    ) -> Result<usize> {
        let mut state = self.0.lock().unwrap();
        let before = state.bundles.len();
        state
            .bundles
            .retain(|bundle_id, _| required_bundle_ids.contains(bundle_id));
        Ok(before - state.bundles.len())
    }
}

/// Captures `content` as a bundled snapshot of a new repo at `repo`.
fn capture(repo: &Path, source: &Path, content: &[u8]) -> String {
    std::fs::create_dir_all(source).unwrap();
    std::fs::write(source.join("file"), content).unwrap();
    let paths = RepoPaths::from_repo_root(repo);
    if !paths.repo_metadata_path.exists() {
        crate::repo::paths::ensure_gutch_repo_layout(&paths, "fixture").unwrap();
    }
    crate::capture::execute::capture_snapshot(crate::capture::contracts::SnapshotCaptureRequest {
        repo_root: repo.to_path_buf(),
        source_dir: source.to_path_buf(),
        explicit_tags: Default::default(),
        chunk_profile: crate::artifact::profiles::ChunkSizingPreset::Chunk16KiB,
        chunker: crate::artifact::profiles::Chunker::FastCdc,
        bundle_profile: Default::default(),
        compression_algorithm: crate::artifact::profiles::CompressionKind::Zstd,
        compression_level: None,
        storage_mode: crate::artifact::profiles::ArtifactStorageMode::Bundled,
        runtime_hints: crate::runtime::RuntimeResourceHints {
            jobs: Some(1),
            ..Default::default()
        },
        control: Default::default(),
    })
    .unwrap()
    .snapshot_id
}

fn bundle_ids(repo: &Path) -> Vec<String> {
    let paths = RepoPaths::from_repo_root(repo);
    let mut ids: Vec<String> = local_bundle_paths(&paths.bundles_root)
        .unwrap()
        .iter()
        .filter_map(|path| crate::repo::paths::bundle_id_from_bundle_path(path))
        .collect();
    ids.sort();
    ids
}

fn push(repo: &Path, provider: &MemoryProvider) -> Result<RepoPushStats> {
    push_repo(repo.to_path_buf(), provider, OperationControl::default())
}

fn pull(repo: &Path, provider: &MemoryProvider, prune: bool) -> RepoPullStats {
    pull_repo(
        PathBuf::from(repo),
        provider,
        None,
        PullConflictPolicy::KeepRemote,
        prune,
        OperationControl::default(),
    )
    .unwrap()
}

#[test]
fn pull_skips_snapshots_whose_remote_artifacts_are_not_ready() {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin");
    let id = capture(&origin, &root.path().join("source"), &[7; 64 * 1024]);
    bind_remote_repo(&origin, "memory").unwrap();
    let provider = MemoryProvider::default();
    push(&origin, &provider).unwrap();
    provider.set_ready(&id, false);

    let clone = root.path().join("clone");
    pull(&clone, &provider, false);
    assert!(provider.take_requested_bundles().is_empty());
    assert!(bundle_ids(&clone).is_empty());

    provider.set_ready(&id, true);
    pull(&clone, &provider, false);
    assert_eq!(bundle_ids(&clone), bundle_ids(&origin));
}

#[test]
fn prune_removes_local_only_snapshots_and_their_bundles_without_requesting_them() {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin");
    capture(&origin, &root.path().join("shared"), &[1; 64 * 1024]);
    bind_remote_repo(&origin, "memory").unwrap();
    let provider = MemoryProvider::default();
    push(&origin, &provider).unwrap();

    let clone = root.path().join("clone");
    pull(&clone, &provider, false);
    let shared_bundles = bundle_ids(&clone);
    capture(&clone, &root.path().join("local"), &[2; 64 * 1024]);
    assert!(bundle_ids(&clone).len() > shared_bundles.len());
    provider.take_requested_bundles();

    let stats = pull(&clone, &provider, true);
    assert!(provider.take_requested_bundles().is_empty());
    assert!(stats.pruned_bundle_count > 0);
    assert_eq!(bundle_ids(&clone), shared_bundles);
    let verified = crate::repo::verify::verify_gutch_repo_artifacts(
        &clone,
        None,
        crate::repo::verify::RepoArtifactVerification::Strict,
    )
    .unwrap();
    assert!(verified.ok);
    assert_eq!(verified.snapshot_count, 1);
}

#[test]
fn push_fails_when_a_required_bundle_is_missing_locally() {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin");
    capture(&origin, &root.path().join("source"), &[3; 64 * 1024]);
    bind_remote_repo(&origin, "memory").unwrap();
    let paths = RepoPaths::from_repo_root(&origin);
    for path in local_bundle_paths(&paths.bundles_root).unwrap() {
        std::fs::remove_file(path).unwrap();
    }

    let provider = MemoryProvider::default();
    let error = push(&origin, &provider).unwrap_err();
    assert!(error.to_string().contains("missing"), "{error}");
    assert!(provider.0.lock().unwrap().bundles.is_empty());
}
