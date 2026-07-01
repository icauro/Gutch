use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;

use gutch::{GutchOperation, GutchProgressEvent, GutchProgressSink};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

/// Unified CLI progress sink that handles all gutch operations via indicatif.
#[derive(Debug)]
pub(crate) struct CliProgressSink {
    bar: ProgressBar,
}

impl CliProgressSink {
    pub(crate) fn new() -> Arc<Self> {
        let bar = if crate::cli::verbose_output_enabled() || !std::io::stderr().is_terminal() {
            ProgressBar::hidden()
        } else {
            ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr())
        };
        bar.set_style(Self::spinner_style());
        bar.set_message("preparing...");
        bar.enable_steady_tick(Duration::from_millis(100));
        Arc::new(Self { bar })
    }

    fn spinner_style() -> ProgressStyle {
        ProgressStyle::with_template("  {spinner} {msg} [{elapsed_precise}]")
            .expect("valid spinner template")
    }

    fn bar_style() -> ProgressStyle {
        ProgressStyle::with_template(
            "  [{bar:40.cyan/blue}] {pos:>4}/{len:<4} {msg} [{elapsed_precise}]",
        )
        .expect("valid progress template")
        .progress_chars("##-")
    }
}

impl GutchProgressSink for CliProgressSink {
    fn emit(&self, event: GutchProgressEvent) {
        if event.done {
            self.bar.finish_and_clear();
            return;
        }

        let label = match event.operation {
            GutchOperation::TargetInsert => "insert",
            GutchOperation::SyncUp => "sync",
            GutchOperation::TargetSourcesSync => "sources",
            GutchOperation::Fetch => "fetch",
        };

        if event.total > 0 {
            self.bar.set_style(Self::bar_style());
            self.bar.set_length(event.total as u64);
            self.bar.set_position(event.current as u64);
            self.bar.set_message(format!("[{}] {}", label, event.message));
            if event.current == 0 {
                self.bar.reset_elapsed();
            }
        } else {
            self.bar.set_style(Self::spinner_style());
            self.bar.set_message(format!("[{}] {}", label, event.message));
        }
    }
}

impl Drop for CliProgressSink {
    fn drop(&mut self) {
        self.bar.finish_and_clear();
    }
}

// Backward-compatible wrapper for BuildProgressSink trait used by build.rs
use gutch::BuildProgressSink;

#[derive(Debug)]
pub(crate) struct TargetInsertProgress {
    inner: Arc<CliProgressSink>,
}

impl TargetInsertProgress {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: CliProgressSink::new(),
        })
    }

    pub(crate) fn as_build_sink(self: &Arc<Self>) -> Arc<dyn BuildProgressSink> {
        self.clone()
    }

    pub(crate) fn as_progress_sink(self: &Arc<Self>) -> Arc<dyn GutchProgressSink> {
        self.inner.clone()
    }
}

impl BuildProgressSink for TargetInsertProgress {
    fn on_file_scan_started(&self, total_files: usize) {
        self.inner.emit(GutchProgressEvent {
            operation: GutchOperation::TargetInsert,
            phase: "scan".to_string(),
            message: "Scanning files".to_string(),
            current: 0,
            total: total_files,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        });
    }

    fn on_file_scanned(&self) {
        // Coarse progress: we report phase transitions, not per-file ticks.
    }

    fn on_bundle_build_started(&self, total_bundles: usize) {
        self.inner.emit(GutchProgressEvent {
            operation: GutchOperation::TargetInsert,
            phase: "build".to_string(),
            message: "Building bundles".to_string(),
            current: 0,
            total: total_bundles,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        });
    }

    fn on_bundle_built(&self) {
        // Coarse progress: we report phase transitions, not per-bundle ticks.
    }
}
