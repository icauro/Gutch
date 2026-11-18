use std::collections::HashMap;
use std::io::Write;

use gutch::api::Result as GutchResult;
use gutch_s3_store::MemoryStore;

use super::*;

/// A bucket in memory that adds up its objects' sizes.
#[derive(Clone, Default)]
struct TestBucket(MemoryStore);

impl RemoteObjectStore for TestBucket {
    fn get(&self, key: &str) -> GutchResult<Option<Vec<u8>>> {
        self.0.get(key)
    }
    fn put(&self, key: &str, bytes: &[u8]) -> GutchResult<()> {
        self.0.put(key, bytes)
    }
    fn put_if_absent(&self, key: &str, bytes: &[u8]) -> GutchResult<bool> {
        self.0.put_if_absent(key, bytes)
    }
    fn delete(&self, key: &str) -> GutchResult<()> {
        self.0.delete(key)
    }
    fn list(&self, prefix: &str) -> GutchResult<Vec<String>> {
        self.0.list(prefix)
    }
}

impl Bucket for TestBucket {
    fn total_bytes(&self) -> Result<u64> {
        Ok(self
            .0
            .objects()
            .values()
            .map(|bytes| bytes.len() as u64)
            .sum())
    }
}

/// Builds made up in memory: a shared library that never changes and an
/// executable that differs per version.
#[derive(Default)]
struct FakeReleases {
    archives: HashMap<String, Vec<u8>>,
    builds: Vec<Build>,
}

impl FakeReleases {
    fn add(&mut self, version: &str) {
        let name = format!("blender-{version}-windows-x64");
        let mut bytes = Vec::new();
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file(format!("{name}/blender.exe"), options)
            .unwrap();
        zip.write_all(&noise(version.as_bytes(), 200_000)).unwrap();
        zip.start_file(format!("{name}/lib/shared.dll"), options)
            .unwrap();
        zip.write_all(&noise(b"shared", 600_000)).unwrap();
        zip.finish().unwrap();
        let url = format!("https://example.test/{name}.zip");
        let sha256 = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.builds.push(Build {
            version: version.to_owned(),
            os: Os::Windows,
            url: url.clone(),
            sha256,
        });
        self.archives.insert(url, bytes);
    }
}

impl ReleaseSource for FakeReleases {
    fn builds(&self) -> Result<Vec<Build>> {
        Ok(self.builds.clone())
    }

    fn download(&self, build: &Build, target: &Path) -> Result<()> {
        fs::write(target, &self.archives[&build.url]).map_err(|error| error.to_string())
    }
}

