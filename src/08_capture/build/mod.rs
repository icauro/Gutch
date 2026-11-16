//! Build staged artifacts and retry once when the source inventory changes.

mod flush;
pub(crate) mod pipeline;
pub(crate) mod progress;
mod stable_source;
pub(crate) mod types;

pub(crate) use stable_source::{BuiltCapture, build_with_stable_source};
