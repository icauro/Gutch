use super::*;
use crate::artifact::profiles::CompressionProfile;

#[test]
fn compress_decompress_roundtrip() {
    let original = b"hello world, this is some test data for compression ".repeat(100);
    let profile = CompressionProfile::default();
    let compressed = compress_with_profile(&original, &profile).unwrap();
    assert!(
        compressed.len() < original.len(),
        "zstd should compress repetitive data"
    );

    let decompressed = decompress_data(&compressed, CompressionKind::Zstd).unwrap();
    assert_eq!(original.as_slice(), decompressed.as_slice());
}

#[test]
fn compress_small_data() {
    let original = b"hi";
    let profile = CompressionProfile::default();
    let compressed = compress_with_profile(original, &profile).unwrap();
    let decompressed = decompress_data(&compressed, CompressionKind::Zstd).unwrap();
    assert_eq!(original.as_slice(), decompressed.as_slice());
}

#[test]
fn compress_segments_roundtrip() {
    let first = b"first segment repeats ".repeat(100);
    let second = b"second segment repeats ".repeat(100);
    let third = b"third segment repeats ".repeat(100);
    let raw_size = (first.len() + second.len() + third.len()) as u64;
    let compressed = compress_segments_with_profile(
        &[first.as_slice(), second.as_slice(), third.as_slice()],
        raw_size,
        &CompressionProfile::default(),
    )
    .unwrap();

    let decompressed = decompress_data(&compressed, CompressionKind::Zstd).unwrap();
    let mut expected = Vec::new();
    expected.extend_from_slice(&first);
    expected.extend_from_slice(&second);
    expected.extend_from_slice(&third);
    assert_eq!(decompressed, expected);
}

#[test]
fn different_levels() {
    let original = b"some data that repeats again and again ".repeat(200);
    let c1 = compress_with_profile(
        &original,
        &CompressionProfile::for_algorithm(CompressionKind::Zstd, Some(1)),
    )
    .unwrap();
    let c9 = compress_with_profile(
        &original,
        &CompressionProfile::for_algorithm(CompressionKind::Zstd, Some(9)),
    )
    .unwrap();
    let d1 = decompress_data(&c1, CompressionKind::Zstd).unwrap();
    let d9 = decompress_data(&c9, CompressionKind::Zstd).unwrap();
    assert_eq!(d1, original);
    assert_eq!(d9, original);
    assert!(
        c9.len() <= c1.len(),
        "higher level should compress as well or better"
    );
}

