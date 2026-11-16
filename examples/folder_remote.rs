//! A remote stored in a local folder.
//!
//! Gutch has no client for any particular server. When the remote is plain
//! storage, an application implements the five-method `RemoteObjectStore`
//! trait and wraps it in `ObjectStoreRemote`, which supplies the rest: the
//! key layout, hashes, readiness and a lock against concurrent pushes. This
//! example stores objects as files, then creates a remote, pushes a
//! repository to it, pulls it into a second repository and installs from the
//! copy. (Servers with their own API implement `RemoteRepoReader`,
//! `RemoteRepoWriter` and `RemoteRepoService` directly.)
//!
//! Run with `cargo run --example folder_remote -- WORK_DIR`.
//! WORK_DIR must not exist yet.

use std::env;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gutch::api::{
    ObjectStoreRemote, RemoteObjectStore, RemoteRepoCreateRequest, Repo, RepoPullRequest, Result,
    create_remote_repo, pull_repo,
};

/// Each key is a file path below the root folder.
struct FolderStore {
    root: PathBuf,
}

impl RemoteObjectStore for FolderStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        match fs::read(self.root.join(key)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Writes a temporary file and renames it, so readers never see half an object.
    fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let path = self.root.join(key);
        fs::create_dir_all(path.parent().unwrap())?;
        let partial = path.with_extension("partial");
        fs::write(&partial, bytes)?;
        fs::rename(partial, path)?;
        Ok(())
    }

    /// `create_new` fails if the file exists, so only one caller can win.
    fn put_if_absent(&self, key: &str, bytes: &[u8]) -> Result<bool> {
        let path = self.root.join(key);
        fs::create_dir_all(path.parent().unwrap())?;
        match fs::File::create_new(path) {
            Ok(mut file) => {
                file.write_all(bytes)?;
                Ok(true)
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn delete(&self, key: &str) -> Result<()> {
        match fs::remove_file(self.root.join(key)) {
            Err(error) if error.kind() != ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }

    /// Gutch lists only folders of plain files, with prefixes ending in `/`.
    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let entries = match fs::read_dir(self.root.join(prefix)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut keys = Vec::new();
        for entry in entries {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".partial") {
                keys.push(format!("{prefix}{name}"));
            }
        }
        Ok(keys)
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
    let remote = ObjectStoreRemote::new(FolderStore {
        root: work_dir.join("remote"),
    });
    let source_dir = work_dir.join("source");
    fs::create_dir_all(&source_dir)?;
    fs::write(
        source_dir.join("readme.txt"),
        "hello from the remote
"
        .repeat(100),
    )?;
    let origin = Repo::init(work_dir.join("origin"))?;
    let snapshot_id = origin.capture(&source_dir, &[])?.snapshot_id;

    // Create a remote repo in scope "team" and bind `origin` to it.
    let remote_repo_id = create_remote_repo(
        &RemoteRepoCreateRequest {
            repo_root: origin.root().to_path_buf(),
            scope_id: "team".into(),
            ..Default::default()
        },
        &remote,
    )?
    .remote_repo_id;
    println!("created remote repo {remote_repo_id}");
    let pushed = origin.push(&remote)?;
    println!("pushed: {} bundles uploaded", pushed.bundle_uploaded_count);

    // Pulling into a new folder creates the repository there.
    let copy = work_dir.join("copy");
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

    let copy = Repo::open(copy)?;
    let app_dir = work_dir.join("app");
    copy.reconstruct(&snapshot_id, &app_dir)?;
    let issues = copy.verify(&snapshot_id, &app_dir)?.issues;
    println!("installed from the pulled copy: {issues}");
    Ok(())
}
