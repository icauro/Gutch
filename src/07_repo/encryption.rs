//! Repository credentials, key slots and the process's unlocked keys.
//!
//! An encrypted repository has a random master key. Its manifest stores the
//! key once per credential, wrapped with AES-256-GCM by a key derived from a
//! password (Argon2id) or a key file. Unlocking unwraps it once and keeps the
//! derived file keys for the rest of the process, so repeated operations on
//! the repository do not repeat the slow password derivation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use zeroize::Zeroize;

use crate::Error;
use crate::Result;
use crate::artifact::seal::SealKeys;
use crate::filesystem;
use crate::manifest::repo_manifest::RepoManifest;
use crate::manifest::{REPO_METADATA_FILE, RepoCipher, RepoEncryption, RepoKeySlot};

const KEY_SIZE: usize = 32;
const NONCE_SIZE: usize = 12;
const KEY_ID_SIZE: usize = 16;
const PASSWORD_SALT_SIZE: usize = 32;
/// Argon2id cost for new password slots: 64 MiB, three passes.
#[cfg(not(test))]
const PASSWORD_MEMORY_KIB: u32 = 64 * 1024;
#[cfg(test)]
const PASSWORD_MEMORY_KIB: u32 = 64;
const PASSWORD_ITERATIONS: u32 = 3;
const PASSWORD_PARALLELISM: u32 = 1;
/// Limits on stored slot costs, so a damaged or hostile manifest cannot make
/// unlocking allocate without bound.
const MAX_PASSWORD_MEMORY_KIB: u32 = 4 * 1024 * 1024;
const MAX_PASSWORD_ITERATIONS: u32 = 64;
const MAX_PASSWORD_PARALLELISM: u32 = 64;

/// Something that unlocks an encrypted repository.
pub enum RepoCredential {
    Password(String),
    /// The 32 bytes of a key file.
    KeyFile([u8; KEY_SIZE]),
}

impl RepoCredential {
    /// Reads a key file: 64 hexadecimal digits, optionally followed by
    /// whitespace.
    pub fn read_key_file(path: &Path) -> Result<Self> {
        let mut text = String::from_utf8(filesystem::read(path)?)
            .map_err(|_| Error::invalid_input(format!("{} is not a key file", path.display())))?;
        let key = decode_hex::<KEY_SIZE>(text.trim());
        text.zeroize();
        key.map(Self::KeyFile)
            .ok_or_else(|| Error::invalid_input(format!("{} is not a key file", path.display())))
    }

    /// Writes a new random key file. Fails if the file exists.
    pub fn create_key_file(path: &Path) -> Result<Self> {
        let key: [u8; KEY_SIZE] = random_bytes()?;
        let mut text = encode_hex(&key);
        text.push('\n');
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, text.as_bytes()));
        text.zeroize();
        written.map_err(|error| Error::io_context("create key file", path, error))?;
        Ok(Self::KeyFile(key))
    }
}

impl Clone for RepoCredential {
    fn clone(&self) -> Self {
        match self {
            Self::Password(password) => Self::Password(password.clone()),
            Self::KeyFile(key) => Self::KeyFile(*key),
        }
    }
}

impl Drop for RepoCredential {
    fn drop(&mut self) {
        match self {
            Self::Password(password) => password.zeroize(),
            Self::KeyFile(key) => key.zeroize(),
        }
    }
}

impl std::fmt::Debug for RepoCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Password(_) => "RepoCredential::Password(..)",
            Self::KeyFile(_) => "RepoCredential::KeyFile(..)",
        })
    }
}

struct MasterKey([u8; KEY_SIZE]);

impl Drop for MasterKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

struct Unlocked {
    master: MasterKey,
    keys: Arc<SealKeys>,
}

#[derive(Default)]
struct Registry {
    default_credential: Option<RepoCredential>,
    credentials: HashMap<PathBuf, RepoCredential>,
    unlocked: HashMap<String, Arc<Unlocked>>,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(Mutex::default);

fn registry() -> MutexGuard<'static, Registry> {
    REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn registry_key(repo_root: &Path) -> PathBuf {
    std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf())
}

/// Sets the credential tried for encrypted repositories that were not
/// unlocked with their own. `None` removes it.
pub fn set_default_repo_credential(credential: Option<RepoCredential>) {
    registry().default_credential = credential;
}

/// Unlocks an encrypted repository for this process. Later operations on it
/// need no credential. Fails if the credential opens none of its key slots.
pub fn unlock_repo(repo_root: &Path, credential: RepoCredential) -> Result<()> {
    let manifest = read_manifest(repo_root)?;
    let Some(encryption) = &manifest.encryption else {
        return Err(Error::invalid_input(format!(
            "repository at {} is not encrypted",
            repo_root.display()
        )));
    };
    let unlocked = open_slots(encryption, &credential)?;
    let mut registry = registry();
    registry
        .unlocked
        .insert(encryption.key_id.clone(), Arc::new(unlocked));
    registry
        .credentials
        .insert(registry_key(repo_root), credential);
    Ok(())
}

