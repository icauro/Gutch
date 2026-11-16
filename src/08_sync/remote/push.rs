//! Push: plans and uploads local snapshots and artifacts to the remote.

use super::artifact_decision::hashes_differ;
use super::link::linked_remote_repo_id;
use super::local_hashes::local_chunk_map_hash_b3;
use super::local_hashes::local_repo_metadata_hash_b3;
use super::local_hashes::local_snapshot_blueprint_hash_b3;
use super::local_hashes::local_snapshot_metadata_hash_b3;
use super::local_hashes::local_snapshot_product_metadata_hash_b3;
use super::progress::RemoteProgress;
use super::provider::RemoteArtifactTarget;
use super::provider::RemoteRepo;
use super::provider::RemoteRepoProvider;
use super::provider::RemoteRepoReader;
use super::provider::RemoteRepoWriter;
use super::provider::RemoteSnapshot;
use super::provider::RepoPushPlan;
use super::provider::RepoPushSnapshotChange;
use super::provider::RepoPushStats;
use crate::Error;
use crate::Result;
use crate::filesystem;
use crate::manifest::SnapshotTagSet;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationControl;
use crate::progress::OperationKind;
use crate::progress::ProgressPhase;
use crate::repo::RepoStatus;
use crate::repo::coordination;
use crate::repo::index::load_chunk_index;
use crate::repo::index::required_bundle_ids;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::bundle_path;
use crate::repo::paths::load_repo_manifest;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use crate::repo::snapshot_manifests::serialize_snapshot_metadata_for_remote;
use crate::repo::snapshot_manifests::serialize_snapshot_product_metadata_for_remote;
use crate::repo::snapshot_manifests::snapshot_blueprint_path;
use crate::repo::status::read_repo_status;
use crate::repo::summarize_snapshot_tags;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

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

pub fn plan_repo_push<P: RemoteRepoReader + ?Sized>(
    repo_root: &Path,
    provider: &P,
) -> Result<RepoPushPlan> {
    let bound_remote_repo_id = linked_remote_repo_id(repo_root)?;
    let repository = provider.resolve_repo(None, Some(&bound_remote_repo_id))?;
    let snapshots = provider.list_snapshots(&repository)?;
    plan_push(repo_root, &repository, &snapshots)
}

