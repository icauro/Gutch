//! Validate artifact URL inputs independently of repository workflows.
use super::types::ArtifactDownload;
use crate::{Error, Result};
use std::collections::HashMap;

pub(crate) fn normalize_artifact_downloads(
    artifacts: &[ArtifactDownload],
) -> Result<HashMap<String, String>> {
    let mut normalized = HashMap::with_capacity(artifacts.len());
    for artifact in artifacts {
        let (artifact_id, url) = normalize_artifact_download(artifact)?;
        if normalized
            .insert(artifact_id.to_string(), url.to_string())
            .is_some()
        {
            return Err(Error::invalid_input(format!(
                "duplicate direct artifact URL for '{}'",
                artifact_id
            )));
        }
    }
    Ok(normalized)
}

pub(crate) fn normalize_artifact_download(artifact: &ArtifactDownload) -> Result<(&str, &str)> {
    let artifact_id = non_empty_trimmed(&artifact.artifact_id, "artifact id")?;
    let url = non_empty_trimmed(&artifact.url, "artifact URL")?;
    // Reject unusable URLs up front; the message omits the URL, which may carry a token.
    let scheme_supported = reqwest::Url::parse(url)
        .is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https") && parsed.has_host());
    if !scheme_supported {
        return Err(Error::invalid_input(format!(
            "artifact URL for '{artifact_id}' must be an absolute http or https URL"
        )));
    }
    Ok((artifact_id, url))
}

fn non_empty_trimmed<'a>(value: &'a str, label: &str) -> Result<&'a str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(Error::invalid_input(format!("{label} is required")));
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_urls_must_be_absolute_http() {
        let download = |url: &str| ArtifactDownload {
            artifact_id: "chunk".into(),
            url: url.into(),
        };
        for url in ["https://host/a?token=x", " http://host/a "] {
            assert!(normalize_artifact_download(&download(url)).is_ok(), "{url}");
        }
        for url in ["file:///tmp/a", "host/a", "ftp://host/a", "https://"] {
            let error = normalize_artifact_download(&download(url)).unwrap_err();
            assert!(!error.to_string().contains(url), "{url}");
        }
    }
}