/// Bytes that do not compress, so sizes are predictable.
fn noise(seed: &[u8], length: usize) -> Vec<u8> {
    let mut state = seed.iter().fold(0x9e37_79b9_7f4a_7c15_u64, |state, byte| {
        (state ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
    });
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn options(work_dir: &Path) -> Options {
    Options {
        work_dir: work_dir.to_path_buf(),
        series: 2,
        max_new: 10,
        budget_bytes: u64::MAX,
        dry_run: false,
    }
}

fn catalog(bucket: &TestBucket) -> Catalog {
    serde_json::from_slice(&bucket.get(CATALOG_KEY).unwrap().unwrap()).unwrap()
}

fn versions(catalog: &Catalog) -> Vec<&str> {
    catalog
        .versions
        .iter()
        .map(|entry| entry.version.as_str())
        .collect()
}

#[test]
fn the_newest_patch_of_the_newest_series_is_wanted() {
    let mut releases = FakeReleases::default();
    for version in ["5.0.1", "5.1.0", "5.1.1", "5.2.0", "5.10.0"] {
        releases.add(version);
    }
    let wanted = wanted_builds(releases.builds.clone(), 3);
    let versions: Vec<_> = wanted.iter().map(|build| build.version.as_str()).collect();
    assert_eq!(versions, ["5.10.0", "5.2.0", "5.1.1"]);
}

#[test]
fn runs_add_new_builds_replace_old_patches_and_upload_only_new_content() {
    let temp = tempfile::tempdir().unwrap();
    let bucket = TestBucket::default();
    let remote = ObjectStoreRemote::new(bucket.clone());
    let mut releases = FakeReleases::default();
    releases.add("5.1.0");
    releases.add("5.2.0");

    let first = run(
        &remote,
        &releases,
        &options(&temp.path().join("w")),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(first.added, ["5.2.0 windows", "5.1.0 windows"]);
    let catalog_one = catalog(&bucket);
    assert_eq!(catalog_one.repo_id, "launcher--blender");
    assert_eq!(versions(&catalog_one), ["5.2.0", "5.1.0"]);
    assert_eq!(catalog_one.versions[0].file_count, 2);
    assert_eq!(catalog_one.versions[0].size, 800_000);
    let size_one = bucket.total_bytes().unwrap();

    // A new patch replaces 5.2.0; the shared library is not uploaded again.
    releases.add("5.2.1");
    let second = run(
        &remote,
        &releases,
        &options(&temp.path().join("w")),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(second.added, ["5.2.1 windows"]);
    assert_eq!(second.removed, ["5.2.0 windows"]);
    assert_eq!(versions(&catalog(&bucket)), ["5.2.1", "5.1.0"]);
    let size_two = bucket.total_bytes().unwrap();
    assert!(
        size_two < size_one + 300_000,
        "the second run stored the shared content again: {size_one} -> {size_two}"
    );

    // Nothing new: nothing changes.
    let third = run(
        &remote,
        &releases,
        &options(&temp.path().join("w")),
        &mut |_| {},
    )
    .unwrap();
    assert!(third.added.is_empty() && third.removed.is_empty());
}

#[test]
fn the_budget_and_max_new_postpone_builds() {
    let temp = tempfile::tempdir().unwrap();
    let bucket = TestBucket::default();
    let remote = ObjectStoreRemote::new(bucket.clone());
    let mut releases = FakeReleases::default();
    releases.add("5.1.0");
    releases.add("5.2.0");

    let mut limited = options(&temp.path().join("w"));
    limited.max_new = 1;
    let report = run(&remote, &releases, &limited, &mut |_| {}).unwrap();
    assert_eq!(report.added, ["5.2.0 windows"]);
    assert_eq!(report.postponed, ["5.1.0 windows"]);

    let mut tight = options(&temp.path().join("w"));
    tight.budget_bytes = bucket.total_bytes().unwrap() + 1000;
    let report = run(&remote, &releases, &tight, &mut |_| {}).unwrap();
    assert!(report.added.is_empty());
    assert_eq!(report.postponed, ["5.1.0 windows"]);
    assert_eq!(versions(&catalog(&bucket)), ["5.2.0"]);
}

#[test]
fn a_dry_run_changes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let bucket = TestBucket::default();
    let remote = ObjectStoreRemote::new(bucket.clone());
    let mut releases = FakeReleases::default();
    releases.add("5.2.0");
    let mut dry = options(&temp.path().join("w"));
    dry.dry_run = true;
    let report = run(&remote, &releases, &dry, &mut |_| {}).unwrap();
    assert_eq!(report.added, ["5.2.0 windows"]);
    assert!(bucket.0.objects().is_empty());
}

#[test]
fn an_empty_release_listing_retires_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let bucket = TestBucket::default();
    let remote = ObjectStoreRemote::new(bucket.clone());
    let mut releases = FakeReleases::default();
    releases.add("5.2.0");
    run(
        &remote,
        &releases,
        &options(&temp.path().join("w")),
        &mut |_| {},
    )
    .unwrap();

    let error = run(
        &remote,
        &FakeReleases::default(),
        &options(&temp.path().join("w")),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(error.contains("lists no builds"), "{error}");
    assert_eq!(versions(&catalog(&bucket)), ["5.2.0"]);
}

#[test]
fn a_damaged_download_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let remote = ObjectStoreRemote::new(TestBucket::default());
    let mut releases = FakeReleases::default();
    releases.add("5.2.0");
    releases.builds[0].sha256 = "0".repeat(64);
    let error = run(
        &remote,
        &releases,
        &options(&temp.path().join("w")),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(error.contains("does not match"), "{error}");
}
