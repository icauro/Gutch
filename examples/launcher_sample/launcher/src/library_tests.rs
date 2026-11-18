use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::thread;

use gutch::api::{
    ObjectStoreRemote, RemoteObjectStore, RemoteRepoCreateRequest, Repo, RepoPushRequest,
    SnapshotCaptureRequest, capture_snapshot, create_remote_repo, push_repo,
};
use gutch_s3_store::MemoryStore;
use launcher_catalog::{CATALOG_KEY, REPO_SCOPE};

use super::*;

/// The keys the server was asked for, in order.
type RequestLog = Arc<Mutex<Vec<String>>>;

/// A bucket served over HTTP, which records the keys it was asked for.
struct Bucket {
    store: MemoryStore,
    url: String,
    requests: RequestLog,
    catalog: Catalog,
}

impl Bucket {
    fn requested_bundles(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .drain(..)
            .filter(|key| key.contains("/bundles/"))
            .collect()
    }

    fn bundle_keys(&self) -> Vec<String> {
        self.store
            .list(&format!("repos/{}/bundles/", self.catalog.repo_id))
            .unwrap()
    }

    fn entry(&self, version: &str) -> CatalogEntry {
        self.catalog
            .versions
            .iter()
            .find(|entry| entry.version == version)
            .unwrap()
            .clone()
    }
}

/// A version and its files' paths and bytes.
type Version<'a> = (&'a str, Vec<(&'a str, Vec<u8>)>);

/// Publishes each version's files as a snapshot, like the publisher does,
/// and writes the catalog.
fn publish(work: &Path, versions: &[Version]) -> Bucket {
    let store = MemoryStore::default();
    let remote = ObjectStoreRemote::new(store.clone());
    let repo_root = work.join("publisher").join("blender");
    Repo::init(&repo_root).unwrap();
    create_remote_repo(
        &RemoteRepoCreateRequest {
            repo_root: repo_root.clone(),
            scope_id: REPO_SCOPE.to_owned(),
            control: Default::default(),
        },
        &remote,
    )
    .unwrap();
    let mut catalog = Catalog {
        format: CATALOG_FORMAT,
        repo_id: format!("{REPO_SCOPE}--blender"),
        versions: Vec::new(),
    };
    for (version, files) in versions {
        let source = work.join("sources").join(version);
        for (path, bytes) in files {
            fs::create_dir_all(source.join(path).parent().unwrap()).unwrap();
            fs::write(source.join(path), bytes).unwrap();
        }
        let captured = capture_snapshot(&SnapshotCaptureRequest {
            repo_root: repo_root.clone(),
            source,
            ..Default::default()
        })
        .unwrap();
        push_repo(
            &RepoPushRequest {
                repo_root: repo_root.clone(),
                control: Default::default(),
            },
            &remote,
        )
        .unwrap();
        catalog.versions.push(CatalogEntry {
            version: (*version).to_owned(),
            os: Os::current().unwrap_or_default(),
            snapshot_id: captured.snapshot_id,
            file_count: files.len() as u64,
            size: files.iter().map(|(_, bytes)| bytes.len() as u64).sum(),
            published_at: String::new(),
        });
    }
    store
        .put(CATALOG_KEY, &serde_json::to_vec(&catalog).unwrap())
        .unwrap();
    let (url, requests) = serve(store.clone());
    Bucket {
        store,
        url,
        requests,
        catalog,
    }
}

fn serve(store: MemoryStore) -> (String, RequestLog) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&requests);
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let store = store.clone();
            let log = Arc::clone(&log);
            thread::spawn(move || respond(stream, &store, &log));
        }
    });
    (url, requests)
}

