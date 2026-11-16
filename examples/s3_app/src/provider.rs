//! Connection presets. Each provider asks only for what differs between its
//! buckets; the endpoint, region and URL style follow from that.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum Provider {
    #[default]
    Aws,
    CloudflareR2,
    Ovh,
    Minio,
    /// Every S3 setting typed in by hand.
    Other,
}

impl Provider {
    pub const ALL: [Self; 5] = [
        Self::Aws,
        Self::CloudflareR2,
        Self::Ovh,
        Self::Minio,
        Self::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Aws => "AWS S3",
            Self::CloudflareR2 => "Cloudflare R2",
            Self::Ovh => "OVHcloud",
            Self::Minio => "MinIO",
            Self::Other => "Other S3-compatible",
        }
    }
}

/// R2 jurisdictions. Buckets outside `default` have their own S3 endpoint.
pub const R2_JURISDICTIONS: [&str; 4] = ["default", "eu", "us", "fedramp"];

/// What a preset fills in.
#[derive(Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub endpoint: String,
    pub region: String,
    pub path_style: bool,
}

pub fn aws(region: &str) -> Endpoint {
    let region = region.trim();
    Endpoint {
        endpoint: format!("https://s3.{region}.amazonaws.com"),
        region: region.to_owned(),
        path_style: false,
    }
}

pub fn ovh(region: &str) -> Endpoint {
    let region = region.trim().to_lowercase();
    Endpoint {
        endpoint: format!("https://s3.{region}.io.cloud.ovh.net"),
        region,
        path_style: false,
    }
}

/// R2 signs requests for the region `auto`.
pub fn r2(account_id: &str, jurisdiction: &str) -> Endpoint {
    let account_id = account_id.trim();
    let host = match jurisdiction {
        "" | "default" => format!("{account_id}.r2.cloudflarestorage.com"),
        jurisdiction => format!("{account_id}.{jurisdiction}.r2.cloudflarestorage.com"),
    };
    Endpoint {
        endpoint: format!("https://{host}"),
        region: "auto".into(),
        path_style: true,
    }
}

/// MinIO accepts `us-east-1` and has no bucket subdomains.
pub fn minio(endpoint: &str) -> Endpoint {
    Endpoint {
        endpoint: endpoint.trim().to_owned(),
        region: "us-east-1".into(),
        path_style: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_build_each_providers_endpoint() {
        assert_eq!(
            aws("eu-central-1").endpoint,
            "https://s3.eu-central-1.amazonaws.com"
        );
        assert_eq!(ovh(" GRA ").endpoint, "https://s3.gra.io.cloud.ovh.net");
        assert_eq!(
            r2("abc", "default"),
            Endpoint {
                endpoint: "https://abc.r2.cloudflarestorage.com".into(),
                region: "auto".into(),
                path_style: true,
            }
        );
        assert_eq!(
            r2("abc", "eu").endpoint,
            "https://abc.eu.r2.cloudflarestorage.com"
        );
        assert!(minio("http://localhost:9000").path_style);
    }
}