fn collect_local_sync_snapshots(
    repo_root: &Path,
) -> Result<(RepoStatus, RepoPaths, Vec<LocalSyncSnapshot>)> {
    let status = read_repo_status(repo_root)?;
    if !status.is_repo {
        return Err(Error::repository(format!(
            "no gutch repo found at {}",
            repo_root.display()
        )));
    }
    let (repo_paths, _) = load_repo_manifest(repo_root)?;
    let mut local_snapshots = Vec::with_capacity(status.snapshots.len());
    for snapshot in &status.snapshots {
        let manifest = load_snapshot_manifest(&repo_paths, &snapshot.snapshot_id)?;
        let summary = summarize_snapshot_tags(&manifest.snapshot_tags);
        local_snapshots.push(LocalSyncSnapshot {
            snapshot_id: manifest.snapshot_id.clone(),
            version: summary.version_tag,
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
) -> RepoPushSnapshotChange {
    RepoPushSnapshotChange {
        snapshot_id: snapshot_id.to_string(),
        version,
        os,
        arch,
        artifact_state,
        tags,
        update_reason,
    }
}

fn remote_snapshot_change(snapshot: &RemoteSnapshot) -> RepoPushSnapshotChange {
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

pub(super) fn summarize_snapshot_tags_from_values(tags: &[String]) -> (String, String, String) {
    let tag_set = SnapshotTagSet::from_compact_strings(tags).unwrap_or_default();
    let summary = summarize_snapshot_tags(&tag_set);
    (summary.version_tag, summary.os, summary.arch)
}

pub(crate) fn plan_push(
    repo_root: &Path,
    repository: &RemoteRepo,
    remote_snapshots: &[RemoteSnapshot],
) -> Result<RepoPushPlan> {
    let (status, repo_paths, local_snapshots) = collect_local_sync_snapshots(repo_root)?;
    plan_push_from_local(
        &status,
        &repo_paths,
        &local_snapshots,
        repository,
        remote_snapshots,
    )
}

fn plan_push_from_local(
    status: &RepoStatus,
    repo_paths: &RepoPaths,
    local_snapshots: &[LocalSyncSnapshot],
    repository: &RemoteRepo,
    remote_snapshots: &[RemoteSnapshot],
) -> Result<RepoPushPlan> {
    let repo_manifest_changed = hashes_differ(
        &local_repo_metadata_hash_b3(repo_paths)?,
        &repository.metadata_hash_b3,
    );
    let chunk_index_changed = hashes_differ(
        &local_chunk_map_hash_b3(repo_paths)?,
        &repository.chunk_map_hash_b3,
    );
    let remote_by_snapshot_id: HashMap<&str, &RemoteSnapshot> = remote_snapshots
        .iter()
        .map(|snapshot| (snapshot.snapshot_id.as_str(), snapshot))
        .collect();
    let mut to_add = Vec::new();
    let mut to_update = Vec::new();
    let mut unchanged = Vec::new();
    for local in local_snapshots {
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
    Ok(RepoPushPlan {
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

/// The inputs shared by every phase of one push.
struct PushContext<'a, P: RemoteRepoWriter + ?Sized> {
    provider: &'a P,
    repository: &'a RemoteRepo,
    repo_paths: &'a RepoPaths,
    control: &'a OperationControl,
}

impl<P: RemoteRepoWriter + ?Sized> PushContext<'_, P> {
    /// Uploads a snapshot's metadata, blueprint and product metadata as three
    /// progress steps.
    fn upload_snapshot(
        &self,
        progress: &mut RemoteProgress,
        stats: &mut RepoPushStats,
        manifest: &SnapshotManifest,
        remote_snapshot: &RemoteSnapshot,
    ) -> Result<()> {
        let snapshot_id = &manifest.snapshot_id;
        self.control.checkpoint()?;
        progress.next_step(
            ProgressPhase::UploadSnapshotMetadata,
            "Uploading snapshot metadata",
            Some(snapshot_id.clone()),
        );
        self.provider.upload_artifact(
            self.repository,
            RemoteArtifactTarget::SnapshotMetadata(remote_snapshot),
            serialize_snapshot_metadata_for_remote(manifest)?,
            self.control,
        )?;
        stats.snapshot_manifest_uploaded_count += 1;

        self.control.checkpoint()?;
        progress.next_step(
            ProgressPhase::UploadSnapshotBlueprint,
            "Uploading snapshot blueprints",
            Some(snapshot_id.clone()),
        );
        self.provider.upload_artifact(
            self.repository,
            RemoteArtifactTarget::SnapshotBlueprint(remote_snapshot),
            filesystem::read(&snapshot_blueprint_path(self.repo_paths, snapshot_id)?)?,
            self.control,
        )?;
        stats.snapshot_blueprint_uploaded_count += 1;

        self.control.checkpoint()?;
        progress.next_step(
            ProgressPhase::UploadSnapshotMetadata,
            "Uploading snapshot product metadata",
            Some(snapshot_id.clone()),
        );
        if let Some(bytes) = serialize_snapshot_product_metadata_for_remote(manifest)? {
            self.provider.upload_artifact(
                self.repository,
                RemoteArtifactTarget::SnapshotProductMetadata(remote_snapshot),
                bytes,
                self.control,
            )?;
            stats.product_metadata_uploaded_count += 1;
        } else if !remote_snapshot.product_metadata_hash_b3.trim().is_empty() {
            self.provider
                .delete_snapshot_product_metadata(self.repository, remote_snapshot)?;
        }
        Ok(())
    }

    /// Uploads each bundle the local snapshots need that the remote does not
    /// have yet, one progress step per bundle.
    fn upload_bundles(
        &self,
        progress: &mut RemoteProgress,
        stats: &mut RepoPushStats,
        required_bundle_ids: &[String],
    ) -> Result<()> {
        for bundle_id in required_bundle_ids {
            self.control.checkpoint()?;
            let bundle_id = bundle_id.clone();
            let bundle_path = bundle_path(self.repo_paths, &bundle_id)?;
            if self.provider.bundle_exists(self.repository, &bundle_id)? {
                stats.bundle_skipped_count += 1;
                progress.next_step(
                    ProgressPhase::UploadBundle,
                    "Skipping bundles already present remotely",
                    Some(bundle_id),
                );
                continue;
            }
            let bundle_bytes = filesystem::read(&bundle_path)?;
            let bundle_size = bundle_bytes.len() as u64;
            progress.next_step(
                ProgressPhase::UploadBundle,
                "Uploading bundles",
                Some(bundle_id.clone()),
            );
            self.provider.upload_artifact(
                self.repository,
                RemoteArtifactTarget::Bundle(&bundle_id),
                bundle_bytes,
                self.control,
            )?;
            stats.bundle_uploaded_count += 1;
            progress.report(
                ProgressPhase::UploadBundle,
                "Uploaded bundle",
                Some(format!("{bundle_id} ({bundle_size} bytes)")),
            );
        }
        Ok(())
    }
}

/// Lists the bundles that the local snapshots need, in sorted order.
fn required_local_bundle_ids(repo_paths: &RepoPaths, status: &RepoStatus) -> Result<Vec<String>> {
    let chunk_index = load_chunk_index(repo_paths)?;
    let mut ids = BTreeSet::new();
    for snapshot in &status.snapshots {
        let manifest = load_snapshot_manifest(repo_paths, &snapshot.snapshot_id)?;
        ids.extend(required_bundle_ids(&manifest, &chunk_index)?);
    }
    Ok(ids.into_iter().collect())
}

pub fn push_repo<P: RemoteRepoProvider + ?Sized>(
    repo: &Path,
    provider: &P,
    control: &OperationControl,
) -> Result<RepoPushStats> {
    coordination::write_with_control(repo, control, || {
        control.checkpoint()?;
        let mut progress = RemoteProgress::new(control, OperationKind::Push);
        let bound_remote_repo_id = linked_remote_repo_id(repo)?;
        let repository = provider.resolve_repo(None, Some(&bound_remote_repo_id))?;
        progress.report(ProgressPhase::Planning, "Planning sync changes", None);
        let remote_snapshots = provider.list_snapshots(&repository)?;
        let (status, repo_paths, local_snapshots) = collect_local_sync_snapshots(repo)?;
        let plan = plan_push_from_local(
            &status,
            &repo_paths,
            &local_snapshots,
            &repository,
            &remote_snapshots,
        )?;
        control.checkpoint()?;
        let required_bundle_ids = required_local_bundle_ids(&repo_paths, &status)?;
        // Fail before any upload so the remote never records an unbacked snapshot.
        for bundle_id in &required_bundle_ids {
            if !bundle_path(&repo_paths, bundle_id)?.is_file() {
                return Err(Error::artifact(format!(
                    "bundle '{bundle_id}' is required by a local snapshot but missing; run repo verify before pushing"
                )));
            }
        }
        let has_chunk_index = repo_paths.chunk_map_path.exists();
        let snapshot_upload_count = plan.to_add.len() + plan.to_update.len();
        progress.set_total_steps(
            plan.to_remove.len()
                + 1
                + usize::from(has_chunk_index)
                + snapshot_upload_count * 3
                + required_bundle_ids.len()
                + 1,
        );
        let mut stats = RepoPushStats {
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
            bundle_count: required_bundle_ids.len(),
            snapshot_added_count: plan.to_add.len(),
            snapshot_removed_count: plan.to_remove.len(),
            snapshot_updated_count: plan.to_update.len(),
            snapshot_unchanged_count: plan.unchanged.len(),
        };
        let context = PushContext {
            provider,
            repository: &repository,
            repo_paths: &repo_paths,
            control,
        };

        let mut remote_by_snapshot_id: HashMap<String, RemoteSnapshot> = remote_snapshots
            .into_iter()
            .map(|snapshot| (snapshot.snapshot_id.clone(), snapshot))
            .collect();
        for change in &plan.to_remove {
            control.checkpoint()?;
            progress.next_step(
                ProgressPhase::RemoveSnapshots,
                "Removing remote snapshots",
                Some(change.snapshot_id.clone()),
            );
            if let Some(snapshot) = remote_by_snapshot_id.remove(&change.snapshot_id) {
                provider.delete_snapshot(&repository, &snapshot)?;
            }
        }

        control.checkpoint()?;
        progress.next_step(
            ProgressPhase::UploadRepoMetadata,
            "Uploading repo metadata",
            None,
        );
        if plan.repo_manifest_changed {
            provider.upload_artifact(
                &repository,
                RemoteArtifactTarget::RepositoryMetadata,
                filesystem::read(&repo_paths.repo_metadata_path)?,
                control,
            )?;
            stats.repo_manifest_uploaded = true;
        }
        if has_chunk_index {
            control.checkpoint()?;
            progress.next_step(
                ProgressPhase::UploadChunkIndex,
                "Uploading chunk index",
                None,
            );
            if plan.chunk_index_changed {
                provider.upload_artifact(
                    &repository,
                    RemoteArtifactTarget::ChunkIndex,
                    filesystem::read(&repo_paths.chunk_map_path)?,
                    control,
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
            context.upload_snapshot(&mut progress, &mut stats, &manifest, &remote_snapshot)?;
        }

        context.upload_bundles(&mut progress, &mut stats, &required_bundle_ids)?;

        control.checkpoint()?;
        progress.next_step(
            ProgressPhase::Prune,
            "Removing unreferenced remote bundles",
            None,
        );
        stats.remote_bundle_deleted_count =
            provider.reconcile_bundles(&repository, &required_bundle_ids)?;
        progress.finish("Sync complete");
        Ok(stats)
    })
}
