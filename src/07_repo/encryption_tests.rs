use super::*;

fn slot_unwraps(encryption: &RepoEncryption, credential: &RepoCredential) -> bool {
    open_slots(encryption, credential).is_ok()
}

#[test]
fn password_and_key_file_slots_open_only_with_their_credential() {
    let password = RepoCredential::Password("correct horse".into());
    let mut encryption = new_repo_encryption(&password).unwrap();
    let master = registry().unlocked.get(&encryption.key_id).unwrap().clone();
    let keys = master.keys.clone();

    let key_file = RepoCredential::KeyFile([7; 32]);
    encryption
        .key_slots
        .push(wrap_master_key(&master.master, &encryption.key_id, &key_file).unwrap());

    for credential in [&password, &key_file] {
        let unlocked = open_slots(&encryption, credential).unwrap();
        assert_eq!(unlocked.master.0, master.master.0);
    }
    assert!(!slot_unwraps(
        &encryption,
        &RepoCredential::Password("wrong".into())
    ));
    assert!(!slot_unwraps(
        &encryption,
        &RepoCredential::KeyFile([8; 32])
    ));
    assert_eq!(format!("{keys:?}"), "SealKeys(..)");
}

#[test]
fn wrapped_keys_are_bound_to_the_key_id() {
    let credential = RepoCredential::KeyFile([9; 32]);
    let mut encryption = new_repo_encryption(&credential).unwrap();
    encryption.key_id = "00".repeat(KEY_ID_SIZE);
    assert!(!slot_unwraps(&encryption, &credential));
}

#[test]
fn rejects_slots_that_ask_for_too_much_work() {
    let error = WrappingKey::from_password("x", &[0; 16], MAX_PASSWORD_MEMORY_KIB + 1, 1, 1)
        .err()
        .unwrap();
    assert!(error.to_string().contains("too much work"));
}

#[test]
fn key_files_round_trip_and_never_overwrite() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("repo.key");
    let RepoCredential::KeyFile(created) = RepoCredential::create_key_file(&path).unwrap() else {
        unreachable!()
    };
    let RepoCredential::KeyFile(read) = RepoCredential::read_key_file(&path).unwrap() else {
        unreachable!()
    };
    assert_eq!(created, read);
    assert!(RepoCredential::create_key_file(&path).is_err());

    std::fs::write(&path, "not hex").unwrap();
    assert!(RepoCredential::read_key_file(&path).is_err());
}

#[test]
fn credentials_hide_their_secrets_in_debug_output() {
    assert_eq!(
        format!("{:?}", RepoCredential::Password("secret".into())),
        "RepoCredential::Password(..)"
    );
}

#[test]
fn hex_round_trips() {
    let bytes = [0u8, 1, 0xab, 0xff];
    assert_eq!(encode_hex(&bytes), "0001abff");
    assert_eq!(decode_hex::<4>("0001ABff"), Some(bytes));
    assert_eq!(decode_hex::<4>("0001ab"), None);
    assert_eq!(decode_hex_vec("0g"), None);
}
