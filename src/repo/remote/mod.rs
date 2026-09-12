//! Provider-neutral remote repo planning and synchronization.
//!
//! A caller supplies a remote repo provider for control-plane lookup and
//! artifact transfer. Gutch owns the comparison rules, local repo
//! mutations, progress, cancellation, and synchronization order.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use crate::Error;
use crate::Result;
use crate::manifest::SnapshotTagSet;
use crate::progress::GutchOperation;
use crate::progress::GutchOperationControl;
use crate::progress::GutchProgressEvent;
use crate::progress::GutchProgressPhase;
use crate::progress::GutchProgressSink;
use crate::repo::RepoCheckStatus;
use crate::repo::index::load_chunk_index;
use crate::repo::index::required_bundle_ids;
use crate::repo::paths::GutchRepoPaths;
use crate::repo::paths::load_gutch_repo_manifest;
use crate::repo::paths::write_gutch_repo_manifest;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use crate::repo::summarize_snapshot_tags;
use crate::repo::tag_references::advisory_tag_references;

pub(crate) const MAX_REMOTE_JSON_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_REMOTE_BINARY_BYTES: u64 = 256 * 1024 * 1024;

mod link;
mod provider;
mod pull;
mod push;
pub use link::bind_repo_remote;
use link::bound_remote_repo_id;
pub use link::inspect_repo_tag_references;
pub use link::link_repo;
pub(crate) use link::linked_remote_repo_id;
pub use link::list_remote_repositories;
pub use link::unlink_repo;
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
use pull::local_bundle_paths;
pub use pull::plan_pull_with_provider;
pub use pull::pull_repo;
pub use push::plan_push_with_provider;
pub use push::push_repo;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtifactDecision {
    Identical,
    Fetch,
    KeepLocalNewer,
}

#[derive(Debug)]
struct LocalSyncSnapshot {
    snapshot_id: String,
    version: String,
    os: String,
    arch: String,
    tags: Vec<String>,
    snapshot_metadata_hash_b3: String,
    snapshot_product_metadata_hash_b3: String,
    snapshot_blueprint_hash_b3: String,
}

fn repo_file_hash(bytes: &[u8]) -> String {
    crate::artifact::hash::hash_hex(&blake3::hash(bytes))
}

fn verify_remote_artifact(bytes: &[u8], expected_hash: &str, label: &str) -> Result<()> {
    let expected_hash = expected_hash.trim();
    if expected_hash.is_empty() {
        return Ok(());
    }
    let actual_hash = repo_file_hash(bytes);
    if actual_hash != expected_hash {
        return Err(Error::HashMismatch {
            expected: format!("{label} {expected_hash}"),
            actual: format!("{label} {actual_hash}"),
        });
    }
    Ok(())
}

fn verify_remote_bundle(bundle_id: &str, bytes: &[u8]) -> Result<()> {
    crate::artifact::bundle::verify_bundle_identity_bytes(bundle_id, bytes)
}

fn hashes_differ(local_hash: &str, remote_hash: &str) -> bool {
    let local_hash = local_hash.trim();
    let remote_hash = remote_hash.trim();
    if local_hash.is_empty() && remote_hash.is_empty() {
        return false;
    }
    remote_hash.is_empty() || local_hash != remote_hash
}

fn local_file_mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn remote_updated_at_is_newer_or_equal(
    remote_updated_at: &str,
    local_modified: Option<std::time::SystemTime>,
) -> bool {
    let Ok(remote) = chrono::DateTime::parse_from_rfc3339(remote_updated_at) else {
        return true;
    };
    let remote_utc = remote.with_timezone(&chrono::Utc);
    let Some(local_modified) = local_modified else {
        return true;
    };
    std::time::SystemTime::from(remote_utc) >= local_modified
}

