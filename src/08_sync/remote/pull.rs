//! Pull: plans and downloads remote snapshots and bundles into the local repository.

use super::ArtifactDecision;
use super::MAX_REMOTE_BINARY_BYTES;
use super::MAX_REMOTE_JSON_BYTES;
use super::RemoteProgress;
use super::decide_artifact;
use super::hashes_differ;
use super::link::bound_remote_repo_id;
use super::local_chunk_map_hash_b3;
use super::local_file_mtime;
use super::local_repo_metadata_hash_b3;
use super::local_snapshot_blueprint_hash_b3;
use super::local_snapshot_metadata_hash_b3;
use super::local_snapshot_product_metadata_hash_b3;
use super::provider::PullConflictPolicy;
use super::provider::RemoteArtifactSource;
use super::provider::RemoteBundleSource;
use super::provider::RemoteRepository;
use super::provider::RemoteRepositoryReader;
use super::provider::RemoteSnapshot;
use super::provider::RepoPullPlan;
use super::provider::RepoPullStats;
use super::verify_remote_artifact;
use crate::Error;
use crate::Result;
use crate::artifact::bundle::BundleIdentityWriter;
use crate::filesystem::from_json_bytes;
use crate::filesystem::write_atomic_with;
use crate::manifest::SnapshotProductMetadata;
use crate::manifest::repo_manifest::RepoManifest;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationControl;
use crate::progress::OperationKind;
use crate::progress::ProgressPhase;
use crate::repo::cleanup::remove_snapshot_from_repo;
use crate::repo::coordination;
use crate::repo::index::binary_codec::load_chunk_index_binary_from_bytes;
use crate::repo::index::load_chunk_index;
use crate::repo::index::preserve_local_chunk_locations;
use crate::repo::index::required_bundle_ids;
use crate::repo::index::write_chunk_index;
use crate::repo::inventory::list_repo_snapshot_ids;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::bundle_id_from_bundle_path;
use crate::repo::paths::bundle_path;
use crate::repo::paths::ensure_gutch_repo_layout;
use crate::repo::paths::load_gutch_repo_manifest;
use crate::repo::paths::write_gutch_repo_manifest;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use crate::repo::snapshot_manifests::snapshot_manifest_from_download;
use crate::repo::snapshot_manifests::snapshot_manifest_from_parts;
use crate::repo::snapshot_manifests::snapshot_metadata_path;
use crate::repo::snapshot_manifests::snapshot_product_metadata_path;
use crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts;
use crate::repo::updates::validate_existing_constraints;
use crate::repo::verify::RepoArtifactVerification;
use crate::repo::verify::verify_snapshot_artifacts;
use crate::snapshot::identity::compute_snapshot_id_from_entries;
use crate::transfer::http::DownloadClient;
use crate::transfer::http::build_download_client;
use crate::transfer::http::download_url_streaming;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::io::BufWriter;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
pub(super) fn compute_pull_plan(
    repo_root: &Path,
    repo_paths: &RepoPaths,
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
            && compute_snapshot_id_from_entries(&local.entries) == snapshot.snapshot_id
            && let Ok((_, local_repo)) = load_gutch_repo_manifest(repo_root)
            && let Ok(index) = load_chunk_index(repo_paths)
            && verify_snapshot_artifacts(
                repo_paths,
                &index,
                &local_repo.compression_profile(),
                &snapshot.snapshot_id,
                RepoArtifactVerification::Identity,
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
        let metadata_path = snapshot_metadata_path(repo_paths, &snapshot.snapshot_id)?;
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
            snapshot_product_metadata_path(repo_paths, &snapshot.snapshot_id)?;
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
        for snapshot_id in list_repo_snapshot_ids(repo_paths)? {
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
    let repo_paths = RepoPaths::from_repo_root(repo_root);
    compute_pull_plan(repo_root, &repo_paths, repository, snapshots, policy)
}

fn source_bytes(
    source: &RemoteArtifactSource,
    client: &DownloadClient,
    maximum_bytes: u64,
    control: &OperationControl,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    write_source(source, client, maximum_bytes, control, &mut bytes)?;
    Ok(bytes)
}

fn write_source(
    source: &RemoteArtifactSource,
    client: &DownloadClient,
    maximum_bytes: u64,
    control: &OperationControl,
    output: &mut impl Write,
) -> Result<()> {
    control.checkpoint()?;
    match source {
        RemoteArtifactSource::Bytes(bytes) => {
            if bytes.len() as u64 > maximum_bytes {
                return Err(Error::repository(
                    "remote artifact exceeded the allowed size",
                ));
            }
            output.write_all(bytes)?;
            Ok(())
        }
        RemoteArtifactSource::Url(url) => download_url_streaming(
            client,
            url,
            control,
            Some(maximum_bytes),
            |_| Ok(()),
            |chunk| {
                output.write_all(chunk)?;
                Ok(())
            },
        ),
    }
}

fn download_and_publish_bundle(
    source: &RemoteArtifactSource,
    client: &DownloadClient,
    bundle_id: &str,
    path: &Path,
    control: &OperationControl,
) -> Result<()> {
    control.checkpoint()?;
    write_atomic_with(path, |file| {
        // Hash while streaming so the bundle is not read back from disk. Keep
        // verification inside the atomic writer: failure leaves the previous
        // cached artifact in place and drops only the owned temporary file.
        let mut writer = BundleIdentityWriter::new(BufWriter::new(file));
        write_source(
            source,
            client,
            MAX_REMOTE_BINARY_BYTES,
            control,
            &mut writer,
        )?;
        writer
            .finish(bundle_id)?
            .into_inner()
            .map_err(|error| Error::Io(error.into_error()))?;
        Ok(())
    })
}

fn write_remote_snapshot_artifacts(
    repo_paths: &RepoPaths,
    snapshot_id: &str,
    metadata: &SnapshotManifest,
    product_metadata: Option<&SnapshotProductMetadata>,
    blueprint_bytes: &[u8],
    control: &OperationControl,
) -> Result<()> {
    let mut metadata = metadata.clone();
    metadata.snapshot_id = snapshot_id.to_string();
    let manifest =
        snapshot_manifest_from_parts(metadata, product_metadata.cloned(), blueprint_bytes)?;
    write_snapshot_manifest_artifacts(repo_paths, &manifest, control)?;
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

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SnapshotBundleIds {
    pub(super) snapshot_id: String,
    pub(super) bundle_ids: Vec<String>,
}

pub(super) fn required_bundle_ids_by_snapshot(
    repo_paths: &RepoPaths,
) -> Result<Vec<SnapshotBundleIds>> {
    let chunk_index = load_chunk_index(repo_paths)?;
    let mut by_snapshot = Vec::new();
    if !repo_paths.snapshots_root.exists() {
        return Ok(by_snapshot);
    }
    for snapshot_id in list_repo_snapshot_ids(repo_paths)? {
        let manifest = load_snapshot_manifest(repo_paths, &snapshot_id)?;
        let mut ids: Vec<String> = required_bundle_ids(&manifest, &chunk_index)?
            .into_iter()
            .collect();
        ids.sort();
        by_snapshot.push(SnapshotBundleIds {
            snapshot_id,
            bundle_ids: ids,
        });
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
            && bundle_id_from_bundle_path(entry.path()).is_some()
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
            return Err(Error::repository(format!(
                "remote provider returned an unexpected bundle '{}'",
                source.bundle_id
            )));
        }
        if bundle_sources
            .insert(source.bundle_id.clone(), source.source)
            .is_some()
        {
            return Err(Error::repository(format!(
                "remote provider returned duplicate bundle '{}'",
                source.bundle_id
            )));
        }
    }
    Ok(())
}

pub(super) fn bundle_source_requests(
    bundles_by_snapshot: &[SnapshotBundleIds],
    missing_bundles: &[String],
) -> Vec<SnapshotBundleIds> {
    let mut unresolved: HashSet<&str> = missing_bundles.iter().map(String::as_str).collect();
    let mut requests = Vec::new();
    for SnapshotBundleIds {
        snapshot_id,
        bundle_ids,
    } in bundles_by_snapshot
    {
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
        requests.push(SnapshotBundleIds {
            snapshot_id: snapshot_id.clone(),
            bundle_ids: missing_for_snapshot,
        });
    }
    requests
}

/// The inputs shared by every phase of one pull.
struct PullContext<'a, P: RemoteRepositoryReader + ?Sized> {
    repo: &'a Path,
    repo_paths: &'a RepoPaths,
    repository: &'a RemoteRepository,
    provider: &'a P,
    download_client: DownloadClient,
    control: &'a OperationControl,
}

impl<P: RemoteRepositoryReader + ?Sized> PullContext<'_, P> {
    fn download(&self, source: &RemoteArtifactSource, maximum_bytes: u64) -> Result<Vec<u8>> {
        source_bytes(source, &self.download_client, maximum_bytes, self.control)
    }

    /// Replaces the local repo manifest with the remote one. Constraints
    /// never change through a pull; they must already match.
    fn download_repo_manifest(&self, repository_existed: bool) -> Result<()> {
        let bytes = self.download(
            &self.provider.repository_metadata_source(self.repository)?,
            MAX_REMOTE_JSON_BYTES,
        )?;
        verify_remote_artifact(&bytes, &self.repository.metadata_hash_b3, "repo metadata")?;
        let mut manifest: RepoManifest = from_json_bytes(&bytes)?;
        let local_constraints = load_gutch_repo_manifest(self.repo)?
            .1
            .constraints
            .unwrap_or_default();
        let incoming_constraints = manifest.constraints.clone().unwrap_or_default();
        if repository_existed && local_constraints != incoming_constraints {
            return Err(Error::repository(
                "remote repository constraints differ; explicitly update local constraints before pulling",
            ));
        }
        validate_existing_constraints(self.repo, &incoming_constraints, "pull")?;
        manifest.remote_repo_id = self.repository.id.clone();
        write_gutch_repo_manifest(self.repo_paths, &manifest)
    }

    fn download_chunk_index(&self) -> Result<()> {
        let bytes = self.download(
            &self.provider.chunk_index_source(self.repository)?,
            MAX_REMOTE_JSON_BYTES,
        )?;
        verify_remote_artifact(&bytes, &self.repository.chunk_map_hash_b3, "chunk index")?;
        let mut chunk_index = load_chunk_index_binary_from_bytes(&bytes)?;
        preserve_local_chunk_locations(self.repo_paths, &mut chunk_index)?;
        write_chunk_index(self.repo_paths, &chunk_index)
    }

    fn download_snapshot(&self, snapshot: &RemoteSnapshot) -> Result<()> {
        let snapshot_id = snapshot.snapshot_id.as_str();
        let sources = self.provider.snapshot_sources(self.repository, snapshot)?;
        let metadata_bytes = self.download(&sources.metadata, MAX_REMOTE_JSON_BYTES)?;
        verify_remote_artifact(
            &metadata_bytes,
            &snapshot.metadata_hash_b3,
            "snapshot metadata",
        )?;
        let metadata = snapshot_manifest_from_download(snapshot_id, &metadata_bytes)?;
        let product_metadata = match sources.product_metadata {
            Some(source) => {
                let bytes = self.download(&source, MAX_REMOTE_JSON_BYTES)?;
                verify_remote_artifact(
                    &bytes,
                    &snapshot.product_metadata_hash_b3,
                    "snapshot product metadata",
                )?;
                Some(SnapshotProductMetadata::from_json_bytes(&bytes)?)
            }
            None => None,
        };
        let blueprint_bytes = self.download(&sources.blueprint, MAX_REMOTE_BINARY_BYTES)?;
        verify_remote_artifact(
            &blueprint_bytes,
            &snapshot.blueprint_hash_b3,
            "snapshot blueprint",
        )?;
        write_remote_snapshot_artifacts(
            self.repo_paths,
            snapshot_id,
            &metadata,
            product_metadata.as_ref(),
            &blueprint_bytes,
            self.control,
        )
    }

    /// Asks the provider for a source for each missing bundle, one request per
    /// snapshot that references them.
    fn missing_bundle_sources(
        &self,
        bundles_by_snapshot: &[SnapshotBundleIds],
        missing_bundles: &[String],
        snapshots_by_id: &HashMap<&str, &RemoteSnapshot>,
    ) -> Result<HashMap<String, RemoteArtifactSource>> {
        let mut bundle_sources = HashMap::new();
        for SnapshotBundleIds {
            snapshot_id,
            bundle_ids,
        } in bundle_source_requests(bundles_by_snapshot, missing_bundles)
        {
            let snapshot = snapshots_by_id.get(snapshot_id.as_str()).ok_or_else(|| {
                Error::repository(format!(
                    "remote snapshot '{snapshot_id}' disappeared during bundle resolution"
                ))
            })?;
            let expected_bundle_ids: HashSet<&str> =
                bundle_ids.iter().map(String::as_str).collect();
            record_bundle_sources(
                &mut bundle_sources,
                &expected_bundle_ids,
                self.provider
                    .bundle_sources(self.repository, snapshot, &bundle_ids)?,
            )?;
        }
        Ok(bundle_sources)
    }

    /// Removes local snapshots the remote does not have and bundles no
    /// remaining snapshot needs. Returns the removed snapshot and bundle counts.
    fn prune(
        &self,
        local_only_snapshots: &[String],
        required_bundles: &HashSet<String>,
    ) -> Result<(usize, usize)> {
        for snapshot_id in local_only_snapshots {
            self.control.checkpoint()?;
            remove_snapshot_from_repo(self.repo, snapshot_id)?;
        }
        let mut pruned_bundle_count = 0;
        for path in local_bundle_paths(&self.repo_paths.bundles_root)? {
            self.control.checkpoint()?;
            if let Some(bundle_id) = bundle_id_from_bundle_path(&path)
                && !required_bundles.contains(&bundle_id)
            {
                fs::remove_file(&path)?;
                pruned_bundle_count += 1;
            }
        }
        Ok((local_only_snapshots.len(), pruned_bundle_count))
    }
}

