//! A remote provider that stores remote repositories in a local folder.
//!
//! Gutch has no client for any particular server: an application connects the
//! remote operations to its own storage by implementing `RemoteRepoReader`,
//! `RemoteRepoWriter` and `RemoteRepoService`. This example implements them
//! over plain files, then creates a remote, pushes a repository to it, pulls
//! it into a second repository and installs from the copy.
//!
//! Run with `cargo run --example folder_remote -- WORK_DIR`.
//! WORK_DIR must not exist yet.

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gutch::api::{
    Error, InstallationReconstructRequest, InstallationVerifyRequest, OperationControl,
    RemoteArtifactSource, RemoteArtifactTarget, RemoteBundleSource, RemoteRepo,
    RemoteRepoCreateRequest, RemoteRepoProvider, RemoteRepoReader, RemoteRepoService,
    RemoteRepoSummary, RemoteRepoWriter, RemoteSnapshot, RemoteSnapshotSources, RepoInitRequest,
    RepoPullRequest, RepoPushRequest, Result, SnapshotCaptureRequest, SnapshotTagSet, TagReference,
    capture_snapshot, create_remote_repo, init_repo, plan_push_repo, pull_repo, push_repo,
    reconstruct_installation, remote_artifact_hash, verify_installation,
};

/// Remote repositories stored as folders under `root`:
///
/// ```text
/// root/SCOPE--NAME/
///   repo_metadata  chunk_index
///   snapshots/SNAPSHOT_ID/{metadata, blueprint, product_metadata}
///   bundles/BUNDLE_ID
/// ```
///
/// A server-backed provider would make the same calls over the network and
/// could return `RemoteArtifactSource::Url` (for example presigned links)
/// instead of bytes.
struct FolderRemote {
    root: PathBuf,
}

impl FolderRemote {
    fn repo_dir(&self, repository: &RemoteRepo) -> PathBuf {
        self.root.join(&repository.id)
    }

    fn snapshot_dir(&self, repository: &RemoteRepo, snapshot_id: &str) -> PathBuf {
        self.repo_dir(repository)
            .join("snapshots")
            .join(snapshot_id)
    }

    /// The remote's view of one snapshot; hashes let Gutch skip unchanged files.
    fn remote_snapshot(
        &self,
        repository: &RemoteRepo,
        snapshot_id: &str,
    ) -> Result<RemoteSnapshot> {
        let dir = self.snapshot_dir(repository, snapshot_id);
        let metadata_hash_b3 = file_hash(&dir.join("metadata"))?;
        let blueprint_hash_b3 = file_hash(&dir.join("blueprint"))?;
        Ok(RemoteSnapshot {
            remote_id: snapshot_id.to_owned(),
            snapshot_id: snapshot_id.to_owned(),
            product_metadata_hash_b3: file_hash(&dir.join("product_metadata"))?,
            artifact_ready: !metadata_hash_b3.is_empty() && !blueprint_hash_b3.is_empty(),
            metadata_hash_b3,
            blueprint_hash_b3,
            uploaded_at: String::new(),
            created_at: String::new(),
            tags: Vec::new(),
            artifact_state: None,
        })
    }
}

impl RemoteRepoReader for FolderRemote {
    /// `remote_repo_id` is an explicit choice (such as `repo pull --remote-repo-id`);
    /// otherwise the repository's bound remote is used.
    fn resolve_repo(
        &self,
        remote_repo_id: Option<&str>,
        bound_remote_repo_id: Option<&str>,
    ) -> Result<RemoteRepo> {
        let id = remote_repo_id
            .or(bound_remote_repo_id)
            .ok_or_else(|| Error::repo("no remote repo given and none is bound"))?;
        let dir = self.root.join(id);
        if !dir.is_dir() {
            return Err(Error::repo(format!("remote repo {id} does not exist")));
        }
        let (scope_id, name) = id.split_once("--").unwrap_or(("", id));
        Ok(RemoteRepo {
            id: id.to_owned(),
            scope_id: scope_id.to_owned(),
            name: name.to_owned(),
            metadata_hash_b3: file_hash(&dir.join("repo_metadata"))?,
            chunk_map_hash_b3: file_hash(&dir.join("chunk_index"))?,
            updated_at: String::new(),
        })
    }

    fn list_snapshots(&self, repository: &RemoteRepo) -> Result<Vec<RemoteSnapshot>> {
        let mut snapshots = Vec::new();
        for snapshot_id in list_names(&self.repo_dir(repository).join("snapshots"))? {
            snapshots.push(self.remote_snapshot(repository, &snapshot_id)?);
        }
        Ok(snapshots)
    }

