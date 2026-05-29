use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;

use gutch::BuildProgressSink;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

#[derive(Debug)]
pub(crate) struct TargetInsertProgress {
    bar: ProgressBar,
}

impl TargetInsertProgress {
    pub(crate) fn new() -> Arc<Self> {
        let bar = if crate::cli::verbose_output_enabled() || !std::io::stderr().is_terminal() {
            ProgressBar::hidden()
        } else {
            ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr())
        };
        bar.set_style(Self::spinner_style());
        bar.set_message("prepare target insert");
        bar.enable_steady_tick(Duration::from_millis(100));
        Arc::new(Self { bar })
    }

    pub(crate) fn as_sink(self: &Arc<Self>) -> Arc<dyn BuildProgressSink> {
        self.clone()
    }

    pub(crate) fn set_message(&self, message: impl Into<String>) {
        self.bar.set_style(Self::spinner_style());
        self.bar.set_message(message.into());
    }

    pub(crate) fn finish_and_clear(&self) {
        self.bar.finish_and_clear();
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

impl BuildProgressSink for TargetInsertProgress {
    fn on_file_scan_started(&self, total_files: usize) {
        self.bar.set_style(Self::bar_style());
        self.bar.set_length(total_files as u64);
        self.bar.set_position(0);
        self.bar.set_message("scan files");
        self.bar.reset_elapsed();
    }

    fn on_file_scanned(&self) {
        self.bar.inc(1);
    }

    fn on_bundle_build_started(&self, total_bundles: usize) {
        self.bar.set_style(Self::bar_style());
        self.bar.set_length(total_bundles as u64);
        self.bar.set_position(0);
        self.bar.set_message("build bundles");
        self.bar.reset_elapsed();
    }

    fn on_bundle_built(&self) {
        self.bar.inc(1);
    }
}

impl Drop for TargetInsertProgress {
    fn drop(&mut self) {
        self.bar.finish_and_clear();
    }
}
