use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;

use crate::api::GutchOperation;
use crate::api::GutchProgressEvent;
use crate::api::GutchProgressSink;
use indicatif::ProgressBar;
use indicatif::ProgressDrawTarget;
use indicatif::ProgressStyle;

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
            GutchOperation::SnapshotCapture => "capture",
            GutchOperation::SnapshotDelete => "delete",
            GutchOperation::SyncUp => "push",
            GutchOperation::Pull => "pull",
            GutchOperation::SnapshotSourcesCapture => "capture-current",
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
