//! Pull: plans and downloads remote snapshots and bundles into the local repository.

use super::artifact_decision::ArtifactDecision;
use super::artifact_decision::decide_artifact;
use super::artifact_decision::hashes_differ;
use super::artifact_decision::local_file_mtime;
use super::artifact_decision::verify_remote_artifact;
use super::link::bound_remote_repo_id;
use super::local_hashes::local_chunk_map_hash_b3;
use super::local_hashes::local_repo_metadata_hash_b3;
use super::local_hashes::local_snapshot_blueprint_hash_b3;
use super::local_hashes::local_snapshot_metadata_hash_b3;
use super::local_hashes::local_snapshot_product_metadata_hash_b3;
use super::progress::RemoteProgress;
use super::provider::PullConflictPolicy;
use super::provider::RemoteArtifactSource;
use super::provider::RemoteBundleSource;
use super::provider::RemoteRepo;
use super::provider::RemoteRepoReader;
use super::provider::RemoteSnapshot;
use super::provider::RepoPullPlan;
use super::provider::RepoPullResult;
use crate::Error;
use crate::Result;
use crate::artifact::bundle::BundleIdentityWriter;
use crate::filesystem::write_atomic_with;
use crate::manifest::RepositoryAccess;
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
use crate::repo::paths::ensure_repo_layout;
use crate::repo::paths::load_repo_manifest;
use crate::repo::paths::write_repo_manifest;
use crate::repo::snapshot_manifests::SNAPSHOT_PRODUCT_METADATA_FILE;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use crate::repo::snapshot_manifests::snapshot_manifest_from_download;
use crate::repo::snapshot_manifests::snapshot_manifest_from_parts;
use crate::repo::snapshot_manifests::snapshot_metadata_path;
use crate::repo::snapshot_manifests::snapshot_product_metadata_path;
use crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts;
use crate::repo::updates::validate_existing_constraints;
use crate::repo::verify::RepoVerifyMode;
use crate::repo::verify::VerifiedArtifacts;
use crate::repo::verify::verify_snapshot_artifacts;
use crate::snapshot::identity::snapshot_id_from_entries;
use crate::sync::limits::{
    MAX_BUNDLE_BYTES, MAX_CHUNK_INDEX_BYTES, MAX_METADATA_BYTES, MAX_SNAPSHOT_BLUEPRINT_BYTES,
};
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
    repository: &RemoteRepo,
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
    let mut kept_local_snapshots = Vec::new();
    // Loaded on first need and shared, as are artifact checks across snapshots.
    let mut local_chunk_index = None;
    let mut verified_artifacts = VerifiedArtifacts::new();
    for snapshot in snapshots {
        // A snapshot whose artifacts are still uploading cannot be pulled yet.
        if !snapshot.artifact_ready {
            snapshots_to_skip.push(snapshot.snapshot_id.clone());
            continue;
        }
        // A healthy local installation does not need another repository's layout.
        // This also avoids attempting to fetch local-only chunks from that layout.
        let blueprint_differs = hashes_differ(
            &local_snapshot_blueprint_hash_b3(repo_paths, &snapshot.snapshot_id)?,
            &snapshot.blueprint_hash_b3,
        );
        if blueprint_differs
            && let Ok(local) = load_snapshot_manifest(repo_paths, &snapshot.snapshot_id)
            && snapshot_id_from_entries(&local.entries) == snapshot.snapshot_id
            && let Some(index) = local_chunk_index
                .get_or_insert_with(|| load_chunk_index(repo_paths).ok())
                .as_ref()
            && verify_snapshot_artifacts(
                repo_paths,
                index,
                &local,
                RepoVerifyMode::Strict,
                &mut verified_artifacts,
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
        if decision == ArtifactDecision::Identical && blueprint_differs {
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
            (ArtifactDecision::KeepLocal, _) | (_, ArtifactDecision::KeepLocal) => {
                ArtifactDecision::KeepLocal
            }
            _ => ArtifactDecision::Identical,
        };
        match decision {
            ArtifactDecision::Identical => snapshots_to_skip.push(snapshot.snapshot_id.clone()),
            ArtifactDecision::Fetch => snapshots_to_download.push(snapshot.snapshot_id.clone()),
            ArtifactDecision::KeepLocal => {
                kept_local_snapshots.push(snapshot.snapshot_id.clone());
                snapshots_to_skip.push(snapshot.snapshot_id.clone());
            }
        }
    }
    snapshots_to_download.sort();
    snapshots_to_skip.sort();
    kept_local_snapshots.sort();

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
        kept_local_snapshots,
        bundle_count: 0,
    })
}

