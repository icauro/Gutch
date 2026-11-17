//! The encryption section of the repository manifest: which cipher protects
//! the repository's files and the key slots that unlock its master key.

use serde::{Deserialize, Serialize};

/// Present only in encrypted repositories. The master key itself is never
/// stored; each slot holds it wrapped by a key derived from one credential.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RepoEncryption {
    pub cipher: RepoCipher,
    /// Random identifier of the master key, bound into every wrapped key.
    pub key_id: String,
    pub key_slots: Vec<RepoKeySlot>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub enum RepoCipher {
    #[serde(rename = "aes-256-gcm")]
    Aes256Gcm,
}

/// The master key wrapped with AES-256-GCM by a key derived from a password
/// (Argon2id) or from a 32-byte key file. Byte fields are lowercase hex.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RepoKeySlot {
    Password {
        memory_kib: u32,
        iterations: u32,
        parallelism: u32,
        salt: String,
        nonce: String,
        wrapped_key: String,
    },
    KeyFile {
        nonce: String,
        wrapped_key: String,
    },
}
