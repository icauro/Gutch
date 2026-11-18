//! What the publisher and the launcher agree on: the catalog of published
//! versions and where each object sits in the public bucket.
//!
//! The bucket cannot be listed anonymously, so the publisher writes
//! `catalog.json` next to the repository, and the launcher reads it to learn
//! the repository ID and the snapshot of each version.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

/// The catalog's key, at the top of the bucket prefix.
pub const CATALOG_KEY: &str = "catalog.json";

/// The remote repository the publisher keeps is `SCOPE--NAME`.
pub const REPO_SCOPE: &str = "launcher";

/// Raised when the catalog's shape changes, so old launchers stop reading it.
pub const CATALOG_FORMAT: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Catalog {
    pub format: u32,
    pub repo_id: String,
    /// Newest first.
    pub versions: Vec<CatalogEntry>,
}

/// One published build.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct CatalogEntry {
    /// Such as `5.2.4`.
    pub version: String,
    pub os: Os,
    pub snapshot_id: String,
    pub file_count: u64,
    /// The installed size in bytes.
    pub size: u64,
    /// When the snapshot was captured, as RFC 3339.
    pub published_at: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    #[default]
    Windows,
    Linux,
}

impl Os {
    pub const ALL: [Self; 2] = [Self::Windows, Self::Linux];

    /// The system this program runs on, when builds exist for it.
    pub fn current() -> Option<Self> {
        Self::parse(std::env::consts::OS)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|os| os.name() == name)
    }

    /// The program to start, relative to the installation folder.
    pub fn executable(self) -> &'static str {
        match self {
            Self::Windows => "blender.exe",
            Self::Linux => "blender",
        }
    }
}

impl Catalog {
    /// The versions built for `os`, newest first once sorted.
    pub fn for_os(&self, os: Os) -> impl Iterator<Item = &CatalogEntry> {
        self.versions.iter().filter(move |entry| entry.os == os)
    }

    /// Sorts newest first, Windows before Linux within a version.
    pub fn sort(&mut self) {
        self.versions.sort_by(|left, right| {
            compare_versions(&right.version, &left.version)
                .then(right.os.name().cmp(left.os.name()))
        });
    }
}

/// Compares dotted version numbers by value, so `5.10` comes after `5.9`.
pub fn compare_versions(left: &str, right: &str) -> Ordering {
    let parts = |version: &str| -> Vec<u64> {
        version
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    parts(left).cmp(&parts(right))
}

/// The plain URLs of the published objects below `base`, such as
/// `https://bucket.s3.gra.io.cloud.ovh.net`, or that plus `/prefix`. The
/// layout under `repos/` is the one `ObjectStoreRemote` writes.
#[derive(Clone, Debug)]
pub struct PublicLayout {
    base: String,
}

impl PublicLayout {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim().trim_end_matches('/').to_owned(),
        }
    }

    pub fn catalog(&self) -> String {
        format!("{}/{CATALOG_KEY}", self.base)
    }

    pub fn repo_metadata(&self, repo_id: &str) -> String {
        format!("{}/repos/{repo_id}/repo_metadata", self.base)
    }

    pub fn chunk_index(&self, repo_id: &str) -> String {
        format!("{}/repos/{repo_id}/chunk_index", self.base)
    }

    pub fn snapshot_metadata(&self, repo_id: &str, snapshot_id: &str) -> String {
        format!(
            "{}/repos/{repo_id}/snapshots/{snapshot_id}/metadata",
            self.base
        )
    }

    pub fn snapshot_blueprint(&self, repo_id: &str, snapshot_id: &str) -> String {
        format!(
            "{}/repos/{repo_id}/snapshots/{snapshot_id}/blueprint",
            self.base
        )
    }

    pub fn bundle(&self, repo_id: &str, bundle_id: &str) -> String {
        format!("{}/repos/{repo_id}/bundles/{bundle_id}", self.base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert_eq!(compare_versions("5.10.0", "5.9.3"), Ordering::Greater);
        assert_eq!(compare_versions("4.5", "4.5.0"), Ordering::Less);
        assert_eq!(compare_versions("5.2.4", "5.2.4"), Ordering::Equal);
    }

    #[test]
    fn the_catalog_sorts_newest_first() {
        let entry = |version: &str, os| CatalogEntry {
            version: version.into(),
            os,
            ..Default::default()
        };
        let mut catalog = Catalog {
            versions: vec![
                entry("5.2.3", Os::Linux),
                entry("5.10.0", Os::Linux),
                entry("5.2.3", Os::Windows),
            ],
            ..Default::default()
        };
        catalog.sort();
        let order: Vec<_> = catalog
            .versions
            .iter()
            .map(|entry| (entry.version.as_str(), entry.os))
            .collect();
        assert_eq!(
            order,
            [
                ("5.10.0", Os::Linux),
                ("5.2.3", Os::Windows),
                ("5.2.3", Os::Linux)
            ]
        );
        assert_eq!(catalog.for_os(Os::Windows).count(), 1);
    }

    #[test]
    fn the_catalog_round_trips_as_json() {
        let catalog = Catalog {
            format: CATALOG_FORMAT,
            repo_id: "launcher--blender".into(),
            versions: vec![CatalogEntry {
                version: "5.2.4".into(),
                os: Os::Linux,
                snapshot_id: "ab".into(),
                file_count: 3,
                size: 10,
                published_at: "2026-11-18T00:00:00Z".into(),
            }],
        };
        let json = serde_json::to_string(&catalog).unwrap();
        assert!(json.contains("\"os\":\"linux\""), "{json}");
        assert_eq!(serde_json::from_str::<Catalog>(&json).unwrap(), catalog);
    }

    #[test]
    fn urls_follow_the_object_store_layout() {
        let layout = PublicLayout::new("https://b.example/");
        assert_eq!(layout.catalog(), "https://b.example/catalog.json");
        assert_eq!(
            layout.bundle("launcher--blender", "ff"),
            "https://b.example/repos/launcher--blender/bundles/ff"
        );
        assert_eq!(
            layout.snapshot_blueprint("r", "s"),
            "https://b.example/repos/r/snapshots/s/blueprint"
        );
    }
}
