use super::*;
pub(super) fn compute_pull_plan(
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
    ) == ArtifactDecision::Fetch;
    let chunk_map_needed = decide_artifact(
        repo_paths.chunk_map_path.exists(),
        local_chunk_map_hash_b3(repo_paths)?,
        &repository.chunk_map_hash_b3,
        &repository.updated_at,
        local_file_mtime(&repo_paths.chunk_map_path),
        policy,
    ) == ArtifactDecision::Fetch;

    let mut snapshots_to_download = Vec::new();
    let mut snapshots_to_skip = Vec::new();
    let mut kept_newer_local_snapshots = Vec::new();
    for snapshot in snapshots {
        // A healthy local installation does not need another repository's layout.
        // This also avoids attempting to fetch local-only chunks from that layout.
        if hashes_differ(
            &local_snapshot_blueprint_hash_b3(repo_paths, &snapshot.snapshot_id)?,
            &snapshot.blueprint_hash_b3,
        ) && let Ok(local) = load_snapshot_manifest(repo_paths, &snapshot.snapshot_id)
            && crate::snapshot::identity::compute_snapshot_id_from_entries(&local.entries)
                == snapshot.snapshot_id
            && let Ok((_, local_repo)) = load_gutch_repo_manifest(repo_root)
            && let Ok(index) = load_chunk_index(repo_paths)
            && crate::repo::verify::verify_snapshot_artifacts(
                repo_paths,
                &index,
                &local_repo.compression_profile(),
                &snapshot.snapshot_id,
                crate::installation::inspect::VerifyMode::Strict,
            )
            .is_ok_and(|result| result.ok)
        {
            snapshots_to_skip.push(snapshot.snapshot_id.clone());
            continue;
        }
        let updated_at = if snapshot.uploaded_at.is_empty() {
            &snapshot.created_at
        } else {
            &snapshot.uploaded_at
        };
        let metadata_path = crate::repo::snapshot_manifests::snapshot_metadata_path(
            repo_paths,
            &snapshot.snapshot_id,
        )?;
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
            crate::repo::snapshot_manifests::snapshot_product_metadata_path(
                repo_paths,
                &snapshot.snapshot_id,
            )?;
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
        for snapshot_id in crate::repo::inventory::list_repo_snapshot_ids(repo_paths)? {
            if !remote_snapshot_ids.contains(snapshot_id.as_str()) {
                local_only_snapshots.push(snapshot_id);
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

fn source_bytes(
    source: &RemoteArtifactSource,
    client: &crate::transfer::http::DownloadClient,
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
            crate::transfer::http::download_url_streaming(
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
    let mut metadata = metadata.clone();
    metadata.snapshot_id = snapshot_id.to_string();
    let manifest = crate::repo::snapshot_manifests::snapshot_manifest_from_parts(
        metadata,
        product_metadata.cloned(),
        blueprint_bytes,
    )?;
    crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts(repo_paths, &manifest)?;
    Ok(())
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

pub(super) fn required_bundle_ids_by_snapshot(
    repo_paths: &GutchRepoPaths,
) -> Result<Vec<(String, Vec<String>)>> {
    let chunk_index = load_chunk_index(repo_paths)?;
    let mut by_snapshot = Vec::new();
    if !repo_paths.snapshots_root.exists() {
        return Ok(by_snapshot);
    }
    for snapshot_id in crate::repo::inventory::list_repo_snapshot_ids(repo_paths)? {
        let manifest = load_snapshot_manifest(repo_paths, &snapshot_id)?;
        let mut ids: Vec<String> = required_bundle_ids(&manifest, &chunk_index)?
            .into_iter()
            .collect();
        ids.sort();
        by_snapshot.push((snapshot_id, ids));
    }
    Ok(by_snapshot)
}

pub(super) fn local_bundle_paths(bundles_root: &Path) -> Result<Vec<PathBuf>> {
    if !bundles_root.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in walkdir::WalkDir::new(bundles_root).sort_by_file_name() {
        let entry = entry?;
        if (entry.file_type().is_file()
            || (entry.file_type().is_symlink() && entry.path().is_file()))
            && crate::repo::paths::bundle_id_from_bundle_path(entry.path()).is_some()
        {
            paths.push(entry.into_path());
        }
    }
    Ok(paths)
}

pub(super) fn record_bundle_sources(
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

pub(super) fn bundle_source_requests(
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
    let operation_root = (&repo).to_path_buf();
    std::fs::create_dir_all(&operation_root)?;
    crate::repo::coordination::write(&operation_root, || {
        control.checkpoint()?;
        let progress = RemoteProgress::new(&control, GutchOperation::Pull);
        let bound_remote_repo_id = bound_remote_repo_id(&repo)?;
        let repository =
            provider.resolve_repository(remote_spec.as_deref(), bound_remote_repo_id.as_deref())?;
        let repo_paths = GutchRepoPaths::from_repo_root(&repo);
        let repository_existed = repo_paths.repo_metadata_path.exists();
        crate::repo::paths::ensure_gutch_repo_layout(&repo_paths, &repository.name)?;
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
        let download_client = crate::transfer::http::build_download_client()?;

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
                crate::filesystem::from_json_bytes(&bytes)?;
            let local_constraints = load_gutch_repo_manifest(&repo)?
                .1
                .constraints
                .unwrap_or_default();
            let incoming_constraints = manifest.constraints.clone().unwrap_or_default();
            if repository_existed && local_constraints != incoming_constraints {
                return Err(Error::manifest(
                    "remote repository constraints differ; explicitly update local constraints before pulling",
                ));
            }
            crate::repo::updates::validate_existing_constraints(
                &repo,
                &incoming_constraints,
                "pull",
            )?;
            manifest.remote_repo_id = repository.id.clone();
            crate::repo::paths::write_gutch_repo_manifest(&repo_paths, &manifest)?;
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
            let mut chunk_index =
                crate::repo::index::binary_codec::load_chunk_index_binary_from_bytes(&bytes)?;
            for (hash, entry) in load_chunk_index(&repo_paths)?.entries {
                let artifact =
                    crate::repo::artifacts::ArtifactRef::from_chunk_entry(&hash.to_hex(), &entry);
                if artifact.local_path(&repo_paths)?.is_file()
                    || chunk_index.entries.get_hash(&hash).is_none()
                {
                    chunk_index.entries.insert(hash, entry);
                }
            }
            crate::repo::index::write_chunk_index(&repo_paths, &chunk_index)?;
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
            let metadata = crate::transfer::download::snapshot_manifest_from_download(
                snapshot_id,
                &metadata_bytes,
            )?;
            let product_metadata = if let Some(source) = sources.product_metadata {
                let product_metadata_bytes =
                    source_bytes(&source, &download_client, MAX_REMOTE_JSON_BYTES, &control)?;
                verify_remote_artifact(
                    &product_metadata_bytes,
                    &snapshot.product_metadata_hash_b3,
                    "snapshot product metadata",
                )?;
                let product_metadata: crate::manifest::SnapshotProductMetadata =
                    crate::filesystem::from_json_bytes(&product_metadata_bytes)?;
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
            GutchProgressPhase::Artifact,
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
            if !crate::repo::paths::bundle_path(&repo_paths, id)?.is_file() {
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
                GutchProgressPhase::Artifact,
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
            let bundle_path = crate::repo::paths::bundle_path(&repo_paths, bundle_id)?;
            crate::filesystem::write(&bundle_path, &bytes)?;
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
                crate::repo::cleanup::remove_snapshot_from_repo(&repo, snapshot_id)?;
                stats.pruned_snapshot_count += 1;
            }
            for path in local_bundle_paths(&repo_paths.bundles_root)? {
                control.checkpoint()?;
                if let Some(bundle_id) = crate::repo::paths::bundle_id_from_bundle_path(&path)
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
    })
}

#[cfg(test)]
mod constraint_tests {
    use super::*;
    #[test]
    fn incoming_snapshot_is_rejected_before_metadata_is_written() {
        let root = tempfile::tempdir().unwrap();
        let paths = GutchRepoPaths::from_repo_root(root.path());
        crate::repo::paths::ensure_gutch_repo_layout(&paths, "test").unwrap();
        crate::repo::updates::update_repository_constraints(
            root.path(),
            crate::api::RepositoryConstraints {
                targets: vec![crate::api::FilesystemTarget::WindowsNtfs],
                ..Default::default()
            },
        )
        .unwrap();
        let entries = vec![crate::manifest::SnapshotEntry::Directory(
            crate::manifest::DirectoryManifest {
                path: "CON".into(),
                read_only: false,
            },
        )];
        let id = crate::snapshot::identity::compute_snapshot_id_from_entries(&entries);
        let mut manifest = crate::manifest::SnapshotManifest::new(id.clone(), Default::default());
        manifest.entries = entries;
        let blueprint_path = root.path().join("incoming.gtcbin");
        crate::manifest::blueprint_codec::write_snapshot_blueprint_binary(
            &blueprint_path,
            &manifest.blueprint(),
        )
        .unwrap();
        let bytes = std::fs::read(blueprint_path).unwrap();
        let error =
            write_remote_snapshot_artifacts(&paths, &id, &manifest, None, &bytes).unwrap_err();
        assert!(matches!(error, Error::PathValidation(_)));
        assert!(!paths.snapshots_root.join(id).exists());
    }
}