    fn repo_metadata_source(&self, repository: &RemoteRepo) -> Result<RemoteArtifactSource> {
        read_source(&self.repo_dir(repository).join("repo_metadata"))
    }

    fn chunk_index_source(&self, repository: &RemoteRepo) -> Result<RemoteArtifactSource> {
        read_source(&self.repo_dir(repository).join("chunk_index"))
    }

    fn snapshot_sources(
        &self,
        repository: &RemoteRepo,
        snapshot: &RemoteSnapshot,
    ) -> Result<RemoteSnapshotSources> {
        let dir = self.snapshot_dir(repository, &snapshot.snapshot_id);
        let product_metadata = dir.join("product_metadata");
        Ok(RemoteSnapshotSources {
            metadata: read_source(&dir.join("metadata"))?,
            blueprint: read_source(&dir.join("blueprint"))?,
            product_metadata: if product_metadata.is_file() {
                Some(read_source(&product_metadata)?)
            } else {
                None
            },
        })
    }

    fn bundle_sources(
        &self,
        repository: &RemoteRepo,
        _snapshot: &RemoteSnapshot,
        bundle_ids: &[String],
    ) -> Result<Vec<RemoteBundleSource>> {
        let bundles = self.repo_dir(repository).join("bundles");
        bundle_ids
            .iter()
            .map(|bundle_id| {
                Ok(RemoteBundleSource {
                    bundle_id: bundle_id.clone(),
                    source: read_source(&bundles.join(bundle_id))?,
                })
            })
            .collect()
    }
}

impl RemoteRepoWriter for FolderRemote {
    fn delete_snapshot(&self, repository: &RemoteRepo, snapshot: &RemoteSnapshot) -> Result<()> {
        remove_if_present(&self.snapshot_dir(repository, &snapshot.snapshot_id))
    }

    fn create_snapshot(
        &self,
        repository: &RemoteRepo,
        snapshot_id: &str,
        _tags: &SnapshotTagSet,
    ) -> Result<RemoteSnapshot> {
        fs::create_dir_all(self.snapshot_dir(repository, snapshot_id))?;
        self.remote_snapshot(repository, snapshot_id)
    }

    fn upload_artifact(
        &self,
        repository: &RemoteRepo,
        target: RemoteArtifactTarget<'_>,
        bytes: Vec<u8>,
        control: &OperationControl,
    ) -> Result<()> {
        control.checkpoint()?;
        let repo_dir = self.repo_dir(repository);
        let path = match target {
            RemoteArtifactTarget::RepositoryMetadata => repo_dir.join("repo_metadata"),
            RemoteArtifactTarget::ChunkIndex => repo_dir.join("chunk_index"),
            RemoteArtifactTarget::SnapshotMetadata(snapshot) => self
                .snapshot_dir(repository, &snapshot.snapshot_id)
                .join("metadata"),
            RemoteArtifactTarget::SnapshotBlueprint(snapshot) => self
                .snapshot_dir(repository, &snapshot.snapshot_id)
                .join("blueprint"),
            RemoteArtifactTarget::SnapshotProductMetadata(snapshot) => self
                .snapshot_dir(repository, &snapshot.snapshot_id)
                .join("product_metadata"),
            RemoteArtifactTarget::Bundle(bundle_id) => repo_dir.join("bundles").join(bundle_id),
        };
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, bytes)?;
        Ok(())
    }

    fn delete_snapshot_product_metadata(
        &self,
        repository: &RemoteRepo,
        snapshot: &RemoteSnapshot,
    ) -> Result<()> {
        let dir = self.snapshot_dir(repository, &snapshot.snapshot_id);
        remove_if_present(&dir.join("product_metadata"))
    }

    fn bundle_exists(&self, repository: &RemoteRepo, bundle_id: &str) -> Result<bool> {
        Ok(self
            .repo_dir(repository)
            .join("bundles")
            .join(bundle_id)
            .is_file())
    }

    /// Deletes bundles no snapshot needs any more; returns how many were deleted.
    fn reconcile_bundles(
        &self,
        repository: &RemoteRepo,
        required_bundle_ids: &[String],
    ) -> Result<usize> {
        let bundles = self.repo_dir(repository).join("bundles");
        let mut deleted = 0;
        for bundle_id in list_names(&bundles)? {
            if !required_bundle_ids.contains(&bundle_id) {
                fs::remove_file(bundles.join(bundle_id))?;
                deleted += 1;
            }
        }
        Ok(deleted)
    }
}

/// Repository management: the part of a provider that owns accounts and scopes.
impl RemoteRepoService for FolderRemote {
    fn repo_provider(&self) -> &dyn RemoteRepoProvider {
        self
    }

