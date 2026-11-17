//! Sealed files: repository files encrypted and authenticated with AES-256-GCM.
//!
//! A sealed file is a header followed by segments, each holding up to
//! `SEGMENT_SIZE` plaintext bytes and a 16-byte tag. Every file gets its own
//! key, derived from the repository's data key, the file's purpose and a
//! random salt in its header, so segment nonces only need to count. The last
//! segment's nonce is marked, so a file cut at a segment boundary does not
//! open (the STREAM construction). Readers decrypt one segment at a time and
//! can seek, so large bundles stream without being held in memory.
//!
//! Metadata sealed whole by `seal_bytes` takes its salt from a keyed hash of
//! its content instead, so the same metadata always seals to the same bytes
//! and sync can compare stored files by hash. That reveals only that two
//! such files are identical.

use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use zeroize::Zeroize;

use crate::Error;
use crate::Result;

pub(crate) const SEALED_MAGIC: &[u8; 16] = b"GUTCH_SEALED\0\0\0\0";
const VERSION: u16 = 1;
const SALT_SIZE: usize = 32;
const HEADER_SIZE: u64 = 16 + 2 + 2 + SALT_SIZE as u64;
const SEGMENT_SIZE: usize = 1024 * 1024;
const TAG_SIZE: usize = 16;
const SEALED_SEGMENT_SIZE: u64 = (SEGMENT_SIZE + TAG_SIZE) as u64;

/// What a sealed file holds. It is part of each file's key, so one kind of
/// file cannot be passed off as another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SealPurpose {
    Bundle = 1,
    LooseChunk = 2,
    ChunkIndex = 3,
    SnapshotMetadata = 4,
    SnapshotProductMetadata = 5,
    SnapshotBlueprint = 6,
}

/// The working keys of an encrypted repository, derived from its master key.
pub struct SealKeys {
    data: [u8; 32],
    salts: [u8; 32],
    names: [u8; 32],
    chunker: [u8; 32],
}

impl SealKeys {
    pub(crate) fn from_master_key(master_key: &[u8; 32]) -> Self {
        Self {
            data: blake3::derive_key("gutch repository 2026-11 file data", master_key),
            salts: blake3::derive_key("gutch repository 2026-11 content salts", master_key),
            names: blake3::derive_key("gutch repository 2026-11 names", master_key),
            chunker: blake3::derive_key("gutch repository 2026-11 chunker seed", master_key),
        }
    }

    /// A keyed hasher for names and identifiers that would otherwise let
    /// someone with a copy of the content confirm it is stored.
    pub(crate) fn name_hasher(&self) -> blake3::Hasher {
        blake3::Hasher::new_keyed(&self.names)
    }

    /// The seed for this repository's chunk boundaries.
    pub(crate) fn chunker_seed(&self) -> &[u8; 32] {
        &self.chunker
    }

    fn file_key(&self, purpose: SealPurpose, salt: &[u8; SALT_SIZE]) -> Result<LessSafeKey> {
        let mut hasher = blake3::Hasher::new_keyed(&self.data);
        hasher.update(&(purpose as u16).to_le_bytes());
        hasher.update(salt);
        let mut key = *hasher.finalize().as_bytes();
        let unbound = UnboundKey::new(&AES_256_GCM, &key);
        key.zeroize();
        unbound
            .map(LessSafeKey::new)
            .map_err(|_| Error::artifact("cannot create a file encryption key"))
    }
}

impl Drop for SealKeys {
    fn drop(&mut self) {
        self.data.zeroize();
        self.salts.zeroize();
        self.names.zeroize();
        self.chunker.zeroize();
    }
}

impl std::fmt::Debug for SealKeys {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SealKeys(..)")
    }
}

fn segment_nonce(segment: u64, last: bool) -> Nonce {
    let mut bytes = [0u8; 12];
    bytes[..8].copy_from_slice(&segment.to_le_bytes());
    bytes[11] = u8::from(last);
    Nonce::assume_unique_for_key(bytes)
}

fn damaged() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "encrypted file is damaged or was written with another key",
    )
}

/// Encrypts everything written to it. `finish` seals the last segment; a
/// writer dropped without it leaves a file that does not open.
pub(crate) struct SealedWriter<W: Write> {
    inner: W,
    key: LessSafeKey,
    segment: Vec<u8>,
    index: u64,
}

impl<W: Write> SealedWriter<W> {
    pub(crate) fn new(inner: W, keys: &SealKeys, purpose: SealPurpose) -> Result<Self> {
        let mut salt = [0u8; SALT_SIZE];
        aws_lc_rs::rand::fill(&mut salt)
            .map_err(|_| Error::artifact("cannot generate an encryption salt"))?;
        Self::with_salt(inner, keys, purpose, salt)
    }

