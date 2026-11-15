//! Engine failures shared by operations and embedding hosts.

mod engine_error;
pub(crate) mod metadata_conflict;
pub(crate) mod path_validation;
pub(crate) mod promotion_diagnostics;
pub use engine_error::{Error, Result};