/// Answers one GET and closes the connection.
fn respond(mut stream: TcpStream, store: &MemoryStore, log: &Mutex<Vec<String>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request = String::new();
    if reader.read_line(&mut request).is_err() {
        return;
    }
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) | Err(_) => break,
            Ok(_) if header == "\r\n" => break,
            Ok(_) => {}
        }
    }
    let key = request
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .trim_start_matches('/')
        .to_owned();
    let body = store.get(&key).ok().flatten();
    log.lock().unwrap().push(key);
    let (status, body) = match body {
        Some(body) => ("200 OK", body),
        None => ("404 Not Found", Vec::new()),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
}

/// Bytes that do not compress, so each file needs real bundle space.
fn noise(seed: u64, length: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn two_versions(work: &Path) -> Bucket {
    publish(
        work,
        &[
            (
                "5.1.0",
                vec![
                    ("blender.exe", noise(1, 300_000)),
                    ("lib/shared.dll", noise(2, 900_000)),
                ],
            ),
            (
                "5.2.0",
                vec![
                    ("blender.exe", noise(3, 300_000)),
                    ("lib/shared.dll", noise(2, 900_000)),
                ],
            ),
        ],
    )
}

#[test]
fn install_verify_repair_and_uninstall() {
    let temp = tempfile::tempdir().unwrap();
    let bucket = two_versions(temp.path());
    let library = Library::new(temp.path().join("data"), &bucket.url);
    let catalog = library.fetch_catalog().unwrap();
    assert_eq!(catalog, {
        let mut sorted = bucket.catalog.clone();
        sorted.sort();
        sorted
    });

    let entry = bucket.entry("5.1.0");
    let folder = library.default_folder(&entry);
    let installed = library
        .install(&catalog, &entry, &folder, None, Default::default())
        .unwrap();
    assert_eq!(
        fs::read(folder.join("lib/shared.dll")).unwrap(),
        noise(2, 900_000)
    );
    let issues = library.verify(&installed, Default::default()).unwrap();
    assert_eq!(issues.missing_file_count() + issues.corrupt_file_count(), 0);

    fs::remove_file(folder.join("blender.exe")).unwrap();
    fs::write(folder.join("lib/shared.dll"), b"damaged").unwrap();
    fs::write(folder.join("notes.txt"), b"mine").unwrap();
    let issues = library.verify(&installed, Default::default()).unwrap();
    assert_eq!(issues.missing_file_count(), 1);
    assert_eq!(issues.corrupt_file_count(), 1);
    let message = library
        .repair(&catalog, &installed, Default::default())
        .unwrap();
    assert_eq!(message, "restored 2 files");
    assert_eq!(
        fs::read(folder.join("blender.exe")).unwrap(),
        noise(1, 300_000)
    );
    assert_eq!(fs::read(folder.join("notes.txt")).unwrap(), b"mine");

    library.uninstall(&installed, Default::default()).unwrap();
    assert!(!folder.exists());
    assert!(load_snapshot_manifest(&library.cache(), &installed.snapshot_id).is_err());
}

#[test]
fn a_second_version_downloads_only_its_new_content() {
    let temp = tempfile::tempdir().unwrap();
    let bucket = two_versions(temp.path());
    let first_bundles: Vec<String> = {
        // The bundles 5.1.0's push stored, which 5.2.0 shares in part.
        let library = Library::new(temp.path().join("probe"), &bucket.url);
        let catalog = library.fetch_catalog().unwrap();
        let entry = bucket.entry("5.1.0");
        bucket.requested_bundles();
        library
            .install(
                &catalog,
                &entry,
                &temp.path().join("probe-install"),
                None,
                Default::default(),
            )
            .unwrap();
        bucket.requested_bundles()
    };
    assert!(!first_bundles.is_empty());
    assert!(first_bundles.len() < bucket.bundle_keys().len());

    let library = Library::new(temp.path().join("data"), &bucket.url);
    let catalog = library.fetch_catalog().unwrap();
    let old = library
        .install(
            &catalog,
            &bucket.entry("5.1.0"),
            &temp.path().join("old"),
            None,
            Default::default(),
        )
        .unwrap();
    bucket.requested_bundles();
    let new = library
        .install(
            &catalog,
            &bucket.entry("5.2.0"),
            &temp.path().join("new"),
            Some(&old),
            Default::default(),
        )
        .unwrap();
    let downloaded = bucket.requested_bundles();
    assert!(!downloaded.is_empty());
    assert!(
        downloaded.iter().all(|key| !first_bundles.contains(key)),
        "downloaded content the older version already had: {downloaded:?}"
    );
    assert_eq!(
        fs::read(new.path.join("lib/shared.dll")).unwrap(),
        noise(2, 900_000)
    );

    // Removing the older version keeps what the newer one needs.
    library.uninstall(&old, Default::default()).unwrap();
    let issues = library.verify(&new, Default::default()).unwrap();
    assert_eq!(issues.missing_file_count() + issues.corrupt_file_count(), 0);
    fs::remove_file(new.path.join("lib/shared.dll")).unwrap();
    library.repair(&catalog, &new, Default::default()).unwrap();
    assert_eq!(
        fs::read(new.path.join("lib/shared.dll")).unwrap(),
        noise(2, 900_000)
    );
}

#[test]
fn installing_into_a_folder_with_files_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let bucket = two_versions(temp.path());
    let library = Library::new(temp.path().join("data"), &bucket.url);
    let catalog = library.fetch_catalog().unwrap();
    let folder = temp.path().join("busy");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("keep.txt"), b"x").unwrap();
    let error = library
        .install(
            &catalog,
            &bucket.entry("5.1.0"),
            &folder,
            None,
            Default::default(),
        )
        .unwrap_err();
    assert!(error.contains("is not empty"), "{error}");
    assert_eq!(fs::read(folder.join("keep.txt")).unwrap(), b"x");
}

