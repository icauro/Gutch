//! A remote provider built on a plain key-value object store.
//!
//! Many remotes are just storage: a folder, an S3 bucket, a blob container.
//! For those, an application implements [`RemoteObjectStore`] and wraps it in
//! [`ObjectStoreRemote`], which implements the full provider traits. Gutch
//! then owns the key layout, the hashes it compares, readiness and bundle
//! cleanup, and a lock that keeps two pushes from interleaving.
//!
//! Key layout, with `/` separating key segments:
//!
//! ```text
//! index/REPO_ID.json                          one record per remote repository
//! repos/REPO_ID/push.lock                     present while a push runs
//! repos/REPO_ID/repo_metadata
//! repos/REPO_ID/chunk_index
//! repos/REPO_ID/snapshot_records/SNAPSHOT_ID.json
//! repos/REPO_ID/snapshots/SNAPSHOT_ID/{metadata, blueprint, product_metadata}
//! repos/REPO_ID/bundles/BUNDLE_ID
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use super::artifact_decision::remote_artifact_hash;
use super::provider::{
    RemoteArtifactSource, RemoteArtifactTarget, RemoteBundleSource, RemoteRepo, RemoteRepoProvider,
    RemoteRepoReader, RemoteRepoService, RemoteRepoSummary, RemoteRepoWriter, RemoteSnapshot,
    RemoteSnapshotSources,
};
use crate::manifest::SnapshotTagSet;
use crate::progress::OperationControl;
use crate::repo::tag_references::TagReference;
use crate::{Error, Result};

/// The storage operations [`ObjectStoreRemote`] needs.
///
/// Keys are `/`-separated paths such as `repos/team--app/bundles/ID`. Each
/// `put` must replace the whole object at once, so a reader sees either the
/// old bytes or the new ones.
pub trait RemoteObjectStore: Send + Sync {
    /// The object's bytes, or `None` when there is no object at `key`.
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;

    /// Creates or replaces the object at `key`.
    fn put(&self, key: &str, bytes: &[u8]) -> Result<()>;

    /// Creates the object only if `key` is free; returns whether it did.
    ///
    /// This must be atomic: when two callers race, exactly one gets `true`.
    /// The push lock depends on it. Object stores provide it as a conditional
    /// write (for example `If-None-Match: *`); a filesystem as create-new.
    fn put_if_absent(&self, key: &str, bytes: &[u8]) -> Result<bool>;

    /// Deletes the object at `key`; deleting a missing object succeeds.
    fn delete(&self, key: &str) -> Result<()>;

    /// The full keys of the objects directly below `prefix`.
    ///
    /// Gutch only lists prefixes that end in `/` and hold plain objects, so a
    /// recursive listing and a one-level listing give the same answer.
    fn list(&self, prefix: &str) -> Result<Vec<String>>;

    /// Whether an object exists at `key`. Override it with a cheaper check
    /// (such as a `HEAD` request) when the store has one; the default reads
    /// the object.
    fn exists(&self, key: &str) -> Result<bool> {
        Ok(self.get(key)?.is_some())
    }

    /// A URL Gutch can download the object from, such as a presigned link.
    /// The default, `None`, makes Gutch read the object with `get` when it
    /// needs it.
    fn download_url(&self, _key: &str) -> Result<Option<String>> {
        Ok(None)
    }
}

