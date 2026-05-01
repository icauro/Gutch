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
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
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
    let mut rng = Rng::new(match &seed {
        Seed::Binary => 0xdead,
        Seed::Audio => 0xbeef,
        Seed::Texture => 0xcafe,
        Seed::Archive => 0xabcd,
        Seed::Config => 0x1234,
        Seed::Text => 0x5678,
    });

    let pattern: Vec<u8> = match seed {
        Seed::Binary => (0..65536).map(|_| rng.next_byte()).collect(),
        Seed::Audio => {
            let mut p = Vec::with_capacity(32768);
            let mut freq = 0u8;
            for _ in 0..32768 {
                freq = freq.wrapping_add(rng.next_byte().wrapping_rem(7).wrapping_sub(3));
                p.push(freq);
            }
            p
        }
        Seed::Texture => {
            let mut p = Vec::with_capacity(4096);
            for _ in 0..1024 {
                p.extend_from_slice(&[rng.next_byte(), rng.next_byte(), rng.next_byte(), 255u8]);
            }
            p
        }
        Seed::Archive => {
            let mut p = Vec::with_capacity(8192);
            for _ in 0..2048 {
                p.extend_from_slice(&rng.next_u32().to_le_bytes());
            }
            p
        }
        Seed::Config => {
            let s = generate_config_json(size);
            std::fs::write(&path, s.as_bytes()).unwrap();
            return;
        }
        Seed::Text => {
            let s = generate_text(size);
            std::fs::write(&path, s.as_bytes()).unwrap();
            return;
        }
    };

    let mut f = std::fs::File::create(&path).unwrap();
    let mut written = 0usize;
    while written < size {
        let take = pattern.len().min(size - written);
        f.write_all(&pattern[..take]).unwrap();
        written += take;
    }
}

fn generate_config_json(size: usize) -> String {
    let mut out = String::with_capacity(size + 64);
    out.push_str("{\n");
    let mut i = 0u32;
    while out.len() + 64 < size {
        out.push_str(&format!(
            "  \"key_{:06}\": \"value_{:06}_{:08}\",\n",
            i, i, i.wrapping_mul(0x9e3779b9)
        ));
        i = i.wrapping_add(1);
    }
    out.push_str("  \"_end\": true\n}\n");
    out
}

fn generate_text(size: usize) -> String {
    let words = [
        "the", "game", "version", "update", "patch", "install", "launcher", "data",
        "file", "config", "build", "release", "asset", "texture", "audio", "level",
        "player", "enemy", "weapon", "health", "score", "save", "load", "menu",
    ];
    let mut out = String::with_capacity(size + 64);
    let mut wi = 0u64;
    while out.len() < size {
        out.push_str(&format!("{} {}\n", words[wi as usize % words.len()], wi));
        wi = wi.wrapping_add(1);
    }
    out
}

pub fn dir_size(dir: &std::path::Path) -> u64 {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
        .sum()
}