#[test]
fn a_catalog_in_a_newer_format_asks_for_a_newer_launcher() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::default();
    let catalog = Catalog {
        format: CATALOG_FORMAT + 1,
        ..Default::default()
    };
    store
        .put(CATALOG_KEY, &serde_json::to_vec(&catalog).unwrap())
        .unwrap();
    let (url, _) = serve(store);
    let error = Library::new(temp.path().to_path_buf(), &url)
        .fetch_catalog()
        .unwrap_err();
    assert!(error.contains("update the launcher"), "{error}");
}

#[test]
fn state_is_saved_and_the_newest_version_is_found() {
    let temp = tempfile::tempdir().unwrap();
    let library = Library::new(temp.path().join("data"), DEFAULT_BUCKET_URL);
    assert_eq!(library.load_state().unwrap(), State::default());
    let installed = |version: &str, os| Installed {
        version: version.to_owned(),
        os,
        snapshot_id: format!("id-{version}"),
        path: temp.path().join(version),
    };
    let state = State {
        installs: vec![
            installed("5.9.0", Os::Windows),
            installed("5.10.0", Os::Windows),
            installed("6.0.0", Os::Linux),
        ],
    };
    library.save_state(&state).unwrap();
    let loaded = library.load_state().unwrap();
    assert_eq!(loaded, state);
    assert_eq!(loaded.newest(Os::Windows).unwrap().version, "5.10.0");
}

#[test]
fn folders_move_with_their_files() {
    let temp = tempfile::tempdir().unwrap();
    let from = temp.path().join("a").join("blender-5.2.0");
    fs::create_dir_all(from.join("lib")).unwrap();
    fs::write(from.join("lib/x.dll"), b"x").unwrap();
    let moved = move_folder(&from, &temp.path().join("b")).unwrap();
    assert_eq!(moved, temp.path().join("b").join("blender-5.2.0"));
    assert_eq!(fs::read(moved.join("lib/x.dll")).unwrap(), b"x");
    assert!(!from.exists());
    fs::create_dir_all(&from).unwrap();
    assert!(move_folder(&from, &temp.path().join("b")).is_err());

    let copied = temp.path().join("copy");
    copy_tree(&moved, &copied).unwrap();
    assert_eq!(fs::read(copied.join("lib/x.dll")).unwrap(), b"x");
}