/// A complete remote provider over a [`RemoteObjectStore`].
///
/// Pushes take a lock object (`repos/REPO_ID/push.lock`) so that only one
/// push changes a remote repository at a time; a second push fails at once
/// instead of waiting. The lock is released when the push ends, whether it
/// succeeded or not. If the process dies during a push the lock stays behind;
/// the error names the key to delete once you are sure no push is running.
///
/// Snapshots a push uploads become visible to pulls only when that push
/// completes. Pulls take no lock: a pull that overlaps a push can fail when
/// an object it needs is replaced or deleted, and is safe to retry.
pub struct ObjectStoreRemote<S> {
    store: Arc<S>,
    held_locks: Mutex<HashMap<String, String>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RepoRecord {
    scope_id: String,
    name: String,
    metadata_hash_b3: String,
    chunk_map_hash_b3: String,
    updated_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SnapshotRecord {
    tags: Vec<String>,
    created_at: String,
    uploaded_at: String,
    metadata_hash_b3: String,
    blueprint_hash_b3: String,
    product_metadata_hash_b3: String,
    ready: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct PushLock {
    token: String,
    started_at: String,
}

impl<S: RemoteObjectStore + 'static> ObjectStoreRemote<S> {
    pub fn new(store: S) -> Self {
        Self {
            store: Arc::new(store),
            held_locks: Mutex::new(HashMap::new()),
        }
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    fn load<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Result<Option<T>> {
        let Some(bytes) = self.store.get(key)? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| Error::repo(format!("remote record '{key}' is unreadable: {error}")))
    }

    fn save<T: Serialize>(&self, key: &str, record: &T) -> Result<()> {
        self.store.put(key, &record_bytes(record)?)
    }

    fn repo_record(&self, repo_id: &str) -> Result<RepoRecord> {
        self.load(&repo_record_key(repo_id))?
            .ok_or_else(|| Error::repo(format!("remote repo '{repo_id}' does not exist")))
    }

    fn update_repo_record(&self, repo_id: &str, edit: impl FnOnce(&mut RepoRecord)) -> Result<()> {
        let mut record = self.repo_record(repo_id)?;
        edit(&mut record);
        record.updated_at = now();
        self.save(&repo_record_key(repo_id), &record)
    }

    fn snapshot_record(&self, repo_id: &str, snapshot_id: &str) -> Result<SnapshotRecord> {
        self.load(&snapshot_record_key(repo_id, snapshot_id))?
            .ok_or_else(|| {
                Error::repo(format!(
                    "remote snapshot '{snapshot_id}' does not exist in '{repo_id}'"
                ))
            })
    }

    /// Records a changed snapshot file; the snapshot stays hidden from pulls
    /// until the push that changed it completes.
    fn update_snapshot_record(
        &self,
        repo_id: &str,
        snapshot_id: &str,
        edit: impl FnOnce(&mut SnapshotRecord),
    ) -> Result<()> {
        let mut record = self.snapshot_record(repo_id, snapshot_id)?;
        edit(&mut record);
        record.uploaded_at = now();
        record.ready = false;
        self.save(&snapshot_record_key(repo_id, snapshot_id), &record)
    }

    fn source(&self, key: String) -> Result<RemoteArtifactSource> {
        if let Some(url) = self.store.download_url(&key)? {
            return Ok(RemoteArtifactSource::Url(url));
        }
        let store = Arc::clone(&self.store);
        Ok(RemoteArtifactSource::Fetch(Arc::new(move || {
            store
                .get(&key)?
                .ok_or_else(|| Error::repo(format!("remote object '{key}' is missing")))
        })))
    }

    fn mark_uploaded_snapshots_ready(&self, repo_id: &str) -> Result<()> {
        for key in self.store.list(&snapshot_records_prefix(repo_id))? {
            let Some(mut record) = self.load::<SnapshotRecord>(&key)? else {
                continue;
            };
            if !record.ready
                && !record.metadata_hash_b3.is_empty()
                && !record.blueprint_hash_b3.is_empty()
            {
                record.ready = true;
                self.save(&key, &record)?;
            }
        }
        Ok(())
    }
}

impl<S: RemoteObjectStore + 'static> RemoteRepoReader for ObjectStoreRemote<S> {
    fn resolve_repo(
        &self,
        remote_repo_id: Option<&str>,
        bound_remote_repo_id: Option<&str>,
    ) -> Result<RemoteRepo> {
        let id = remote_repo_id
            .or(bound_remote_repo_id)
            .ok_or_else(|| Error::repo("no remote repo given and none is bound"))?;
        check_key_segment("remote repo ID", id)?;
        let record = self.repo_record(id)?;
        Ok(RemoteRepo {
            id: id.to_owned(),
            scope_id: record.scope_id,
            name: record.name,
            metadata_hash_b3: record.metadata_hash_b3,
            chunk_map_hash_b3: record.chunk_map_hash_b3,
            updated_at: record.updated_at,
        })
    }

    fn list_snapshots(&self, repository: &RemoteRepo) -> Result<Vec<RemoteSnapshot>> {
        let prefix = snapshot_records_prefix(&repository.id);
        let mut snapshots = Vec::new();
        for key in self.store.list(&prefix)? {
            let Some(snapshot_id) = key
                .strip_prefix(&prefix)
                .and_then(|name| name.strip_suffix(".json"))
            else {
                continue;
            };
            if let Some(record) = self.load(&key)? {
                snapshots.push(remote_snapshot(snapshot_id, record));
            }
        }
        snapshots.sort_by(|left, right| left.snapshot_id.cmp(&right.snapshot_id));
        Ok(snapshots)
    }

    fn repo_metadata_source(&self, repository: &RemoteRepo) -> Result<RemoteArtifactSource> {
        self.source(repo_key(&repository.id, "repo_metadata"))
    }

    fn chunk_index_source(&self, repository: &RemoteRepo) -> Result<RemoteArtifactSource> {
        self.source(repo_key(&repository.id, "chunk_index"))
    }

    fn snapshot_sources(
        &self,
        repository: &RemoteRepo,
        snapshot: &RemoteSnapshot,
    ) -> Result<RemoteSnapshotSources> {
        let key = |file| snapshot_file_key(&repository.id, &snapshot.snapshot_id, file);
        Ok(RemoteSnapshotSources {
            metadata: self.source(key("metadata"))?,
            blueprint: self.source(key("blueprint"))?,
            product_metadata: if snapshot.product_metadata_hash_b3.is_empty() {
                None
            } else {
                Some(self.source(key("product_metadata"))?)
            },
        })
    }

    fn bundle_sources(
        &self,
        repository: &RemoteRepo,
        _snapshot: &RemoteSnapshot,
        bundle_ids: &[String],
    ) -> Result<Vec<RemoteBundleSource>> {
        bundle_ids
            .iter()
            .map(|bundle_id| {
                Ok(RemoteBundleSource {
                    bundle_id: bundle_id.clone(),
                    source: self.source(bundle_key(&repository.id, bundle_id)?)?,
                })
            })
            .collect()
    }
}

impl<S: RemoteObjectStore + 'static> RemoteRepoWriter for ObjectStoreRemote<S> {
    fn begin_push(&self, repository: &RemoteRepo) -> Result<()> {
        let key = push_lock_key(&repository.id);
        let lock = PushLock {
            token: lock_token(),
            started_at: now(),
        };
        if !self.store.put_if_absent(&key, &record_bytes(&lock)?)? {
            let started_at = self
                .load::<PushLock>(&key)
                .ok()
                .flatten()
                .map_or_else(|| "an unknown time".to_owned(), |held| held.started_at);
            return Err(Error::repo(format!(
                "another push to remote repo '{}' has been running since {started_at}; \
                 if no push is running, delete the remote object '{key}' and push again",
                repository.id
            )));
        }
        self.held_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(repository.id.clone(), lock.token);
        Ok(())
    }