pub fn pull_repo<P: RemoteRepositoryReader + ?Sized>(
    repo: PathBuf,
    provider: &P,
    remote_spec: Option<String>,
    policy: PullConflictPolicy,
    prune: bool,
    control: OperationControl,
) -> Result<RepoPullStats> {
    fs::create_dir_all(&repo)?;
    coordination::write(&repo, || {
        control.checkpoint()?;
        let mut progress = RemoteProgress::new(&control, OperationKind::Pull);
        let bound_remote_repo_id = bound_remote_repo_id(&repo)?;
        let repository =
            provider.resolve_repository(remote_spec.as_deref(), bound_remote_repo_id.as_deref())?;
        let repo_paths = RepoPaths::from_repo_root(&repo);
        let repository_existed = repo_paths.repo_metadata_path.exists();
        ensure_gutch_repo_layout(&repo_paths, &repository.name)?;
        progress.report(ProgressPhase::Planning, "Planning pull changes", None);
        let snapshots = provider.list_snapshots(&repository)?;
        let mut snapshots_by_id = HashMap::with_capacity(snapshots.len());
        for snapshot in &snapshots {
            // Keep the first occurrence if a provider repeats an ID.
            snapshots_by_id
                .entry(snapshot.snapshot_id.as_str())
                .or_insert(snapshot);
        }
        let plan = compute_pull_plan(&repo, &repo_paths, &repository, &snapshots, policy)?;
        control.checkpoint()?;
        let context = PullContext {
            repo: &repo,
            repo_paths: &repo_paths,
            repository: &repository,
            provider,
            download_client: build_download_client()?,
            control: &control,
        };

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
        progress.set_total_steps(
            1 + usize::from(plan.repo_manifest_needed)
                + usize::from(plan.chunk_map_needed)
                + plan.snapshots_to_download.len()
                + 2,
        );

        if plan.repo_manifest_needed {
            control.checkpoint()?;
            progress.next_step(
                ProgressPhase::RepoMetadata,
                "Downloading repo metadata",
                None,
            );
            context.download_repo_manifest(repository_existed)?;
            stats.repo_manifest_downloaded = true;
        }

        if plan.chunk_map_needed {
            control.checkpoint()?;
            progress.next_step(ProgressPhase::ChunkIndex, "Downloading chunk map", None);
            context.download_chunk_index()?;
            stats.chunk_map_downloaded = true;
        }

        let snapshot_count = plan.snapshots_to_download.len();
        for (index, snapshot_id) in plan.snapshots_to_download.iter().enumerate() {
            control.checkpoint()?;
            progress.next_step(
                ProgressPhase::SnapshotMetadata,
                format!(
                    "Downloading snapshot metadata ({}/{snapshot_count})",
                    index + 1
                ),
                Some(snapshot_id.clone()),
            );
            let snapshot = snapshots_by_id.get(snapshot_id.as_str()).ok_or_else(|| {
                Error::repository(format!(
                    "remote snapshot '{snapshot_id}' disappeared during pull"
                ))
            })?;
            context.download_snapshot(snapshot)?;
            stats.snapshot_downloaded_count += 1;
        }

        progress.next_step(ProgressPhase::Artifact, "Resolving bundles", None);
        let bundles_by_snapshot = required_bundle_ids_by_snapshot(&repo_paths)?;
        let required_bundles: HashSet<String> = bundles_by_snapshot
            .iter()
            .flat_map(|snapshot| snapshot.bundle_ids.iter())
            .cloned()
            .collect();
        let mut missing_bundles = Vec::new();
        for bundle_id in &required_bundles {
            if !bundle_path(&repo_paths, bundle_id)?.is_file() {
                missing_bundles.push(bundle_id.clone());
            }
        }
        missing_bundles.sort();
        let bundle_sources = context.missing_bundle_sources(
            &bundles_by_snapshot,
            &missing_bundles,
            &snapshots_by_id,
        )?;
        for (index, bundle_id) in missing_bundles.iter().enumerate() {
            control.checkpoint()?;
            let source = bundle_sources.get(bundle_id).ok_or_else(|| {
                Error::repository(format!(
                    "remote provider did not return a source for required bundle '{bundle_id}'"
                ))
            })?;
            progress.report(
                ProgressPhase::Artifact,
                format!(
                    "Downloading bundles ({}/{})",
                    index + 1,
                    missing_bundles.len()
                ),
                Some(bundle_id.clone()),
            );
            download_and_publish_bundle(
                source,
                &context.download_client,
                bundle_id,
                &bundle_path(&repo_paths, bundle_id)?,
                &control,
            )?;
        }
        stats.bundle_downloaded_count = missing_bundles.len();
        stats.bundle_skipped_count = required_bundles.len() - missing_bundles.len();

        if prune {
            control.checkpoint()?;
            progress.next_step(ProgressPhase::Prune, "Removing local-only content", None);
            (stats.pruned_snapshot_count, stats.pruned_bundle_count) =
                context.prune(&plan.local_only_snapshots, &required_bundles)?;
        } else {
            stats.local_only_snapshots_kept = plan.local_only_snapshots.len();
        }

        progress.finish("Pull complete");
        Ok(stats)
    })
}

