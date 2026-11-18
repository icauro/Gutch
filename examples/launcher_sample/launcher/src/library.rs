//! The launcher's work apart from the window: reading the catalog, and
//! installing, checking, repairing, moving and removing versions.
//!
//! Downloads land in a local repository, the cache, which keeps each
//! installed version's snapshot and the bundles fetched for it. Installing a
//! version next to an older one reuses the older one's files, so only the
//! changed content is downloaded.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use gutch::api::{
    ArtifactDownload, InstallationFetchAndReconstructRequest, InstallationIssues,
    InstallationRepairRequest, InstallationVerifyRequest, OperationControl, SnapshotDeleteRequest,
    SnapshotFetchRequest, delete_snapshot, fetch_and_reconstruct_installation, fetch_snapshot,
    load_snapshot_manifest, repair_installation, verify_installation,
};
use launcher_catalog::{CATALOG_FORMAT, Catalog, CatalogEntry, Os, PublicLayout};
use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, String>;

/// Where the sample's builds are published.
pub const DEFAULT_BUCKET_URL: &str = "https://gutch-sample.s3.gra.io.cloud.ovh.net";

/// One installed version.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Installed {
    pub version: String,
    pub os: Os,
    pub snapshot_id: String,
    pub path: PathBuf,
}

/// What the launcher remembers between runs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct State {
    pub installs: Vec<Installed>,
}

impl State {
    /// The newest installed version for `os`, whose files a new install of
    /// the same system can reuse.
    pub fn newest(&self, os: Os) -> Option<&Installed> {
        self.installs
            .iter()
            .filter(|installed| installed.os == os)
            .max_by(|left, right| launcher_catalog::compare_versions(&left.version, &right.version))
    }
}

/// The app's folder name: readable on Windows, lowercase without spaces
/// elsewhere, as each system's own apps name theirs.
#[cfg(windows)]
const APP_FOLDER: &str = "Gutch Launcher Sample";
#[cfg(not(windows))]
const APP_FOLDER: &str = "gutch-launcher-sample";

#[derive(Clone, Debug)]
pub struct Library {
    data_dir: PathBuf,
    state_dir: PathBuf,
    layout: PublicLayout,
}

impl Library {
    /// Keeps the cache, the default install folder and the state in
    /// `data_dir`.
    pub fn new(data_dir: PathBuf, bucket_url: &str) -> Self {
        Self {
            state_dir: data_dir.clone(),
            data_dir,
            layout: PublicLayout::new(bucket_url),
        }
    }

    /// Uses the system's folders for the current user: the local data
    /// folder (`%LOCALAPPDATA%` on Windows, `$XDG_DATA_HOME` or
    /// `~/.local/share` on Linux), and on Linux the state folder
    /// (`$XDG_STATE_HOME` or `~/.local/state`) for the state.
    pub fn for_current_user(bucket_url: &str) -> Result<Self> {
        let data_dir = dirs::data_local_dir()
            .ok_or("this system has no local data folder for the current user")?
            .join(APP_FOLDER);
        let mut library = Self::new(data_dir, bucket_url);
        if let Some(state_dir) = dirs::state_dir() {
            library.state_dir = state_dir.join(APP_FOLDER);
        }
        Ok(library)
    }

    /// Where versions install unless the user picks another folder.
    pub fn versions_dir(&self) -> PathBuf {
        self.data_dir.join("versions")
    }

    fn cache(&self) -> PathBuf {
        self.data_dir.join("cache")
    }

    fn state_path(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }

    /// Where a version installs unless the user picks another folder.
    pub fn default_folder(&self, entry: &CatalogEntry) -> PathBuf {
        self.versions_dir()
            .join(format!("blender-{}", entry.version))
    }

