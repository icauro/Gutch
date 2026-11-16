//! Runs a reconstruction plan: stages the tree, schedules artifact and source
//! jobs, verifies written files and publishes the result.

mod apply;
pub(crate) mod entries;
mod handles;
pub(crate) mod job_io;
pub(crate) mod jobs;
mod output_preparer;
mod pipeline;
pub(crate) mod publish;
mod workers;

pub(crate) use apply::apply_reconstruct_pipeline;
