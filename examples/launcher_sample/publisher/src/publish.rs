//! One publishing run: makes the bucket's repository hold the wanted Blender
//! builds, then rewrites the catalog.
//!
//! The run starts from a copy of the repository pulled with metadata only, so
//! it never downloads published builds. New builds are captured with
//! `reuse_remote_bundles`, which lets them reuse content that only the bucket
//! holds, and each push uploads only the new bundles.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use gutch::api::{
    ObjectStoreRemote, OperationControl, PullBundles, RemoteObjectStore, RemoteRepoBindRequest,
    RemoteRepoCreateRequest, RemoteRepoReader, RemoteRepoService, RemoteSnapshot, Repo,
    RepoPullRequest, RepoPushRequest, SnapshotCaptureRequest, SnapshotDeleteRequest,
    SnapshotTagSet, bind_remote_repo, capture_snapshot, create_remote_repo, delete_snapshot,
    load_snapshot_manifest, pull_repo, push_repo,
};
use launcher_catalog::{
    CATALOG_FORMAT, CATALOG_KEY, Catalog, CatalogEntry, Os, REPO_SCOPE, compare_versions,
};
use sha2::{Digest, Sha256};

use crate::archive::extract;

pub type Result<T> = std::result::Result<T, String>;

/// The local folder name, which becomes the repository's name.
const REPO_NAME: &str = "blender";

/// One official build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Build {
    pub version: String,
    pub os: Os,
    pub url: String,
    /// Lowercase hex, as the release server publishes it.
    pub sha256: String,
}

impl Build {
    fn label(&self) -> String {
        format!("{} {}", self.version, self.os.name())
    }

    fn file_name(&self) -> &str {
        self.url.rsplit('/').next().unwrap_or(&self.url)
    }

    /// `5.2.4` gives `5.2`.
    fn series(&self) -> &str {
        self.version
            .rsplit_once('.')
            .map_or(&self.version, |(series, _)| series)
    }
}

/// Where builds come from.
pub trait ReleaseSource {
    fn builds(&self) -> Result<Vec<Build>>;
    /// Saves the build's archive at `target`.
    fn download(&self, build: &Build, target: &Path) -> Result<()>;
}

/// The bucket, which must also report how much it holds.
pub trait Bucket: RemoteObjectStore + 'static {
    fn total_bytes(&self) -> Result<u64>;
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Holds the repository copy, downloads and extracted builds; emptied first.
    pub work_dir: PathBuf,
    /// Publish the newest patch release of this many series, such as 5.2.
    pub series: usize,
    /// Add at most this many builds in one run.
    pub max_new: usize,
    /// Stop adding builds when the bucket would grow past this.
    pub budget_bytes: u64,
    /// Show what would change without changing anything.
    pub dry_run: bool,
}

#[derive(Debug, Default)]
pub struct Report {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Wanted builds left for a later run, by `max_new` or the budget.
    pub postponed: Vec<String>,
    pub bucket_bytes: u64,
}

