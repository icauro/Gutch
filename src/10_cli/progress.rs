//! Terminal progress rendering.

use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;

use crate::cli::reporting::MIB;
use crate::cli::reporting::mib;
use crate::cli::verbose_output_enabled;
use crate::progress::LiveFeedback;
use crate::progress::OperationKind;
use crate::progress::ProgressEvent;
use crate::progress::ProgressSink;
use indicatif::ProgressBar;
use indicatif::ProgressDrawTarget;
use indicatif::ProgressStyle;
use std::io;
use std::sync::Mutex;

/// Unified CLI progress sink that handles all gutch operations via indicatif.
#[derive(Debug)]
pub(crate) struct CliProgressSink {
    bar: ProgressBar,
    latest: Mutex<Option<LiveFeedback>>,
}

impl CliProgressSink {
    pub(crate) fn new() -> Arc<Self> {
        let bar = if verbose_output_enabled() || !io::stderr().is_terminal() {
            ProgressBar::hidden()
        } else {
            ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr())
        };
        bar.set_style(Self::spinner_style());
        bar.set_message("preparing...");
        bar.enable_steady_tick(Duration::from_millis(100));
        Arc::new(Self {
            bar,
            latest: Mutex::new(None),
        })
    }

    fn spinner_style() -> ProgressStyle {
        ProgressStyle::with_template("  {spinner} {wide_msg} [{elapsed_precise}]")
            .expect("valid spinner template")
    }

    fn bar_style() -> ProgressStyle {
        ProgressStyle::with_template(
            "  [{bar:20.cyan/blue}] {pos:>4}/{len:<4} {wide_msg} [{elapsed_precise}]",
        )
        .expect("valid progress template")
        .progress_chars("##-")
    }
}

impl ProgressSink for CliProgressSink {
    fn emit(&self, event: ProgressEvent) {
        let rates = event
            .feedback
            .as_ref()
            .map(render_feedback)
            .unwrap_or_default();
        if let Some(feedback) = &event.feedback
            && let Ok(mut latest) = self.latest.lock()
        {
            *latest = Some(feedback.clone());
        }
        if event.done {
            self.bar.finish_and_clear();
            return;
        }

        let label = match event.operation {
            OperationKind::SnapshotCapture => "capture",
            OperationKind::SnapshotDelete => "delete",
            OperationKind::Push => "push",
            OperationKind::Pull => "pull",
            OperationKind::SnapshotSourcesCapture => "capture-current",
            OperationKind::Fetch => "fetch",
            OperationKind::Verify => "verify",
        };

        if event.total > 0 {
            self.bar.set_style(Self::bar_style());
            self.bar.set_length(event.total as u64);
            self.bar.set_position(event.current as u64);
            self.bar
                .set_message(format!("[{}] {} {}", label, event.message, rates));
            if event.current == 0 {
                self.bar.reset_elapsed();
            }
        } else {
            self.bar.set_style(Self::spinner_style());
            self.bar
                .set_message(format!("[{}] {} {}", label, event.message, rates));
        }
    }
}

impl Drop for CliProgressSink {
    fn drop(&mut self) {
        self.bar.finish_and_clear();
        if let Ok(latest) = self.latest.lock()
            && let Some(feedback) = latest.as_ref()
        {
            let text = render_feedback(feedback);
            if !text.is_empty() {
                eprintln!("{text}");
            }
        }
    }
}

fn render_feedback(feedback: &LiveFeedback) -> String {
    let rate = |value: Option<f64>| {
        value
            .map(|n| format!("{:.1}", n / MIB))
            .unwrap_or_else(|| "--".into())
    };
    [
        ("Download", &feedback.download),
        ("Output write", &feedback.output_write),
    ]
    .into_iter()
    .filter_map(|(name, value)| {
        value.as_ref().map(|v| {
            format!(
                "{name}: {:.1} MiB, {} MiB/s (avg {})",
                mib(v.bytes),
                rate(v.recent_bytes_per_second),
                rate(v.average_bytes_per_second)
            )
        })
    })
    .collect::<Vec<_>>()
    .join(" | ")
}
