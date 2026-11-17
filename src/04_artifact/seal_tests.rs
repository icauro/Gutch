use std::io::{Cursor, Read, Seek, SeekFrom, Write};

use super::*;

fn keys(byte: u8) -> SealKeys {
    SealKeys::from_master_key(&[byte; 32])
}

fn sample(len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| (index * 31 + index / 7) as u8)
        .collect()
}

fn read_all(keys: &SealKeys, purpose: SealPurpose, stored: Vec<u8>) -> Result<Vec<u8>> {
    open_bytes(Some(keys), purpose, stored)
}

#[test]
fn round_trips_lengths_around_segment_boundaries() {
    let keys = keys(1);
    for len in [
        0,
        1,
        SEGMENT_SIZE - 1,
        SEGMENT_SIZE,
        SEGMENT_SIZE + 1,
        2 * SEGMENT_SIZE,
        2 * SEGMENT_SIZE + 17,
    ] {
        let plaintext = sample(len);
        let stored = seal_bytes(Some(&keys), SealPurpose::Bundle, plaintext.clone()).unwrap();
        let expected_segments = len.max(1).div_ceil(SEGMENT_SIZE);
        assert_eq!(
            stored.len(),
            HEADER_SIZE as usize + len + expected_segments * TAG_SIZE,
            "len {len}"
        );
        assert_eq!(
            read_all(&keys, SealPurpose::Bundle, stored).unwrap(),
            plaintext
        );
    }
}

#[test]
fn plain_repositories_store_bytes_unchanged() {
    let plaintext = sample(100);
    let stored = seal_bytes(None, SealPurpose::Bundle, plaintext.clone()).unwrap();
    assert_eq!(stored, plaintext);
    assert_eq!(
        open_bytes(None, SealPurpose::Bundle, stored).unwrap(),
        plaintext
    );
}

#[test]
fn whole_metadata_seals_the_same_way_each_time() {
    let keys = keys(2);
    let first = seal_bytes(Some(&keys), SealPurpose::SnapshotMetadata, sample(64)).unwrap();
    let second = seal_bytes(Some(&keys), SealPurpose::SnapshotMetadata, sample(64)).unwrap();
    let other_purpose = seal_bytes(Some(&keys), SealPurpose::ChunkIndex, sample(64)).unwrap();
    let other_content = seal_bytes(Some(&keys), SealPurpose::SnapshotMetadata, sample(65)).unwrap();
    assert_eq!(first, second);
    assert_ne!(
        first[..HEADER_SIZE as usize],
        other_purpose[..HEADER_SIZE as usize]
    );
    assert_ne!(
        first[..HEADER_SIZE as usize],
        other_content[..HEADER_SIZE as usize]
    );
}

#[test]
fn streamed_files_seal_differently_each_time() {
    let keys = keys(2);
    let seal = || {
        let mut writer = StoredWriter::new(Vec::new(), Some(&keys), SealPurpose::Bundle).unwrap();
        writer.write_all(&sample(64)).unwrap();
        writer.finish().unwrap()
    };
    assert_ne!(seal(), seal());
}

#[test]
fn rejects_wrong_key_purpose_and_tampering() {
    let keys = keys(3);
    let stored = seal_bytes(Some(&keys), SealPurpose::ChunkIndex, sample(5000)).unwrap();

    assert!(read_all(&self::keys(4), SealPurpose::ChunkIndex, stored.clone()).is_err());
    assert!(read_all(&keys, SealPurpose::Bundle, stored.clone()).is_err());

    let mut tampered = stored.clone();
    let middle = tampered.len() / 2;
    tampered[middle] ^= 1;
    assert!(read_all(&keys, SealPurpose::ChunkIndex, tampered).is_err());

    let mut salted = stored;
    salted[30] ^= 1;
    assert!(read_all(&keys, SealPurpose::ChunkIndex, salted).is_err());
}

#[test]
fn rejects_a_file_cut_at_a_segment_boundary() {
    let keys = keys(5);
    let stored = seal_bytes(
        Some(&keys),
        SealPurpose::Bundle,
        sample(2 * SEGMENT_SIZE + 9),
    )
    .unwrap();
    let cut = HEADER_SIZE as usize + 2 * (SEGMENT_SIZE + TAG_SIZE);
    assert!(read_all(&keys, SealPurpose::Bundle, stored[..cut].to_vec()).is_err());
    assert!(read_all(&keys, SealPurpose::Bundle, stored[..cut - 3].to_vec()).is_err());
}

#[test]
fn plain_bytes_are_rejected_in_an_encrypted_repository() {
    let error = read_all(&keys(6), SealPurpose::Bundle, sample(500)).unwrap_err();
    assert!(error.to_string().contains("expected an encrypted file"));
}

#[test]
fn seeks_and_reads_across_segments() {
    let keys = keys(7);
    let plaintext = sample(3 * SEGMENT_SIZE + 123);
    let stored = seal_bytes(Some(&keys), SealPurpose::Bundle, plaintext.clone()).unwrap();
    let mut reader = SealedReader::new(Cursor::new(stored), &keys, SealPurpose::Bundle).unwrap();
    assert_eq!(reader.plaintext_len(), plaintext.len() as u64);

    let end = reader.seek(SeekFrom::End(-40)).unwrap() as usize;
    let mut tail = Vec::new();
    reader.read_to_end(&mut tail).unwrap();
    assert_eq!(tail, plaintext[end..]);

    let start = SEGMENT_SIZE - 10;
    reader.seek(SeekFrom::Start(start as u64)).unwrap();
    let mut across = vec![0u8; 30];
    reader.read_exact(&mut across).unwrap();
    assert_eq!(across, plaintext[start..start + 30]);
}

#[test]
fn stored_writer_and_reader_match_the_byte_helpers() {
    let keys = keys(8);
    let plaintext = sample(SEGMENT_SIZE + 5);
    let mut writer = StoredWriter::new(Vec::new(), Some(&keys), SealPurpose::LooseChunk).unwrap();
    for piece in plaintext.chunks(4096) {
        writer.write_all(piece).unwrap();
    }
    let stored = writer.finish().unwrap();
    let mut reader =
        StoredReader::new(Cursor::new(stored), Some(&keys), SealPurpose::LooseChunk).unwrap();
    let mut decoded = Vec::new();
    reader.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, plaintext);
}

#[test]
fn debug_output_hides_keys() {
    assert_eq!(format!("{:?}", keys(9)), "SealKeys(..)");
}