    fn list_repos(&self) -> Result<Vec<RemoteRepoSummary>> {
        Ok(list_names(&self.root)?
            .into_iter()
            .map(|id| {
                let (scope, name) = id.split_once("--").unwrap_or(("", &id));
                RemoteRepoSummary {
                    scope_id: scope.to_owned(),
                    scope_name: scope.to_owned(),
                    scope_slug: scope.to_owned(),
                    repository_id: id.clone(),
                    repository_name: name.to_owned(),
                }
            })
            .collect())
    }

    fn create_repo(&self, scope_id: &str, name: &str) -> Result<String> {
        let id = format!("{scope_id}--{name}");
        fs::create_dir_all(self.root.join(&id))?;
        Ok(id)
    }

    fn delete_repo(&self, repository: &RemoteRepo) -> Result<()> {
        remove_if_present(&self.repo_dir(repository))
    }

    /// Tags the remote's own rules refer to; this provider has no rules.
    fn tag_references(&self, _repository: &RemoteRepo) -> Result<Vec<TagReference>> {
        Ok(Vec::new())
    }
}

fn main() -> ExitCode {
    let arguments: Vec<PathBuf> = env::args_os().skip(1).map(PathBuf::from).collect();
    let [work_dir] = arguments.as_slice() else {
        eprintln!("usage: folder_remote WORK_DIR");
        return ExitCode::from(2);
    };
    if work_dir.exists() {
        eprintln!(
            "error: {} already exists; pick a new WORK_DIR",
            work_dir.display()
        );
        return ExitCode::from(2);
    }
    match run(work_dir) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(work_dir: &Path) -> Result<()> {
    let remote = FolderRemote {
        root: work_dir.join("remote"),
    };
    let origin = work_dir.join("origin");
    let copy = work_dir.join("copy");
    let source_dir = work_dir.join("source");
    fs::create_dir_all(&source_dir)?;
    fs::write(
        source_dir.join("readme.txt"),
        "hello from the remote\n".repeat(100),
    )?;

    init_repo(&RepoInitRequest {
        repo_root: origin.clone(),
        ..Default::default()
    })?;
    let snapshot_id = capture_snapshot(&SnapshotCaptureRequest {
        repo_root: origin.clone(),
        source_dir,
        ..Default::default()
    })?
    .snapshot_id;

    // Create a remote repo in scope "team" and bind `origin` to it.
    let remote_repo_id = create_remote_repo(
        &RemoteRepoCreateRequest {
            repo_root: origin.clone(),
            scope_id: "team".into(),
            ..Default::default()
        },
        &remote,
    )?
    .remote_repo_id;
    println!("created remote repo {remote_repo_id}");

    let push = RepoPushRequest {
        repo_root: origin,
        ..Default::default()
    };
    let plan = plan_push_repo(&push, &remote)?;
    println!("push plan: {} snapshots to add", plan.to_add.len());
    let pushed = push_repo(&push, &remote)?;
    println!("pushed: {} bundles uploaded", pushed.bundle_uploaded_count);

    // Pulling into a new folder creates the repository there.
    let pulled = pull_repo(
        &RepoPullRequest {
            repo_root: copy.clone(),
            remote_repo_id: Some(remote_repo_id),
            ..Default::default()
        },
        &remote,
    )?;
    println!(
        "pulled into {}: {} snapshots, {} bundles",
        copy.display(),
        pulled.snapshot_downloaded_count,
        pulled.bundle_downloaded_count
    );

    let app_dir = work_dir.join("app");
    reconstruct_installation(&InstallationReconstructRequest {
        repo_root: copy.clone(),
        snapshot_id: snapshot_id.clone(),
        destination_dir: app_dir.clone(),
        ..Default::default()
    })?;
    let issues = verify_installation(&InstallationVerifyRequest {
        repo_root: copy,
        snapshot_id,
        destination_dir: app_dir,
        ..Default::default()
    })?
    .issues;
    println!(
        "installed from the pulled copy: {} missing, {} corrupt",
        issues.missing_files.len(),
        issues.corrupt_files.len()
    );
    Ok(())
}

/// The hash Gutch expects for a stored file, or "" when it is absent.
fn file_hash(path: &Path) -> Result<String> {
    match fs::read(path) {
        Ok(bytes) => Ok(remote_artifact_hash(&bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error.into()),
    }
}

fn read_source(path: &Path) -> Result<RemoteArtifactSource> {
    Ok(RemoteArtifactSource::Bytes(fs::read(path)?))
}

fn list_names(dir: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    match fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries {
                names.push(entry?.file_name().to_string_lossy().into_owned());
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    names.sort();
    Ok(names)
}

fn remove_if_present(path: &Path) -> Result<()> {
    let result = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    match result {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}
