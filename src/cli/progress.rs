use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use gutch::{GutchOperation, GutchProgressEvent, GutchProgressPhase, GutchProgressSink};
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
            GutchOperation::SnapshotCreate => "create",
            GutchOperation::SnapshotDelete => "delete",
            GutchOperation::SyncUp => "push",
            GutchOperation::Pull => "pull",
            GutchOperation::SnapshotSourcesSync => "sources",
            GutchOperation::Fetch => "fetch",
            GutchOperation::Verify => "verify",
        };

        if event.total > 0 {
            self.bar.set_style(Self::bar_style());
            self.bar.set_length(event.total as u64);
            self.bar.set_position(event.current as u64);
            self.bar
                .set_message(format!("[{}] {}", label, event.message));
            if event.current == 0 {
                self.bar.reset_elapsed();
            }
        } else {
            self.bar.set_style(Self::spinner_style());
            self.bar
                .set_message(format!("[{}] {}", label, event.message));
        }
    }
}

impl Drop for CliProgressSink {
    fn drop(&mut self) {
        self.bar.finish_and_clear();
    }
}

use gutch::BuildProgressSink;

#[derive(Debug)]
pub(crate) struct CliBuildProgressSink {
    inner: Arc<CliProgressSink>,
    scanned_files: AtomicUsize,
    total_files: AtomicUsize,
    built_bundles: AtomicUsize,
    total_bundles: AtomicUsize,
}

impl CliBuildProgressSink {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: CliProgressSink::new(),
            scanned_files: AtomicUsize::new(0),
            total_files: AtomicUsize::new(0),
            built_bundles: AtomicUsize::new(0),
            total_bundles: AtomicUsize::new(0),
        })
    }

    pub(crate) fn as_build_sink(self: &Arc<Self>) -> Arc<dyn BuildProgressSink> {
        self.clone()
    }
}

impl BuildProgressSink for CliBuildProgressSink {
    fn on_file_scan_started(&self, total_files: usize) {
        self.scanned_files.store(0, Ordering::Relaxed);
        self.total_files.store(total_files, Ordering::Relaxed);
        self.inner.emit(GutchProgressEvent {
            operation: GutchOperation::SnapshotCreate,
            phase: GutchProgressPhase::Scan,
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
        let current = self.scanned_files.fetch_add(1, Ordering::Relaxed) + 1;
        self.inner.emit(GutchProgressEvent {
            operation: GutchOperation::SnapshotCreate,
            phase: GutchProgressPhase::Scan,
            message: "Scanning files".to_string(),
            current,
            total: self.total_files.load(Ordering::Relaxed),
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        });
    }

    fn on_bundle_build_started(&self, total_bundles: usize) {
        self.built_bundles.store(0, Ordering::Relaxed);
        self.total_bundles.store(total_bundles, Ordering::Relaxed);
        self.inner.emit(GutchProgressEvent {
            operation: GutchOperation::SnapshotCreate,
            phase: GutchProgressPhase::Build,
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
        let current = self.built_bundles.fetch_add(1, Ordering::Relaxed) + 1;
        self.inner.emit(GutchProgressEvent {
            operation: GutchOperation::SnapshotCreate,
            phase: GutchProgressPhase::Build,
            message: "Building bundles".to_string(),
            current,
            total: self.total_bundles.load(Ordering::Relaxed),
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        });
    }
}
