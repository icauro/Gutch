use gutch as g;

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreatorRepoTargetsResponse {
    pub(crate) tiers: Vec<CreatorTierResponse>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreatorTierResponse {
    pub(crate) id: i64,
    pub(crate) slug: String,
    pub(crate) tier_name: String,
    pub(crate) targets: Vec<CreatorTargetResponse>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreatorTargetResponse {
    pub(crate) id: i64,
    pub(crate) target_id: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpsertTargetResponse {
    pub(crate) id: i64,
}

pub(crate) fn request_json_from_backend<T: for<'de> serde::Deserialize<'de>>(
    request: reqwest::blocking::RequestBuilder,
) -> g::Result<T> {
    let response = request
        .send()
        .map_err(|err| g::Error::manifest(format!("backend request failed: {}", err)))?;
    if !response.status().is_success() {
        return Err(g::Error::manifest(parse_backend_error_response(response)));
    }
    response
        .json::<T>()
        .map_err(|err| g::Error::manifest(format!("failed to decode backend response: {}", err)))
}


pub fn parse_backend_error_response(response: reqwest::blocking::Response) -> String {
    let status = response.status();
    let text = response.text().unwrap_or_default();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(message) = value
            .get("message")
            .and_then(|value| value.as_str())
            .or_else(|| value.get("error").and_then(|value| value.as_str()))
        {
            let trimmed = message.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    if text.trim().is_empty() {
        return format!("request failed with status {}", status);
    }
    format!("request failed with status {}", status)
}

pub(crate) fn slugify_like_backend(value: &str) -> String {
    let trimmed = value.trim().to_ascii_lowercase();
    let mut slug = String::new();
    let mut last_hyphen = false;
    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_hyphen = false;
        } else if !last_hyphen {
            slug.push('-');
            last_hyphen = true;
        }
    }
    slug.trim_matches('-').to_string()
}

pub(crate) fn upload_raw_file(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    path: &str,
    session_id: &str,
    bytes: Vec<u8>,
    content_type: &str,
) -> g::Result<()> {
    let response = client
        .put(format!("{}{}", endpoint.trim_end_matches('/'), path))
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {}", session_id))
        .header("Content-Type", content_type)
        .body(bytes)
        .send()
        .map_err(|err| g::Error::manifest(format!("upload request failed: {}", err)))?;
    if !response.status().is_success() {
        return Err(g::Error::manifest(parse_backend_error_response(response)));
    }
    Ok(())
}