    fn finish_push(&self, repository: &RemoteRepo, succeeded: bool) -> Result<()> {
        let token = self
            .held_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&repository.id);
        let marked = if succeeded {
            self.mark_uploaded_snapshots_ready(&repository.id)
        } else {
            Ok(())
        };
        // Release only our own lock: someone may have deleted a stale one and
        // started another push.
        let key = push_lock_key(&repository.id);
        if let Some(token) = token
            && self
                .load::<PushLock>(&key)?
                .is_some_and(|held| held.token == token)
        {
            self.store.delete(&key)?;
        }
        marked
    }

    fn delete_snapshot(&self, repository: &RemoteRepo, snapshot: &RemoteSnapshot) -> Result<()> {
        let snapshot_id = &snapshot.snapshot_id;
        check_key_segment("snapshot ID", snapshot_id)?;
        self.store
            .delete(&snapshot_record_key(&repository.id, snapshot_id))?;
        for file in ["metadata", "blueprint", "product_metadata"] {
            self.store
                .delete(&snapshot_file_key(&repository.id, snapshot_id, file))?;
        }
        Ok(())
    }

    fn create_snapshot(
        &self,
        repository: &RemoteRepo,
        snapshot_id: &str,
        tags: &SnapshotTagSet,
    ) -> Result<RemoteSnapshot> {
        check_key_segment("snapshot ID", snapshot_id)?;
        let record = SnapshotRecord {
            tags: tags.to_canonical_strings(),
            created_at: now(),
            ..Default::default()
        };
        self.save(&snapshot_record_key(&repository.id, snapshot_id), &record)?;
        Ok(remote_snapshot(snapshot_id, record))
    }

    fn upload_artifact(
        &self,
        repository: &RemoteRepo,
        target: RemoteArtifactTarget<'_>,
        bytes: Vec<u8>,
        control: &OperationControl,
    ) -> Result<()> {
        control.checkpoint()?;
        let repo_id = repository.id.as_str();
        let hash = remote_artifact_hash(&bytes);
        let (snapshot, file) = match target {
            RemoteArtifactTarget::RepositoryMetadata => {
                self.store
                    .put(&repo_key(repo_id, "repo_metadata"), &bytes)?;
                return self.update_repo_record(repo_id, |record| record.metadata_hash_b3 = hash);
            }
            RemoteArtifactTarget::ChunkIndex => {
                self.store.put(&repo_key(repo_id, "chunk_index"), &bytes)?;
                return self.update_repo_record(repo_id, |record| record.chunk_map_hash_b3 = hash);
            }
            RemoteArtifactTarget::Bundle(bundle_id) => {
                return self.store.put(&bundle_key(repo_id, bundle_id)?, &bytes);
            }
            RemoteArtifactTarget::SnapshotMetadata(snapshot) => (snapshot, "metadata"),
            RemoteArtifactTarget::SnapshotBlueprint(snapshot) => (snapshot, "blueprint"),
            RemoteArtifactTarget::SnapshotProductMetadata(snapshot) => {
                (snapshot, "product_metadata")
            }
        };
        let snapshot_id = &snapshot.snapshot_id;
        check_key_segment("snapshot ID", snapshot_id)?;
        self.store
            .put(&snapshot_file_key(repo_id, snapshot_id, file), &bytes)?;
        self.update_snapshot_record(repo_id, snapshot_id, |record| match file {
            "metadata" => record.metadata_hash_b3 = hash,
            "blueprint" => record.blueprint_hash_b3 = hash,
            _ => record.product_metadata_hash_b3 = hash,
        })
    }

    fn delete_snapshot_product_metadata(
        &self,
        repository: &RemoteRepo,
        snapshot: &RemoteSnapshot,
    ) -> Result<()> {
        let snapshot_id = &snapshot.snapshot_id;
        check_key_segment("snapshot ID", snapshot_id)?;
        self.store.delete(&snapshot_file_key(
            &repository.id,
            snapshot_id,
            "product_metadata",
        ))?;
        self.update_snapshot_record(&repository.id, snapshot_id, |record| {
            record.product_metadata_hash_b3.clear()
        })
    }

    fn bundle_exists(&self, repository: &RemoteRepo, bundle_id: &str) -> Result<bool> {
        self.store.exists(&bundle_key(&repository.id, bundle_id)?)
    }

    fn reconcile_bundles(
        &self,
        repository: &RemoteRepo,
        required_bundle_ids: &[String],
    ) -> Result<usize> {
        let prefix = bundles_prefix(&repository.id);
        let mut deleted = 0;
        for key in self.store.list(&prefix)? {
            let bundle_id = key.strip_prefix(&prefix).unwrap_or(&key);
            if !required_bundle_ids.iter().any(|id| id == bundle_id) {
                self.store.delete(&key)?;
                deleted += 1;
            }
        }
        Ok(deleted)
    }
}

