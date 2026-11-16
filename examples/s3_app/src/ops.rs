//! What the app does, without the window: each operation takes an
//! `OperationControl` so the window can show progress and cancel it.
//!
//! The app keeps a local copy of each remote repository in the cache
//! folder, at `CACHE/SCOPE/NAME`. Gutch captures into that copy and installs
//! from it, and `pull_repo`/`push_repo` keep it in step with the bucket.
//! Installing, browsing and saving a file download only the snapshot they
//! use; a capture downloads everything first, because a push makes the
//! bucket match the copy.

use std::fs;
use std::path::{Path, PathBuf};

use gutch::api::{
    Error, InstallationReconstructRequest, InstallationRepairRequest, InstallationVerifyRequest,
    ObjectStoreRemote, OperationControl, PullBundles, RemoteObjectStore, RemoteRepoBindRequest,
    RemoteRepoCreateRequest, RemoteRepoReader, RemoteRepoService, RemoteRepoSummary,
    RemoteSnapshot, Repo, RepoPullRequest, RepoPushRequest, Result, SnapshotCaptureOutcome,
    SnapshotCaptureRequest, SnapshotFileReadRequest, SnapshotTagSet, bind_remote_repo,
    capture_snapshot, create_remote_repo, load_snapshot_manifest, pull_repo, push_repo,
    read_snapshot_file, reconstruct_installation, repair_installation, verify_installation,
};

pub struct Remote<S> {
    remote: ObjectStoreRemote<S>,
    cache_dir: PathBuf,
}