fn decide_artifact(
    local_exists: bool,
    local_hash: String,
    remote_hash: &str,
    remote_updated_at: &str,
    local_modified: Option<std::time::SystemTime>,
    policy: PullConflictPolicy,
) -> ArtifactDecision {
    if !local_exists || hashes_differ(&local_hash, remote_hash) {
        match policy {
            PullConflictPolicy::KeepLocal => {
                if local_exists {
                    ArtifactDecision::KeepLocalNewer
                } else {
                    ArtifactDecision::Fetch
                }
            }
            PullConflictPolicy::KeepRemote => ArtifactDecision::Fetch,
            PullConflictPolicy::KeepNewer => {
                if remote_updated_at_is_newer_or_equal(remote_updated_at, local_modified) {
                    ArtifactDecision::Fetch
                } else {
                    ArtifactDecision::KeepLocalNewer
                }
            }
        }
    } else {
        ArtifactDecision::Identical
    }
}

fn local_repo_metadata_hash_b3(repo_paths: &GutchRepoPaths) -> Result<String> {
    if !repo_paths.repo_metadata_path.exists() {
        return Ok(String::new());
    }
    Ok(repo_file_hash(&crate::filesystem::read(
        &repo_paths.repo_metadata_path,
    )?))
}

fn local_chunk_map_hash_b3(repo_paths: &GutchRepoPaths) -> Result<String> {
    if !repo_paths.chunk_map_path.exists() {
        return Ok(String::new());
    }
    Ok(repo_file_hash(&crate::filesystem::read(
        &repo_paths.chunk_map_path,
    )?))
}

fn local_snapshot_metadata_hash_b3(
    repo_paths: &GutchRepoPaths,
    snapshot_id: &str,
) -> Result<String> {
    let metadata_path =
        crate::repo::snapshot_manifests::snapshot_metadata_path(repo_paths, snapshot_id)?;
    let blueprint_path =
        crate::repo::snapshot_manifests::snapshot_blueprint_path(repo_paths, snapshot_id)?;
    if !metadata_path.exists() || !blueprint_path.exists() {
        return Ok(String::new());
    }
    let manifest = load_snapshot_manifest(repo_paths, snapshot_id)?;
    Ok(repo_file_hash(
        &crate::repo::snapshot_manifests::serialize_snapshot_metadata_for_remote(&manifest)?,
    ))
}

fn local_snapshot_product_metadata_hash_b3(
    repo_paths: &GutchRepoPaths,
    snapshot_id: &str,
) -> Result<String> {
    let metadata_path =
        crate::repo::snapshot_manifests::snapshot_metadata_path(repo_paths, snapshot_id)?;
    let blueprint_path =
        crate::repo::snapshot_manifests::snapshot_blueprint_path(repo_paths, snapshot_id)?;
    if !metadata_path.exists() || !blueprint_path.exists() {
        return Ok(String::new());
    }
    let manifest = load_snapshot_manifest(repo_paths, snapshot_id)?;
    let Some(bytes) =
        crate::repo::snapshot_manifests::serialize_snapshot_product_metadata_for_remote(&manifest)?
    else {
        return Ok(String::new());
    };
    Ok(repo_file_hash(&bytes))
}

fn local_snapshot_blueprint_hash_b3(
    repo_paths: &GutchRepoPaths,
    snapshot_id: &str,
) -> Result<String> {
    let path = crate::repo::snapshot_manifests::snapshot_blueprint_path(repo_paths, snapshot_id)?;
    if !path.exists() {
        return Ok(String::new());
    }
    Ok(crate::artifact::hash::hash_hex(
        &crate::artifact::hash::hash_file(&path)?,
    ))
}

struct RemoteProgress {
    sink: Option<std::sync::Arc<dyn GutchProgressSink>>,
    operation: GutchOperation,
}

impl RemoteProgress {
    fn new(control: &GutchOperationControl, operation: GutchOperation) -> Self {
        Self {
            sink: control.progress().clone(),
            operation,
        }
    }

