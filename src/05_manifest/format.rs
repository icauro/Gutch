//! Versioned stored documents and the repository format descriptor.
//!
//! Every stored JSON document starts with `document_type` and `document_version`.
//! Reading checks those markers before the exact parse, so a document written by
//! another Gutch release fails with an [`Error::UnsupportedFormat`] instead of an
//! unknown-field error. `repo_metadata.json` also declares the repository format
//! and the features a reader or writer must understand.

use std::fmt;
use std::marker::PhantomData;

use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{Error, Result};

pub const REPO_METADATA_FILE: &str = "repo_metadata.json";

/// The newest repository format this release reads and writes.
pub const REPO_FORMAT_VERSION: u32 = 1;

/// Repository features this release understands. Every name ever released
/// stays reserved, even after support for it ends.
const KNOWN_REPOSITORY_FEATURES: &[&str] = &[];

/// A JSON document Gutch stores, identified by fixed type and version markers.
pub trait VersionedDocument: DeserializeOwned {
    const DOCUMENT_TYPE: &'static str;
    /// The only version this release reads and writes.
    const DOCUMENT_VERSION: &'static str;
}

/// Parse a versioned document: check its markers, then parse it exactly.
/// `source` names the document in errors, such as its file name.
pub(crate) fn parse_document<T: VersionedDocument>(source: &str, bytes: &[u8]) -> Result<T> {
    check_document_markers::<T>(source, bytes)?;
    Ok(serde_json::from_slice(bytes)?)
}

/// Check only the markers, ignoring every other field.
pub(crate) fn check_document_markers<T: VersionedDocument>(
    source: &str,
    bytes: &[u8],
) -> Result<()> {
    #[derive(Deserialize)]
    struct Markers {
        document_type: Option<String>,
        document_version: Option<String>,
    }
    let markers: Markers = serde_json::from_slice(bytes)?;
    if markers.document_type.as_deref() != Some(T::DOCUMENT_TYPE) {
        return Err(Error::manifest(format!(
            "{source} is not a {} document (document_type is {})",
            T::DOCUMENT_TYPE,
            describe(markers.document_type.as_deref()),
        )));
    }
    match markers.document_version {
        Some(found) if found == T::DOCUMENT_VERSION => Ok(()),
        found => Err(Error::UnsupportedFormat {
            subject: source.to_owned(),
            needs_newer_gutch: found
                .as_deref()
                .is_some_and(|found| is_newer_version(found, T::DOCUMENT_VERSION)),
            found: format!("document_version {}", describe(found.as_deref())),
            supported: format!("document_version {}", T::DOCUMENT_VERSION),
        }),
    }
}

fn describe(value: Option<&str>) -> String {
    value.map_or_else(|| "missing".to_owned(), |value| format!("'{value}'"))
}

/// True when `found` is a well-formed `major.minor.patch` above `supported`.
fn is_newer_version(found: &str, supported: &str) -> bool {
    fn parse(version: &str) -> Option<[u64; 3]> {
        let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
        let parsed = [parts.next()??, parts.next()??, parts.next()??];
        parts.next().is_none().then_some(parsed)
    }
    matches!((parse(found), parse(supported)), (Some(found), Some(supported)) if found > supported)
}

/// The fixed `document_type` of document `T`. It serializes as that string and
/// deserializes only from it.
pub struct DocumentType<T>(PhantomData<fn() -> T>);

/// The fixed `document_version` of document `T`. It serializes as that string
/// and deserializes only from it.
pub struct DocumentVersion<T>(PhantomData<fn() -> T>);

macro_rules! fixed_marker {
    ($marker:ident, $constant:ident, $field:literal) => {
        impl<T: VersionedDocument> $marker<T> {
            pub const VALUE: &'static str = T::$constant;
        }
        impl<T> Default for $marker<T> {
            fn default() -> Self {
                Self(PhantomData)
            }
        }
        impl<T> Clone for $marker<T> {
            fn clone(&self) -> Self {
                *self
            }
        }
        impl<T> Copy for $marker<T> {}
        impl<T> PartialEq for $marker<T> {
            fn eq(&self, _: &Self) -> bool {
                true
            }
        }
        impl<T> Eq for $marker<T> {}
        impl<T: VersionedDocument> fmt::Debug for $marker<T> {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(T::$constant)
            }
        }
        impl<T: VersionedDocument> Serialize for $marker<T> {
            fn serialize<S: serde::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(T::$constant)
            }
        }
        impl<'de, T: VersionedDocument> Deserialize<'de> for $marker<T> {
            fn deserialize<D: serde::Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                if value != T::$constant {
                    return Err(serde::de::Error::custom(format!(
                        concat!("unsupported ", $field, " '{}'; expected '{}'"),
                        value,
                        T::$constant
                    )));
                }
                Ok(Self::default())
            }
        }
        #[cfg(test)]
        impl<T: VersionedDocument> schemars::JsonSchema for $marker<T> {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                format!(concat!(stringify!($marker), "_{}"), T::$constant).into()
            }
            fn inline_schema() -> bool {
                true
            }
            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!({"type": "string", "const": T::$constant})
            }
        }
    };
}