impl<S: RemoteObjectStore + 'static> Remote<S> {
    pub fn new(store: S, cache_dir: PathBuf) -> Self {
        Self {
            remote: ObjectStoreRemote::new(store),
            cache_dir,
        }
    }

    pub fn repos(&self) -> Result<Vec<RemoteRepoSummary>> {
        self.remote.list_repos()
    }

    /// Creates an empty local copy and the remote repository it is bound to,
    /// then pushes the empty copy so others can pull it at once.
    /// Returns the remote repository ID, `SCOPE--NAME`.
    pub fn create_repo(
        &self,
        scope: &str,
        name: &str,
        control: OperationControl,
    ) -> Result<String> {
        let repo_root = self.cache_dir.join(scope.trim()).join(name.trim());
        if repo_root.exists() {
            return Err(Error::InvalidInput(format!(
                "{} already exists; delete it or pick another name",
                repo_root.display()
            )));
        }
        let repo = Repo::init(&repo_root)?;
        let created = create_remote_repo(
            &RemoteRepoCreateRequest {
                repo_root: repo.root().to_path_buf(),
                scope_id: scope.trim().to_owned(),
                control: control.clone(),
            },
            &self.remote,
        );
        if created.is_err() {
            // Leave nothing behind that later looks like a copy of a remote.
            fs::remove_dir_all(&repo_root)?;
        }
        let repo_id = created?.remote_repo_id;
        push_repo(&RepoPushRequest { repo_root, control }, &self.remote)?;
        Ok(repo_id)
    }

    /// The remote's snapshots, newest first.
    pub fn snapshots(&self, repo_id: &str) -> Result<Vec<RemoteSnapshot>> {
        let repository = self.remote.resolve_repo(Some(repo_id), None)?;
        let mut snapshots = self.remote.list_snapshots(&repository)?;
        snapshots.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        Ok(snapshots)
    }

    /// Captures `source` with tags such as `version=1.2 os=windows` and
    /// pushes the new snapshot.
    pub fn capture_and_push(
        &self,
        repo_id: &str,
        source: &Path,
        tags: &str,
        control: OperationControl,
    ) -> Result<String> {
        let tags: Vec<String> = tags
            .split([' ', ',', ';'])
            .filter(|tag| !tag.is_empty())
            .map(str::to_owned)
            .collect();
        let tags = SnapshotTagSet::from_compact_strings(&tags)?;
        // Push makes the remote match the local copy, so first take in
        // everything others pushed.
        let repo_root = self.pull(repo_id, Vec::new(), PullBundles::All, control.clone())?;
        let captured = capture_snapshot(&SnapshotCaptureRequest {
            repo_root: repo_root.clone(),
            source: source.to_path_buf(),
            tags,
            control: control.clone(),
            ..Default::default()
        })?;
        // A pulled copy does not record where it came from; push needs that.
        bind_remote_repo(&RemoteRepoBindRequest {
            repo_root: repo_root.clone(),
            remote_repo_id: repo_id.to_owned(),
            control: control.clone(),
        })?;
        let pushed = push_repo(&RepoPushRequest { repo_root, control }, &self.remote)?;
        let captured = match captured.outcome {
            SnapshotCaptureOutcome::Created { snapshot_id } => format!("captured {snapshot_id}"),
            SnapshotCaptureOutcome::Repaired { snapshot_id }
            | SnapshotCaptureOutcome::Unchanged { snapshot_id } => {
                format!("{snapshot_id} was already captured")
            }
        };
        Ok(format!(
            "{captured}; uploaded {} bundles",
            pushed.bundle_uploaded_count
        ))
    }

    /// Installs a snapshot into `destination`. When `installed_snapshot_id`
    /// names the snapshot already there, unchanged content is kept in place.
    pub fn install(
        &self,
        repo_id: &str,
        snapshot_id: &str,
        destination: &Path,
        installed_snapshot_id: Option<&str>,
        control: OperationControl,
    ) -> Result<String> {
        // The installed snapshot's manifest tells Gutch what is already there.
        let mut snapshot_ids = vec![snapshot_id.to_owned()];
        if let Some(installed) = installed_snapshot_id
            && self
                .snapshots(repo_id)?
                .iter()
                .any(|snapshot| snapshot.snapshot_id == installed)
        {
            snapshot_ids.push(installed.to_owned());
        }
        let repo_root = self.pull(repo_id, snapshot_ids, PullBundles::All, control.clone())?;
        let installed = reconstruct_installation(&InstallationReconstructRequest {
            repo_root,
            snapshot_id: snapshot_id.to_owned(),
            destination_dir: destination.to_path_buf(),
            reuse_snapshot_id: installed_snapshot_id.map(str::to_owned),
            control,
            ..Default::default()
        })?;
        Ok(format!(
            "installed {snapshot_id} to {}; {} of {} chunks reused",
            destination.display(),
            installed.reused_chunk_count,
            installed.required_chunk_count
        ))
    }

    pub fn verify(
        &self,
        repo_id: &str,
        snapshot_id: &str,
        destination: &Path,
        control: OperationControl,
    ) -> Result<String> {
        let repo_root = self.pull(
            repo_id,
            vec![snapshot_id.to_owned()],
            PullBundles::All,
            control.clone(),
        )?;
        let issues = verify_installation(&InstallationVerifyRequest {
            repo_root,
            snapshot_id: snapshot_id.to_owned(),
            destination_dir: destination.to_path_buf(),
            control,
            ..Default::default()
        })?
        .issues;
        Ok(format!("{}: {issues}", destination.display()))
    }

    /// Restores missing and changed files. Files the snapshot does not have stay.
    pub fn repair(
        &self,
        repo_id: &str,
        snapshot_id: &str,
        destination: &Path,
        control: OperationControl,
    ) -> Result<String> {
        let repo_root = self.pull(
            repo_id,
            vec![snapshot_id.to_owned()],
            PullBundles::All,
            control.clone(),
        )?;
        let repaired = repair_installation(&InstallationRepairRequest {
            repo_root,
            snapshot_id: snapshot_id.to_owned(),
            destination_dir: destination.to_path_buf(),
            control,
            ..Default::default()
        })?;
        Ok(format!(
            "repaired {} files in {}",
            repaired.missing_file_count + repaired.corrupt_file_count,
            destination.display()
        ))
    }

    /// The paths and sizes of a snapshot's files, sorted by path.
    pub fn files(
        &self,
        repo_id: &str,
        snapshot_id: &str,
        control: OperationControl,
    ) -> Result<Vec<(String, u64)>> {
        let snapshot_ids = vec![snapshot_id.to_owned()];
        let repo_root = self.pull(repo_id, snapshot_ids, PullBundles::None, control)?;
        let manifest = load_snapshot_manifest(&repo_root, snapshot_id)?;
        let mut files: Vec<(String, u64)> = manifest
            .files()
            .map(|file| (file.path.clone(), file.size))
            .collect();
        files.sort();
        Ok(files)
    }

    /// Writes one file of a snapshot to `target` without installing the
    /// snapshot, downloading only the bundles that hold the file.
    pub fn save_file(
        &self,
        repo_id: &str,
        snapshot_id: &str,
        path: &str,
        target: &Path,
        control: OperationControl,
    ) -> Result<String> {
        let snapshot_ids = vec![snapshot_id.to_owned()];
        let bundles = PullBundles::Files(vec![path.to_owned()]);
        let repo_root = self.pull(repo_id, snapshot_ids, bundles, control.clone())?;
        let content = read_snapshot_file(&SnapshotFileReadRequest {
            repo_root,
            snapshot_id: snapshot_id.to_owned(),
            path: path.to_owned(),
            control,
        })?;
        fs::write(target, &content)?;
        Ok(format!(
            "saved {path} ({} bytes) to {}",
            content.len(),
            target.display()
        ))
    }

    /// Brings the local copy up to date, creating it on first use. With
    /// `snapshot_ids`, downloads only those snapshots; empty downloads all.
    /// `bundles` says which of their bundles to download.
    fn pull(
        &self,
        repo_id: &str,
        snapshot_ids: Vec<String>,
        bundles: PullBundles,
        control: OperationControl,
    ) -> Result<PathBuf> {
        let repository = self.remote.resolve_repo(Some(repo_id), None)?;
        let repo_root = self
            .cache_dir
            .join(&repository.scope_id)
            .join(&repository.name);
        pull_repo(
            &RepoPullRequest {
                repo_root: repo_root.clone(),
                remote_repo_id: Some(repo_id.to_owned()),
                prune: true,
                snapshot_ids,
                bundles,
                control,
                ..Default::default()
            },
            &self.remote,
        )?;
        Ok(repo_root)
    }
}

