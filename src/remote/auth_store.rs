use keyring::Entry;
use serde::{Deserialize, Serialize};

const SHARED_AUTH_SERVICE_NAME: &str = "Launcher";
const SHARED_AUTH_ACCOUNT_NAME: &str = "desktop-auth-state";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedAuthSecrets {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub pending_auth_state: Option<String>,
    #[serde(default)]
    pub pending_auth_token: Option<String>,
}

fn auth_entry() -> Result<Entry, String> {
    Entry::new(SHARED_AUTH_SERVICE_NAME, SHARED_AUTH_ACCOUNT_NAME)
        .map_err(|err| format!("failed to open shared auth secure storage entry: {}", err))
}

fn normalize_secret(value: &Option<String>) -> Option<String> {
    value
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
}

fn normalize_auth_secrets(secrets: SharedAuthSecrets) -> SharedAuthSecrets {
    SharedAuthSecrets {
        session_id: normalize_secret(&secrets.session_id),
        pending_auth_state: normalize_secret(&secrets.pending_auth_state),
        pending_auth_token: normalize_secret(&secrets.pending_auth_token),
    }
}

pub fn load_shared_auth_secrets() -> Result<SharedAuthSecrets, String> {
    let entry = auth_entry()?;
    let raw = match entry.get_password() {
        Ok(value) => value,
        Err(keyring::Error::NoEntry) => return Ok(SharedAuthSecrets::default()),
        Err(err) => {
            return Err(format!(
                "failed to read shared auth secure storage entry: {}",
                err
            ));
        }
    };

    let secrets = serde_json::from_str::<SharedAuthSecrets>(&raw)
        .map_err(|err| format!("failed to parse shared auth secure storage entry: {}", err))?;
    Ok(normalize_auth_secrets(secrets))
}

pub fn save_shared_auth_secrets(secrets: &SharedAuthSecrets) -> Result<(), String> {
    let normalized = normalize_auth_secrets(secrets.clone());
    if normalized.session_id.is_none()
        && normalized.pending_auth_state.is_none()
        && normalized.pending_auth_token.is_none()
    {
        return clear_shared_auth_secrets();
    }

    let raw = serde_json::to_string(&normalized).map_err(|err| {
        format!(
            "failed to serialize shared auth secure storage entry: {}",
            err
        )
    })?;
    auth_entry()?
        .set_password(&raw)
        .map_err(|err| format!("failed to write shared auth secure storage entry: {}", err))
}

pub fn clear_shared_auth_secrets() -> Result<(), String> {
    match auth_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(format!(
            "failed to delete shared auth secure storage entry: {}",
            err
        )),
    }
}

pub fn shared_session_id() -> Result<Option<String>, String> {
    Ok(load_shared_auth_secrets()?.session_id)
}
