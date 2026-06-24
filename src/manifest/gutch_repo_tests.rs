use super::*;

#[test]
fn test_gutch_repo_manifest_new() {
    let repo = GutchRepoManifest::new("base".into());
    assert_eq!(repo.format, CONTENT_REPO_FORMAT);
    assert_eq!(repo.format_version, "1.0.0");
    assert_eq!(repo.name, "base");
}
