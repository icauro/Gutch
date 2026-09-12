use super::*;
pub fn plan_push_with_provider<P: RemoteRepositoryReader + ?Sized>(
    repo_root: &Path,
    provider: &P,
) -> Result<RepoPushPlan> {
    let bound_remote_repo_id = linked_remote_repo_id(repo_root)?;
    let repository = provider.resolve_repository(None, Some(&bound_remote_repo_id))?;
    let snapshots = provider.list_snapshots(&repository)?;
    plan_push(repo_root, &repository, &snapshots)
}

fn collect_local_sync_snapshots(
    repo_root: &Path,
) -> Result<(RepoStatus, GutchRepoPaths, Vec<LocalSyncSnapshot>)> {
    let status = crate::repo::status::read_repo_status(repo_root)?;
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
    (summary.version_id, summary.os, summary.arch)
}

pub(crate) fn plan_push(
    repo_root: &Path,
    repository: &RemoteRepository,
    remote_snapshots: &[RemoteSnapshot],
) -> Result<RepoPushPlan> {
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

struct SnapshotUploadContext<'a, P: RemoteRepositoryWriter + ?Sized> {
    provider: &'a P,
    repository: &'a RemoteRepository,
    repo_paths: &'a GutchRepoPaths,
    control: &'a GutchOperationControl,
    stats: &'a mut RepoPushStats,
    progress: &'a RemoteProgress,
    step: &'a mut usize,
    total_steps: usize,
}

fn write_snapshot_uploads<P: RemoteRepositoryWriter + ?Sized>(
    context: &mut SnapshotUploadContext<'_, P>,
    snapshot: &crate::repo::status::RepoSnapshotStatus,
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
        crate::repo::snapshot_manifests::serialize_snapshot_metadata_for_remote(&manifest)?,
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
    let blueprint_path = crate::repo::snapshot_manifests::snapshot_blueprint_path(
        context.repo_paths,
        &snapshot.snapshot_id,
    )?;
    context.provider.upload_snapshot_blueprint(
        context.repository,
        remote_snapshot,
        crate::filesystem::read(&blueprint_path)?,
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
    if let Some(bytes) =
        crate::repo::snapshot_manifests::serialize_snapshot_product_metadata_for_remote(&manifest)?
    {
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

pub fn push_repo<P: RemoteRepositoryProvider + ?Sized>(
    repo: PathBuf,
    provider: &P,
    control: GutchOperationControl,
) -> Result<RepoPushStats> {
    let operation_root = (&repo).to_path_buf();
    crate::repo::coordination::write(&operation_root, || {
        control.checkpoint()?;
        let progress = RemoteProgress::new(&control, GutchOperation::Push);
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
        let plan = plan_push(&repo, &repository, &remote_snapshots)?;
        control.checkpoint()?;
        let (status, repo_paths, _) = collect_local_sync_snapshots(&repo)?;
        let bundle_paths = local_bundle_paths(&repo_paths.bundles_root)?;
        let required_bundle_ids = {
            let chunk_index = load_chunk_index(&repo_paths)?;
            let mut ids = BTreeSet::new();
            for snapshot in &status.snapshots {
                let manifest = load_snapshot_manifest(&repo_paths, &snapshot.snapshot_id)?;
                ids.extend(required_bundle_ids(&manifest, &chunk_index)?);
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
                crate::filesystem::read(&repo_paths.repo_metadata_path)?,
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
                    crate::filesystem::read(&repo_paths.chunk_map_path)?,
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
            let Some(bundle_id) = crate::repo::paths::bundle_id_from_bundle_path(&bundle_path)
            else {
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
            let bundle_bytes = crate::filesystem::read(&bundle_path)?;
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
    })
}