    fn emit(
        &self,
        phase: GutchProgressPhase,
        message: impl Into<String>,
        current: usize,
        total: usize,
        detail: Option<String>,
        done: bool,
    ) {
        crate::progress::emit_progress(
            &self.sink,
            GutchProgressEvent {
                feedback: None,
                operation: self.operation,
                phase,
                message: message.into(),
                current,
                total,
                transferred_bytes: 0,
                total_bytes: 0,
                detail,
                done,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::pull::{bundle_source_requests, compute_pull_plan, record_bundle_sources};
    use super::push::summarize_snapshot_tags_from_values;
    use super::*;

    struct EmptyReader;

    impl RemoteRepositoryReader for EmptyReader {
        fn resolve_repository(
            &self,
            _remote_spec: Option<&str>,
            _bound_remote_repo_id: Option<&str>,
        ) -> Result<RemoteRepository> {
            Ok(RemoteRepository {
                id: "remote-1".to_string(),
                scope_id: String::new(),
                name: "fixture".to_string(),
                metadata_hash_b3: String::new(),
                chunk_map_hash_b3: String::new(),
                updated_at: String::new(),
            })
        }

        fn list_snapshots(&self, _repository: &RemoteRepository) -> Result<Vec<RemoteSnapshot>> {
            Ok(Vec::new())
        }

        fn repository_metadata_source(
            &self,
            _repository: &RemoteRepository,
        ) -> Result<RemoteArtifactSource> {
            Ok(RemoteArtifactSource::Bytes(Vec::new()))
        }

        fn chunk_index_source(
            &self,
            _repository: &RemoteRepository,
        ) -> Result<RemoteArtifactSource> {
            Ok(RemoteArtifactSource::Bytes(Vec::new()))
        }

        fn snapshot_sources(
            &self,
            _repository: &RemoteRepository,
            _snapshot: &RemoteSnapshot,
        ) -> Result<RemoteSnapshotSources> {
            Ok(RemoteSnapshotSources {
                metadata: RemoteArtifactSource::Bytes(Vec::new()),
                blueprint: RemoteArtifactSource::Bytes(Vec::new()),
                product_metadata: None,
            })
        }

        fn bundle_sources(
            &self,
            _repository: &RemoteRepository,
            _snapshot: &RemoteSnapshot,
            _bundle_ids: &[String],
        ) -> Result<Vec<RemoteBundleSource>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn conflict_policy_parser_is_provider_neutral() {
        assert_eq!(
            PullConflictPolicy::parse("keep-local").unwrap(),
            PullConflictPolicy::KeepLocal
        );
        assert_eq!(
            PullConflictPolicy::parse("keep-remote").unwrap(),
            PullConflictPolicy::KeepRemote
        );
        assert_eq!(
            PullConflictPolicy::parse("keep-newer").unwrap(),
            PullConflictPolicy::KeepNewer
        );
        assert!(PullConflictPolicy::parse("overwrite").is_err());
    }

    #[test]
    fn tag_values_are_summarized_without_provider_types() {
        assert_eq!(
            summarize_snapshot_tags_from_values(&[
                "version::1.2.3".into(),
                "os::linux".into(),
                "-loader-steam".into()
            ]),
            ("1.2.3".into(), "linux".into(), "".into())
        );
    }

    #[test]
    fn remote_artifact_hashes_are_verified_before_persistence() {
        let bytes = b"remote artifact";
        let expected = repo_file_hash(bytes);
        assert!(verify_remote_artifact(bytes, &expected, "metadata").is_ok());
        assert!(verify_remote_artifact(b"tampered", &expected, "metadata").is_err());
    }

    #[test]
    fn remote_bundle_content_hash_is_verified() {
        let payload = b"bundle payload";
        let bundle_hash = repo_file_hash(payload);
        let mut bundle_bytes = payload.to_vec();
        bundle_bytes.extend_from_slice(blake3::hash(payload).as_bytes());

        assert!(verify_remote_bundle(&bundle_hash, &bundle_bytes).is_ok());
        bundle_bytes[0] ^= 1;
        assert!(verify_remote_bundle(&bundle_hash, &bundle_bytes).is_err());
    }

    #[test]
    fn remote_bundle_identity_matches_local_verification() {
        // Local strict verification and remote download share
        // verify_bundle_identity_bytes, so both reject the same failures.
        let payload = b"bundle payload";
        let bundle_hash = repo_file_hash(payload);
        let mut bundle_bytes = payload.to_vec();
        bundle_bytes.extend_from_slice(blake3::hash(payload).as_bytes());

        assert!(verify_remote_bundle(&bundle_hash, &bundle_bytes).is_ok());
        assert!(
            crate::artifact::bundle::verify_bundle_identity_bytes(&bundle_hash, &bundle_bytes)
                .is_ok()
        );

        // Corrupt trailing ID: content hash still matches, trailing does not.
        let mut trailing = bundle_bytes.clone();
        let last = trailing.len() - 1;
        trailing[last] = if trailing[last] == b'0' { b'1' } else { b'0' };
        assert!(verify_remote_bundle(&bundle_hash, &trailing).is_err());
        assert!(
            crate::artifact::bundle::verify_bundle_identity_bytes(&bundle_hash, &trailing).is_err()
        );

        // Truncated body without a trailing ID.
        assert!(verify_remote_bundle(&bundle_hash, b"short").is_err());
        assert!(
            crate::artifact::bundle::verify_bundle_identity_bytes(&bundle_hash, b"short").is_err()
        );
    }

    #[test]
    fn empty_remote_hashes_do_not_force_a_change() {
        assert!(!hashes_differ("", ""));
        assert!(hashes_differ("abc", ""));
        assert!(hashes_differ("", "abc"));
        assert!(!hashes_differ("abc", "abc"));
    }

    fn mtime_ago(seconds: u64) -> std::time::SystemTime {
        std::time::SystemTime::now() - std::time::Duration::from_secs(seconds)
    }

    #[test]
    fn identical_hashes_are_never_fetched() {
        for policy in [
            PullConflictPolicy::KeepLocal,
            PullConflictPolicy::KeepRemote,
            PullConflictPolicy::KeepNewer,
        ] {
            let decision = decide_artifact(
                true,
                "same".to_string(),
                "same",
                "2026-08-08T10:00:00Z",
                Some(mtime_ago(100)),
                policy,
            );
            assert_eq!(decision, ArtifactDecision::Identical);
        }
    }

    #[test]
    fn keep_local_preserves_existing_content_but_fetches_missing_content() {
        let existing = decide_artifact(
            true,
            "local".to_string(),
            "remote",
            "2026-08-08T10:00:00Z",
            Some(mtime_ago(100)),
            PullConflictPolicy::KeepLocal,
        );
        assert_eq!(existing, ArtifactDecision::KeepLocalNewer);

        let missing = decide_artifact(
            false,
            String::new(),
            "remote",
            "2026-08-08T10:00:00Z",
            None,
            PullConflictPolicy::KeepLocal,
        );
        assert_eq!(missing, ArtifactDecision::Fetch);
    }

    #[test]
    fn pull_keeps_a_healthy_equivalent_layout_but_replaces_missing_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("file"), vec![37; 512 * 1024]).unwrap();
        let capture = |name: &str, chunk_profile| {
            let repo = root.path().join(name);
            let paths = GutchRepoPaths::from_repo_root(&repo);
            crate::repo::paths::ensure_gutch_repo_layout(&paths, name).unwrap();
            let captured = crate::capture::execute::capture_repo_snapshot(
                crate::capture::execute::RepoSnapshotCaptureRequest {
                    repo_root: repo.clone(),
                    source_dir: source.clone(),
                    explicit_tags: Default::default(),
                    chunk_profile,
                    bundle_profile: Default::default(),
                    compression_algorithm: crate::manifest::CompressionKind::Zstd,
                    compression_level: None,
                    storage_mode: crate::manifest::ArtifactStorageMode::Bundled,
                    runtime_hints: crate::runtime::RuntimeResourceHints { jobs: Some(1) },
                    control: Default::default(),
                },
            )
            .unwrap();
            (repo, paths, captured.snapshot_id)
        };
        let (local, paths, id) =
            capture("local", crate::manifest::ArtifactSizingPreset::Chunk16KiB);
        let (_, remote_paths, remote_id) =
            capture("remote", crate::manifest::ArtifactSizingPreset::Chunk1MiB);
        assert_eq!(id, remote_id);
        let remote = RemoteRepository {
            id: "remote".into(),
            scope_id: "scope".into(),
            name: "remote".into(),
            metadata_hash_b3: String::new(),
            chunk_map_hash_b3: String::new(),
            updated_at: String::new(),
        };
        let snapshot = RemoteSnapshot {
            remote_id: "remote-snapshot".into(),
            snapshot_id: id.clone(),
            metadata_hash_b3: String::new(),
            product_metadata_hash_b3: String::new(),
            blueprint_hash_b3: local_snapshot_blueprint_hash_b3(&remote_paths, &id).unwrap(),
            uploaded_at: String::new(),
            created_at: String::new(),
            tags: vec![],
            artifact_state: None,
            artifact_ready: true,
        };
        assert_ne!(
            local_snapshot_blueprint_hash_b3(&paths, &id).unwrap(),
            snapshot.blueprint_hash_b3
        );
        let plan = compute_pull_plan(
            &local,
            &paths,
            &remote,
            &[snapshot.clone()],
            PullConflictPolicy::KeepRemote,
        )
        .unwrap();
        assert_eq!(plan.snapshots_to_skip, vec![id.clone()]);
        assert!(plan.snapshots_to_download.is_empty());
        for path in local_bundle_paths(&paths.bundles_root).unwrap() {
            std::fs::remove_file(path).unwrap();
        }
        let plan = compute_pull_plan(
            &local,
            &paths,
            &remote,
            &[snapshot],
            PullConflictPolicy::KeepRemote,
        )
        .unwrap();
        assert_eq!(plan.snapshots_to_download, vec![id]);
    }

    #[test]
    fn pull_plan_preserves_local_repository_metadata_and_index() {
        let root = tempfile::tempdir().unwrap();
        let paths = GutchRepoPaths::from_repo_root(root.path());
        crate::repo::paths::ensure_gutch_repo_layout(&paths, "local").unwrap();
        crate::repo::index::write_chunk_index(
            &paths,
            &crate::repo::index::load_chunk_index(&paths).unwrap(),
        )
        .unwrap();
        let remote = RemoteRepository {
            id: "remote".into(),
            scope_id: "scope".into(),
            name: "remote".into(),
            metadata_hash_b3: "a".repeat(64),
            chunk_map_hash_b3: "b".repeat(64),
            updated_at: "2000-01-01T00:00:00Z".into(),
        };
        for (policy, download) in [
            (PullConflictPolicy::KeepLocal, false),
            (PullConflictPolicy::KeepNewer, false),
            (PullConflictPolicy::KeepRemote, true),
        ] {
            let plan = compute_pull_plan(root.path(), &paths, &remote, &[], policy).unwrap();
            assert_eq!(plan.repo_manifest_needed, download);
            assert_eq!(plan.chunk_map_needed, download);
        }
    }

    #[test]
    fn keep_remote_fetches_even_when_local_is_newer() {
        let decision = decide_artifact(
            true,
            "local".to_string(),
            "remote",
            "2026-08-08T10:00:00Z",
            Some(mtime_ago(100)),
            PullConflictPolicy::KeepRemote,
        );
        assert_eq!(decision, ArtifactDecision::Fetch);
    }

    #[test]
    fn keep_newer_uses_remote_updated_at() {
        let local_newer = decide_artifact(
            true,
            "local".to_string(),
            "remote",
            "2026-08-01T10:00:00Z",
            Some(mtime_ago(100)),
            PullConflictPolicy::KeepNewer,
        );
        assert_eq!(local_newer, ArtifactDecision::KeepLocalNewer);

        let remote_newer = decide_artifact(
            true,
            "local".to_string(),
            "remote",
            "2030-08-01T10:00:00Z",
            Some(mtime_ago(100)),
            PullConflictPolicy::KeepNewer,
        );
        assert_eq!(remote_newer, ArtifactDecision::Fetch);
    }

    #[test]
    fn unparseable_remote_timestamp_fetches() {
        let decision = decide_artifact(
            true,
            "local".to_string(),
            "remote",
            "not-a-timestamp",
            Some(mtime_ago(100)),
            PullConflictPolicy::KeepNewer,
        );
        assert_eq!(decision, ArtifactDecision::Fetch);
    }

    #[test]
    fn pull_planning_accepts_a_reader_without_host_or_http() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let plan = plan_pull_with_provider(
            directory.path(),
            &EmptyReader,
            None,
            PullConflictPolicy::KeepRemote,
        )
        .expect("pull plan");

        assert_eq!(plan.remote_repo_id, "remote-1");
        assert!(plan.repo_manifest_needed);
        assert!(plan.chunk_map_needed);
        assert!(plan.snapshots_to_download.is_empty());
    }

    #[test]
    fn binding_a_remote_repo_keeps_the_id_opaque() {
        let directory = tempfile::tempdir().expect("temporary directory");
        crate::repo::paths::ensure_gutch_repo_layout(
            &GutchRepoPaths::from_repo_root(directory.path()),
            "fixture",
        )
        .expect("repo layout");

        bind_repo_remote(directory.path(), "provider-specific-id").expect("bind repo");
        assert_eq!(
            linked_remote_repo_id(directory.path()).expect("linked id"),
            "provider-specific-id"
        );
    }

    #[test]
    fn binding_rejects_an_empty_remote_repo_id() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let error = bind_repo_remote(directory.path(), " ").expect_err("empty id must fail");
        assert!(error.to_string().contains("remote repo ID is required"));
    }