/// Forgets every credential and unlocked key held by this process.
pub fn lock_repos() {
    let mut registry = registry();
    registry.default_credential = None;
    registry.credentials.clear();
    registry.unlocked.clear();
}

/// Adds a key slot for another credential to an unlocked repository.
pub fn add_repo_credential(repo_root: &Path, credential: &RepoCredential) -> Result<()> {
    super::coordination::write(repo_root, || {
        let (paths, mut manifest) = super::paths::load_repo_manifest(repo_root)?;
        let Some(encryption) = manifest.encryption.as_mut() else {
            return Err(Error::invalid_input(format!(
                "repository at {} is not encrypted",
                repo_root.display()
            )));
        };
        let unlocked = unlocked(repo_root, encryption)?;
        let slot = wrap_master_key(&unlocked.master, &encryption.key_id, credential)?;
        encryption.key_slots.push(slot);
        super::paths::write_repo_manifest(&paths, &manifest)
    })
}

/// Rejects incoming repository metadata whose encryption differs from the
/// local repository's: their files could not be read with one set of keys.
pub(crate) fn check_same_repo_key(
    local: Option<&RepoEncryption>,
    incoming: Option<&RepoEncryption>,
) -> Result<()> {
    if local.map(|e| &e.key_id) == incoming.map(|e| &e.key_id) {
        return Ok(());
    }
    Err(Error::repo(match (local, incoming) {
        (None, Some(_)) => "the remote repository is encrypted but the local one is not",
        (Some(_), None) => "the local repository is encrypted but the remote one is not",
        _ => "the local and remote repositories are encrypted with different keys",
    }))
}

/// The file keys of an encrypted repository, unlocking it if needed.
pub(crate) fn repo_keys(repo_root: &Path, encryption: &RepoEncryption) -> Result<Arc<SealKeys>> {
    Ok(unlocked(repo_root, encryption)?.keys.clone())
}

fn unlocked(repo_root: &Path, encryption: &RepoEncryption) -> Result<Arc<Unlocked>> {
    let credential = {
        let registry = registry();
        if let Some(unlocked) = registry.unlocked.get(&encryption.key_id) {
            return Ok(unlocked.clone());
        }
        registry
            .credentials
            .get(&registry_key(repo_root))
            .or(registry.default_credential.as_ref())
            .cloned()
    };
    let Some(credential) = credential else {
        return Err(Error::repo(format!(
            "repository at {} is encrypted; unlock it with a password or key file",
            repo_root.display()
        )));
    };
    let unlocked = Arc::new(open_slots(encryption, &credential)?);
    registry()
        .unlocked
        .insert(encryption.key_id.clone(), unlocked.clone());
    Ok(unlocked)
}

/// The encryption section and file keys for a new repository.
pub(crate) fn new_repo_encryption(credential: &RepoCredential) -> Result<RepoEncryption> {
    let master = MasterKey(random_bytes()?);
    let key_id = encode_hex(&random_bytes::<KEY_ID_SIZE>()?);
    let slot = wrap_master_key(&master, &key_id, credential)?;
    let keys = Arc::new(SealKeys::from_master_key(&master.0));
    let encryption = RepoEncryption {
        cipher: RepoCipher::Aes256Gcm,
        key_id: key_id.clone(),
        key_slots: vec![slot],
    };
    registry()
        .unlocked
        .insert(key_id, Arc::new(Unlocked { master, keys }));
    Ok(encryption)
}

fn read_manifest(repo_root: &Path) -> Result<RepoManifest> {
    let path = repo_root.join(REPO_METADATA_FILE);
    RepoManifest::from_json_bytes(REPO_METADATA_FILE, &filesystem::read(&path)?)
}

fn open_slots(encryption: &RepoEncryption, credential: &RepoCredential) -> Result<Unlocked> {
    let RepoCipher::Aes256Gcm = encryption.cipher;
    for slot in &encryption.key_slots {
        if let Some(master) = unwrap_master_key(slot, &encryption.key_id, credential)? {
            let keys = Arc::new(SealKeys::from_master_key(&master.0));
            return Ok(Unlocked { master, keys });
        }
    }
    Err(Error::repo(match credential {
        RepoCredential::Password(_) => "wrong password for the encrypted repository",
        RepoCredential::KeyFile(_) => "wrong key file for the encrypted repository",
    }))
}

struct WrappingKey([u8; KEY_SIZE]);

impl Drop for WrappingKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl WrappingKey {
    fn from_key_file(key: &[u8; KEY_SIZE]) -> Self {
        Self(blake3::derive_key(
            "gutch repository 2026-11 key file slot",
            key,
        ))
    }

