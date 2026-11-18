//! Connection checks and plain-words causes for failed requests.

use gutch::api::{Error, RemoteObjectStore, Result};

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
mod tests {
    use super::*;
    use crate::MemoryStore;

    #[test]
    fn the_bucket_check_passes_and_leaves_nothing_behind() {
        let store = MemoryStore::default();
        let message = check_bucket(&store).unwrap();
        assert!(message.starts_with("connection works"), "{message}");
        assert!(store.objects().is_empty());
    }

    /// A bucket that ignores `If-None-Match`.
    #[derive(Default)]
    struct UnconditionalStore(MemoryStore);

    impl RemoteObjectStore for UnconditionalStore {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
            self.0.get(key)
        }
        fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
            self.0.put(key, bytes)
        }
        fn put_if_absent(&self, key: &str, bytes: &[u8]) -> Result<bool> {
            self.0.put(key, bytes).map(|()| true)
        }
        fn delete(&self, key: &str) -> Result<()> {
            self.0.delete(key)
        }
        fn list(&self, prefix: &str) -> Result<Vec<String>> {
            self.0.list(prefix)
        }
    }

    #[test]
    fn the_bucket_check_reports_missing_conditional_writes() {
        let store = UnconditionalStore::default();
        let error = check_bucket(&store).unwrap_err().to_string();
        assert!(error.contains("ignores conditional writes"), "{error}");
        assert!(store.0.objects().is_empty());
    }

    #[test]
    fn errors_get_a_plain_cause() {
        let explained = explain("S3: could not list: 404 Not Found NoSuchBucket");
        assert!(
            explained.ends_with("the bucket does not exist."),
            "{explained}"
        );
        assert_eq!(explain("something else"), "something else");
    }
}
