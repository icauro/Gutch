//! Finding official Blender builds on the release server.
//!
//! `download.blender.org/release/` lists one folder per series, such as
//! `Blender5.2/`. Each folder lists the builds and a `blender-X.Y.Z.sha256`
//! file with the SHA-256 of every build of that version.

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::Path;
use std::time::Duration;

use launcher_catalog::{Os, compare_versions};
use reqwest::blocking::Client;

use crate::publish::{Build, ReleaseSource};

pub const BLENDER_RELEASES: &str = "https://download.blender.org/release/";

/// The release server, or a copy of its layout.
pub struct HttpReleases {
    base: String,
    min_version: String,
    client: Client,
}

impl HttpReleases {
    /// Series older than `min_version` (such as `4.2`) are not read.
    pub fn new(base: &str, min_version: &str) -> Result<Self, String> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(None)
            .user_agent("gutch-launcher-sample-publisher")
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            base: format!("{}/", base.trim().trim_end_matches('/')),
            min_version: min_version.to_owned(),
            client,
        })
    }

    fn text(&self, url: &str) -> Result<String, String> {
        let response = self
            .client
            .get(url)
            .send()
            .and_then(|response| response.error_for_status())
            .map_err(|error| format!("could not read {url}: {error}"))?;
        response
            .text()
            .map_err(|error| format!("could not read {url}: {error}"))
    }
}

impl ReleaseSource for HttpReleases {
    fn builds(&self) -> Result<Vec<Build>, String> {
        let mut builds = Vec::new();
        for series in series_folders(&self.text(&self.base)?) {
            if compare_versions(&series, &self.min_version).is_lt() {
                continue;
            }
            let folder = format!("{}Blender{series}/", self.base);
            let listing = self.text(&folder)?;
            let mut sums = HashMap::new();
            for version in sum_files(&listing) {
                let text = self.text(&format!("{folder}blender-{version}.sha256"))?;
                sums.extend(parse_sums(&text));
            }
            for (file_name, version, os) in build_files(&listing) {
                // A build without a published checksum cannot be checked.
                let Some(sha256) = sums.get(&file_name) else {
                    continue;
                };
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
    fn sums_map_file_names_to_hashes() {
        let hash = "2fd14c50987426897ba60237ee996edf2b4151f4b1a72a460ca20be58c1d2d86";
        let sums = parse_sums(&format!(
            "{hash}  blender-5.2.4-linux-x64.tar.xz\nnot a line\n"
        ));
        assert_eq!(sums.len(), 1);
        assert_eq!(sums["blender-5.2.4-linux-x64.tar.xz"], hash);
    }
}