#[test]
fn uncompressed_data_roundtrips_without_magic_sniffing() {
    let original = [0x28, 0xB5, 0x2F, 0xFD, b'r', b'a', b'w', b'!'];
    let decompressed = decompress_data(&original, CompressionKind::None).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn lz4_roundtrip() {
    let original = b"lz4 compression adapter data ".repeat(200);
    let profile = CompressionProfile::for_algorithm(CompressionKind::Lz4, Some(9));
    let compressed = compress_with_profile(&original, &profile).unwrap();
    let decompressed = decompress_data(&compressed, CompressionKind::Lz4).unwrap();

    assert_eq!(decompressed, original);
    assert!(compressed.len() < original.len());
}

#[test]
fn compression_levels_are_strictly_validated() {
    let zstd_invalid = CompressionProfile::for_algorithm(CompressionKind::Zstd, Some(23));
    let lz4_invalid = CompressionProfile::for_algorithm(CompressionKind::Lz4, Some(10));
    let none_invalid = CompressionProfile::for_algorithm(CompressionKind::None, Some(1));
    let none_zero = CompressionProfile::for_algorithm(CompressionKind::None, Some(0));

    assert!(validate_profile(&zstd_invalid).is_err());
    assert!(validate_profile(&lz4_invalid).is_err());
    assert!(validate_profile(&none_invalid).is_err());
    assert!(validate_profile(&none_zero).is_err());
}

#[test]
fn streaming_size_does_not_return_partial_counts_after_source_failure() {
    for codec in [
        CompressionKind::None,
        CompressionKind::Zstd,
        CompressionKind::Lz4,
    ] {
        let profile = CompressionProfile::for_algorithm(codec, None);
        let error = compressed_size_from_stream(&profile, 12, |writer| {
            writer.write_all(b"first")?;
            Err(crate::Error::artifact("sample source failed"))
        })
        .unwrap_err();
        assert!(error.to_string().contains("sample source failed"));
    }
}

#[test]
fn zstd_window_follows_content_size_within_supported_range() {
    assert_eq!(zstd_window_log(0), ZSTD_MIN_WINDOW_LOG);
    assert_eq!(zstd_window_log(1024), 10);
    assert_eq!(zstd_window_log(1025), 11);
    assert_eq!(zstd_window_log(8 << 20), 23);
    assert_eq!(zstd_window_log((8 << 20) + 1), 24);
    assert_eq!(zstd_window_log(u64::MAX), ZSTD_WINDOW_LOG);
}

/// Encode and decode like the artifact codecs do, then compare the memory the
/// zstd contexts really hold with the admission estimates.
#[test]
fn zstd_working_memory_estimates_cover_real_contexts() {
    use zstd::zstd_safe::CCtx;
    use zstd::zstd_safe::CParameter;
    use zstd::zstd_safe::DCtx;
    use zstd::zstd_safe::DParameter;
    use zstd::zstd_safe::InBuffer;
    use zstd::zstd_safe::OutBuffer;
    use zstd::zstd_safe::zstd_sys::ZSTD_EndDirective;

    let mut state = 0x9e37_79b9_u32;
    for raw_size in [1000, 300_000, (1 << 20) + 1] {
        // Half noise, half repeats, so matches and literals both occur.
        let raw: Vec<u8> = (0..raw_size)
            .map(|index| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                if index % 2048 < 1024 {
                    (state >> 24) as u8
                } else {
                    (index % 251) as u8
                }
            })
            .collect();
        for level in [1, 3, 19] {
            let profile = CompressionProfile::for_algorithm(CompressionKind::Zstd, Some(level));
            let mut encoder = CCtx::create();
            encoder
                .set_parameter(CParameter::CompressionLevel(level))
                .unwrap();
            encoder
                .set_parameter(CParameter::WindowLog(ZSTD_WINDOW_LOG))
                .unwrap();
            encoder
                .set_pledged_src_size(Some(raw.len() as u64))
                .unwrap();
            let mut encoded = vec![0; zstd::zstd_safe::compress_bound(raw.len())];
            let mut output = OutBuffer::around(&mut encoded[..]);
            for piece in raw.chunks(32 * 1024) {
                let mut input = InBuffer::around(piece);
                while input.pos() < piece.len() {
                    encoder
                        .compress_stream2(
                            &mut output,
                            &mut input,
                            ZSTD_EndDirective::ZSTD_e_continue,
                        )
                        .unwrap();
                }
            }
            let mut empty = InBuffer::around(&[][..]);
            while encoder
                .compress_stream2(&mut output, &mut empty, ZSTD_EndDirective::ZSTD_e_end)
                .unwrap()
                != 0
            {}
            let encoded_len = output.pos();
            let encode_estimate = encode_working_memory_bytes(&profile, raw.len() as u64);
            assert!(
                encoder.sizeof() as u64 <= encode_estimate,
                "level {level}, {raw_size} bytes: encoder holds {}, estimate {encode_estimate}",
                encoder.sizeof()
            );

            let mut decoder = DCtx::create();
            decoder
                .set_parameter(DParameter::WindowLogMax(ZSTD_WINDOW_LOG))
                .unwrap();
            let mut decoded = vec![0; 64 * 1024];
            let mut input = InBuffer::around(&encoded[..encoded_len]);
            let mut decoded_len = 0;
            loop {
                let mut output = OutBuffer::around(&mut decoded[..]);
                let remaining = decoder.decompress_stream(&mut output, &mut input).unwrap();
                decoded_len += output.pos();
                if remaining == 0 {
                    break;
                }
            }
            assert_eq!(decoded_len, raw.len());
            let decode_estimate = decode_working_memory_bytes(raw.len() as u64);
            assert!(
                decoder.sizeof() as u64 <= decode_estimate,
                "level {level}, {raw_size} bytes: decoder holds {}, estimate {decode_estimate}",
                decoder.sizeof()
            );
        }
    }
}

#[test]
fn working_memory_estimates_shrink_for_small_artifacts() {
    let profile = CompressionProfile::default();
    let small = encode_working_memory_bytes(&profile, 8 << 20);
    let largest = encode_working_memory_bytes(&profile, u64::MAX);
    assert!(small * 4 < largest, "small {small}, largest {largest}");
    assert!(largest >= 1 << ZSTD_WINDOW_LOG);
    assert!(decode_working_memory_bytes(8 << 20) * 4 < decode_working_memory_bytes(u64::MAX));
    assert!(decode_working_memory_bytes(u64::MAX) >= 1 << ZSTD_WINDOW_LOG);
}
