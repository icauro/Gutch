//! Bounded metadata groups. The directory authenticates framing against corruption;
//! callers still need the externally trusted artifact digest for authenticity.
use std::io::Read;

use crate::artifact::binary_io::{
    read_exact_array, read_u8, read_u16, read_u32, write_u16, write_u32,
};
use crate::{Error, Result};

pub(crate) const MAX_METADATA_BYTES: usize = 256 * 1024 * 1024;
const VERSION: u16 = 1;
const DESCRIPTOR_BYTES: usize = 44;
const FIXED_HEADER_BYTES: usize = 24;
const DIGEST_BYTES: usize = 32;
const MAX_GROUP_COUNT: usize = 16;
const REQUIRED_GROUP_FLAG: u8 = 1;
const RAW_CODEC: u8 = 0;
const ZSTD_CODEC: u8 = 1;

pub(crate) fn encode(magic: &[u8; 16], groups: &[Vec<u8>]) -> Result<Vec<u8>> {
    if groups.is_empty() || groups.len() > MAX_GROUP_COUNT {
        return Err(Error::manifest("invalid metadata group count"));
    }
    let mut directory = magic.to_vec();
    write_u16(&mut directory, VERSION)?;
    write_u16(&mut directory, 0)?;
    write_u32(&mut directory, groups.len() as u32)?;
    let mut payloads = Vec::new();
    let mut decoded_total = 0usize;
    for (index, group) in groups.iter().enumerate() {
        decoded_total = decoded_total
            .checked_add(group.len())
            .filter(|size| *size <= MAX_METADATA_BYTES)
            .ok_or_else(|| Error::manifest("metadata exceeds decoded size limit"))?;
        let compressed = zstd::stream::encode_all(group.as_slice(), 3)?;
        let (codec, payload) = if compressed.len() < group.len() {
            (ZSTD_CODEC, compressed.as_slice())
        } else {
            (RAW_CODEC, group.as_slice())
        };
        write_u16(&mut directory, (index + 1) as u16)?;
        directory.extend_from_slice(&[REQUIRED_GROUP_FLAG, codec]);
        write_u32(&mut directory, payload.len() as u32)?;
        write_u32(&mut directory, group.len() as u32)?;
        directory.extend_from_slice(blake3::hash(payload).as_bytes());
        payloads.extend_from_slice(payload);
    }
    let digest = blake3::hash(&directory);
    directory.extend_from_slice(digest.as_bytes());
    directory.extend_from_slice(&payloads);
    if directory.len() > MAX_METADATA_BYTES {
        return Err(Error::manifest("metadata exceeds encoded size limit"));
    }
    Ok(directory)
}

pub(crate) fn decode(bytes: &[u8], magic: &[u8; 16], group_count: usize) -> Result<Vec<Vec<u8>>> {
    if bytes.len() > MAX_METADATA_BYTES
        || bytes.len() < FIXED_HEADER_BYTES + DIGEST_BYTES
        || &bytes[..16] != magic
    {
        return Err(Error::manifest("invalid metadata envelope or size"));
    }
    let mut header = &bytes[16..FIXED_HEADER_BYTES];
    if read_u16(&mut header)? != VERSION || read_u16(&mut header)? != 0 {
        return Err(Error::manifest("unsupported metadata version or features"));
    }
    let count = read_u32(&mut header)? as usize;
    // Refuse unknown optional groups as well: this API materializes data that may
    // be rewritten, so silently discarding an extension would be lossy.
    if count != group_count || count > MAX_GROUP_COUNT {
        return Err(Error::manifest(
            "unsupported metadata groups; refusing lossy rewrite",
        ));
    }
    let directory_end = FIXED_HEADER_BYTES + count * DESCRIPTOR_BYTES;
    if bytes.len() < directory_end + DIGEST_BYTES
        || blake3::hash(&bytes[..directory_end]).as_bytes()
            != &bytes[directory_end..directory_end + DIGEST_BYTES]
    {
        return Err(Error::manifest("metadata directory integrity mismatch"));
    }
    // Validate framing before trusting encoded lengths or allocating decoded groups.
    let mut descriptors = &bytes[FIXED_HEADER_BYTES..directory_end];
    let mut payload = &bytes[directory_end + DIGEST_BYTES..];
    let mut groups = Vec::with_capacity(count);
    let mut decoded_total = 0usize;
    for index in 0..count {
        if read_u16(&mut descriptors)? as usize != index + 1
            || read_u8(&mut descriptors)? != REQUIRED_GROUP_FLAG
        {
            return Err(Error::manifest("unsupported metadata group or flags"));
        }
        let codec = read_u8(&mut descriptors)?;
        let encoded_size = read_u32(&mut descriptors)? as usize;
        let decoded_size = read_u32(&mut descriptors)? as usize;
        let digest = read_exact_array::<32>(&mut descriptors)?;
        decoded_total = decoded_total
            .checked_add(decoded_size)
            .filter(|size| *size <= MAX_METADATA_BYTES)
            .ok_or_else(|| Error::manifest("metadata exceeds decoded size limit"))?;
        if encoded_size > payload.len() {
            return Err(Error::manifest("truncated metadata group"));
        }
        let (encoded, rest) = payload.split_at(encoded_size);
        payload = rest;
        if blake3::hash(encoded).as_bytes() != &digest {
            return Err(Error::manifest("metadata group integrity mismatch"));
        }
        let decoded = match codec {
            RAW_CODEC if encoded_size == decoded_size => encoded.to_vec(),
            ZSTD_CODEC => {
                let mut decoder = zstd::stream::read::Decoder::new(encoded)?;
                decoder.window_log_max(28)?;
                // Reserve a whole group only when bounded by twice the actual input.
                // A partial hint can waste capacity when highly compressed groups grow.
                let initial_capacity = if decoded_size <= encoded_size * 2 {
                    decoded_size
                } else {
                    0
                };
                let mut output = Vec::with_capacity(initial_capacity);
                decoder
                    .take(decoded_size as u64 + 1)
                    .read_to_end(&mut output)?;
                if output.len() != decoded_size {
                    return Err(Error::manifest("metadata decoded length mismatch"));
                }
                output
            }
            _ => return Err(Error::manifest("invalid metadata codec or raw length")),
        };
        groups.push(decoded);
    }
    if !payload.is_empty() {
        return Err(Error::manifest("trailing metadata bytes"));
    }
    Ok(groups)
}

