use super::chunk_stream::{ChunkBoundary, ChunkStream};
use super::jc::{JumpChunker, Parameters};
use crate::progress::OperationControl;
use std::io::{self, Cursor, Read};

pub(super) fn random_bytes(length: usize, mut seed: u64) -> Vec<u8> {
    (0..length)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        })
        .collect()
}

#[test]
fn plakar_boundaries_match_pinned_go_oracle() {
    let cases: serde_json::Value = serde_json::from_str(GO_ORACLE).unwrap();
    for case in cases.as_array().unwrap() {
        let value = |name: &str| case[name].as_u64().unwrap() as usize;
        let data = random_bytes(value("length"), case["seed"].as_u64().unwrap());
        let boundary =
            JumpChunker::new(value("min"), value("avg"), value("max"), Parameters::Plakar);
        let mut offset = 0;
        let mut cuts = Vec::new();
        while offset < data.len() {
            let length = boundary.cut(&data[offset..]);
            cuts.push(length);
            offset += length;
        }
        assert_eq!(serde_json::to_value(cuts).unwrap(), case["cuts"], "{case}");
    }
}

#[test]
fn both_variants_preserve_data_bounds_and_reader_independence() {
    struct ShortReads<'a>(Cursor<&'a [u8]>, usize);
    impl Read for ShortReads<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let length = out.len().min(self.1);
            self.0.read(&mut out[..length])
        }
    }
    for parameters in [Parameters::Plakar, Parameters::PaperScaled] {
        for data in [
            Vec::new(),
            vec![1],
            vec![0; 131_073],
            random_bytes(262_181, 917),
        ] {
            let boundary = JumpChunker::new(2048, 8192, 32768, parameters);
            assert_eq!(boundary.cut(&[]), 0);
            for read_size in [31, 4093, usize::MAX] {
                let mut stream = ChunkStream::with_boundary(
                    ShortReads(Cursor::new(&data), read_size),
                    JumpChunker::new(2048, 8192, 32768, parameters),
                    0,
                );
                let mut offset = 0;
                while let Some(chunk) = stream.next(&OperationControl::default()).unwrap() {
                    let length = boundary.cut(&data[offset..]);
                    assert_eq!(chunk.offset as usize, offset);
                    assert_eq!(chunk.data, &data[offset..offset + length]);
                    assert!(length > 0 && length <= 32768);
                    assert!(length >= 2048 || offset + length == data.len());
                    offset += length;
                }
                assert_eq!(offset, data.len());
            }
        }
    }
}

// Generated with pinned go-cdc-chunkers v1.1.0; recipe in the experiment README.
const GO_ORACLE: &str = r#"[
{"min":2048,"avg":8192,"max":65536,"length":0,"seed":917,"cuts":[]},
{"min":2048,"avg":8192,"max":65536,"length":1,"seed":917,"cuts":[1]},
{"min":2048,"avg":8192,"max":65536,"length":2047,"seed":917,"cuts":[2047]},
{"min":2048,"avg":8192,"max":65536,"length":2049,"seed":917,"cuts":[2049]},
{"min":2048,"avg":8192,"max":65536,"length":8191,"seed":917,"cuts":[8191]},
{"min":2048,"avg":8192,"max":65536,"length":8192,"seed":917,"cuts":[8192]},
{"min":2048,"avg":8192,"max":65536,"length":8193,"seed":917,"cuts":[8193]},
{"min":2048,"avg":8192,"max":65536,"length":65573,"seed":917,"cuts":[13209,2134,8154,21732,2185,3397,2612,2237,8468,1445]},
{"min":32768,"avg":131072,"max":524288,"length":0,"seed":917,"cuts":[]},
{"min":32768,"avg":131072,"max":524288,"length":1,"seed":917,"cuts":[1]},
{"min":32768,"avg":131072,"max":524288,"length":32767,"seed":917,"cuts":[32767]},
{"min":32768,"avg":131072,"max":524288,"length":32769,"seed":917,"cuts":[32769]},
{"min":32768,"avg":131072,"max":524288,"length":131071,"seed":917,"cuts":[131071]},
{"min":32768,"avg":131072,"max":524288,"length":131072,"seed":917,"cuts":[131072]},
{"min":32768,"avg":131072,"max":524288,"length":131073,"seed":917,"cuts":[131073]},
{"min":32768,"avg":131072,"max":524288,"length":524325,"seed":917,"cuts":[232806,33789,33927,33139,100450,32948,32996,24270]},
{"min":262144,"avg":1048576,"max":4194304,"length":0,"seed":917,"cuts":[]},
{"min":262144,"avg":1048576,"max":4194304,"length":1,"seed":917,"cuts":[1]},
{"min":262144,"avg":1048576,"max":4194304,"length":262143,"seed":917,"cuts":[262143]},
{"min":262144,"avg":1048576,"max":4194304,"length":262145,"seed":917,"cuts":[262145]},
{"min":262144,"avg":1048576,"max":4194304,"length":1048575,"seed":917,"cuts":[263425,785150]},
{"min":262144,"avg":1048576,"max":4194304,"length":1048576,"seed":917,"cuts":[263425,785151]},
{"min":262144,"avg":1048576,"max":4194304,"length":1048577,"seed":917,"cuts":[263425,785152]},
{"min":262144,"avg":1048576,"max":4194304,"length":4194341,"seed":917,"cuts":[263425,788974,789333,263212,1314406,263848,511143]}
]"#;
