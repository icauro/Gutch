use std::collections::BTreeMap;
use std::sync::Mutex;

use super::*;

/// Stands in for the bucket.
#[derive(Default)]
struct MemoryStore {
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl RemoteObjectStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.objects.lock().unwrap().get(key).cloned())
    }

    fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        self.objects
            .lock()
            .unwrap()
            .insert(key.to_owned(), bytes.to_vec());
        Ok(())
    }

    fn put_if_absent(&self, key: &str, bytes: &[u8]) -> Result<bool> {
        let mut objects = self.objects.lock().unwrap();
        if objects.contains_key(key) {
            return Ok(false);
        }
        objects.insert(key.to_owned(), bytes.to_vec());
        Ok(true)
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.objects.lock().unwrap().remove(key);
        Ok(())
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let objects = self.objects.lock().unwrap();
        Ok(objects
            .keys()
            .filter(|key| {
                key.strip_prefix(prefix)
                    .is_some_and(|rest| !rest.contains('/'))
            })
            .cloned()
            .collect())
    }
}

/// Two app instances with their own caches share one store, like two
/// machines sharing a bucket.
struct SharedStore(std::sync::Arc<MemoryStore>);

impl RemoteObjectStore for SharedStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.0.get(key)
    }
    fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        self.0.put(key, bytes)
    }
    fn put_if_absent(&self, key: &str, bytes: &[u8]) -> Result<bool> {
        self.0.put_if_absent(key, bytes)
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.0.delete(key)
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        self.0.list(prefix)
    }
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text.repeat(500)).unwrap();
}

#[test]
fn capture_on_one_machine_install_and_repair_on_another() {
    let temp = tempfile::tempdir().unwrap();
    let bucket = std::sync::Arc::new(MemoryStore::default());
    let first = Remote::new(SharedStore(bucket.clone()), temp.path().join("cache1"));
    let second = Remote::new(SharedStore(bucket), temp.path().join("cache2"));

    let repo_id = first
        .create_repo("team", "game", OperationControl::default())
        .unwrap();
    assert_eq!(repo_id, "team--game");
    assert_eq!(second.repos().unwrap()[0].repository_id, repo_id);
    assert!(second.snapshots(&repo_id).unwrap().is_empty());

    let source = temp.path().join("source");
    write(&source.join("data/a.txt"), "first\n");
    write(&source.join("b.txt"), "second\n");
    let message = first
        .capture_and_push(
            &repo_id,
            &source,
            "version=1.0",
            OperationControl::default(),
        )
        .unwrap();
    assert!(message.starts_with("captured "), "{message}");

    let snapshots = second.snapshots(&repo_id).unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].tags, ["version=1.0"]);
    let old_id = snapshots[0].snapshot_id.clone();

    let app = temp.path().join("app");
    second
        .install(&repo_id, &old_id, &app, None, OperationControl::default())
        .unwrap();
    fs::write(app.join("b.txt"), "damaged").unwrap();
    let verified = second
        .verify(&repo_id, &old_id, &app, OperationControl::default())
        .unwrap();
    assert!(
        verified.ends_with("0 missing, 1 corrupt, 0 extra"),
        "{verified}"
    );
    second
        .repair(&repo_id, &old_id, &app, OperationControl::default())
        .unwrap();
    let verified = second
        .verify(&repo_id, &old_id, &app, OperationControl::default())
        .unwrap();
    assert!(
        verified.ends_with("0 missing, 0 corrupt, 0 extra"),
        "{verified}"
    );

    // The second machine pushes 1.1; the first updates its install in place.
    write(&source.join("b.txt"), "changed\n");
    second
        .capture_and_push(
            &repo_id,
            &source,
            "version=1.1",
            OperationControl::default(),
        )
        .unwrap();
    let snapshots = first.snapshots(&repo_id).unwrap();
    assert_eq!(snapshots.len(), 2);
    let new_id = snapshots
        .iter()
        .find(|snapshot| snapshot.snapshot_id != old_id)
        .unwrap()
        .snapshot_id
        .clone();
    first
        .install(&repo_id, &old_id, &app, None, OperationControl::default())
        .unwrap();
    let message = first
        .install(
            &repo_id,
            &new_id,
            &app,
            Some(&old_id),
            OperationControl::default(),
        )
        .unwrap();
    assert!(!message.contains(" 0 of "), "{message}");
    assert_eq!(
        fs::read_to_string(app.join("b.txt")).unwrap(),
        "changed\n".repeat(500)
    );
}

#[test]
fn create_repo_refuses_a_taken_name_and_leaves_no_copy() {
    let temp = tempfile::tempdir().unwrap();
    let bucket = std::sync::Arc::new(MemoryStore::default());
    let first = Remote::new(SharedStore(bucket.clone()), temp.path().join("cache1"));
    let second = Remote::new(SharedStore(bucket), temp.path().join("cache2"));
    first
        .create_repo("team", "game", OperationControl::default())
        .unwrap();

    let error = second
        .create_repo("team", "game", OperationControl::default())
        .unwrap_err();

    assert!(error.to_string().contains("already exists"), "{error}");
    assert!(!temp.path().join("cache2/team/game").exists());
}

#[test]
fn capture_rejects_malformed_tags_before_touching_the_remote() {
    let temp = tempfile::tempdir().unwrap();
    let remote = Remote::new(MemoryStore::default(), temp.path().join("cache"));
    let error = remote
        .capture_and_push(
            "team--missing",
            temp.path(),
            "version",
            OperationControl::default(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("use key=value"), "{error}");
}
