//! Verification modes, timings and the issues a destination check reports.

use std::time::Duration;

use crate::runtime::RuntimePlan;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InstallationVerifyMode {
    #[default]
    Fast,
    Strict,
    /// Inspect entry metadata only; this does not establish content integrity.
    MetadataOnly,
}

impl InstallationVerifyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallationVerifyMode::Fast => "fast",
            InstallationVerifyMode::Strict => "strict",
            InstallationVerifyMode::MetadataOnly => "metadata-only",
        }
    }

    pub(crate) fn checks_chunk_hashes(self) -> bool {
        matches!(self, InstallationVerifyMode::Strict)
    }
}

#[derive(Debug, Clone, Default)]
pub struct VerifyTimings {
    pub(crate) verify_mode: InstallationVerifyMode,
    pub(crate) runtime_plan: Option<RuntimePlan>,
    pub(crate) prepare_wall: Duration,
    pub(crate) verify_files_wall: Duration,
    pub(crate) extra_file_scan_wall: Duration,
    pub(crate) stat: Duration,
    pub(crate) file_read: Duration,
    pub(crate) file_read_bytes: u64,
    pub(crate) file_hash: Duration,
    pub(crate) file_hash_bytes: u64,
    pub(crate) chunk_hash: Duration,
    pub(crate) chunk_hash_bytes: u64,
    pub(crate) total: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallationIssueKind {
    Missing,
    Corrupt,
    Extra,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationIssue {
    pub kind: InstallationIssueKind,
    pub path: String,
    pub detail: String,
}

impl InstallationIssue {
    pub(crate) fn missing(path: &str, detail: String) -> Self {
        Self::new(InstallationIssueKind::Missing, path, detail)
    }

    pub(crate) fn corrupt(path: &str, detail: String) -> Self {
        Self::new(InstallationIssueKind::Corrupt, path, detail)
    }

    pub(crate) fn extra(path: &str, detail: String) -> Self {
        Self::new(InstallationIssueKind::Extra, path, detail)
    }

    fn new(kind: InstallationIssueKind, path: &str, detail: String) -> Self {
        Self {
            kind,
            path: path.to_string(),
            detail,
        }
    }
}

/// What verification found, by kind. Extra files are reported but do not
/// stop an installation from matching its snapshot.
#[derive(Debug, Clone, Default)]
pub struct InstallationIssues {
    pub missing_files: Vec<InstallationIssue>,
    pub corrupt_files: Vec<InstallationIssue>,
    pub extra_files: Vec<InstallationIssue>,
}

/// Prints the counts, such as `0 missing, 1 corrupt, 0 extra`.
impl std::fmt::Display for InstallationIssues {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} missing, {} corrupt, {} extra",
            self.missing_file_count(),
            self.corrupt_file_count(),
            self.extra_file_count()
        )
    }
}

impl InstallationIssues {
    /// True when no snapshot file is missing or corrupt; extra files are allowed.
    pub fn matches_snapshot(&self) -> bool {
        self.missing_files.is_empty() && self.corrupt_files.is_empty()
    }

    pub fn missing_file_count(&self) -> usize {
        self.missing_files.len()
    }

    pub fn corrupt_file_count(&self) -> usize {
        self.corrupt_files.len()
    }

    pub fn extra_file_count(&self) -> usize {
        self.extra_files.len()
    }

    pub(crate) fn push(&mut self, issue: InstallationIssue) {
        match issue.kind {
            InstallationIssueKind::Missing => self.missing_files.push(issue),
            InstallationIssueKind::Corrupt => self.corrupt_files.push(issue),
            InstallationIssueKind::Extra => self.extra_files.push(issue),
        }
    }

    pub fn into_messages(self) -> Vec<String> {
        self.missing_files
            .into_iter()
            .chain(self.corrupt_files)
            .chain(self.extra_files)
            .map(|issue| issue.detail)
            .collect()
    }
}
