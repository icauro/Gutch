use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryMode {
    #[default]
    Auto,
    Low,
    Balanced,
    High,
}

impl MemoryMode {
    pub fn as_str(self) -> &'static str {
        match self {
            MemoryMode::Auto => "auto",
            MemoryMode::Low => "low",
            MemoryMode::Balanced => "balanced",
            MemoryMode::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EffectiveMemoryMode {
    Low,
    Balanced,
    High,
}

impl EffectiveMemoryMode {
    pub fn as_str(self) -> &'static str {
        match self {
            EffectiveMemoryMode::Low => "low",
            EffectiveMemoryMode::Balanced => "balanced",
            EffectiveMemoryMode::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ChunkStorageMode {
    Memory,
    TempSpill,
}

impl ChunkStorageMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ChunkStorageMode::Memory => "memory",
            ChunkStorageMode::TempSpill => "temp-spill",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeResourceHints {
    pub memory_mode: MemoryMode,
    pub max_memory_mib: Option<u64>,
    pub jobs: Option<usize>,
}

impl Default for RuntimeResourceHints {
    fn default() -> Self {
        Self {
            memory_mode: MemoryMode::Auto,
            max_memory_mib: None,
            jobs: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimePlan {
    pub requested_memory_mode: MemoryMode,
    pub effective_memory_mode: EffectiveMemoryMode,
    pub available_memory_mib: Option<u64>,
    pub memory_budget_mib: u64,
    pub worker_count: usize,
    pub chunk_storage_mode: ChunkStorageMode,
}

pub fn plan_build_runtime(hints: &RuntimeResourceHints, compression_unit: &str) -> RuntimePlan {
    plan_runtime(hints, RuntimeWorkload::Build, compression_unit)
}

pub fn plan_verify_runtime(hints: &RuntimeResourceHints) -> RuntimePlan {
    plan_runtime(
        hints,
        RuntimeWorkload::Verify,
        crate::manifest::COMPRESSION_UNIT_CHUNK,
    )
}

pub fn plan_reconstruct_runtime(hints: &RuntimeResourceHints) -> RuntimePlan {
    plan_runtime(
        hints,
        RuntimeWorkload::Reconstruct,
        crate::manifest::COMPRESSION_UNIT_CHUNK,
    )
}

fn plan_runtime(
    hints: &RuntimeResourceHints,
    workload: RuntimeWorkload,
    compression_unit: &str,
) -> RuntimePlan {
    let available_memory_mib = hints.max_memory_mib.or_else(detect_available_memory_mib);
    let effective_memory_mode =
        resolve_effective_memory_mode(hints.memory_mode, available_memory_mib);
    let available_workers = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .max(1);

    let worker_count = hints.jobs.unwrap_or_else(|| match workload {
        RuntimeWorkload::Build => match effective_memory_mode {
            EffectiveMemoryMode::Low => available_workers.min(2),
            EffectiveMemoryMode::Balanced => available_workers.min(4),
            EffectiveMemoryMode::High => available_workers,
        },
        RuntimeWorkload::Verify => match effective_memory_mode {
            EffectiveMemoryMode::Low => 1,
            EffectiveMemoryMode::Balanced => available_workers.min(3),
            EffectiveMemoryMode::High => available_workers.min(6),
        },
        RuntimeWorkload::Reconstruct => match effective_memory_mode {
            EffectiveMemoryMode::Low => 1,
            EffectiveMemoryMode::Balanced => available_workers.min(2),
            EffectiveMemoryMode::High => available_workers.min(4),
        },
    });
    let worker_count = worker_count.max(1).min(available_workers);

    let memory_budget_mib = hints.max_memory_mib.unwrap_or_else(|| match effective_memory_mode {
        EffectiveMemoryMode::Low => available_memory_mib.map(|mib| mib.min(2048)).unwrap_or(2048),
        EffectiveMemoryMode::Balanced => {
            available_memory_mib.map(|mib| mib.min(8192)).unwrap_or(8192)
        }
        EffectiveMemoryMode::High => {
            available_memory_mib.map(|mib| mib.min(32768)).unwrap_or(32768)
        }
    });

    let chunk_storage_mode = match effective_memory_mode {
        EffectiveMemoryMode::High if compression_unit != crate::manifest::COMPRESSION_UNIT_BUNDLE => {
            ChunkStorageMode::Memory
        }
        _ => ChunkStorageMode::TempSpill,
    };

    RuntimePlan {
        requested_memory_mode: hints.memory_mode,
        effective_memory_mode,
        available_memory_mib,
        memory_budget_mib,
        worker_count,
        chunk_storage_mode,
    }
}

pub fn install_in_thread_pool<T: Send, F: FnOnce() -> T + Send>(
    worker_count: usize,
    work: F,
) -> Result<T> {
    if worker_count <= 1 {
        return Ok(work());
    }

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(worker_count)
        .build()
        .map_err(|err| Error::manifest(format!("failed to create thread pool: {}", err)))?;
    Ok(pool.install(work))
}

#[derive(Clone, Copy)]
enum RuntimeWorkload {
    Build,
    Verify,
    Reconstruct,
}

fn resolve_effective_memory_mode(
    requested_mode: MemoryMode,
    available_memory_mib: Option<u64>,
) -> EffectiveMemoryMode {
    match requested_mode {
        MemoryMode::Low => EffectiveMemoryMode::Low,
        MemoryMode::Balanced => EffectiveMemoryMode::Balanced,
        MemoryMode::High => EffectiveMemoryMode::High,
        MemoryMode::Auto => match available_memory_mib {
            Some(mib) if mib <= 4 * 1024 => EffectiveMemoryMode::Low,
            Some(mib) if mib <= 16 * 1024 => EffectiveMemoryMode::Balanced,
            Some(_) => EffectiveMemoryMode::High,
            None => EffectiveMemoryMode::Balanced,
        },
    }
}

fn detect_available_memory_mib() -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        return detect_available_memory_mib_windows();
    }

    #[cfg(target_os = "linux")]
    {
        return detect_available_memory_mib_linux();
    }

    #[allow(unreachable_code)]
    None
}

#[cfg(target_os = "windows")]
fn detect_available_memory_mib_windows() -> Option<u64> {
    #[repr(C)]
    struct MemoryStatusEx {
        dw_length: u32,
        dw_memory_load: u32,
        ull_total_phys: u64,
        ull_avail_phys: u64,
        ull_total_page_file: u64,
        ull_avail_page_file: u64,
        ull_total_virtual: u64,
        ull_avail_virtual: u64,
        ull_avail_extended_virtual: u64,
    }

    unsafe extern "system" {
        fn GlobalMemoryStatusEx(lp_buffer: *mut MemoryStatusEx) -> i32;
    }

    let mut status = MemoryStatusEx {
        dw_length: std::mem::size_of::<MemoryStatusEx>() as u32,
        dw_memory_load: 0,
        ull_total_phys: 0,
        ull_avail_phys: 0,
        ull_total_page_file: 0,
        ull_avail_page_file: 0,
        ull_total_virtual: 0,
        ull_avail_virtual: 0,
        ull_avail_extended_virtual: 0,
    };
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    if ok == 0 {
        None
    } else {
        Some(status.ull_avail_phys / (1024 * 1024))
    }
}

#[cfg(target_os = "linux")]
fn detect_available_memory_mib_linux() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in meminfo.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let kib = rest
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<u64>().ok())?;
            return Some(kib / 1024);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_mode_uses_low_threshold() {
        let plan = plan_build_runtime(
            &RuntimeResourceHints {
                memory_mode: MemoryMode::Auto,
                max_memory_mib: Some(2048),
                jobs: None,
            },
            crate::manifest::COMPRESSION_UNIT_CHUNK,
        );
        assert_eq!(plan.effective_memory_mode, EffectiveMemoryMode::Low);
        assert_eq!(plan.chunk_storage_mode, ChunkStorageMode::TempSpill);
    }

    #[test]
    fn high_mode_prefers_memory_for_chunk_compression() {
        let plan = plan_build_runtime(
            &RuntimeResourceHints {
                memory_mode: MemoryMode::High,
                max_memory_mib: Some(32768),
                jobs: Some(8),
            },
            crate::manifest::COMPRESSION_UNIT_CHUNK,
        );
        assert_eq!(plan.effective_memory_mode, EffectiveMemoryMode::High);
        assert_eq!(plan.chunk_storage_mode, ChunkStorageMode::Memory);
        assert_eq!(plan.worker_count, 8);
    }
}
