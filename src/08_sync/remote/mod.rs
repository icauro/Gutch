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

pub use link::bind_remote_repo;
pub use link::create_remote_repo;
pub use link::inspect_repo_tag_references;
pub use link::list_remote_repositories;
pub use link::unbind_remote_repo;
pub use provider::PullConflictPolicy;
pub use provider::RemoteArtifactSource;
pub use provider::RemoteBundleSource;
pub use provider::RemoteRepository;
pub use provider::RemoteRepositoryCommandProvider;
pub use provider::RemoteRepositoryProvider;
pub use provider::RemoteRepositoryReader;
pub use provider::RemoteRepositorySummary;
pub use provider::RemoteRepositoryWriter;
pub use provider::RemoteSnapshot;
pub use provider::RemoteSnapshotSources;
pub use provider::RemoteTagReferencesResult;
pub use provider::RepoPullPlan;
pub use provider::RepoPullStats;
pub use provider::RepoPushPlan;
pub use provider::RepoPushSnapshotChange;
pub use provider::RepoPushStats;
pub use pull::plan_pull_with_provider;
pub use pull::pull_repo;
pub use push::plan_push_with_provider;
pub use push::push_repo;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

#[cfg(test)]
mod roundtrip_tests;
