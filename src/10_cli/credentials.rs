//! Repository credentials taken from the environment, so that passwords stay
//! out of command lines and shell history.

use crate::repo::encryption::{RepoCredential, set_default_repo_credential};
use std::path::PathBuf;

pub(crate) const PASSWORD_VARIABLE: &str = "GUTCH_REPO_PASSWORD";
pub(crate) const KEY_FILE_VARIABLE: &str = "GUTCH_REPO_KEY_FILE";
pub(crate) const NEW_PASSWORD_VARIABLE: &str = "GUTCH_REPO_NEW_PASSWORD";

/// Makes the environment's credential, if any, the one tried for encrypted
/// repositories. Plain repositories never use it.
pub(crate) fn use_environment_credential() -> crate::Result<()> {
    set_default_repo_credential(environment_credential()?);
    Ok(())
}

/// The password in GUTCH_REPO_PASSWORD, else the key file named by
/// GUTCH_REPO_KEY_FILE.
pub(crate) fn environment_credential() -> crate::Result<Option<RepoCredential>> {
    if let Some(credential) = password_variable(PASSWORD_VARIABLE)? {
        return Ok(Some(credential));
    }
    match std::env::var_os(KEY_FILE_VARIABLE) {
        Some(path) if !path.is_empty() => {
            RepoCredential::read_key_file(&PathBuf::from(path)).map(Some)
        }
        _ => Ok(None),
    }
}

pub(crate) fn password_variable(name: &str) -> crate::Result<Option<RepoCredential>> {
    match std::env::var(name) {
        Ok(password) if !password.is_empty() => Ok(Some(RepoCredential::Password(password))),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(crate::Error::invalid_input(format!(
            "{name} is not valid Unicode"
        ))),
    }
}
