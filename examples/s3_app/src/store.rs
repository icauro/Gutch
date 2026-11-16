//! A `RemoteObjectStore` backed by an S3-compatible bucket.
//!
//! Each request is a presigned URL (rusty-s3 signs it, reqwest sends it), so
//! the same code talks to AWS S3, MinIO, Cloudflare R2 and other services
//! with the S3 API. Errors never include the URLs, because they carry
//! signatures.

use std::time::Duration;

use gutch::api::{Error, RemoteObjectStore, Result};
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use rusty_s3::actions::ListObjectsV2;
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};

/// How long a signed URL stays valid. Bundle downloads that Gutch starts
/// from `download_url` must begin within this time.
const SIGNED_URL_LIFETIME: Duration = Duration::from_secs(3600);

/// Where the bucket is and how to sign in. The secret is held only in memory.
#[derive(Clone, Default)]
pub struct S3Settings {
    /// For example `https://s3.eu-central-1.amazonaws.com` or `http://localhost:9000`.
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    /// Keys are stored below this folder of the bucket; empty for the bucket root.
    pub prefix: String,
    /// `endpoint/bucket/key` instead of `bucket.endpoint/key`; MinIO needs it.
    pub path_style: bool,
    pub access_key_id: String,
    pub secret_access_key: String,
}

pub struct S3Store {
    bucket: Bucket,
    credentials: Credentials,
    /// Empty, or ends with `/`.
    prefix: String,
    client: Client,
}

impl S3Store {
    pub fn new(settings: &S3Settings) -> Result<Self> {
        let endpoint = settings
            .endpoint
            .trim()
            .parse()
            .map_err(|error| Error::InvalidInput(format!("S3 endpoint: {error}")))?;
        let url_style = if settings.path_style {
            UrlStyle::Path
        } else {
            UrlStyle::VirtualHost
        };
        let bucket = Bucket::new(
            endpoint,
            url_style,
            settings.bucket.trim().to_owned(),
            settings.region.trim().to_owned(),
        )
        .map_err(|error| Error::InvalidInput(format!("S3 bucket: {error}")))?;
        let prefix = settings.prefix.trim().trim_matches('/');
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|error| s3_error("start the HTTP client", "", &error.to_string()))?;
        Ok(Self {
            bucket,
            credentials: Credentials::new(
                settings.access_key_id.trim(),
                settings.secret_access_key.trim(),
            ),
            prefix: if prefix.is_empty() {
                String::new()
            } else {
                format!("{prefix}/")
            },
            client,
        })
    }

    fn object(&self, key: &str) -> String {
        format!("{}{key}", self.prefix)
    }

    fn send(
        &self,
        action: &str,
        key: &str,
        request: reqwest::blocking::RequestBuilder,
    ) -> Result<Response> {
        request
            .send()
            .map_err(|error| s3_error(action, key, &error.without_url().to_string()))
    }
}

