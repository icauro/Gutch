use super::link::linked_remote_repo_id;
use super::pull::local_bundle_paths;
use std::collections::{HashMap, HashSet};

use super::local_hashes::local_snapshot_blueprint_hash_b3;
use super::pull::{
    SnapshotBundleIds, bundle_source_requests, compute_pull_plan, record_bundle_sources,
};
use super::push::summarize_snapshot_tags_from_values;
use super::*;
use crate::Result;
use crate::repo::paths::RepoPaths;

struct EmptyReader;

impl RemoteRepoReader for EmptyReader {
    fn resolve_repo(
        &self,
        _remote_repo_id: Option<&str>,
        _bound_remote_repo_id: Option<&str>,
    ) -> Result<RemoteRepo> {
        Ok(RemoteRepo {
            id: "remote-1".to_string(),
            scope_id: String::new(),
            name: "fixture".to_string(),
            metadata_hash_b3: String::new(),
            chunk_map_hash_b3: String::new(),
            updated_at: String::new(),
        })
    }

    fn list_snapshots(&self, _repository: &RemoteRepo) -> Result<Vec<RemoteSnapshot>> {
        Ok(Vec::new())
    }

    fn repo_metadata_source(&self, _repository: &RemoteRepo) -> Result<RemoteArtifactSource> {
        Ok(RemoteArtifactSource::Bytes(Vec::new()))
    }

    fn chunk_index_source(&self, _repository: &RemoteRepo) -> Result<RemoteArtifactSource> {
        Ok(RemoteArtifactSource::Bytes(Vec::new()))
    }

    fn snapshot_sources(
        &self,
        _repository: &RemoteRepo,
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
        _repository: &RemoteRepo,
        _snapshot: &RemoteSnapshot,
        _bundle_ids: &[String],
    ) -> Result<Vec<RemoteBundleSource>> {
        Ok(Vec::new())
    }
}

#[test]
fn conflict_policy_values_match_the_cli_spelling() {
    use clap::ValueEnum;
    let parse = |value| PullConflictPolicy::from_str(value, false);
    assert_eq!(parse("keep-local"), Ok(PullConflictPolicy::KeepLocal));
    assert_eq!(parse("keep-remote"), Ok(PullConflictPolicy::KeepRemote));
    assert_eq!(parse("keep-newer"), Ok(PullConflictPolicy::KeepNewer));
    assert!(parse("overwrite").is_err());
}

#[test]
fn tag_values_are_summarized_without_provider_types() {
    assert_eq!(
        summarize_snapshot_tags_from_values(&[
            "version=1.2.3".into(),
            "os=linux".into(),
            "loader=steam".into()
        ]),
        ("1.2.3".into(), "linux".into(), "".into())
    );
}

#[test]
fn pull_keeps_a_healthy_equivalent_layout_but_replaces_missing_artifacts() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("file"), vec![37; 512 * 1024]).unwrap();
    let capture = |name: &str, chunk_profile| {
        let repo = root.path().join(name);
        let paths = RepoPaths::from_repo_root(&repo);
        crate::repo::paths::ensure_repo_layout(&paths, name).unwrap();
        let captured = crate::capture::sources::capture_snapshot(
            &crate::capture::contracts::SnapshotCaptureRequest {
                repo_root: repo.clone(),
                source: source.clone(),
                tags: Default::default(),
                chunk_profile,
                chunker: crate::artifact::profiles::Chunker::FastCdc,
                bundle_profile: Default::default(),
                compression_algorithm: crate::artifact::profiles::CompressionKind::Zstd,
                compression_level: None,
                storage_mode: crate::artifact::profiles::ArtifactStorageMode::Bundled,
                runtime_hints: crate::runtime::RuntimeResourceHints {
                    jobs: Some(1),
                    ..Default::default()
                },
                control: Default::default(),
            },
        )
        .unwrap();
        (repo, paths, captured.snapshot_id)
    };
    let (local, paths, id) = capture(
        "local",
        crate::artifact::profiles::ChunkSizingPreset::Chunk16KiB,
    );
    let (_, remote_paths, remote_id) = capture(
        "remote",
        crate::artifact::profiles::ChunkSizingPreset::Chunk1MiB,
    );
    assert_eq!(id, remote_id);
    let remote = RemoteRepo {
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
        std::slice::from_ref(&snapshot),
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
    let paths = RepoPaths::from_repo_root(root.path());
    crate::repo::paths::ensure_repo_layout(&paths, "local").unwrap();
    crate::repo::index::write_chunk_index(
        &paths,
        &crate::repo::index::load_chunk_index(&paths).unwrap(),
    )
    .unwrap();
    let remote = RemoteRepo {
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
fn pull_planning_accepts_a_reader_without_host_or_http() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let request = RepoPullRequest {
        repo_root: directory.path().to_path_buf(),
        on_conflict: PullConflictPolicy::KeepRemote,
        ..Default::default()
    };
    let plan = plan_pull_repo(&request, &EmptyReader).expect("pull plan");

    assert_eq!(plan.remote_repo_id, "remote-1");
    assert!(plan.repo_manifest_needed);
    // The reader reports no chunk index hash: the remote has none to fetch.
    assert!(!plan.chunk_map_needed);
    assert!(plan.snapshots_to_download.is_empty());
}

fn bind_request(repo_root: &std::path::Path, remote_repo_id: &str) -> RemoteRepoBindRequest {
    RemoteRepoBindRequest {
        repo_root: repo_root.to_path_buf(),
        remote_repo_id: remote_repo_id.to_owned(),
        ..Default::default()
    }
}

#[test]
fn binding_a_remote_repo_keeps_the_id_opaque() {
    let directory = tempfile::tempdir().expect("temporary directory");
    crate::repo::paths::ensure_repo_layout(&RepoPaths::from_repo_root(directory.path()), "fixture")
        .expect("repo layout");

    bind_remote_repo(&bind_request(directory.path(), "provider-specific-id")).expect("bind repo");
    assert_eq!(
        linked_remote_repo_id(directory.path()).expect("linked id"),
        "provider-specific-id"
    );
}

#[test]
fn binding_rejects_an_empty_remote_repo_id() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let error =
        bind_remote_repo(&bind_request(directory.path(), " ")).expect_err("empty id must fail");
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
            SnapshotBundleIds {
                snapshot_id: "snapshot-a".to_string(),
                bundle_ids: vec!["shared".to_string()],
            },
            SnapshotBundleIds {
                snapshot_id: "snapshot-b".to_string(),
                bundle_ids: vec!["shared".to_string(), "unique".to_string()],
            },
        ],
        &["shared".to_string(), "unique".to_string()],
    );
    assert_eq!(
        requests,
        vec![
            SnapshotBundleIds {
                snapshot_id: "snapshot-a".to_string(),
                bundle_ids: vec!["shared".to_string()]
            },
            SnapshotBundleIds {
                snapshot_id: "snapshot-b".to_string(),
                bundle_ids: vec!["unique".to_string()]
            },
        ]
    );
}