    fn with_salt(
        mut inner: W,
        keys: &SealKeys,
        purpose: SealPurpose,
        salt: [u8; SALT_SIZE],
    ) -> Result<Self> {
        inner.write_all(SEALED_MAGIC)?;
        inner.write_all(&VERSION.to_le_bytes())?;
        inner.write_all(&(purpose as u16).to_le_bytes())?;
        inner.write_all(&salt)?;
        Ok(Self {
            inner,
            key: keys.file_key(purpose, &salt)?,
            segment: Vec::with_capacity(SEGMENT_SIZE + TAG_SIZE),
            index: 0,
        })
    }

    fn seal_segment(&mut self, last: bool) -> io::Result<()> {
        self.key
            .seal_in_place_append_tag(
                segment_nonce(self.index, last),
                Aad::empty(),
                &mut self.segment,
            )
            .map_err(|_| io::Error::other("cannot encrypt a file segment"))?;
        self.inner.write_all(&self.segment)?;
        self.segment.clear();
        self.index += 1;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<W> {
        self.seal_segment(true)?;
        Ok(self.inner)
    }
}

impl<W: Write> Write for SealedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        // A full segment is sealed only once more data arrives, because the
        // last segment is sealed differently.
        if self.segment.len() == SEGMENT_SIZE {
            self.seal_segment(false)?;
        }
        let taken = bytes.len().min(SEGMENT_SIZE - self.segment.len());
        self.segment.extend_from_slice(&bytes[..taken]);
        Ok(taken)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Decrypts a sealed file, one authenticated segment at a time.
pub(crate) struct SealedReader<R: Read + Seek> {
    inner: R,
    key: LessSafeKey,
    sealed_len: u64,
    segment_count: u64,
    plaintext_len: u64,
    position: u64,
    loaded: Option<u64>,
    last_verified: bool,
    segment: Vec<u8>,
}

impl<R: Read + Seek> SealedReader<R> {
    pub(crate) fn new(mut inner: R, keys: &SealKeys, purpose: SealPurpose) -> Result<Self> {
        let sealed_len = inner.seek(SeekFrom::End(0))?;
        if sealed_len < HEADER_SIZE + TAG_SIZE as u64 {
            return Err(Error::artifact("encrypted file is truncated"));
        }
        inner.seek(SeekFrom::Start(0))?;
        let mut header = [0u8; HEADER_SIZE as usize];
        inner.read_exact(&mut header)?;
        if &header[..16] != SEALED_MAGIC {
            return Err(Error::artifact(
                "expected an encrypted file in an encrypted repository",
            ));
        }
        if u16::from_le_bytes([header[16], header[17]]) != VERSION {
            return Err(Error::artifact("unsupported encrypted file version"));
        }
        if u16::from_le_bytes([header[18], header[19]]) != purpose as u16 {
            return Err(Error::artifact("encrypted file holds another kind of data"));
        }
        let salt: [u8; SALT_SIZE] = header[20..].try_into().expect("salt fits the header");
        let body = sealed_len - HEADER_SIZE;
        let segment_count = body.div_ceil(SEALED_SEGMENT_SIZE);
        let last_len = body - (segment_count - 1) * SEALED_SEGMENT_SIZE;
        if last_len < TAG_SIZE as u64 {
            return Err(Error::artifact("encrypted file is truncated"));
        }
        Ok(Self {
            inner,
            key: keys.file_key(purpose, &salt)?,
            sealed_len,
            segment_count,
            plaintext_len: body - segment_count * TAG_SIZE as u64,
            position: 0,
            loaded: None,
            last_verified: false,
            segment: Vec::with_capacity(SEGMENT_SIZE + TAG_SIZE),
        })
    }

    pub(crate) fn plaintext_len(&self) -> u64 {
        self.plaintext_len
    }

    fn load(&mut self, index: u64) -> io::Result<()> {
        let start = HEADER_SIZE + index * SEALED_SEGMENT_SIZE;
        let len = SEALED_SEGMENT_SIZE.min(self.sealed_len - start);
        let last = index + 1 == self.segment_count;
        self.loaded = None;
        self.inner.seek(SeekFrom::Start(start))?;
        self.segment.resize(len as usize, 0);
        self.inner.read_exact(&mut self.segment)?;
        let plain_len = self
            .key
            .open_in_place(segment_nonce(index, last), Aad::empty(), &mut self.segment)
            .map_err(|_| damaged())?
            .len();
        self.segment.truncate(plain_len);
        self.loaded = Some(index);
        self.last_verified |= last;
        Ok(())
    }
}

impl<R: Read + Seek> Read for SealedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.position >= self.plaintext_len {
            // Reaching the end proves the file was not cut short.
            if !self.last_verified {
                self.load(self.segment_count - 1)?;
            }
            return Ok(0);
        }
        if buffer.is_empty() {
            return Ok(0);
        }
        let index = self.position / SEGMENT_SIZE as u64;
        if self.loaded != Some(index) {
            self.load(index)?;
        }
        let offset = (self.position - index * SEGMENT_SIZE as u64) as usize;
        let available = &self.segment[offset..];
        let count = available.len().min(buffer.len());
        buffer[..count].copy_from_slice(&available[..count]);
        self.position += count as u64;
        Ok(count)
    }
}