pub(crate) fn read_file(path: &std::path::Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_METADATA_BYTES as u64 {
        return Err(Error::manifest("metadata exceeds encoded size limit"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_METADATA_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_METADATA_BYTES {
        return Err(Error::manifest("metadata exceeds encoded size limit"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    const MAGIC: &[u8; 16] = crate::artifact::format::SNAPSHOT_BLUEPRINT_MAGIC;

    fn resign(bytes: &mut [u8], count: usize) {
        let end = 24 + count * DESCRIPTOR_BYTES;
        let digest = blake3::hash(&bytes[..end]);
        bytes[end..end + 32].copy_from_slice(digest.as_bytes());
    }

    #[test]
    fn roundtrips_mild_strong_and_raw_compression_groups() {
        let mut mild = Vec::new();
        for index in 0..256u32 {
            let hash = blake3::hash(&index.to_le_bytes());
            mild.extend_from_slice(hash.as_bytes());
            mild.extend_from_slice(hash.as_bytes());
        }
        let groups = vec![mild, vec![0; 16384], vec![1, 2, 3]];
        let encoded = encode(MAGIC, &groups).unwrap();
        assert_eq!(encoded[FIXED_HEADER_BYTES + 3], ZSTD_CODEC);
        assert_eq!(
            encoded[FIXED_HEADER_BYTES + DESCRIPTOR_BYTES + 3],
            ZSTD_CODEC
        );
        assert_eq!(
            encoded[FIXED_HEADER_BYTES + 2 * DESCRIPTOR_BYTES + 3],
            RAW_CODEC
        );
        assert_eq!(decode(&encoded, MAGIC, groups.len()).unwrap(), groups);
    }

    #[test]
    fn framing_and_decoded_bounds_are_enforced_before_materialization() {
        let groups = vec![vec![0; 4096], vec![1, 2, 3]];
        let valid = encode(MAGIC, &groups).unwrap();
        assert_eq!(decode(&valid, MAGIC, 2).unwrap(), groups);
        for offset in 0..valid.len() {
            let mut invalid = valid.clone();
            invalid[offset] ^= 1;
            assert!(decode(&invalid, MAGIC, 2).is_err(), "offset {offset}");
        }
        for offset in [24, 26, 27, 28, 32] {
            let mut invalid = valid.clone();
            invalid[offset] = 255;
            resign(&mut invalid, 2);
            assert!(decode(&invalid, MAGIC, 2).is_err(), "descriptor {offset}");
        }
        for declared in [
            1u32,
            MAX_METADATA_BYTES as u32,
            (MAX_METADATA_BYTES + 1) as u32,
        ] {
            let mut invalid = valid.clone();
            invalid[32..36].copy_from_slice(&declared.to_le_bytes());
            resign(&mut invalid, 2);
            assert!(decode(&invalid, MAGIC, 2).is_err());
        }
        let mut unknown_optional = valid.clone();
        unknown_optional[26] = 0;
        resign(&mut unknown_optional, 2);
        assert!(decode(&unknown_optional, MAGIC, 2).is_err());
        let mut trailing = valid;
        trailing.push(0);
        assert!(decode(&trailing, MAGIC, 2).is_err());
    }
}
