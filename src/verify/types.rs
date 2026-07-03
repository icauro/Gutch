use std::time::Duration;

use crate::runtime::RuntimePlan;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VerifyMode {
    #[default]
    Fast,
    Strict,
}

impl VerifyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            VerifyMode::Fast => "fast",
            VerifyMode::Strict => "strict",
        }
    }

    pub(crate) fn checks_chunk_hashes(self) -> bool {
        matches!(self, VerifyMode::Strict)
    }
}

#[derive(Debug, Clone, Default)]
pub struct VerifyTimings {
    pub verify_mode: VerifyMode,
    pub runtime_plan: Option<RuntimePlan>,
    pub stat: Duration,
    pub file_read: Duration,
    pub file_read_bytes: u64,
    pub file_hash: Duration,
    pub file_hash_bytes: u64,
    pub chunk_hash: Duration,
    pub chunk_hash_bytes: u64,
    pub total: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairIssueKind {
    Missing,
    Corrupt,
    Extra,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairIssue {
    pub kind: RepairIssueKind,
    pub path: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct RepairClassification {
    pub missing_files: Vec<RepairIssue>,
    pub corrupt_files: Vec<RepairIssue>,
    pub extra_files: Vec<RepairIssue>,
}

impl RepairClassification {
    pub(crate) fn push(&mut self, issue: RepairIssue) {
        match issue.kind {
            RepairIssueKind::Missing => self.missing_files.push(issue),
            RepairIssueKind::Corrupt => self.corrupt_files.push(issue),
            RepairIssueKind::Extra => self.extra_files.push(issue),
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