impl<R: Read + Seek> Seek for SealedReader<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let target = match position {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(delta) => self.plaintext_len.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid seek position"))?;
        self.position = target;
        Ok(target)
    }
}

/// A stored file, decrypted when the repository is encrypted.
pub(crate) enum StoredReader<R: Read + Seek> {
    Plain(R),
    Sealed(Box<SealedReader<R>>),
}

impl<R: Read + Seek> StoredReader<R> {
    pub(crate) fn new(inner: R, keys: Option<&SealKeys>, purpose: SealPurpose) -> Result<Self> {
        Ok(match keys {
            None => Self::Plain(inner),
            Some(keys) => Self::Sealed(Box::new(SealedReader::new(inner, keys, purpose)?)),
        })
    }
}

impl<R: Read + Seek> Read for StoredReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(reader) => reader.read(buffer),
            Self::Sealed(reader) => reader.read(buffer),
        }
    }
}

impl<R: Read + Seek> Seek for StoredReader<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        match self {
            Self::Plain(reader) => reader.seek(position),
            Self::Sealed(reader) => reader.seek(position),
        }
    }
}

/// A stored file being written, encrypted when the repository is encrypted.
pub(crate) enum StoredWriter<W: Write> {
    Plain(W),
    Sealed(Box<SealedWriter<W>>),
}

impl<W: Write> StoredWriter<W> {
    pub(crate) fn new(inner: W, keys: Option<&SealKeys>, purpose: SealPurpose) -> Result<Self> {
        Ok(match keys {
            None => Self::Plain(inner),
            Some(keys) => Self::Sealed(Box::new(SealedWriter::new(inner, keys, purpose)?)),
        })
    }

    pub(crate) fn finish(self) -> Result<W> {
        match self {
            Self::Plain(writer) => Ok(writer),
            Self::Sealed(writer) => writer.finish(),
        }
    }
}

impl<W: Write> Write for StoredWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(writer) => writer.write(bytes),
            Self::Sealed(writer) => writer.write(bytes),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(writer) => writer.flush(),
            Self::Sealed(writer) => writer.flush(),
        }
    }
}

/// The stored size of a file holding `plaintext_len` bytes.
pub(crate) fn stored_len(keys: Option<&SealKeys>, plaintext_len: u64) -> u64 {
    match keys {
        None => plaintext_len,
        Some(_) => {
            let segments = plaintext_len.max(1).div_ceil(SEGMENT_SIZE as u64);
            HEADER_SIZE + plaintext_len + segments * TAG_SIZE as u64
        }
    }
}

/// The bytes to store for `plaintext`: unchanged without keys. The salt comes
/// from the content, so equal plaintexts seal to equal bytes.
pub(crate) fn seal_bytes(
    keys: Option<&SealKeys>,
    purpose: SealPurpose,
    plaintext: Vec<u8>,
) -> Result<Vec<u8>> {
    let Some(keys) = keys else {
        return Ok(plaintext);
    };
    let segments = plaintext.len() / SEGMENT_SIZE + 1;
    let capacity = HEADER_SIZE as usize + plaintext.len() + segments * TAG_SIZE;
    let salt = *blake3::Hasher::new_keyed(&keys.salts)
        .update(&(purpose as u16).to_le_bytes())
        .update(&plaintext)
        .finalize()
        .as_bytes();
    let mut writer = SealedWriter::with_salt(Vec::with_capacity(capacity), keys, purpose, salt)?;
    writer.write_all(&plaintext)?;
    writer.finish()
}

/// The plaintext of stored bytes: unchanged without keys.
pub(crate) fn open_bytes(
    keys: Option<&SealKeys>,
    purpose: SealPurpose,
    stored: Vec<u8>,
) -> Result<Vec<u8>> {
    let Some(keys) = keys else {
        return Ok(stored);
    };
    let mut reader = SealedReader::new(Cursor::new(stored), keys, purpose)?;
    let mut plaintext = Vec::with_capacity(reader.plaintext_len() as usize);
    reader.read_to_end(&mut plaintext)?;
    Ok(plaintext)
}

#[cfg(test)]
#[path = "seal_tests.rs"]
mod tests;
