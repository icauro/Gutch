//! Push and pull through an in-memory provider that records artifact hashes
//! the way a real remote does.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::artifact_decision::remote_artifact_hash;
use super::pull::local_bundle_paths;
use super::*;
use crate::Result;
use crate::manifest::SnapshotTagSet;
use crate::progress::OperationControl;
use crate::repo::paths::RepoPaths;

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
    remote_artifact_hash(bytes)
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

impl RemoteRepoReader for MemoryProvider {
    fn resolve_repo(
        &self,
        _remote_repo_id: Option<&str>,
        _bound_remote_repo_id: Option<&str>,
    ) -> Result<RemoteRepo> {
        let state = self.0.lock().unwrap();
        Ok(RemoteRepo {
            id: "memory".into(),
            scope_id: "scope".into(),
            name: "memory".into(),
            metadata_hash_b3: hash(&state.metadata),
            chunk_map_hash_b3: hash(&state.chunk_index),
            updated_at: String::new(),
        })
    }

    fn list_snapshots(&self, _repository: &RemoteRepo) -> Result<Vec<RemoteSnapshot>> {
        let state = self.0.lock().unwrap();
        Ok(state
            .snapshots
            .iter()
            .map(|(id, snapshot)| Self::remote_snapshot(id, snapshot))
            .collect())
    }

    fn repo_metadata_source(&self, _repository: &RemoteRepo) -> Result<RemoteArtifactSource> {
        Ok(RemoteArtifactSource::Bytes(
            self.0.lock().unwrap().metadata.clone(),
        ))
    }

    fn chunk_index_source(&self, _repository: &RemoteRepo) -> Result<RemoteArtifactSource> {
        Ok(RemoteArtifactSource::Bytes(
            self.0.lock().unwrap().chunk_index.clone(),
        ))
    }

