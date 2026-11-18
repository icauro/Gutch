//! Publishes official Blender builds to a public S3 bucket as a Gutch
//! repository, for the Gutch Launcher Sample. Run by the scheduled workflow;
//! see the sample's README.
//!
//! The bucket comes from `S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET` and the
//! optional `S3_PREFIX`; the keys from `AWS_ACCESS_KEY_ID` and
//! `AWS_SECRET_ACCESS_KEY`.

mod archive;
mod publish;
mod releases;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use gutch::api::{ObjectStoreRemote, RemoteObjectStore};
use gutch_s3_store::{S3Settings, S3Store, check_bucket, explain};

use crate::publish::{Bucket, Options, Result};
use crate::releases::{BLENDER_RELEASES, HttpReleases};

#[derive(Parser)]
#[command(about = "Publish Blender builds for the Gutch Launcher Sample")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check the keys, conditional writes and anonymous downloads.
    Check,
    /// Add new builds, remove old ones and rewrite catalog.json.
    Publish {
        /// Holds the repository copy and the downloads; emptied first.
        #[arg(long, default_value = "publish-work")]
        work_dir: PathBuf,
        /// Ignore series older than this.
        #[arg(long, default_value = "5.0")]
        min_version: String,
        /// Keep the newest patch release of this many series.
        #[arg(long, default_value_t = 8)]
        series: usize,
        /// Add at most this many builds in one run.
        #[arg(long, default_value_t = 4)]
        max_new: usize,
        /// Stop adding builds when the bucket would grow past this.
        #[arg(long, default_value_t = 18.0)]
        budget_gib: f64,
        /// The release server, or a mirror of its layout.
        #[arg(long, default_value = BLENDER_RELEASES)]
        releases_url: String,
        /// Another mirror that must publish the same checksums; repeatable.
        #[arg(long = "confirm-url")]
        confirm_urls: Vec<String>,
        /// Show what would change without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
}

/// The S3 store, which can also add up the bucket's size.
struct S3Bucket(S3Store);

impl RemoteObjectStore for S3Bucket {
    fn get(&self, key: &str) -> gutch::api::Result<Option<Vec<u8>>> {
        self.0.get(key)
    }
    fn put(&self, key: &str, bytes: &[u8]) -> gutch::api::Result<()> {
        self.0.put(key, bytes)
    }
    fn put_if_absent(&self, key: &str, bytes: &[u8]) -> gutch::api::Result<bool> {
        self.0.put_if_absent(key, bytes)
    }
    fn delete(&self, key: &str) -> gutch::api::Result<()> {
        self.0.delete(key)
    }
    fn list(&self, prefix: &str) -> gutch::api::Result<Vec<String>> {
        self.0.list(prefix)
    }
    fn exists(&self, key: &str) -> gutch::api::Result<bool> {
        self.0.exists(key)
    }
    fn download_url(&self, key: &str) -> gutch::api::Result<Option<String>> {
        self.0.download_url(key)
    }
}

impl Bucket for S3Bucket {
    fn total_bytes(&self) -> Result<u64> {
        self.0
            .total_bytes("")
            .map_err(|error| explain(&error.to_string()))
    }
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Check => check(),
        Command::Publish {
            work_dir,
            min_version,
            series,
            max_new,
            budget_gib,
            releases_url,
            confirm_urls,
            dry_run,
        } => run_publish(
            &Options {
                work_dir,
                series,
                max_new,
                budget_bytes: (budget_gib * (1u64 << 30) as f64) as u64,
                dry_run,
            },
            &releases_url,
            &confirm_urls,
            &min_version,
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {}", explain(&error));
            ExitCode::FAILURE
        }
    }
}

fn run_publish(
    options: &Options,
    releases_url: &str,
    confirm_urls: &[String],
    min_version: &str,
) -> Result<()> {
    let settings = settings(true)?;
    let remote = ObjectStoreRemote::new(S3Bucket(
        S3Store::new(&settings).map_err(|error| error.to_string())?,
    ));
    let source = HttpReleases::new(releases_url, confirm_urls, min_version)?;
    let report = publish::run(&remote, &source, options, &mut |line| println!("{line}"))?;
    let list = |items: &[String]| {
        if items.is_empty() {
            "none".to_owned()
        } else {
            items.join(", ")
        }
    };
    let verb = if options.dry_run { "would " } else { "" };
    println!("{verb}add: {}", list(&report.added));
    println!("{verb}remove: {}", list(&report.removed));
    println!("left for a later run: {}", list(&report.postponed));
    println!(
        "bucket holds {:.2} GiB",
        report.bucket_bytes as f64 / (1u64 << 30) as f64
    );
    println!("catalog: {}", public_base(&settings)? + "/catalog.json");
    Ok(())
}

/// Checks the bucket as `check_bucket` does, then that an object uploaded
/// with the public-read ACL downloads without keys.
fn check() -> Result<()> {
    let private = settings(false)?;
    let store = S3Store::new(&private).map_err(|error| error.to_string())?;
    println!(
        "{}",
        check_bucket(&store).map_err(|error| error.to_string())?
    );

    let public = S3Store::new(&settings(true)?).map_err(|error| error.to_string())?;
    let probe = "launcher-sample-public-check";
    public
        .put(probe, b"public")
        .map_err(|error| error.to_string())?;
    let url = format!("{}/{probe}", public_base(&private)?);
    let fetched = reqwest::blocking::get(&url).and_then(|response| response.error_for_status());
    store.delete(probe).map_err(|error| error.to_string())?;
    match fetched {
        Ok(_) => {
            println!("anonymous downloads work: {url}");
            Ok(())
        }
        Err(error) => Err(format!(
            "an object uploaded as public-read cannot be downloaded without keys ({error}); \
             the bucket may block public ACLs"
        )),
    }
}

fn settings(public_read: bool) -> Result<S3Settings> {
    Ok(S3Settings {
        endpoint: required("S3_ENDPOINT")?,
        region: required("S3_REGION")?,
        bucket: required("S3_BUCKET")?,
        prefix: std::env::var("S3_PREFIX").unwrap_or_default(),
        path_style: false,
        access_key_id: required("AWS_ACCESS_KEY_ID")?,
        secret_access_key: required("AWS_SECRET_ACCESS_KEY")?,
        public_read,
    })
}

/// The bucket's plain URL, `https://BUCKET.ENDPOINT_HOST/PREFIX`.
fn public_base(settings: &S3Settings) -> Result<String> {
    let (scheme, host) = settings
        .endpoint
        .trim()
        .trim_end_matches('/')
        .split_once("://")
        .ok_or("S3_ENDPOINT must start with https://")?;
    let prefix = settings.prefix.trim().trim_matches('/');
    let base = format!("{scheme}://{}.{host}", settings.bucket.trim());
    Ok(if prefix.is_empty() {
        base
    } else {
        format!("{base}/{prefix}")
    })
}

fn required(name: &str) -> Result<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("set {name}"))
}
