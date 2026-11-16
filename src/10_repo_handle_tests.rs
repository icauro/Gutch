use super::Repo;

#[test]
fn handle_runs_the_common_operations() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let app = root.path().join("app");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("a.txt"), "one\n".repeat(100)).unwrap();

    let repo = Repo::init(root.path().join("repo")).unwrap();
    assert!(Repo::init(repo.root()).is_err());
    let old_id = repo
        .capture(&source, &[("version", "1")])
        .unwrap()
        .snapshot_id;
    std::fs::write(source.join("a.txt"), "two\n".repeat(100)).unwrap();
    let new_id = repo.capture(&source, &[]).unwrap().snapshot_id;
    assert_eq!(repo.diff(&old_id, &new_id).unwrap().changes.len(), 1);

    repo.reconstruct(&new_id, &app).unwrap();
    std::fs::write(app.join("a.txt"), "damaged").unwrap();
    let issues = repo.verify(&new_id, &app).unwrap().issues;
    assert_eq!(issues.to_string(), "0 missing, 1 corrupt, 0 extra");
    assert_eq!(repo.repair(&new_id, &app).unwrap().corrupt_file_count, 1);

    let reopened = Repo::open(repo.root()).unwrap();
    assert!(
        reopened
            .verify(&new_id, &app)
            .unwrap()
            .issues
            .matches_snapshot()
    );
    assert!(Repo::open(root.path().join("missing")).is_err());
    assert!(repo.capture(&source, &[("", "bad")]).is_err());
}
