//! Per-run CLI settings: verbose output, the operation profile and its control.

use crate::profiling::Profiler;
use crate::progress::OperationControl;
use crate::progress::OperationWarning;
use crate::progress::WarningSink;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

pub(crate) type Profile = Option<Arc<Profiler>>;

static VERBOSE_OUTPUT_ENABLED: AtomicBool = AtomicBool::new(false);
pub(crate) const SECTION_SEPARATOR: &str = "================================";

pub(crate) fn set_verbose_output_enabled(enabled: bool) {
    VERBOSE_OUTPUT_ENABLED.store(enabled, Ordering::Relaxed);
}

pub(crate) fn verbose_output_enabled() -> bool {
    VERBOSE_OUTPUT_ENABLED.load(Ordering::Relaxed)
}

/// Reports engine warnings on stderr with their structured details.
#[derive(Debug)]
struct StderrWarnings;

impl WarningSink for StderrWarnings {
    fn warn(&self, warning: OperationWarning) {
        use std::io::Write;
        // A closed stderr must never turn a successful operation into an error.
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "Warning: {warning}");
        let _ = serde_json::to_writer(&mut stderr, &warning);
        let _ = writeln!(stderr);
    }
}

pub(crate) fn control_with_profile(profile: &Profile) -> OperationControl {
    let control = OperationControl::default().with_warnings(Arc::new(StderrWarnings));
    match profile {
        Some(profiler) => control.with_profiler(Arc::clone(profiler)),
        None => control,
    }
}