pub(crate) fn plan_pull(
    repo_root: &Path,
    repository: &RemoteRepo,
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
                return Err(Error::download(
                    "remote artifact exceeded the allowed size",
                    false,
                ));
            }
            output.write_all(bytes)?;
            Ok(())
        }
        RemoteArtifactSource::Url(url) => download_url_streaming(
            client,
            url,
            control,
            maximum_bytes,
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
        write_source(source, client, MAX_BUNDLE_BYTES, control, &mut writer)?;
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

/// Which repository to pull into and how. The provider is passed beside the request.
#[derive(Debug, Clone, Default)]
pub struct RepoPullRequest {
    pub repo_root: PathBuf,
    /// Remote repository to pull from; `None` uses the bound remote.
    pub remote_repo_id: Option<String>,
    pub on_conflict: PullConflictPolicy,
    /// Delete local snapshots that the remote no longer has.
    pub prune: bool,
    pub control: OperationControl,
}

pub fn plan_pull_repo(
    request: &RepoPullRequest,
    provider: &dyn RemoteRepoReader,
) -> Result<RepoPullPlan> {
    request.control.checkpoint()?;
    let repo_root = request.repo_root.as_path();
    let bound_remote_repo_id = bound_remote_repo_id(repo_root)?;
    let repository = provider.resolve_repo(
        request.remote_repo_id.as_deref(),
        bound_remote_repo_id.as_deref(),
    )?;
    let snapshots = provider.list_snapshots(&repository)?;
    plan_pull(repo_root, &repository, &snapshots, request.on_conflict)
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
            return Err(Error::repo(format!(
                "remote provider returned an unexpected bundle '{}'",
                source.bundle_id
            )));
        }
        if bundle_sources
            .insert(source.bundle_id.clone(), source.source)
            .is_some()
        {
            return Err(Error::repo(format!(
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
struct PullContext<'a, P: RemoteRepoReader + ?Sized> {
    repo: &'a Path,
    repo_paths: &'a RepoPaths,
    repository: &'a RemoteRepo,
    provider: &'a P,
    download_client: DownloadClient,
    control: &'a OperationControl,
}

impl<P: RemoteRepoReader + ?Sized> PullContext<'_, P> {
    fn download(&self, source: &RemoteArtifactSource, maximum_bytes: u64) -> Result<Vec<u8>> {
        source_bytes(source, &self.download_client, maximum_bytes, self.control)
    }

    /// Replaces the local repo manifest with the remote one. Constraints
    /// never change through a pull; they must already match.
    fn download_repo_manifest(&self, repository_existed: bool) -> Result<()> {
        let bytes = self.download(
            &self.provider.repo_metadata_source(self.repository)?,
            MAX_METADATA_BYTES,
        )?;
        verify_remote_artifact(&bytes, &self.repository.metadata_hash_b3, "repo metadata")?;
        let mut manifest = RepoManifest::from_json_bytes("remote repo_metadata.json", &bytes)?;
        // The remote metadata replaces the local copy, so this release must be
        // able to write what it describes.
        manifest.repository_format.check(RepositoryAccess::Write)?;
        let local_constraints = load_repo_manifest(self.repo)?
            .1
            .constraints
            .unwrap_or_default();
        let incoming_constraints = manifest.constraints.clone().unwrap_or_default();
        if repository_existed && local_constraints != incoming_constraints {
            return Err(Error::repo(
                "remote repository constraints differ; explicitly update local constraints before pulling",
            ));
        }
        validate_existing_constraints(self.repo, &incoming_constraints, "pull")?;
        manifest.remote_repo_id = Some(self.repository.id.clone());
        write_repo_manifest(self.repo_paths, &manifest)
    }

    fn download_chunk_index(&self) -> Result<()> {
        let bytes = self.download(
            &self.provider.chunk_index_source(self.repository)?,
            MAX_CHUNK_INDEX_BYTES,
        )?;
        verify_remote_artifact(&bytes, &self.repository.chunk_map_hash_b3, "chunk index")?;
        let mut chunk_index = load_chunk_index_binary_from_bytes(&bytes)?;
        preserve_local_chunk_locations(self.repo_paths, &mut chunk_index)?;
        write_chunk_index(self.repo_paths, &chunk_index)
    }

    fn download_snapshot(&self, snapshot: &RemoteSnapshot) -> Result<()> {
        let snapshot_id = snapshot.snapshot_id.as_str();
        let sources = self.provider.snapshot_sources(self.repository, snapshot)?;
        let metadata_bytes = self.download(&sources.metadata, MAX_METADATA_BYTES)?;
        verify_remote_artifact(
            &metadata_bytes,
            &snapshot.metadata_hash_b3,
            "snapshot metadata",
        )?;
        let metadata = snapshot_manifest_from_download(snapshot_id, &metadata_bytes)?;
        let product_metadata = match sources.product_metadata {
            Some(source) => {
                let bytes = self.download(&source, MAX_METADATA_BYTES)?;
                verify_remote_artifact(
                    &bytes,
                    &snapshot.product_metadata_hash_b3,
                    "snapshot product metadata",
                )?;
                Some(SnapshotProductMetadata::from_json_bytes(
                    SNAPSHOT_PRODUCT_METADATA_FILE,
                    &bytes,
                )?)
            }
            None => None,
        };
        let blueprint_bytes = self.download(&sources.blueprint, MAX_SNAPSHOT_BLUEPRINT_BYTES)?;
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
                Error::repo(format!(
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

    /// Removes local snapshots the remote does not have.
    fn prune_snapshots(&self, local_only_snapshots: &[String]) -> Result<usize> {
        for snapshot_id in local_only_snapshots {
            self.control.checkpoint()?;
            remove_snapshot_from_repo(self.repo, snapshot_id)?;
        }
        Ok(local_only_snapshots.len())
    }

    /// Removes bundles no remaining snapshot needs. Deletion is deferred until
    /// the repository transaction commits.
    fn prune_bundles(&self, required_bundles: &HashSet<String>) -> Result<usize> {
        let mut pruned_bundle_count = 0;
        for path in local_bundle_paths(&self.repo_paths.bundles_root)? {
            self.control.checkpoint()?;
            if let Some(bundle_id) = bundle_id_from_bundle_path(&path)
                && !required_bundles.contains(&bundle_id)
            {
                if !coordination::defer_artifact_deletion(&path)? {
                    fs::remove_file(&path)?;
                }
                pruned_bundle_count += 1;
            }
        }
        Ok(pruned_bundle_count)
    }
}

pub fn pull_repo(
    request: &RepoPullRequest,
    provider: &dyn RemoteRepoReader,
) -> Result<RepoPullResult> {
    let repo_root = request.repo_root.as_path();
    let control = &request.control;
    let policy = request.on_conflict;
    let prune = request.prune;
    fs::create_dir_all(repo_root)?;
    coordination::write_with_control(repo_root, control, || {
        control.checkpoint()?;
        let mut progress = RemoteProgress::new(control, OperationKind::Pull);
        let bound_remote_repo_id = bound_remote_repo_id(repo_root)?;
        let repository = provider.resolve_repo(
            request.remote_repo_id.as_deref(),
            bound_remote_repo_id.as_deref(),
        )?;
        let repo_paths = RepoPaths::from_repo_root(repo_root);
        let repository_existed = repo_paths.repo_metadata_path.exists();
        ensure_repo_layout(&repo_paths, &repository.name)?;
        progress.report(ProgressPhase::Planning, "Planning pull changes", None);
        let snapshots = provider.list_snapshots(&repository)?;
        let mut snapshots_by_id = HashMap::with_capacity(snapshots.len());
        for snapshot in &snapshots {
            // Keep the first occurrence if a provider repeats an ID.
            snapshots_by_id
                .entry(snapshot.snapshot_id.as_str())
                .or_insert(snapshot);
        }
        let plan = compute_pull_plan(repo_root, &repo_paths, &repository, &snapshots, policy)?;
        control.checkpoint()?;
        let context = PullContext {
            repo: repo_root,
            repo_paths: &repo_paths,
            repository: &repository,
            provider,
            download_client: build_download_client()?,
            control,
        };

        let mut stats = RepoPullResult {
            command: "repo pull",
            repo_root: repo_paths.repo_root.display().to_string(),
            remote_repo_id: repository.id.clone(),
            repo_manifest_downloaded: false,
            chunk_map_downloaded: false,
            snapshot_downloaded_count: 0,
            snapshot_skipped_count: plan.snapshots_to_skip.len(),
            bundle_downloaded_count: 0,
            bundle_skipped_count: 0,
            kept_local_count: plan.kept_local_snapshots.len(),
            local_only_snapshots_kept: 0,
            pruned_snapshot_count: 0,
            pruned_bundle_count: 0,
        };
        // Repo metadata, chunk map, one per snapshot, bundle resolution, bundle pruning.
        progress.set_total_steps(
            usize::from(plan.repo_manifest_needed)
                + usize::from(plan.chunk_map_needed)
                + plan.snapshots_to_download.len()
                + 1
                + usize::from(prune),
        );

        if plan.repo_manifest_needed {
            control.checkpoint()?;
            progress.next_step(
                ProgressPhase::RepoMetadata,
                "Downloading repo_root metadata",
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
                Error::repo(format!(
                    "remote snapshot '{snapshot_id}' disappeared during pull"
                ))
            })?;
            context.download_snapshot(snapshot)?;
            stats.snapshot_downloaded_count += 1;
        }

        if prune {
            // Pruned snapshots neither request bundles nor keep them alive.
            control.checkpoint()?;
            stats.pruned_snapshot_count = context.prune_snapshots(&plan.local_only_snapshots)?;
        } else {
            stats.local_only_snapshots_kept = plan.local_only_snapshots.len();
        }

        progress.next_step(ProgressPhase::Artifact, "Resolving bundles", None);
        let local_bundles_by_snapshot = required_bundle_ids_by_snapshot(&repo_paths)?;
        let kept_bundles: HashSet<String> = local_bundles_by_snapshot
            .iter()
            .flat_map(|snapshot| snapshot.bundle_ids.iter())
            .cloned()
            .collect();
        // Only ready remote snapshots can supply bundles; kept local-only
        // snapshots retain whatever bundles they already have.
        let bundles_by_snapshot: Vec<SnapshotBundleIds> = local_bundles_by_snapshot
            .into_iter()
            .filter(|snapshot| {
                snapshots_by_id
                    .get(snapshot.snapshot_id.as_str())
                    .is_some_and(|remote| remote.artifact_ready)
            })
            .collect();
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
                Error::repo(format!(
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
                control,
            )?;
        }
        stats.bundle_downloaded_count = missing_bundles.len();
        stats.bundle_skipped_count = required_bundles.len() - missing_bundles.len();

        if prune {
            control.checkpoint()?;
            progress.next_step(ProgressPhase::Prune, "Removing unreferenced bundles", None);
            stats.pruned_bundle_count = context.prune_bundles(&kept_bundles)?;
        }

        progress.finish("Pull complete");
        Ok(stats)
    })
}

#[cfg(test)]
#[path = "pull_tests.rs"]
mod tests;