fixed_marker!(DocumentType, DOCUMENT_TYPE, "document_type");
fixed_marker!(DocumentVersion, DOCUMENT_VERSION, "document_version");

/// What a repository needs from the Gutch release that opens it.
///
/// A reader must understand every feature in `features_required_to_read`. A
/// writer must also understand `features_required_to_write`. Any release may
/// ignore `optional_features`. Features describe repository capabilities, such
/// as new files or binary groups; a changed document field changes that
/// document's version instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RepoFormat {
    pub version: u32,
    pub features_required_to_read: Vec<String>,
    pub features_required_to_write: Vec<String>,
    pub optional_features: Vec<String>,
}

impl Default for RepoFormat {
    /// The format new repositories get.
    fn default() -> Self {
        Self {
            version: REPO_FORMAT_VERSION,
            features_required_to_read: Vec::new(),
            features_required_to_write: Vec::new(),
            optional_features: Vec::new(),
        }
    }
}

/// Whether an operation only reads a repository or also changes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepositoryAccess {
    Read,
    Write,
}

impl RepoFormat {
    /// Read only the repository format from `repo_metadata.json` bytes,
    /// ignoring every other field, so a newer repository is refused before
    /// the exact parse.
    pub(crate) fn from_repo_metadata(bytes: &[u8]) -> Result<Self> {
        #[derive(Deserialize)]
        struct Probe {
            repository_format: Option<LenientFormat>,
        }
        #[derive(Deserialize)]
        struct LenientFormat {
            version: u32,
            #[serde(default)]
            features_required_to_read: Vec<String>,
            #[serde(default)]
            features_required_to_write: Vec<String>,
            #[serde(default)]
            optional_features: Vec<String>,
        }
        let format = serde_json::from_slice::<Probe>(bytes)?
            .repository_format
            .ok_or_else(|| Error::manifest("repo_metadata.json has no repository_format"))?;
        Ok(Self {
            version: format.version,
            features_required_to_read: format.features_required_to_read,
            features_required_to_write: format.features_required_to_write,
            optional_features: format.optional_features,
        })
    }

    /// Refuse a repository this release cannot open for `access`.
    pub(crate) fn check(&self, access: RepositoryAccess) -> Result<()> {
        let unsupported = |found: String, needs_newer_gutch: bool| Error::UnsupportedFormat {
            subject: "repository".to_owned(),
            found,
            supported: supported_formats(),
            needs_newer_gutch,
        };
        if self.version != REPO_FORMAT_VERSION {
            return Err(unsupported(
                format!("format {}", self.version),
                self.version > REPO_FORMAT_VERSION,
            ));
        }
        let required = match access {
            RepositoryAccess::Read => &[&self.features_required_to_read][..],
            RepositoryAccess::Write => &[
                &self.features_required_to_read,
                &self.features_required_to_write,
            ][..],
        };
        let unknown: Vec<&str> = required
            .iter()
            .flat_map(|features| features.iter())
            .map(String::as_str)
            .filter(|feature| !KNOWN_REPOSITORY_FEATURES.contains(feature))
            .collect();
        if !unknown.is_empty() {
            let purpose = match access {
                RepositoryAccess::Read => "reading",
                RepositoryAccess::Write => "writing",
            };
            // An unknown feature is newer than this release by construction.
            return Err(unsupported(
                format!("features {} for {purpose}", unknown.join(", ")),
                true,
            ));
        }
        Ok(())
    }

    /// Reject malformed feature lists in a document this release writes.
    pub(crate) fn validate(&self) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        for feature in self
            .features_required_to_read
            .iter()
            .chain(&self.features_required_to_write)
            .chain(&self.optional_features)
        {
            let valid = !feature.is_empty()
                && feature
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
            if !valid || !seen.insert(feature.as_str()) {
                return Err(Error::manifest(format!(
                    "repository feature '{feature}' is malformed or listed twice"
                )));
            }
        }
        Ok(())
    }
}

fn supported_formats() -> String {
    if KNOWN_REPOSITORY_FEATURES.is_empty() {
        format!("format {REPO_FORMAT_VERSION} without features")
    } else {
        format!(
            "format {REPO_FORMAT_VERSION} with features {}",
            KNOWN_REPOSITORY_FEATURES.join(", ")
        )
    }
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
