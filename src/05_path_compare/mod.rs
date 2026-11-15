//! Compares two physical directory trees by kind, content and link target.

mod compare;

pub use compare::{PathComparisonResult, compare_paths};