    #[test]
    fn bundle_source_validation_rejects_unknown_and_duplicate_ids() {
        let expected = HashSet::from(["bundle-a"]);
        let mut sources = HashMap::new();
        let error = record_bundle_sources(
            &mut sources,
            &expected,
            vec![RemoteBundleSource {
                bundle_id: "bundle-b".to_string(),
                source: RemoteArtifactSource::Bytes(vec![]),
            }],
        )
        .expect_err("unknown bundle should be rejected");
        assert!(error.to_string().contains("unexpected bundle"));

        record_bundle_sources(
            &mut sources,
            &expected,
            vec![RemoteBundleSource {
                bundle_id: "bundle-a".to_string(),
                source: RemoteArtifactSource::Bytes(vec![]),
            }],
        )
        .expect("first bundle should be accepted");
        let error = record_bundle_sources(
            &mut sources,
            &expected,
            vec![RemoteBundleSource {
                bundle_id: "bundle-a".to_string(),
                source: RemoteArtifactSource::Bytes(vec![]),
            }],
        )
        .expect_err("duplicate bundle should be rejected");
        assert!(error.to_string().contains("duplicate bundle"));
    }

    #[test]
    fn bundle_source_requests_deduplicate_shared_bundles() {
        let requests = bundle_source_requests(
            &[
                ("snapshot-a".to_string(), vec!["shared".to_string()]),
                (
                    "snapshot-b".to_string(),
                    vec!["shared".to_string(), "unique".to_string()],
                ),
            ],
            &["shared".to_string(), "unique".to_string()],
        );
        assert_eq!(
            requests,
            vec![
                ("snapshot-a".to_string(), vec!["shared".to_string()]),
                ("snapshot-b".to_string(), vec!["unique".to_string()]),
            ]
        );
    }
}