    fn snapshot_sources(
        &self,
        _repository: &RemoteRepo,
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
        _repository: &RemoteRepo,
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

impl RemoteRepoWriter for MemoryProvider {
    fn delete_snapshot(&self, _repository: &RemoteRepo, snapshot: &RemoteSnapshot) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .snapshots
            .remove(&snapshot.snapshot_id);
        Ok(())
    }

    fn create_snapshot(
        &self,
        _repository: &RemoteRepo,
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

    fn upload_artifact(
        &self,
        _repository: &RemoteRepo,
        target: RemoteArtifactTarget<'_>,
        bytes: Vec<u8>,
        _control: &OperationControl,
    ) -> Result<()> {
        match target {
            RemoteArtifactTarget::RepositoryMetadata => self.0.lock().unwrap().metadata = bytes,
            RemoteArtifactTarget::ChunkIndex => self.0.lock().unwrap().chunk_index = bytes,
            RemoteArtifactTarget::SnapshotMetadata(snapshot) => {
                self.update(snapshot, |stored| stored.metadata = bytes)
            }
            RemoteArtifactTarget::SnapshotBlueprint(snapshot) => {
                self.update(snapshot, |stored| stored.blueprint = bytes)
            }
            RemoteArtifactTarget::SnapshotProductMetadata(snapshot) => {
                self.update(snapshot, |stored| stored.product_metadata = Some(bytes))
            }
            RemoteArtifactTarget::Bundle(bundle_id) => {
                self.0
                    .lock()
                    .unwrap()
                    .bundles
                    .insert(bundle_id.to_string(), bytes);
            }
        }
        Ok(())
    }

    fn delete_snapshot_product_metadata(
        &self,
        _repository: &RemoteRepo,
        snapshot: &RemoteSnapshot,
    ) -> Result<()> {
        self.update(snapshot, |stored| stored.product_metadata = None);
        Ok(())
    }

    fn bundle_exists(&self, _repository: &RemoteRepo, bundle_id: &str) -> Result<bool> {
        Ok(self.0.lock().unwrap().bundles.contains_key(bundle_id))
    }

    fn reconcile_bundles(
        &self,
        _repository: &RemoteRepo,
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
    capture_reusing(repo, source, content, false)
}

fn capture_reusing(
    repo: &Path,
    source: &Path,
    content: &[u8],
    reuse_remote_bundles: bool,
) -> String {
    std::fs::create_dir_all(source).unwrap();
    std::fs::write(source.join("file"), content).unwrap();
    let paths = RepoPaths::from_repo_root(repo);
    if !paths.repo_metadata_path.exists() {
        crate::repo::paths::ensure_repo_layout(&paths, "fixture").unwrap();
    }
    crate::capture::sources::capture_snapshot(&crate::capture::contracts::SnapshotCaptureRequest {
        repo_root: repo.to_path_buf(),
        source: source.to_path_buf(),
        tags: Default::default(),
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
        reuse_remote_bundles,
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

fn push(repo: &Path, provider: &MemoryProvider) -> Result<RepoPushResult> {
    let request = RepoPushRequest {
        repo_root: repo.to_path_buf(),
        ..Default::default()
    };
    push_repo(&request, provider)
}

fn bind(repo: &Path) {
    bind_remote_repo(&RemoteRepoBindRequest {
        repo_root: repo.to_path_buf(),
        remote_repo_id: "memory".to_owned(),
        ..Default::default()
    })
    .unwrap();
}

fn pull(repo: &Path, provider: &MemoryProvider, prune: bool) -> RepoPullResult {
    let request = RepoPullRequest {
        repo_root: repo.to_path_buf(),
        on_conflict: PullConflictPolicy::KeepRemote,
        prune,
        ..Default::default()
    };
    pull_repo(&request, provider).unwrap()
}

#[test]
fn pull_skips_snapshots_whose_remote_artifacts_are_not_ready() {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin");
    let id = capture(&origin, &root.path().join("source"), &[7; 64 * 1024]);
    bind(&origin);
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
    bind(&origin);
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
    let verified = crate::repo::verify::verify_repo(&crate::repo::verify::RepoVerifyRequest {
        repo_root: clone.clone(),
        snapshot_id: None,
        mode: crate::repo::verify::RepoVerifyMode::Strict,
        control: Default::default(),
    })
    .unwrap();
    assert!(verified.ok);
    assert_eq!(verified.snapshot_count, 1);
}

#[test]
fn pull_downloads_only_the_selected_snapshots() {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin");
    let first = capture(&origin, &root.path().join("source"), &[3; 64 * 1024]);
    let first_bundles = bundle_ids(&origin);
    let second = capture(&origin, &root.path().join("source"), &[4; 64 * 1024]);
    bind(&origin);
    let provider = MemoryProvider::default();
    push(&origin, &provider).unwrap();

    let clone = root.path().join("clone");
    let pull_only = |snapshot_ids: Vec<String>| {
        pull_repo(
            &RepoPullRequest {
                repo_root: clone.clone(),
                on_conflict: PullConflictPolicy::KeepRemote,
                prune: true,
                snapshot_ids,
                ..Default::default()
            },
            &provider,
        )
    };
    let error = pull_only(vec!["missing".into()]).unwrap_err();
    assert!(
        error.to_string().contains("no snapshot 'missing'"),
        "{error}"
    );

    pull_only(vec![first.clone()]).unwrap();
    let local_ids =
        crate::repo::inventory::list_repo_snapshot_ids(&RepoPaths::from_repo_root(&clone));
    assert_eq!(local_ids.unwrap(), vec![first.clone()]);
    assert_eq!(bundle_ids(&clone), first_bundles);

    // The kept snapshot is on the remote, so pruning leaves it alone.
    pull_only(vec![second.clone()]).unwrap();
    let mut expected = vec![first, second];
    expected.sort();
    let local_ids =
        crate::repo::inventory::list_repo_snapshot_ids(&RepoPaths::from_repo_root(&clone));
    assert_eq!(local_ids.unwrap(), expected);
    assert_eq!(bundle_ids(&clone), bundle_ids(&origin));
}

#[test]
fn push_fails_when_a_required_bundle_is_missing_locally() {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin");
    capture(&origin, &root.path().join("source"), &[3; 64 * 1024]);
    bind(&origin);
    let paths = RepoPaths::from_repo_root(&origin);
    for path in local_bundle_paths(&paths.bundles_root).unwrap() {
        std::fs::remove_file(path).unwrap();
    }

    let provider = MemoryProvider::default();
    let error = push(&origin, &provider).unwrap_err();
    assert!(error.to_string().contains("missing"), "{error}");
    assert!(provider.0.lock().unwrap().bundles.is_empty());
}

#[test]
fn pull_step_total_matches_the_steps_reported() {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin");
    capture(&origin, &root.path().join("source"), &[5; 64 * 1024]);
    bind(&origin);
    let provider = MemoryProvider::default();
    push(&origin, &provider).unwrap();

    for (clone, prune) in [("plain", false), ("pruned", true)] {
        let steps = std::sync::Arc::new(crate::test_util::RecordedProgress::default());
        let request = RepoPullRequest {
            repo_root: root.path().join(clone),
            on_conflict: PullConflictPolicy::KeepRemote,
            prune,
            // Record every step directly: live UI feedback intentionally coalesces them.
            control: OperationControl::default().with_replaced_progress(steps.clone()),
            ..Default::default()
        };
        pull_repo(&request, &provider).unwrap();
        let events = steps.events();
        let finished = events.last().unwrap();
        assert!(finished.done);
        let last_step = events
            .iter()
            .filter(|event| !event.done)
            .map(|event| event.current)
            .max();
        assert_eq!(last_step, Some(finished.total), "prune: {prune}");
    }
}

/// A key-value store in memory; puts under `fail_prefix` fail.
#[derive(Default)]
struct MemoryStore {
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
    fail_prefix: Mutex<Option<String>>,
}

impl MemoryStore {
    fn keys(&self, prefix: &str) -> Vec<String> {
        self.list(prefix).unwrap()
    }
}

impl RemoteObjectStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.objects.lock().unwrap().get(key).cloned())
    }

    fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        if let Some(prefix) = self.fail_prefix.lock().unwrap().as_deref()
            && key.contains(prefix)
        {
            return Err(crate::Error::repo("store unavailable"));
        }
        self.objects
            .lock()
            .unwrap()
            .insert(key.to_owned(), bytes.to_vec());
        Ok(())
    }

    fn put_if_absent(&self, key: &str, bytes: &[u8]) -> Result<bool> {
        let mut objects = self.objects.lock().unwrap();
        if objects.contains_key(key) {
            return Ok(false);
        }
        objects.insert(key.to_owned(), bytes.to_vec());
        Ok(true)
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.objects.lock().unwrap().remove(key);
        Ok(())
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .objects
            .lock()
            .unwrap()
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect())
    }
}

/// A new repo at `origin` with one snapshot, linked to a new remote repo.
fn linked_origin(root: &Path, remote: &ObjectStoreRemote<MemoryStore>) -> (PathBuf, String) {
    let origin = root.join("origin");
    capture(&origin, &root.join("source"), &[9; 64 * 1024]);
    let remote_repo_id = create_remote_repo(
        &RemoteRepoCreateRequest {
            repo_root: origin.clone(),
            scope_id: "team".into(),
            ..Default::default()
        },
        remote,
    )
    .unwrap()
    .remote_repo_id;
    (origin, remote_repo_id)
}

fn push_to(repo: &Path, remote: &ObjectStoreRemote<MemoryStore>) -> Result<RepoPushResult> {
    push_repo(
        &RepoPushRequest {
            repo_root: repo.to_path_buf(),
            ..Default::default()
        },
        remote,
    )
}

fn pull_from(repo: &Path, remote_repo_id: &str, remote: &ObjectStoreRemote<MemoryStore>) {
    pull_repo(
        &RepoPullRequest {
            repo_root: repo.to_path_buf(),
            remote_repo_id: Some(remote_repo_id.to_owned()),
            on_conflict: PullConflictPolicy::KeepRemote,
            ..Default::default()
        },
        remote,
    )
    .unwrap();
}

#[test]
fn object_store_remote_round_trips_a_repository() {
    let root = tempfile::tempdir().unwrap();
    let remote = ObjectStoreRemote::new(MemoryStore::default());
    let (origin, remote_repo_id) = linked_origin(root.path(), &remote);
    assert_eq!(remote_repo_id, "team--fixture");

    let pushed = push_to(&origin, &remote).unwrap();
    assert_eq!(pushed.snapshot_added_count, 1);
    assert!(pushed.bundle_uploaded_count > 0);
    let again = push_to(&origin, &remote).unwrap();
    assert_eq!(again.snapshot_unchanged_count, 1);
    assert_eq!(again.bundle_uploaded_count, 0);
    assert!(
        !remote
            .store()
            .objects
            .lock()
            .unwrap()
            .contains_key("repos/team--fixture/push.lock")
    );

    let copy = root.path().join("copy");
    pull_from(&copy, &remote_repo_id, &remote);
    assert_eq!(bundle_ids(&copy), bundle_ids(&origin));
    let repos = remote.list_repos().unwrap();
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].repository_name, "fixture");

    let repository = remote.resolve_repo(Some(&remote_repo_id), None).unwrap();
    remote.delete_repo(&repository).unwrap();
    assert!(remote.store().keys("").is_empty());
}