pub fn run<B: Bucket>(
    remote: &ObjectStoreRemote<B>,
    source: &dyn ReleaseSource,
    options: &Options,
    log: &mut dyn FnMut(&str),
) -> Result<Report> {
    let control = OperationControl::default();
    let repo_root = options.work_dir.join(REPO_SCOPE).join(REPO_NAME);
    if options.work_dir.exists() {
        fs::remove_dir_all(&options.work_dir).map_err(|error| error.to_string())?;
    }
    let repo_id = format!("{REPO_SCOPE}--{REPO_NAME}");
    let exists = remote
        .list_repos()
        .map_err(|error| error.to_string())?
        .iter()
        .any(|repo| repo.repository_id == repo_id);
    let published = if exists {
        pull_copy(remote, &repo_root, &repo_id, &control)?;
        published_builds(remote, &repo_id)?
    } else {
        Vec::new()
    };
    let wanted = wanted_builds(source.builds()?, options.series);

    let wanted_keys: HashSet<_> = wanted.iter().map(key).collect();
    let published_keys: HashSet<_> = published
        .iter()
        .map(|(version, os, _)| (version.clone(), *os))
        .collect();
    let to_remove: Vec<_> = published
        .iter()
        .filter(|(version, os, _)| !wanted_keys.contains(&(version.clone(), *os)))
        .collect();
    let mut to_add: Vec<_> = wanted
        .iter()
        .filter(|build| !published_keys.contains(&key(build)))
        .collect();

    let mut report = Report::default();
    if to_add.len() > options.max_new {
        report.postponed = to_add[options.max_new..]
            .iter()
            .map(|build| build.label())
            .collect();
        to_add.truncate(options.max_new);
    }
    if options.dry_run {
        report.removed = to_remove
            .iter()
            .map(|(version, os, _)| format!("{version} {}", os.name()))
            .collect();
        report.added = to_add.iter().map(|build| build.label()).collect();
        report.bucket_bytes = remote.store().total_bytes()?;
        return Ok(report);
    }
    if !exists {
        create_copy(remote, &repo_root, &control)?;
    }

    // Removing first frees space for the new builds.
    for (version, os, snapshot_id) in &to_remove {
        log(&format!("removing {version} {}", os.name()));
        delete_snapshot(&SnapshotDeleteRequest {
            repo_root: repo_root.clone(),
            snapshot_id: snapshot_id.clone(),
            control: control.clone(),
        })
        .map_err(|error| error.to_string())?;
        report.removed.push(format!("{version} {}", os.name()));
    }
    if !to_remove.is_empty() {
        push(remote, &repo_root, &control)?;
    }

    for (index, build) in to_add.iter().enumerate() {
        log(&format!("downloading {}", build.url));
        let archive = download(source, build, &options.work_dir)?;
        let archive_bytes = fs::metadata(&archive)
            .map_err(|error| error.to_string())?
            .len();
        // New content is at most the archive's size, give or take compression.
        if remote.store().total_bytes()? + archive_bytes > options.budget_bytes {
            log(&format!("{} would exceed the budget", build.label()));
            report
                .postponed
                .extend(to_add[index..].iter().map(|build| build.label()));
            break;
        }
        log(&format!("capturing {}", build.label()));
        add_build(build, &archive, &repo_root, &options.work_dir, &control)?;
        let pushed = push(remote, &repo_root, &control)?;
        log(&format!("pushed {}: {pushed} new bundles", build.label()));
        report.added.push(build.label());
    }

    write_catalog(remote, &repo_root, &repo_id)?;
    report.bucket_bytes = remote.store().total_bytes()?;
    Ok(report)
}

fn key(build: &Build) -> (String, Os) {
    (build.version.clone(), build.os)
}

/// The newest patch release of the newest `series` series, for each system.
pub fn wanted_builds(mut builds: Vec<Build>, series: usize) -> Vec<Build> {
    builds.sort_by(|left, right| compare_versions(&right.version, &left.version));
    let mut newest_series: Vec<&str> = Vec::new();
    for build in &builds {
        if !newest_series.contains(&build.series()) {
            newest_series.push(build.series());
        }
    }
    newest_series.truncate(series);
    let mut seen = HashSet::new();
    builds
        .iter()
        .filter(|build| newest_series.contains(&build.series()))
        .filter(|build| seen.insert((build.series().to_owned(), build.os)))
        .cloned()
        .collect()
}

/// Pulls the repository's metadata into a new local copy, or creates the
/// repository on the first run. Returns its ID.
/// Creates the repository here and on the remote.
fn create_copy<B: Bucket>(
    remote: &ObjectStoreRemote<B>,
    repo_root: &Path,
    control: &OperationControl,
) -> Result<()> {
    Repo::init(repo_root).map_err(|error| error.to_string())?;
    create_remote_repo(
        &RemoteRepoCreateRequest {
            repo_root: repo_root.to_path_buf(),
            scope_id: REPO_SCOPE.to_owned(),
            control: control.clone(),
        },
        remote,
    )
    .map_err(|error| error.to_string())?;
    push(remote, repo_root, control)?;
    Ok(())
}

/// Pulls the snapshot metadata and chunk index, without bundles, and binds
/// the copy to the remote.
fn pull_copy<B: Bucket>(
    remote: &ObjectStoreRemote<B>,
    repo_root: &Path,
    repo_id: &str,
    control: &OperationControl,
) -> Result<()> {
    pull_repo(
        &RepoPullRequest {
            repo_root: repo_root.to_path_buf(),
            remote_repo_id: Some(repo_id.to_owned()),
            bundles: PullBundles::None,
            control: control.clone(),
            ..Default::default()
        },
        remote,
    )
    .map_err(|error| error.to_string())?;
    bind_remote_repo(&RemoteRepoBindRequest {
        repo_root: repo_root.to_path_buf(),
        remote_repo_id: repo_id.to_owned(),
        control: control.clone(),
    })
    .map_err(|error| error.to_string())?;
    Ok(())
}

