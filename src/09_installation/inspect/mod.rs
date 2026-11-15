//! File and tree inspection; metadata checks remain distinct from content verification.

mod classify;
mod content;
mod inventory;
mod non_files;
mod types;

pub(crate) use classify::{
    InspectionSpans, VerifyRequest, classify_directory, classify_expected_entries,
};
pub use types::{RepairClassification, RepairIssue, RepairIssueKind, VerifyMode, VerifyTimings};
