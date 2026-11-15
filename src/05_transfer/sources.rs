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
            return Err(Error::manifest(format!(
                "duplicate direct artifact URL for '{}'",
                artifact_id
            )));
        }
    }
    Ok(normalized)
}

pub(crate) fn normalize_artifact_download(artifact: &ArtifactDownload) -> Result<(&str, &str)> {
    Ok((
        non_empty_trimmed(&artifact.artifact_id, "artifact id")?,
        non_empty_trimmed(&artifact.url, "artifact URL")?,
    ))
}

fn non_empty_trimmed<'a>(value: &'a str, label: &str) -> Result<&'a str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(Error::manifest(format!("{label} is required")));
    }
    Ok(trimmed)
}
