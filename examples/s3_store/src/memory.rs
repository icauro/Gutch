//! An in-memory object store that stands in for a bucket in tests.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use gutch::api::{RemoteObjectStore, Result};

/// Objects kept in memory. Clones share the same objects, like two machines
/// using one bucket.
#[derive(Clone, Default)]
pub struct MemoryStore {
    objects: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
}

impl MemoryStore {
    /// A copy of every object, by key.
    pub fn objects(&self) -> BTreeMap<String, Vec<u8>> {
        self.objects.lock().unwrap().clone()
    }
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
