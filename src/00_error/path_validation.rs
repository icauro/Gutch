//! Typed filename-compatibility failure carried by [`super::Error`].
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PathViolation {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflicting_path: Option<String>,
    pub target: String,
    pub code: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PathValidationFailure {
    pub operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    pub violations: Vec<PathViolation>,
}

impl Display for PathValidationFailure {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{} rejected: {} filename compatibility violation(s).",
            self.operation,
            self.violations.len()
        )?;
        if let Some(id) = &self.snapshot_id {
            writeln!(f, "Snapshot: {id}")?;
        }
        for violation in self.violations.iter().take(10) {
            // Debug escaping keeps control/bidi characters from altering terminal output.
            writeln!(
                f,
                "{:?}: {}: {}",
                violation.path, violation.target, violation.reason
            )?;
            if let Some(other) = &violation.conflicting_path {
                writeln!(f, "  Conflicts with {other:?}")?;
            }
        }
        if self.violations.len() > 10 {
            writeln!(
                f,
                "{} more; use --error-json-out for full details.",
                self.violations.len() - 10
            )?;
        }
        write!(
            f,
            "Rename incompatible source files, change repository constraints, or select a compatible destination."
        )
    }
}