#[cfg(test)]
mod constraint_tests {
    use super::*;
    #[test]
    fn incoming_snapshot_is_rejected_before_metadata_is_written() {
        let root = tempfile::tempdir().unwrap();
        let paths = RepoPaths::from_repo_root(root.path());
        crate::repo::paths::ensure_gutch_repo_layout(&paths, "test").unwrap();
        crate::repo::updates::update_repository_constraints(
            root.path(),
            crate::manifest::constraints::RepositoryConstraints {
                targets: vec![crate::filesystem::constraints::FilesystemTarget::WindowsNtfs],
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
        let error = write_remote_snapshot_artifacts(
            &paths,
            &id,
            &manifest,
            None,
            &bytes,
            &OperationControl::default(),
        )
        .unwrap_err();
        assert!(matches!(error, Error::PathValidation(_)));
        assert!(!paths.snapshots_root.join(id).exists());
    }
}

#[cfg(test)]
mod streaming_tests {
    use super::*;

    fn bundle_source() -> (String, RemoteArtifactSource) {
        let bytes = b"remote bundle payload".repeat(100);
        let hash = crate::artifact::hash::ChunkHash::of(&bytes);
        let mut builder = crate::artifact::bundle::BundleBuilder::new(Default::default());
        builder.add_chunk(hash, bytes);
        let (id, bytes) = builder.build_with_id().unwrap();
        (id, RemoteArtifactSource::Bytes(bytes))
    }

    #[test]
    fn streamed_bundle_is_verified_before_replacing_cached_content() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("bundle");
        let client = crate::transfer::http::build_download_client().unwrap();
        let control = OperationControl::default();
        let (id, source) = bundle_source();
        download_and_publish_bundle(&source, &client, &id, &destination, &control).unwrap();
        let healthy = std::fs::read(&destination).unwrap();
        let RemoteArtifactSource::Bytes(mut damaged) = source else {
            unreachable!()
        };
        damaged[0] ^= 1;
        assert!(
            download_and_publish_bundle(
                &RemoteArtifactSource::Bytes(damaged),
                &client,
                &id,
                &destination,
                &control,
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&destination).unwrap(), healthy);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn bounded_source_rejects_oversized_bytes_before_writing() {
        let client = crate::transfer::http::build_download_client().unwrap();
        let source = RemoteArtifactSource::Bytes(vec![1, 2, 3]);
        let mut output = Vec::new();
        assert!(
            write_source(
                &source,
                &client,
                2,
                &OperationControl::default(),
                &mut output
            )
            .is_err()
        );
        assert!(output.is_empty());
        write_source(
            &source,
            &client,
            3,
            &OperationControl::default(),
            &mut output,
        )
        .unwrap();
        assert_eq!(output, [1, 2, 3]);
    }

    #[test]
    fn cancelled_bundle_publication_keeps_the_existing_cache() {
        #[derive(Debug)]
        struct Cancelled;
        impl crate::progress::CancellationHook for Cancelled {
            fn is_cancelled(&self) -> bool {
                true
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("bundle");
        std::fs::write(&destination, b"previous").unwrap();
        let client = crate::transfer::http::build_download_client().unwrap();
        let control = OperationControl::default().with_cancellation(std::sync::Arc::new(Cancelled));
        let (id, source) = bundle_source();
        assert!(
            download_and_publish_bundle(&source, &client, &id, &destination, &control).is_err()
        );
        assert_eq!(std::fs::read(destination).unwrap(), b"previous");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
