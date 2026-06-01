use reqwest::blocking::Client;
use serde::Deserialize;

use crate::remote::backend::parse_backend_error_response;

const GUTCH_USER_AGENT: &str = "Gutch CLI";

pub(super) fn build_http_client() -> Result<Client, String> {
    Client::builder()
        .user_agent(GUTCH_USER_AGENT)
        .build()
        .map_err(|err| format!("failed to create auth client: {}", err))
}

pub(super) fn request_json<T: for<'de> Deserialize<'de>>(
    _client: &Client,
    request: reqwest::blocking::RequestBuilder,
) -> Result<T, String> {
    let response = request
        .send()
        .map_err(|err| format!("backend request failed: {}", err))?;
    if !response.status().is_success() {
        return Err(parse_backend_error_response(response));
    }
    response
        .json::<T>()
        .map_err(|err| format!("failed to decode backend response: {}", err))
}

pub(super) fn request_no_content(
    _client: &Client,
    request: reqwest::blocking::RequestBuilder,
) -> Result<(), String> {
    let response = request
        .send()
        .map_err(|err| format!("backend request failed: {}", err))?;
    if !response.status().is_success() {
        return Err(parse_backend_error_response(response));
    }
    Ok(())
}