impl RemoteObjectStore for S3Store {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let object = self.object(key);
        let url = self
            .bucket
            .get_object(Some(&self.credentials), &object)
            .sign(SIGNED_URL_LIFETIME);
        let response = self.send("read", key, self.client.get(url))?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(None),
            status if status.is_success() => {
                let bytes = response
                    .bytes()
                    .map_err(|error| s3_error("read", key, &error.without_url().to_string()))?;
                Ok(Some(bytes.to_vec()))
            }
            _ => Err(failed("read", key, response)),
        }
    }

    /// A single PUT replaces the object at once.
    fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let object = self.object(key);
        let url = self
            .bucket
            .put_object(Some(&self.credentials), &object)
            .sign(SIGNED_URL_LIFETIME);
        let response = self.send("write", key, self.client.put(url).body(bytes.to_vec()))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(failed("write", key, response))
        }
    }

    /// `If-None-Match: *` makes the bucket refuse the write when the key
    /// exists (412), or while another conditional write to it runs (409).
    fn put_if_absent(&self, key: &str, bytes: &[u8]) -> Result<bool> {
        let object = self.object(key);
        let url = self
            .bucket
            .put_object(Some(&self.credentials), &object)
            .sign(SIGNED_URL_LIFETIME);
        let request = self
            .client
            .put(url)
            .header("If-None-Match", "*")
            .body(bytes.to_vec());
        let response = self.send("create", key, request)?;
        match response.status() {
            StatusCode::PRECONDITION_FAILED | StatusCode::CONFLICT => Ok(false),
            status if status.is_success() => Ok(true),
            _ => Err(failed("create", key, response)),
        }
    }

    fn delete(&self, key: &str) -> Result<()> {
        let object = self.object(key);
        let url = self
            .bucket
            .delete_object(Some(&self.credentials), &object)
            .sign(SIGNED_URL_LIFETIME);
        let response = self.send("delete", key, self.client.delete(url))?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(failed("delete", key, response))
        }
    }

    /// The `/` delimiter keeps the listing to objects directly below `prefix`.
    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let full_prefix = self.object(prefix);
        let mut keys = Vec::new();
        let mut continuation = None::<String>;
        loop {
            let mut action = self.bucket.list_objects_v2(Some(&self.credentials));
            action.with_prefix(full_prefix.as_str());
            action.with_delimiter("/");
            if let Some(token) = &continuation {
                action.with_continuation_token(token.clone());
            }
            let url = action.sign(SIGNED_URL_LIFETIME);
            let response = self.send("list", prefix, self.client.get(url))?;
            if !response.status().is_success() {
                return Err(failed("list", prefix, response));
            }
            let text = response
                .text()
                .map_err(|error| s3_error("list", prefix, &error.without_url().to_string()))?;
            let page = ListObjectsV2::parse_response(&text)
                .map_err(|error| s3_error("list", prefix, &error.to_string()))?;
            keys.extend(
                page.contents.into_iter().filter_map(|object| {
                    object.key.strip_prefix(&self.prefix).map(ToOwned::to_owned)
                }),
            );
            match page.next_continuation_token {
                Some(token) => continuation = Some(token),
                None => return Ok(keys),
            }
        }
    }

    /// HEAD avoids downloading the object.
    fn exists(&self, key: &str) -> Result<bool> {
        let object = self.object(key);
        let url = self
            .bucket
            .head_object(Some(&self.credentials), &object)
            .sign(SIGNED_URL_LIFETIME);
        let response = self.send("check", key, self.client.head(url))?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(false),
            status if status.is_success() => Ok(true),
            _ => Err(failed("check", key, response)),
        }
    }

    /// Gutch downloads bundles from presigned links itself, in parallel.
    fn download_url(&self, key: &str) -> Result<Option<String>> {
        let object = self.object(key);
        let url = self
            .bucket
            .get_object(Some(&self.credentials), &object)
            .sign(SIGNED_URL_LIFETIME);
        Ok(Some(url.into()))
    }
}

/// The bucket's XML error names the cause, for example `AccessDenied`.
fn failed(action: &str, key: &str, response: Response) -> Error {
    let status = response.status();
    let body = response.text().unwrap_or_default();
    let detail = match error_code(&body) {
        Some(code) => format!("{status} {code}"),
        None => status.to_string(),
    };
    s3_error(action, key, &detail)
}

fn error_code(body: &str) -> Option<&str> {
    let (_, rest) = body.split_once("<Code>")?;
    Some(rest.split_once("</Code>")?.0)
}

fn s3_error(action: &str, key: &str, detail: &str) -> Error {
    if key.is_empty() {
        Error::repo(format!("S3: could not {action}: {detail}"))
    } else {
        Error::repo(format!("S3: could not {action} '{key}': {detail}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(prefix: &str) -> S3Store {
        S3Store::new(&S3Settings {
            endpoint: "http://localhost:9000".into(),
            region: "us-east-1".into(),
            bucket: "builds".into(),
            prefix: prefix.into(),
            path_style: true,
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn keys_go_below_the_prefix_folder() {
        assert_eq!(store("").object("index/a.json"), "index/a.json");
        assert_eq!(
            store("/gutch/").object("index/a.json"),
            "gutch/index/a.json"
        );
    }

    #[test]
    fn download_links_are_presigned_path_style_urls() {
        let url = store("gutch")
            .download_url("repos/r/bundles/b")
            .unwrap()
            .unwrap();
        assert!(url.starts_with("http://localhost:9000/builds/gutch/repos/r/bundles/b?"));
        assert!(url.contains("X-Amz-Signature="));
    }

    #[test]
    fn error_codes_come_from_the_xml_body() {
        let body = "<Error><Code>AccessDenied</Code><Message>no</Message></Error>";
        assert_eq!(error_code(body), Some("AccessDenied"));
        assert_eq!(error_code("not xml"), None);
    }

    #[test]
    fn a_bad_endpoint_is_reported_as_input() {
        let error = S3Store::new(&S3Settings {
            endpoint: "localhost:9000".into(),
            bucket: "builds".into(),
            ..Default::default()
        })
        .err()
        .unwrap();
        assert!(error.to_string().contains("S3"), "{error}");
    }
}
