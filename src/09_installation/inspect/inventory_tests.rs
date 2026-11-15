use super::NameCase;
use std::fs;

#[test]
fn insensitive_name_keys_fold_unicode_case() {
    assert_eq!(
        NameCase::Insensitive.key("Äpfel/ä.txt"),
        NameCase::Insensitive.key("äPFEL/Ä.TXT")
    );
    assert_ne!(
        NameCase::Sensitive.key("Ä.txt"),
        NameCase::Sensitive.key("ä.txt")
    );
    // A letter whose uppercase form is two letters keeps its own spelling.
    assert_eq!(NameCase::Insensitive.key("straße"), "STRAßE");
}

#[test]
fn name_case_probe_matches_the_filesystem() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("Probe.txt"), b"x").unwrap();
    let expected = if temp.path().join("PROBE.TXT").exists() {
        NameCase::Insensitive
    } else {
        NameCase::Sensitive
    };
    assert_eq!(NameCase::detect(temp.path()), expected);
}