#[test]
fn a_repository_pushed_before_its_first_capture_can_be_pulled() {
    let root = tempfile::tempdir().unwrap();
    let remote = ObjectStoreRemote::new(MemoryStore::default());
    let origin = root.path().join("origin");
    crate::repo::paths::ensure_repo_layout(&RepoPaths::from_repo_root(&origin), "fixture").unwrap();
    let remote_repo_id = create_remote_repo(
        &RemoteRepoCreateRequest {
            repo_root: origin.clone(),
            scope_id: "team".into(),
            ..Default::default()
        },
        &remote,
    )
    .unwrap()
    .remote_repo_id;
    push_to(&origin, &remote).unwrap();

    let copy = root.path().join("copy");
    pull_from(&copy, &remote_repo_id, &remote);

    assert!(
        RepoPaths::from_repo_root(&copy)
            .repo_metadata_path
            .is_file()
    );
}

#[test]
fn object_store_remote_allows_one_push_at_a_time() {
    let root = tempfile::tempdir().unwrap();
    let remote = ObjectStoreRemote::new(MemoryStore::default());
    let (origin, remote_repo_id) = linked_origin(root.path(), &remote);
    let repository = remote.resolve_repo(Some(&remote_repo_id), None).unwrap();

    remote.begin_push(&repository).unwrap();
    let error = push_to(&origin, &remote).unwrap_err().to_string();
    assert!(error.contains("repos/team--fixture/push.lock"), "{error}");
    remote.finish_push(&repository, false).unwrap();

    push_to(&origin, &remote).unwrap();
}

