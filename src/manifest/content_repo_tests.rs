use super::*;

#[test]
fn test_content_repo_manifest_new() {
    let repo = ContentRepoManifest::new("base".into());
    assert_eq!(repo.format, CONTENT_REPO_FORMAT);
    assert_eq!(repo.format_version, "1.0.0");
    assert_eq!(repo.content_repo_id, "base");
}
