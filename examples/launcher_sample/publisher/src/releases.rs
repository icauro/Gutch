//! Finding official Blender builds on the release server.
//!
//! `download.blender.org/release/` lists one folder per series, such as
//! `Blender5.2/`. Each folder lists the builds and a `blender-X.Y.Z.sha256`
//! file with the SHA-256 of every build of that version.
//!
//! Mirrors copy this layout. A build read from a mirror can be confirmed by
//! other mirrors: it is used only once each of them publishes the same
//! SHA-256 for it.

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::Path;
use std::time::Duration;

use launcher_catalog::{Os, compare_versions};
use reqwest::StatusCode;
use reqwest::blocking::Client;

use crate::publish::{Build, ReleaseSource};

pub const BLENDER_RELEASES: &str = "https://download.blender.org/release/";

/// The release server, or a copy of its layout.
pub struct HttpReleases {
    base: String,
    /// Other copies of the layout whose checksums must match.
    confirm: Vec<String>,
    min_version: String,
    client: Client,
}

impl HttpReleases {
    /// Series older than `min_version` (such as `5.0`) are not read.
    /// Each of `confirm` must publish the same SHA-256 for a build.
    pub fn new(base: &str, confirm: &[String], min_version: &str) -> Result<Self, String> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(None)
            .user_agent("gutch-launcher-sample-publisher")
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            base: folder_url(base),
            confirm: confirm.iter().map(|base| folder_url(base)).collect(),
            min_version: min_version.to_owned(),
            client,
        })
    }

    fn text(&self, url: &str) -> Result<String, String> {
        self.text_if_present(url)?
            .ok_or_else(|| format!("could not read {url}: 404 Not Found"))
    }

    /// `None` when the server has no such file.
    fn text_if_present(&self, url: &str) -> Result<Option<String>, String> {
        let failed = |error: &dyn std::fmt::Display| format!("could not read {url}: {error}");
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|error| failed(&error))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = response
            .error_for_status()
            .map_err(|error| failed(&error))?;
        response.text().map(Some).map_err(|error| failed(&error))
    }

    /// The checksums `base` publishes for `versions` of `series`; files a
    /// mirror has not copied yet are missing.
    fn sums(
        &self,
        base: &str,
        series: &str,
        versions: &[String],
    ) -> Result<HashMap<String, String>, String> {
        let mut sums = HashMap::new();
        for version in versions {
            let url = format!("{base}Blender{series}/blender-{version}.sha256");
            if let Some(text) = self.text_if_present(&url)? {
                sums.extend(parse_sums(&text));
            }
        }
        Ok(sums)
    }
}

fn folder_url(base: &str) -> String {
    format!("{}/", base.trim().trim_end_matches('/'))
}

/// Whether every other source publishes `sha256` for `file_name`. A missing
/// checksum leaves the build for a later run; a different one is an error.
fn confirmed(
    file_name: &str,
    sha256: &str,
    others: &[HashMap<String, String>],
) -> Result<bool, String> {
    for other in others {
        match other.get(file_name) {
            None => return Ok(false),
            Some(other) if other != sha256 => {
                return Err(format!(
                    "the mirrors publish different SHA-256 values for {file_name}; \
                     refusing it"
                ));
            }
            Some(_) => {}
        }
    }
    Ok(true)
}

impl ReleaseSource for HttpReleases {
    fn builds(&self) -> Result<Vec<Build>, String> {
        // A wrong address fails here, not as builds that never confirm.
        for base in &self.confirm {
            self.text(base)?;
        }
        let mut builds = Vec::new();
        for series in series_folders(&self.text(&self.base)?) {
            if compare_versions(&series, &self.min_version).is_lt() {
                continue;
            }
            let folder = format!("{}Blender{series}/", self.base);
            let listing = self.text(&folder)?;
            let versions = sum_files(&listing);
            let sums = self.sums(&self.base, &series, &versions)?;
            let others = self
                .confirm
                .iter()
                .map(|base| self.sums(base, &series, &versions))
                .collect::<Result<Vec<_>, _>>()?;
            for (file_name, version, os) in build_files(&listing) {
                // A build without a published checksum cannot be checked.
                let Some(sha256) = sums.get(&file_name) else {
                    continue;
                };
                if !confirmed(&file_name, sha256, &others)? {
                    continue;
                }
                builds.push(Build {
                    version,
                    os,
                    url: format!("{folder}{file_name}"),
                    sha256: sha256.clone(),
                });
            }
        }
        Ok(builds)
    }

    fn download(&self, build: &Build, target: &Path) -> Result<(), String> {
        let failed =
            |error: &dyn std::fmt::Display| format!("could not download {}: {error}", build.url);
        let mut response = self
            .client
            .get(&build.url)
            .send()
            .and_then(|response| response.error_for_status())
            .map_err(|error| failed(&error))?;
        let mut file = File::create(target).map_err(|error| failed(&error))?;
        io::copy(&mut response, &mut file).map_err(|error| failed(&error))?;
        Ok(())
    }
}

