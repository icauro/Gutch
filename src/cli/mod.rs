pub(crate) mod commands;
pub(crate) mod common;
pub(crate) mod interactive;
pub(crate) mod path_compare;
pub(crate) mod progress;
pub(crate) mod repo;
pub(crate) mod reporting;
pub(crate) mod target;
pub(crate) mod target_install;

use std::sync::atomic::{AtomicBool, Ordering};

static VERBOSE_OUTPUT_ENABLED: AtomicBool = AtomicBool::new(false);
pub(crate) const SECTION_SEPARATOR: &str = "================================";

pub(crate) fn set_verbose_output_enabled(enabled: bool) {
    VERBOSE_OUTPUT_ENABLED.store(enabled, Ordering::Relaxed);
}

pub(crate) fn verbose_output_enabled() -> bool {
    VERBOSE_OUTPUT_ENABLED.load(Ordering::Relaxed)
}
