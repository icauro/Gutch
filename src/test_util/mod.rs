use std::io::Write;

pub enum Seed {
    Binary,
    Audio,
    Texture,
    Archive,
    Config,
    Text,
}

pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u16) -> Self {
        Rng { state: seed as u64 }
    }

    fn next(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    pub fn next_u32(&mut self) -> u32 {
        self.next() as u32
    }

    pub fn next_byte(&mut self) -> u8 {
        (self.next() >> 32) as u8
    }
}

pub fn make_file(path: std::path::PathBuf, size: usize, seed: Seed) {
    let mut random_generator = Rng::new(match &seed {
        Seed::Binary => 0xdead,
        Seed::Audio => 0xbeef,
        Seed::Texture => 0xcafe,
        Seed::Archive => 0xabcd,
        Seed::Config => 0x1234,
        Seed::Text => 0x5678,
    });

    let pattern: Vec<u8> = match seed {
        Seed::Binary => (0..65536).map(|_| random_generator.next_byte()).collect(),
        Seed::Audio => {
            let mut pattern_bytes = Vec::with_capacity(32768);
            let mut sample_value = 0u8;
            for _ in 0..32768 {
                sample_value = sample_value
                    .wrapping_add(random_generator.next_byte().wrapping_rem(7).wrapping_sub(3));
                pattern_bytes.push(sample_value);
            }
            pattern_bytes
        }
        Seed::Texture => {
            let mut pattern_bytes = Vec::with_capacity(4096);
            for _ in 0..1024 {
                pattern_bytes.extend_from_slice(&[
                    random_generator.next_byte(),
                    random_generator.next_byte(),
                    random_generator.next_byte(),
                    255u8,
                ]);
            }
            pattern_bytes
        }
        Seed::Archive => {
            let mut pattern_bytes = Vec::with_capacity(8192);
            for _ in 0..2048 {
                pattern_bytes.extend_from_slice(&random_generator.next_u32().to_le_bytes());
            }
            pattern_bytes
        }
        Seed::Config => {
            let generated_text = generate_config_json(size);
            std::fs::write(&path, generated_text.as_bytes()).unwrap();
            return;
        }
        Seed::Text => {
            let generated_text = generate_text(size);
            std::fs::write(&path, generated_text.as_bytes()).unwrap();
            return;
        }
    };

    let mut output_file = std::fs::File::create(&path).unwrap();
    let mut written = 0usize;
    while written < size {
        let take = pattern.len().min(size - written);
        output_file.write_all(&pattern[..take]).unwrap();
        written += take;
    }
}

fn generate_config_json(size: usize) -> String {
    let mut output_text = String::with_capacity(size + 64);
    output_text.push_str("{\n");
    let mut setting_index = 0u32;
    while output_text.len() + 64 < size {
        output_text.push_str(&format!(
            "  \"key_{:06}\": \"value_{:06}_{:08}\",\n",
            setting_index,
            setting_index,
            setting_index.wrapping_mul(0x9e3779b9)
        ));
        setting_index = setting_index.wrapping_add(1);
    }
    output_text.push_str("  \"_end\": true\n}\n");
    output_text
}

fn generate_text(size: usize) -> String {
    let words = [
        "the", "game", "version", "update", "patch", "install", "launcher", "data", "file",
        "config", "capture", "release", "asset", "texture", "audio", "level", "player", "enemy",
        "weapon", "health", "score", "save", "load", "menu",
    ];
    let mut output_text = String::with_capacity(size + 64);
    let mut word_index = 0u64;
    while output_text.len() < size {
        output_text.push_str(&format!(
            "{} {}\n",
            words[word_index as usize % words.len()],
            word_index
        ));
        word_index = word_index.wrapping_add(1);
    }
    output_text
}

pub fn dir_size(dir: &std::path::Path) -> u64 {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
        .sum()
}

pub fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(name)
}

pub fn recreate_dir(path: &std::path::Path) {
    let _ = std::fs::remove_dir_all(path);
    std::fs::create_dir_all(path).unwrap();
}

pub(crate) fn chunk_entries(
    mut entries: std::collections::BTreeMap<String, crate::repo::index::ChunkIndexEntry>,
) -> crate::repo::index::ChunkIndexEntries {
    let mut ordinals = std::collections::HashMap::new();
    for entry in entries.values_mut() {
        let ordinal = ordinals.entry(entry.bundle_id.clone()).or_insert(0u32);
        entry.chunk_ordinal = *ordinal;
        *ordinal += 1;
    }
    entries
        .into_iter()
        .map(|(hash, entry)| {
            (
                crate::artifact::hash::ChunkHash::parse_hex(&hash).unwrap(),
                entry,
            )
        })
        .collect()
}

pub(crate) fn loose_bytes(
    bytes: &[u8],
    profile: &crate::artifact::profiles::CompressionProfile,
) -> Vec<u8> {
    let mut encoded = Vec::new();
    crate::artifact::loose::write_loose_chunk(
        &mut encoded,
        crate::artifact::hash::ChunkHash::from_blake3(blake3::hash(bytes)),
        bytes,
        profile,
    )
    .unwrap();
    encoded
}

/// Populate declared locations for metadata-only fixtures; these tests never decode artifacts.
pub fn index_manifest(
    paths: &crate::repo::paths::GutchRepoPaths,
    manifest: &crate::manifest::SnapshotManifest,
) {
    let entries = manifest
        .files()
        .flat_map(|file| &file.chunks)
        .map(|chunk| {
            (
                chunk.chunk_hash,
                crate::repo::index::ChunkIndexEntry::new(
                    chunk.raw_size,
                    0,
                    chunk.chunk_hash.to_hex().into(),
                    chunk.raw_size,
                    crate::artifact::profiles::ArtifactStorageMode::Bundled,
                ),
            )
        })
        .collect();
    crate::repo::index::write_chunk_index(
        paths,
        &crate::repo::index::ChunkIndex {
            format_version: "1".into(),
            entries,
        },
    )
    .unwrap();
}