#[test]
fn object_store_remote_hides_snapshots_until_their_push_completes() {
    let root = tempfile::tempdir().unwrap();
    let remote = ObjectStoreRemote::new(MemoryStore::default());
    let (origin, remote_repo_id) = linked_origin(root.path(), &remote);

    *remote.store().fail_prefix.lock().unwrap() = Some("/bundles/".into());
    push_to(&origin, &remote).unwrap_err();
    assert!(
        remote
            .store()
            .keys("repos/team--fixture/push.lock")
            .is_empty()
    );
    let copy = root.path().join("copy");
    pull_from(&copy, &remote_repo_id, &remote);
    assert!(bundle_ids(&copy).is_empty());

    *remote.store().fail_prefix.lock().unwrap() = None;
    let pushed = push_to(&origin, &remote).unwrap();
    assert_eq!(pushed.snapshot_updated_count, 1);
    pull_from(&copy, &remote_repo_id, &remote);
    assert_eq!(bundle_ids(&copy), bundle_ids(&origin));
}

#[test]
fn object_store_remote_rejects_names_that_change_the_key_layout() {
    let remote = ObjectStoreRemote::new(MemoryStore::default());
    remote.create_repo("team", "app").unwrap();
    assert!(remote.create_repo("team", "app").is_err());
    assert!(remote.create_repo("team", "../app").is_err());
    assert!(remote.create_repo("", "app").is_err());
}