/// The `href` values of a server's folder listing.
fn links(listing: &str) -> impl Iterator<Item = &str> {
    listing
        .split("href=\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"').map(|(link, _)| link))
}

/// `Blender5.2/` gives `5.2`.
fn series_folders(listing: &str) -> Vec<String> {
    links(listing)
        .filter_map(|link| link.strip_prefix("Blender")?.strip_suffix('/'))
        .filter(|series| is_version(series))
        .map(str::to_owned)
        .collect()
}

/// `blender-5.2.4.sha256` gives `5.2.4`.
fn sum_files(listing: &str) -> Vec<String> {
    links(listing)
        .filter_map(|link| link.strip_prefix("blender-")?.strip_suffix(".sha256"))
        .filter(|version| is_version(version))
        .map(str::to_owned)
        .collect()
}

/// The x64 builds this sample publishes: `blender-5.2.4-windows-x64.zip` and
/// `blender-5.2.4-linux-x64.tar.xz`.
fn build_files(listing: &str) -> Vec<(String, String, Os)> {
    links(listing)
        .filter_map(|link| {
            let rest = link.strip_prefix("blender-")?;
            let (version, os) = if let Some(version) = rest.strip_suffix("-windows-x64.zip") {
                (version, Os::Windows)
            } else {
                (rest.strip_suffix("-linux-x64.tar.xz")?, Os::Linux)
            };
            is_version(version).then(|| (link.to_owned(), version.to_owned(), os))
        })
        .collect()
}

/// Lines of `HASH  FILE_NAME`.
fn parse_sums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (hash, name) = line.trim().split_once(char::is_whitespace)?;
            let hash = hash.to_ascii_lowercase();
            (hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .then(|| (name.trim().trim_start_matches('*').to_owned(), hash))
        })
        .collect()
}

/// Release versions are digits and dots; this skips betas and candidates.
fn is_version(text: &str) -> bool {
    !text.is_empty()
        && text
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = r#"<a href="../">../</a>
<a href="Blender2.79/">Blender2.79/</a>
<a href="Blender5.2/">Blender5.2/</a>
<a href="Blender5.3/">Blender5.3/</a>
<a href="BlenderBenchmark2.0/">BlenderBenchmark2.0/</a>"#;

    const SERIES: &str = r#"<a href="blender-5.2.4-linux-x64.tar.xz">x</a>
<a href="blender-5.2.4-macos-arm64.dmg">x</a>
<a href="blender-5.2.4-windows-arm64.zip">x</a>
<a href="blender-5.2.4-windows-x64.msi">x</a>
<a href="blender-5.2.4-windows-x64.zip">x</a>
<a href="blender-5.2.4.sha256">x</a>
<a href="blender-5.2.5-rc-windows-x64.zip">x</a>"#;

    #[test]
    fn the_listing_gives_series_builds_and_sums() {
        assert_eq!(series_folders(ROOT), ["2.79", "5.2", "5.3"]);
        assert_eq!(sum_files(SERIES), ["5.2.4"]);
        assert_eq!(
            build_files(SERIES),
            [
                (
                    "blender-5.2.4-linux-x64.tar.xz".to_owned(),
                    "5.2.4".to_owned(),
                    Os::Linux
                ),
                (
                    "blender-5.2.4-windows-x64.zip".to_owned(),
                    "5.2.4".to_owned(),
                    Os::Windows
                ),
            ]
        );
    }

    #[test]
    fn only_stable_versions_count() {
        assert!(is_version("5.2.4"));
        assert!(is_version("5.3"));
        for unstable in [
            "5.3.0-rc1",
            "5.3.0-beta",
            "5.3.0-alpha",
            "5.3rc",
            "",
            "5..1",
        ] {
            assert!(!is_version(unstable), "{unstable}");
        }
        let listing = r#"<a href="blender-5.3.0-rc1-windows-x64.zip">x</a>
<a href="blender-5.3.0-windows-x64.zip">x</a>"#;
        let versions: Vec<_> = build_files(listing)
            .into_iter()
            .map(|(_, version, _)| version)
            .collect();
        assert_eq!(versions, ["5.3.0"]);
    }

    #[test]
    fn other_mirrors_must_publish_the_same_checksum() {
        let sums = |hash: &str| HashMap::from([("b.zip".to_owned(), hash.to_owned())]);
        assert!(confirmed("b.zip", "aa", &[]).unwrap());
        assert!(confirmed("b.zip", "aa", &[sums("aa"), sums("aa")]).unwrap());
        assert!(!confirmed("b.zip", "aa", &[sums("aa"), HashMap::new()]).unwrap());
        let error = confirmed("b.zip", "aa", &[sums("aa"), sums("bb")]).unwrap_err();
        assert!(error.contains("different SHA-256"), "{error}");
    }

    #[test]
    fn sums_map_file_names_to_hashes() {
        let hash = "2fd14c50987426897ba60237ee996edf2b4151f4b1a72a460ca20be58c1d2d86";
        let sums = parse_sums(&format!(
            "{hash}  blender-5.2.4-linux-x64.tar.xz\nnot a line\n"
        ));
        assert_eq!(sums.len(), 1);
        assert_eq!(sums["blender-5.2.4-linux-x64.tar.xz"], hash);
    }
}
