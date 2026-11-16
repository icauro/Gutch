//! Provider-neutral remote repo planning and synchronization.
//!
//! A caller supplies a remote repo provider for control-plane lookup and
//! artifact transfer. Gutch owns the comparison rules, local repo
//! mutations, progress, cancellation, and synchronization order.

mod artifact_decision;
mod link;
mod local_hashes;
mod progress;
mod provider;
mod pull;
mod push;

pub use link::RemoteRepoBindRequest;
pub use link::RemoteRepoBindResult;
pub use link::RemoteRepoCreateRequest;
pub use link::RemoteRepoCreateResult;
pub use link::RemoteRepoUnbindRequest;
pub use link::RemoteRepoUnbindResult;
pub use link::bind_remote_repo;
pub use link::create_remote_repo;
pub use link::remote_tag_references;
pub use link::unbind_remote_repo;
pub use provider::PullConflictPolicy;
pub use provider::RemoteArtifactSource;
pub use provider::RemoteArtifactTarget;
pub use provider::RemoteBundleSource;
pub use provider::RemoteRepo;
pub use provider::RemoteRepoProvider;
pub use provider::RemoteRepoReader;
pub use provider::RemoteRepoService;
pub use provider::RemoteRepoSummary;
pub use provider::RemoteRepoWriter;
pub use provider::RemoteSnapshot;
pub use provider::RemoteSnapshotSources;
pub use provider::RemoteTagReferencesResult;
pub use provider::RepoPullPlan;
pub use provider::RepoPullResult;
pub use provider::RepoPushPlan;
pub use provider::RepoPushResult;
pub use provider::RepoPushSnapshotChange;
pub use pull::RepoPullRequest;
pub use pull::plan_pull_repo;
pub use pull::pull_repo;
pub use push::RepoPushRequest;
pub use push::plan_push_repo;
pub use push::push_repo;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

#[cfg(test)]
mod roundtrip_tests;
