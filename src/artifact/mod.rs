//! Repository-independent chunking, packaging profiles, compression, and artifact codecs.

pub(crate) mod bundle;
pub(crate) mod chunk;
pub(crate) mod compression;
pub(crate) mod format;
pub(crate) mod hash;
pub(crate) mod integrity;
pub(crate) mod loose;
pub(crate) mod metadata;
pub(crate) mod profiles;
pub(crate) mod reference;

pub(crate) mod binary_io;

pub(crate) mod chunk_stream;

mod fastcdc_v2020;

#[cfg(test)]
#[path = "../../tests/benchmarks/cdc_experiment.rs"]
mod cdc_experiment;
mod gear;
mod jc;
#[cfg(test)]
mod jc_tests;

mod ram;

#[cfg(test)]
#[path = "../../tests/benchmarks/ram_diagnosis.rs"]
mod ram_diagnosis;
