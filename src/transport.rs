use std::io::Read;
use std::time::Duration;

use crate::{Error, GutchOperationControl, Result};

pub(crate) fn build_download_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent("Gutch")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30 * 60))
        .build()
        .map_err(|error| Error::manifest(format!("failed to create download client: {error}")))
}

pub(crate) fn download_url_streaming<F, C>(
    client: &reqwest::blocking::Client,
    url: &str,
    control: &GutchOperationControl,
    maximum_bytes: Option<u64>,
    mut on_content_length: C,
    mut on_chunk: F,
) -> Result<()>
where
    F: FnMut(&[u8]) -> Result<()>,
    C: FnMut(Option<u64>) -> Result<()>,
{
    control.checkpoint()?;
    let response = client
        .get(url)
        .send()
        .map_err(|error| Error::manifest(format!("download request failed: {error}")))?;
    if !response.status().is_success() {
        return Err(Error::manifest(format!(
            "download request failed with status {}",
            response.status()
        )));
    }

    let total_bytes = response.content_length();
    if maximum_bytes.is_some_and(|maximum| total_bytes.is_some_and(|total| total > maximum)) {
        return Err(Error::manifest("download exceeded the allowed size"));
    }
    on_content_length(total_bytes)?;

    read_bounded_chunks(
        response,
        maximum_bytes,
        || control.checkpoint(),
        |chunk| on_chunk(chunk),
    )?;
    Ok(())
}

pub(crate) fn read_bounded_chunks<R, F, C>(
    mut reader: R,
    maximum_bytes: Option<u64>,
    mut checkpoint: C,
    mut on_chunk: F,
) -> Result<u64>
where
    R: Read,
    F: FnMut(&[u8]) -> Result<()>,
    C: FnMut() -> Result<()>,
{
    checkpoint()?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut transferred_bytes = 0_u64;
    loop {
        checkpoint()?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        transferred_bytes = transferred_bytes.saturating_add(read as u64);
        if maximum_bytes.is_some_and(|maximum| transferred_bytes > maximum) {
            return Err(Error::manifest("response exceeded the allowed size"));
        }
        on_chunk(&buffer[..read])?;
    }
    checkpoint()?;
    Ok(transferred_bytes)
}

pub fn read_bounded_response<C>(
    mut response: reqwest::blocking::Response,
    maximum_bytes: u64,
    checkpoint: C,
) -> Result<Vec<u8>>
where
    C: FnMut() -> Result<()>,
{
    if response
        .content_length()
        .is_some_and(|length| length > maximum_bytes)
    {
        return Err(Error::manifest("response exceeded the allowed size"));
    }
    let mut bytes = Vec::new();
    read_bounded_chunks(&mut response, Some(maximum_bytes), checkpoint, |chunk| {
        bytes.extend_from_slice(chunk);
        Ok(())
    })?;
    Ok(bytes)
}
