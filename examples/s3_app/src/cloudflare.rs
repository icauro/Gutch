//! Signs in to Cloudflare R2 with one API token: finds the account, lists and
//! creates buckets, and derives the S3 keys from the token. Cloudflare
//! documents the derivation: the access key ID is the token's ID and the
//! secret is the SHA-256 of the token. Errors never include the token.

use std::time::Duration;

use gutch::api::{Error, Result};
use reqwest::blocking::{Client, RequestBuilder};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

const API: &str = "https://api.cloudflare.com/client/v4";

/// Where API tokens are made: R2 overview, then Manage API tokens.
pub const TOKEN_PAGE: &str = "https://dash.cloudflare.com/?to=/:account/r2/overview";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Account {
    pub id: String,
    pub name: String,
}

/// What signing in found.
#[derive(Clone, Debug)]
pub struct R2SignIn {
    /// The accounts the token can see; empty when it may not list them.
    pub accounts: Vec<Account>,
    pub account_id: String,
    /// `None` when the token may not list buckets.
    pub buckets: Option<Vec<String>>,
    pub access_key_id: String,
    pub secret_access_key: String,
}

/// Finds the account (`account_id`, or the first one the token sees), the
/// token's S3 keys, and the account's buckets in `jurisdiction`.
pub fn sign_in(token: &str, account_id: &str, jurisdiction: &str) -> Result<R2SignIn> {
    let api = Cloudflare::new(token)?;
    let accounts = match api.accounts() {
        Ok(accounts) => accounts,
        // Some tokens may not list accounts; a typed account ID still works.
        Err(_) if !account_id.trim().is_empty() => Vec::new(),
        Err(error) => return Err(error),
    };
    let account_id = match account_id.trim() {
        "" => match accounts.first() {
            Some(account) => account.id.clone(),
            None => {
                return Err(Error::InvalidInput(
                    "this token sees no account; type the account ID shown on the \
                     Cloudflare R2 overview page"
                        .into(),
                ));
            }
        },
        id => id.to_owned(),
    };
    let access_key_id = api.token_id(&account_id)?;
    Ok(R2SignIn {
        buckets: api.buckets(&account_id, jurisdiction).ok(),
        accounts,
        account_id,
        access_key_id,
        secret_access_key: s3_secret(token),
    })
}

/// Creates a bucket; needs a token with R2 write permission for the account.
pub fn create_bucket(token: &str, account_id: &str, jurisdiction: &str, name: &str) -> Result<()> {
    let api = Cloudflare::new(token)?;
    let url = format!("{API}/accounts/{}/r2/buckets", account_id.trim());
    let body = serde_json::json!({ "name": name.trim() });
    let request = api.request(api.client.post(url), jurisdiction).json(&body);
    api.send::<serde_json::Value>("create the bucket", request)?;
    Ok(())
}

/// The S3 secret for an API token: its SHA-256, in lowercase hex.
pub fn s3_secret(token: &str) -> String {
    Sha256::digest(token.trim().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct Cloudflare {
    client: Client,
    token: String,
}

#[derive(Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    errors: Vec<ApiError>,
    result: Option<T>,
}

#[derive(Deserialize)]
struct ApiError {
    code: i64,
    message: String,
}

#[derive(Deserialize)]
struct TokenStatus {
    id: String,
}

#[derive(Deserialize)]
struct BucketList {
    #[serde(default)]
    buckets: Vec<Bucket>,
}

#[derive(Deserialize)]
struct Bucket {
    name: String,
}

impl Cloudflare {
    fn new(token: &str) -> Result<Self> {
        let token = token.trim();
        if token.is_empty() {
            return Err(Error::InvalidInput("paste a Cloudflare API token".into()));
        }
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|error| api_error("start the HTTP client", &error.to_string()))?;
        Ok(Self {
            client,
            token: token.to_owned(),
        })
    }

    fn request(&self, request: RequestBuilder, jurisdiction: &str) -> RequestBuilder {
        let request = request.bearer_auth(&self.token);
        match jurisdiction {
            "" | "default" => request,
            jurisdiction => request.header("cf-r2-jurisdiction", jurisdiction),
        }
    }

    fn get<T: DeserializeOwned>(&self, action: &str, path: &str, jurisdiction: &str) -> Result<T> {
        let request = self.request(self.client.get(format!("{API}{path}")), jurisdiction);
        self.send(action, request)
    }

    fn send<T: DeserializeOwned>(&self, action: &str, request: RequestBuilder) -> Result<T> {
        let response = request
            .send()
            .map_err(|error| api_error(action, &error.without_url().to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .map_err(|error| api_error(action, &error.without_url().to_string()))?;
        let envelope: Envelope<T> = serde_json::from_str(&text)
            .map_err(|_| api_error(action, &format!("unexpected answer ({status})")))?;
        match envelope.result {
            Some(result) if envelope.success => Ok(result),
            _ => Err(api_error(action, &describe(status, &envelope.errors))),
        }
    }

    fn accounts(&self) -> Result<Vec<Account>> {
        self.get("list accounts", "/accounts?per_page=50", "")
    }

    /// User tokens and account tokens are verified at different addresses.
    fn token_id(&self, account_id: &str) -> Result<String> {
        match self.get::<TokenStatus>("check the token", "/user/tokens/verify", "") {
            Ok(status) => Ok(status.id),
            Err(_) => {
                let path = format!("/accounts/{account_id}/tokens/verify");
                Ok(self.get::<TokenStatus>("check the token", &path, "")?.id)
            }
        }
    }

    /// The first 1000 buckets, by name.
    fn buckets(&self, account_id: &str, jurisdiction: &str) -> Result<Vec<String>> {
        let path = format!("/accounts/{account_id}/r2/buckets?per_page=1000");
        let list: BucketList = self.get("list buckets", &path, jurisdiction)?;
        Ok(list.buckets.into_iter().map(|bucket| bucket.name).collect())
    }
}

fn describe(status: reqwest::StatusCode, errors: &[ApiError]) -> String {
    let detail = errors
        .iter()
        .map(|error| format!("{} {}", error.code, error.message))
        .collect::<Vec<_>>()
        .join("; ");
    let hint = match status.as_u16() {
        401 => " (the token is wrong or expired)",
        403 => " (the token lacks the R2 permission for this)",
        _ => "",
    };
    if detail.is_empty() {
        format!("{status}{hint}")
    } else {
        format!("{detail}{hint}")
    }
}

fn api_error(action: &str, detail: &str) -> Error {
    Error::repo(format!("Cloudflare: could not {action}: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_s3_secret_is_the_tokens_sha256() {
        assert_eq!(
            s3_secret(" abc "),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn api_errors_name_the_cause() {
        let errors = [ApiError {
            code: 10000,
            message: "Authentication error".into(),
        }];
        assert_eq!(
            describe(reqwest::StatusCode::FORBIDDEN, &errors),
            "10000 Authentication error (the token lacks the R2 permission for this)"
        );
        assert_eq!(
            describe(reqwest::StatusCode::UNAUTHORIZED, &[]),
            "401 Unauthorized (the token is wrong or expired)"
        );
    }
}