/// Remote repository IDs are `SCOPE--NAME`.
impl<S: RemoteObjectStore + 'static> RemoteRepoService for ObjectStoreRemote<S> {
    fn repo_provider(&self) -> &dyn RemoteRepoProvider {
        self
    }

    fn list_repos(&self) -> Result<Vec<RemoteRepoSummary>> {
        let mut repos = Vec::new();
        for key in self.store.list("index/")? {
            let Some(repo_id) = key
                .strip_prefix("index/")
                .and_then(|name| name.strip_suffix(".json"))
            else {
                continue;
            };
            if let Some(record) = self.load::<RepoRecord>(&key)? {
                repos.push(RemoteRepoSummary {
                    scope_name: record.scope_id.clone(),
                    scope_slug: record.scope_id.clone(),
                    scope_id: record.scope_id,
                    repository_id: repo_id.to_owned(),
                    repository_name: record.name,
                });
            }
        }
        repos.sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
        Ok(repos)
    }

    fn create_repo(&self, scope_id: &str, name: &str) -> Result<String> {
        check_key_segment("scope", scope_id)?;
        check_key_segment("repository name", name)?;
        let repo_id = format!("{scope_id}--{name}");
        let record = RepoRecord {
            scope_id: scope_id.to_owned(),
            name: name.to_owned(),
            updated_at: now(),
            ..Default::default()
        };
        if !self
            .store
            .put_if_absent(&repo_record_key(&repo_id), &record_bytes(&record)?)?
        {
            return Err(Error::repo(format!(
                "remote repo '{repo_id}' already exists"
            )));
        }
        Ok(repo_id)
    }

    fn delete_repo(&self, repository: &RemoteRepo) -> Result<()> {
        let repo_id = repository.id.as_str();
        for snapshot in self.list_snapshots(repository)? {
            self.delete_snapshot(repository, &snapshot)?;
        }
        for key in self.store.list(&bundles_prefix(repo_id))? {
            self.store.delete(&key)?;
        }
        for name in ["repo_metadata", "chunk_index", "push.lock"] {
            self.store.delete(&repo_key(repo_id, name))?;
        }
        self.store.delete(&repo_record_key(repo_id))
    }

    /// A plain store has no tag rules of its own.
    fn tag_references(&self, _repository: &RemoteRepo) -> Result<Vec<TagReference>> {
        Ok(Vec::new())
    }
}