/// Returns how many bundles it uploaded.
fn push<B: Bucket>(
    remote: &ObjectStoreRemote<B>,
    repo_root: &Path,
    control: &OperationControl,
) -> Result<usize> {
    let pushed = push_repo(
        &RepoPushRequest {
            repo_root: repo_root.to_path_buf(),
            control: control.clone(),
        },
        remote,
    )
    .map_err(|error| error.to_string())?;
    Ok(pushed.bundle_uploaded_count)
}

/// Each published snapshot's version, system and ID.
fn published_builds<B: Bucket>(
    remote: &ObjectStoreRemote<B>,
    repo_id: &str,
) -> Result<Vec<(String, Os, String)>> {
    Ok(remote_snapshots(remote, repo_id)?
        .into_iter()
        .filter_map(|snapshot| {
            let (version, os) = build_tags(&snapshot)?;
            Some((version, os, snapshot.snapshot_id))
        })
        .collect())
}

fn remote_snapshots<B: Bucket>(
    remote: &ObjectStoreRemote<B>,
    repo_id: &str,
) -> Result<Vec<RemoteSnapshot>> {
    let repository = remote
        .resolve_repo(Some(repo_id), None)
        .map_err(|error| error.to_string())?;
    remote
        .list_snapshots(&repository)
        .map_err(|error| error.to_string())
}

/// The `version` and `os` tags the publisher gives each snapshot.
fn build_tags(snapshot: &RemoteSnapshot) -> Option<(String, Os)> {
    let tag = |name: &str| {
        snapshot
            .tags
            .iter()
            .find_map(|tag| tag.strip_prefix(name)?.strip_prefix('='))
    };
    Some((tag("version")?.to_owned(), Os::parse(tag("os")?)?))
}

/// Downloads the archive and checks it against the published SHA-256.
fn download(source: &dyn ReleaseSource, build: &Build, work_dir: &Path) -> Result<PathBuf> {
    let folder = work_dir.join("downloads");
    fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let archive = folder.join(build.file_name());
    source.download(build, &archive)?;
    let mut hasher = Sha256::new();
    let mut file = File::open(&archive).map_err(|error| error.to_string())?;
    let mut buffer = vec![0; 1 << 20];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != build.sha256 {
        return Err(format!(
            "{} does not match its published SHA-256",
            build.file_name()
        ));
    }
    Ok(archive)
}

/// Extracts the archive, captures its top folder and deletes both.
fn add_build(
    build: &Build,
    archive: &Path,
    repo_root: &Path,
    work_dir: &Path,
    control: &OperationControl,
) -> Result<()> {
    let extracted = work_dir.join("extracted");
    let source = extract(archive, &extracted)?;
    let tags = SnapshotTagSet::from_compact_strings(&[
        format!("version={}", build.version),
        format!("os={}", build.os.name()),
    ])
    .map_err(|error| error.to_string())?;
    capture_snapshot(&SnapshotCaptureRequest {
        repo_root: repo_root.to_path_buf(),
        source,
        tags,
        reuse_remote_bundles: true,
        control: control.clone(),
        ..Default::default()
    })
    .map_err(|error| error.to_string())?;
    fs::remove_dir_all(&extracted).map_err(|error| error.to_string())?;
    fs::remove_file(archive).map_err(|error| error.to_string())?;
    Ok(())
}

/// Lists every published build, with sizes from the snapshot metadata.
fn write_catalog<B: Bucket>(
    remote: &ObjectStoreRemote<B>,
    repo_root: &Path,
    repo_id: &str,
) -> Result<()> {
    let mut catalog = Catalog {
        format: CATALOG_FORMAT,
        repo_id: repo_id.to_owned(),
        versions: Vec::new(),
    };
    for snapshot in remote_snapshots(remote, repo_id)? {
        let Some((version, os)) = build_tags(&snapshot) else {
            continue;
        };
        let manifest = load_snapshot_manifest(repo_root, &snapshot.snapshot_id)
            .map_err(|error| error.to_string())?;
        catalog.versions.push(CatalogEntry {
            version,
            os,
            file_count: manifest.files().count() as u64,
            size: manifest.files().map(|file| file.size).sum(),
            snapshot_id: snapshot.snapshot_id,
            published_at: snapshot.created_at,
        });
    }
    catalog.sort();
    let json = serde_json::to_vec_pretty(&catalog).map_err(|error| error.to_string())?;
    remote
        .store()
        .put(CATALOG_KEY, &json)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
#[path = "publish_tests.rs"]
mod tests;