    fn from_password(
        password: &str,
        salt: &[u8],
        memory_kib: u32,
        iterations: u32,
        parallelism: u32,
    ) -> Result<Self> {
        if memory_kib > MAX_PASSWORD_MEMORY_KIB
            || iterations > MAX_PASSWORD_ITERATIONS
            || parallelism > MAX_PASSWORD_PARALLELISM
        {
            return Err(Error::repo("password key slot asks for too much work"));
        }
        let params = argon2::Params::new(memory_kib, iterations, parallelism, Some(KEY_SIZE))
            .map_err(|error| Error::repo(format!("invalid password key slot: {error}")))?;
        let argon =
            argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
        let mut key = [0u8; KEY_SIZE];
        argon
            .hash_password_into(password.as_bytes(), salt, &mut key)
            .map_err(|error| Error::repo(format!("cannot derive the password key: {error}")))?;
        Ok(Self(key))
    }

    fn aead(&self) -> LessSafeKey {
        LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &self.0).expect("32-byte AES key"))
    }
}

fn wrap_master_key(
    master: &MasterKey,
    key_id: &str,
    credential: &RepoCredential,
) -> Result<RepoKeySlot> {
    let nonce: [u8; NONCE_SIZE] = random_bytes()?;
    let seal = |wrapping: &WrappingKey| -> Result<String> {
        let mut sealed = master.0.to_vec();
        wrapping
            .aead()
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(key_id.as_bytes()),
                &mut sealed,
            )
            .map_err(|_| Error::repo("cannot wrap the repository key"))?;
        Ok(encode_hex(&sealed))
    };
    Ok(match credential {
        RepoCredential::Password(password) => {
            let salt: [u8; PASSWORD_SALT_SIZE] = random_bytes()?;
            let wrapping = WrappingKey::from_password(
                password,
                &salt,
                PASSWORD_MEMORY_KIB,
                PASSWORD_ITERATIONS,
                PASSWORD_PARALLELISM,
            )?;
            RepoKeySlot::Password {
                memory_kib: PASSWORD_MEMORY_KIB,
                iterations: PASSWORD_ITERATIONS,
                parallelism: PASSWORD_PARALLELISM,
                salt: encode_hex(&salt),
                nonce: encode_hex(&nonce),
                wrapped_key: seal(&wrapping)?,
            }
        }
        RepoCredential::KeyFile(key) => RepoKeySlot::KeyFile {
            nonce: encode_hex(&nonce),
            wrapped_key: seal(&WrappingKey::from_key_file(key))?,
        },
    })
}

/// The master key from `slot`, or `None` when the credential does not open it.
fn unwrap_master_key(
    slot: &RepoKeySlot,
    key_id: &str,
    credential: &RepoCredential,
) -> Result<Option<MasterKey>> {
    let (wrapping, nonce, wrapped_key) = match (slot, credential) {
        (
            RepoKeySlot::Password {
                memory_kib,
                iterations,
                parallelism,
                salt,
                nonce,
                wrapped_key,
            },
            RepoCredential::Password(password),
        ) => {
            let salt = decode_hex_vec(salt).ok_or_else(invalid_slot)?;
            let wrapping = WrappingKey::from_password(
                password,
                &salt,
                *memory_kib,
                *iterations,
                *parallelism,
            )?;
            (wrapping, nonce, wrapped_key)
        }
        (RepoKeySlot::KeyFile { nonce, wrapped_key }, RepoCredential::KeyFile(key)) => {
            (WrappingKey::from_key_file(key), nonce, wrapped_key)
        }
        _ => return Ok(None),
    };
    let nonce = decode_hex::<NONCE_SIZE>(nonce).ok_or_else(invalid_slot)?;
    let mut sealed = decode_hex_vec(wrapped_key).ok_or_else(invalid_slot)?;
    let opened = wrapping.aead().open_in_place(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(key_id.as_bytes()),
        &mut sealed,
    );
    let master = match opened {
        Ok(plain) if plain.len() == KEY_SIZE => {
            Some(MasterKey(plain.try_into().expect("checked length")))
        }
        Ok(_) => return Err(invalid_slot()),
        Err(_) => None,
    };
    sealed.zeroize();
    Ok(master)
}

fn invalid_slot() -> Error {
    Error::repo("damaged key slot in the repository manifest")
}

fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    aws_lc_rs::rand::fill(&mut bytes)
        .map_err(|_| Error::repo("cannot generate random key material"))?;
    Ok(bytes)
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[usize::from(byte >> 4)] as char);
        text.push(DIGITS[usize::from(byte & 15)] as char);
    }
    text
}

fn decode_hex_vec(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            let digit = |byte: u8| (byte as char).to_digit(16);
            Some((digit(pair[0])? * 16 + digit(pair[1])?) as u8)
        })
        .collect()
}

fn decode_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let mut bytes = decode_hex_vec(text)?;
    let array = bytes.as_slice().try_into().ok();
    bytes.zeroize();
    array
}

#[cfg(test)]
#[path = "encryption_tests.rs"]
mod tests;
