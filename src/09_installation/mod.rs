//! Installation inspection, planning, reconstruction, repair, and publication.
//!
//! Operations (`install`, `verify`, `repair`, `corrupt`, `fetch`, `preview`,
//! `state`) sit at this level; `plan` and `prepare` choose the work, and
//! `execute` stages, writes, verifies and publishes it.

pub(crate) mod contracts;
mod corrupt;
pub(crate) mod execute;
pub(crate) mod fetch;
pub(crate) mod inspect;
pub(crate) mod install;
pub(crate) mod plan;
pub(crate) mod prepare;
pub(crate) mod preview;
mod repair;
pub(crate) mod source;
pub(crate) mod staging;
pub(crate) mod state;
mod verify;

pub use contracts::InstallationCorruptPlan;
pub use contracts::InstallationCorruptRequest;
pub use contracts::InstallationReconstructOutcome;
pub use contracts::InstallationReconstructResult;
pub use contracts::InstallationRepairOutcome;
pub use contracts::InstallationRepairPlanRequest;
pub use contracts::InstallationRepairPlanResult;
pub use contracts::InstallationRepairRequest;
pub use contracts::InstallationRepairResult;
pub use contracts::InstallationRequest;
pub use contracts::InstallationVerifyRequest;
pub use contracts::InstallationVerifyResult;
pub use contracts::ReconstructTimings;
pub use contracts::ResolvedSnapshotOperationState;
pub use corrupt::corrupt_installation;
pub use install::reconstruct_installation;
pub use repair::plan_repair_installation;
pub use repair::repair_installation;
pub use state::resolve_snapshot_state;
pub use verify::verify_installation;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