/// Checks that the keys may list, write and delete below the prefix and
/// that the bucket honours conditional writes, which Gutch needs to let only
/// one push into a repository at a time. Leaves nothing behind.
pub fn check_bucket(store: &impl RemoteObjectStore) -> Result<String> {
    let found = store.list("")?.len();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos());
    let probe = format!("gutch-connection-check-{nanos}");
    let created = store.put_if_absent(&probe, b"check");
    let refused = created
        .as_ref()
        .ok()
        .map(|_| store.put_if_absent(&probe, b"again"));
    store.delete(&probe)?;
    if !created? {
        return Err(Error::repo(format!(
            "the test object '{probe}' already existed; try again"
        )));
    }
    if refused.transpose()? != Some(false) {
        return Err(Error::repo(
            "the bucket accepts the keys but ignores conditional writes \
             (If-None-Match), so two pushes could overwrite each other; \
             use a provider that supports them",
        ));
    }
    Ok(format!(
        "connection works: {found} objects at the top of the prefix; \
         writes, deletes and conditional writes are allowed"
    ))
}

/// Adds a plain-words cause to a failed S3 request.
pub fn explain(error: &str) -> String {
    let hints = [
        (
            [
                "InvalidAccessKeyId",
                "SignatureDoesNotMatch",
                "401 Unauthorized",
            ]
            .as_slice(),
            "the access key ID or secret key is wrong",
        ),
        (
            ["AccessDenied", "403 Forbidden"].as_slice(),
            "the keys are valid but may not use this bucket or prefix",
        ),
        (["NoSuchBucket"].as_slice(), "the bucket does not exist"),
        (
            [
                "AuthorizationHeaderMalformed",
                "PermanentRedirect",
                "301 Moved",
            ]
            .as_slice(),
            "the bucket is in another region",
        ),
        (
            ["error sending request", "dns error", "connection refused"].as_slice(),
            "the endpoint cannot be reached; check its address and your network",
        ),
    ];
    if !error.starts_with("S3:") {
        return error.to_owned();
    }
    match hints
        .iter()
        .find(|(signs, _)| signs.iter().any(|sign| error.contains(sign)))
    {
        Some((_, hint)) => format!("{error}. Likely cause: {hint}."),
        None => error.to_owned(),
    }
}

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;