fn remote_snapshot(snapshot_id: &str, record: SnapshotRecord) -> RemoteSnapshot {
    RemoteSnapshot {
        remote_id: snapshot_id.to_owned(),
        snapshot_id: snapshot_id.to_owned(),
        metadata_hash_b3: record.metadata_hash_b3,
        product_metadata_hash_b3: record.product_metadata_hash_b3,
        blueprint_hash_b3: record.blueprint_hash_b3,
        uploaded_at: record.uploaded_at,
        created_at: record.created_at,
        tags: record.tags,
        artifact_state: None,
        artifact_ready: record.ready,
    }
}

/// Rejects values that would change the key layout.
fn check_key_segment(what: &str, value: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value != "."
        && value != ".."
        && !value
            .chars()
            .any(|character| matches!(character, '/' | '\\') || character.is_control());
    if valid {
        Ok(())
    } else {
        Err(Error::invalid_input(format!(
            "{what} '{value}' cannot be used in a remote object key"
        )))
    }
}

fn repo_record_key(repo_id: &str) -> String {
    format!("index/{repo_id}.json")
}

fn repo_key(repo_id: &str, name: &str) -> String {
    format!("repos/{repo_id}/{name}")
}

fn push_lock_key(repo_id: &str) -> String {
    repo_key(repo_id, "push.lock")
}

fn snapshot_records_prefix(repo_id: &str) -> String {
    format!("repos/{repo_id}/snapshot_records/")
}

fn snapshot_record_key(repo_id: &str, snapshot_id: &str) -> String {
    format!("{}{snapshot_id}.json", snapshot_records_prefix(repo_id))
}

fn snapshot_file_key(repo_id: &str, snapshot_id: &str, file: &str) -> String {
    format!("repos/{repo_id}/snapshots/{snapshot_id}/{file}")
}

fn bundles_prefix(repo_id: &str) -> String {
    format!("repos/{repo_id}/bundles/")
}

fn bundle_key(repo_id: &str, bundle_id: &str) -> Result<String> {
    check_key_segment("bundle ID", bundle_id)?;
    Ok(format!("{}{bundle_id}", bundles_prefix(repo_id)))
}

fn record_bytes<T: Serialize>(record: &T) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(record)
        .map_err(|error| Error::repo(format!("cannot encode a remote record: {error}")))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Distinguishes this process's lock from one another push took after a
/// stale lock was deleted.
fn lock_token() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
    format!(
        "{}-{nanos}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