    pub fn load_state(&self) -> Result<State> {
        match fs::read(self.state_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| format!("cannot read {}: {error}", self.state_path().display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(error) => Err(format!(
                "cannot read {}: {error}",
                self.state_path().display()
            )),
        }
    }

    pub fn save_state(&self, state: &State) -> Result<()> {
        let failed = |error: &dyn std::fmt::Display| {
            format!("cannot save {}: {error}", self.state_path().display())
        };
        fs::create_dir_all(&self.state_dir).map_err(|error| failed(&error))?;
        let bytes = serde_json::to_vec_pretty(state).map_err(|error| failed(&error))?;
        // Written beside and renamed, so a crash never leaves half a file.
        let temporary = self.state_path().with_extension("json.new");
        fs::write(&temporary, bytes).map_err(|error| failed(&error))?;
        fs::rename(&temporary, self.state_path()).map_err(|error| failed(&error))
    }

    pub fn fetch_catalog(&self) -> Result<Catalog> {
        let url = self.layout.catalog();
        let failed = |error: &dyn std::fmt::Display| format!("cannot read {url}: {error}");
        let bytes = reqwest::blocking::get(&url)
            .and_then(|response| response.error_for_status())
            .and_then(|response| response.bytes())
            .map_err(|error| failed(&error))?;
        let mut catalog: Catalog =
            serde_json::from_slice(&bytes).map_err(|error| failed(&error))?;
        if catalog.format != CATALOG_FORMAT {
            return Err(format!(
                "the catalog has format {}; this launcher reads format {CATALOG_FORMAT}, \
                 so update the launcher",
                catalog.format
            ));
        }
        catalog.sort();
        Ok(catalog)
    }

    /// Downloads and installs `entry` into the new folder `destination`.
    /// With `reuse`, an installed version of the same system, files it shares
    /// with the new one are copied from it instead of downloaded.
    pub fn install(
        &self,
        catalog: &Catalog,
        entry: &CatalogEntry,
        destination: &Path,
        reuse: Option<&Installed>,
        control: OperationControl,
    ) -> Result<Installed> {
        let occupied = fs::read_dir(destination).is_ok_and(|mut entries| entries.next().is_some());
        if occupied {
            return Err(format!(
                "{} is not empty; pick an empty or new folder",
                destination.display()
            ));
        }
        let reuse = reuse.filter(|installed| installed.os == entry.os && installed.path.is_dir());
        fetch_and_reconstruct_installation(&InstallationFetchAndReconstructRequest {
            fetch_request: self.fetch_request(catalog, &entry.snapshot_id, control),
            destination_dir: destination.to_path_buf(),
            staging_dir: None,
            reuse_snapshot_id: reuse.map(|installed| installed.snapshot_id.clone()),
            reuse_dir: reuse.map(|installed| installed.path.clone()),
            runtime_hints: Default::default(),
        })
        .map_err(|error| error.to_string())?;
        Ok(Installed {
            version: entry.version.clone(),
            os: entry.os,
            snapshot_id: entry.snapshot_id.clone(),
            path: destination.to_path_buf(),
        })
    }

    /// Compares the installed files with the snapshot.
    pub fn verify(
        &self,
        installed: &Installed,
        control: OperationControl,
    ) -> Result<InstallationIssues> {
        Ok(verify_installation(&InstallationVerifyRequest {
            repo_root: self.cache(),
            snapshot_id: installed.snapshot_id.clone(),
            destination_dir: installed.path.clone(),
            control,
            ..Default::default()
        })
        .map_err(|error| error.to_string())?
        .issues)
    }

    /// Restores missing and changed files; files the user added stay. The
    /// bundles a repair needs are downloaded first while the version is
    /// still published.
    pub fn repair(
        &self,
        catalog: &Catalog,
        installed: &Installed,
        control: OperationControl,
    ) -> Result<String> {
        let issues = self.verify(installed, control.clone())?;
        let damaged = issues.missing_file_count() + issues.corrupt_file_count();
        if damaged == 0 {
            return Ok("nothing to repair".into());
        }
        let published = catalog
            .versions
            .iter()
            .any(|entry| entry.snapshot_id == installed.snapshot_id);
        if published {
            fetch_snapshot(&self.fetch_request(catalog, &installed.snapshot_id, control.clone()))
                .map_err(|error| error.to_string())?;
        }
        let repaired = repair_installation(&InstallationRepairRequest {
            repo_root: self.cache(),
            snapshot_id: installed.snapshot_id.clone(),
            destination_dir: installed.path.clone(),
            remove_extra_files: false,
            control,
            ..Default::default()
        })
        .map_err(|error| {
            if published {
                error.to_string()
            } else {
                format!("{error}; this version is no longer published, so reinstall another")
            }
        })?;
        Ok(format!(
            "restored {} files",
            repaired.missing_file_count + repaired.corrupt_file_count
        ))
    }

    /// Removes the folder and the snapshot's cached bundles that no other
    /// installed version needs.
    pub fn uninstall(&self, installed: &Installed, control: OperationControl) -> Result<()> {
        if installed.path.exists() {
            fs::remove_dir_all(&installed.path)
                .map_err(|error| format!("cannot remove {}: {error}", installed.path.display()))?;
        }
        let cached = load_snapshot_manifest(&self.cache(), &installed.snapshot_id).is_ok();
        if cached {
            delete_snapshot(&SnapshotDeleteRequest {
                repo_root: self.cache(),
                snapshot_id: installed.snapshot_id.clone(),
                control,
            })
            .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn fetch_request(
        &self,
        catalog: &Catalog,
        snapshot_id: &str,
        control: OperationControl,
    ) -> SnapshotFetchRequest {
        let layout = self.layout.clone();
        let repo_id = catalog.repo_id.clone();
        let bundle_urls = move |ids: &[String], _: &OperationControl| {
            Ok(ids
                .iter()
                .map(|id| ArtifactDownload {
                    artifact_id: id.clone(),
                    url: layout.bundle(&repo_id, id),
                })
                .collect())
        };
        SnapshotFetchRequest {
            repo_root: self.cache(),
            snapshot_id: snapshot_id.to_owned(),
            repo_metadata_url: Some(self.layout.repo_metadata(&catalog.repo_id)),
            metadata_url: self.layout.snapshot_metadata(&catalog.repo_id, snapshot_id),
            product_metadata_url: None,
            blueprint_url: self
                .layout
                .snapshot_blueprint(&catalog.repo_id, snapshot_id),
            chunk_index_url: Some(self.layout.chunk_index(&catalog.repo_id)),
            artifacts: Vec::new(),
            artifact_url_provider: Some(Arc::new(bundle_urls)),
            transfer_policy: Default::default(),
            control,
        }
    }
}

/// Moves an installed version's folder into `new_parent` and returns its
/// new path. A move to another drive copies, then removes the original.
pub fn move_folder(from: &Path, new_parent: &Path) -> Result<PathBuf> {
    let name = from
        .file_name()
        .ok_or_else(|| format!("{} has no folder name", from.display()))?;
    let target = new_parent.join(name);
    if target.exists() {
        return Err(format!("{} already exists", target.display()));
    }
    fs::create_dir_all(new_parent)
        .map_err(|error| format!("cannot create {}: {error}", new_parent.display()))?;
    if fs::rename(from, &target).is_ok() {
        return Ok(target);
    }
    if let Err(error) = copy_tree(from, &target) {
        let _ = fs::remove_dir_all(&target);
        return Err(format!("cannot copy to {}: {error}", target.display()));
    }
    fs::remove_dir_all(from)
        .map_err(|error| format!("copied, but cannot remove {}: {error}", from.display()))?;
    Ok(target)
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_symlink() {
            copy_link(&entry.path(), &target)?;
        } else {
            // Copies the permissions too, so executables stay executable.
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_link(from: &Path, to: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(from)?, to)
}

#[cfg(not(unix))]
fn copy_link(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::copy(from, to).map(|_| ())
}

/// Starts the installed version without waiting for it.
pub fn launch(installed: &Installed) -> Result<()> {
    let program = installed.path.join(installed.os.executable());
    Command::new(&program)
        .current_dir(&installed.path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("cannot start {}: {error}", program.display()))
}

#[cfg(test)]
#[path = "library_tests.rs"]
mod tests;