#[test]
fn fetching_one_file_downloads_only_the_bundles_holding_it() {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin");
    let source = root.path().join("source");
    let shared = [5; 64 * 1024];
    capture(&origin, &source, &shared);
    let shared_bundles = bundle_ids(&origin);
    // The second snapshot reuses the first one's bundle for `file` and adds
    // a bundle for `other`.
    std::fs::write(source.join("other"), [6; 64 * 1024]).unwrap();
    let snapshot = capture(&origin, &source, &shared);
    bind(&origin);
    let provider = MemoryProvider::default();
    push(&origin, &provider).unwrap();
    provider.take_requested_bundles();

    let clone = root.path().join("clone");
    let metadata_only = pull_repo(
        &RepoPullRequest {
            repo_root: clone.clone(),
            remote_repo_id: Some("memory".into()),
            snapshot_ids: vec![snapshot.clone()],
            bundles: PullBundles::None,
            ..Default::default()
        },
        &provider,
    )
    .unwrap();
    assert_eq!(metadata_only.snapshot_downloaded_count, 1);
    assert!(provider.take_requested_bundles().is_empty());
    assert!(bundle_ids(&clone).is_empty());

    let fetch = |path: &str| {
        fetch_snapshot_file(
            &SnapshotFileFetchRequest {
                repo_root: clone.clone(),
                remote_repo_id: Some("memory".into()),
                snapshot_id: snapshot.clone(),
                path: path.into(),
                ..Default::default()
            },
            &provider,
        )
    };
    assert_eq!(fetch("file").unwrap(), shared);
    assert_eq!(provider.take_requested_bundles(), shared_bundles);
    assert_eq!(bundle_ids(&clone), shared_bundles);

    assert_eq!(fetch("other").unwrap(), [6; 64 * 1024]);
    assert_eq!(bundle_ids(&clone), bundle_ids(&origin));
    // Fetching a file again downloads nothing.
    provider.take_requested_bundles();
    fetch("file").unwrap();
    assert!(provider.take_requested_bundles().is_empty());
    let error = fetch("missing").unwrap_err();
    assert!(
        error.to_string().contains("has no file 'missing'"),
        "{error}"
    );
}

#[test]
fn a_metadata_only_copy_can_capture_and_push_new_snapshots() {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin");
    capture(&origin, &root.path().join("first"), &[1; 64 * 1024]);
    bind(&origin);
    let provider = MemoryProvider::default();
    push(&origin, &provider).unwrap();
    let first_bundles = bundle_ids(&origin);

    // A publisher keeps only metadata and the chunk index, never old bundles.
    let publisher = root.path().join("publisher");
    pull_repo(
        &RepoPullRequest {
            repo_root: publisher.clone(),
            remote_repo_id: Some("memory".into()),
            bundles: PullBundles::None,
            ..Default::default()
        },
        &provider,
    )
    .unwrap();
    assert!(bundle_ids(&publisher).is_empty());
    provider.take_requested_bundles();

    // The new snapshot repeats the first file and adds another.
    let second_source = root.path().join("second");
    std::fs::create_dir_all(&second_source).unwrap();
    std::fs::write(second_source.join("other"), [2; 64 * 1024]).unwrap();
    let second = capture_reusing(&publisher, &second_source, &[1; 64 * 1024], true);
    let new_bundles = bundle_ids(&publisher);
    assert!(!new_bundles.is_empty());
    assert!(new_bundles.iter().all(|id| !first_bundles.contains(id)));

    let pushed = push(&publisher, &provider).unwrap();
    assert_eq!(pushed.bundle_uploaded_count, new_bundles.len());
    assert!(provider.take_requested_bundles().is_empty());
    // A copy of the first snapshot's bundles stays on the remote.
    let full = root.path().join("full");
    pull(&full, &provider, false);
    let mut expected: Vec<String> = first_bundles.iter().chain(&new_bundles).cloned().collect();
    expected.sort();
    assert_eq!(bundle_ids(&full), expected);
    let local_ids =
        crate::repo::inventory::list_repo_snapshot_ids(&RepoPaths::from_repo_root(&full)).unwrap();
    assert!(local_ids.contains(&second));
}
