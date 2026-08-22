mod direct;
mod orchestrator;
mod staging;

// Re-export everything reconstruct/mod.rs needs.
pub(super) use orchestrator::{
    prepare_reconstruct, read_loose_repo_chunks, rebuild_files_from_chunks,
    reconstruct_from_bundle_paths_and_local_chunks, verify_chunk_hash,
};

// Re-export test-only functions.
#[cfg(test)]
pub(super) use direct::should_use_direct_bundle_to_file_reconstruct;
#[cfg(test)]
pub(super) use direct::should_use_direct_bundle_to_file_reconstruct_experimental;
#[cfg(test)]
pub(super) use staging::should_stage_bundle_payloads_in_memory;
