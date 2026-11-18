//! What a running job shows: its step, how far it got, and its network and
//! disk speeds over the last half minute. Gutch downloads and writes files
//! at the same time, so both speeds can be busy at once.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use eframe::egui;
use gutch::api::{LiveFeedback, ProgressEvent, ProgressPhase, Throughput};

/// How often the speed graph takes a sample, and how many it keeps.
const SAMPLE_EVERY: Duration = Duration::from_millis(250);
const SAMPLES: usize = 120;

const NETWORK_COLOR: egui::Color32 = egui::Color32::from_rgb(86, 156, 255);
const DISK_COLOR: egui::Color32 = egui::Color32::from_rgb(110, 200, 120);

pub struct Progress {
    /// Bytes the job writes in all, when known.
    expected_bytes: Option<u64>,
    last: Option<ProgressEvent>,
    /// Downloads done and needed, as the last download event counted them.
    parts: Option<(usize, usize)>,
    feedback: LiveFeedback,
    /// Recent network and disk speeds, in bytes per second, oldest first.
    history: VecDeque<[f64; 2]>,
    sampled_at: Option<Instant>,
}

impl Progress {
    pub fn new(expected_bytes: Option<u64>) -> Self {
        Self {
            expected_bytes,
            last: None,
            parts: None,
            feedback: LiveFeedback::default(),
            history: VecDeque::new(),
            sampled_at: None,
        }
    }

    pub fn record(&mut self, event: ProgressEvent) {
        if event.phase == ProgressPhase::Artifact && event.total > 0 {
            self.parts = Some((event.current, event.total));
        }
        if let Some(feedback) = &event.feedback {
            self.feedback = feedback.clone();
            let now = Instant::now();
            if self
                .sampled_at
                .is_none_or(|at| now.duration_since(at) >= SAMPLE_EVERY)
            {
                self.sampled_at = Some(now);
                self.history.push_back([
                    speed_of(&feedback.download),
                    speed_of(&feedback.output_write),
                ]);
                if self.history.len() > SAMPLES {
                    self.history.pop_front();
                }
            }
        }
        self.last = Some(event);
    }

    fn written(&self) -> Option<u64> {
        self.feedback.output_write.as_ref().map(|write| write.bytes)
    }

    fn fraction(&self) -> Option<f32> {
        if let (Some(expected), Some(written)) = (self.expected_bytes, self.written())
            && expected > 0
        {
            return Some((written as f64 / expected as f64).min(1.0) as f32);
        }
        let event = self.last.as_ref()?;
        if event.total_bytes > 0 {
            Some(event.transferred_bytes as f32 / event.total_bytes as f32)
        } else if event.total > 0 {
            Some(event.current as f32 / event.total as f32)
        } else {
            None
        }
    }

    /// The step in plain words.
    pub fn step(&self, title: &str) -> String {
        let busy = |stream: &Option<Throughput>| stream.as_ref().is_some_and(|s| !s.finished);
        match (
            busy(&self.feedback.download),
            busy(&self.feedback.output_write),
        ) {
            (true, true) => return "Downloading and writing files".into(),
            (true, false) => return "Downloading".into(),
            (false, true) => return "Writing files".into(),
            (false, false) => {}
        }
        let Some(event) = &self.last else {
            return format!("{title}…");
        };
        match event.phase {
            ProgressPhase::Prepare
            | ProgressPhase::RepoMetadata
            | ProgressPhase::SnapshotMetadata
            | ProgressPhase::ChunkIndex
            | ProgressPhase::Planning => "Preparing".into(),
            ProgressPhase::Artifact => "Downloading".into(),
            ProgressPhase::Reconstruct => "Writing files".into(),
            ProgressPhase::Verify | ProgressPhase::Scan => "Checking files".into(),
            _ => event.message.clone(),
        }
    }

    /// Amounts and time left, such as "Downloaded 412 MB (63 of 91 parts) ·
    /// Written 600 MB of 993 MB · about 1 min left".
    fn amounts(&self) -> String {
        let mut parts = Vec::new();
        if let Some(download) = &self.feedback.download {
            let mut text = format!("Downloaded {}", size(download.bytes));
            if let Some((done, needed)) = self.parts {
                text.push_str(&format!(" ({done} of {needed} parts)"));
            }
            parts.push(text);
        }
        if let Some(written) = self.written() {
            parts.push(match self.expected_bytes {
                Some(expected) => format!("Written {} of {}", size(written), size(expected)),
                None => format!("Written {}", size(written)),
            });
            let rate = self
                .feedback
                .output_write
                .as_ref()
                .and_then(|write| write.recent_bytes_per_second)
                .unwrap_or(0.0);
            if let Some(expected) = self.expected_bytes
                && rate > 0.0
                && written < expected
            {
                parts.push(time_left((expected - written) as f64 / rate));
            }
        }
        parts.join(" · ")
    }

    /// The full view, for the row the job works on.
    pub fn show(&self, ui: &mut egui::Ui, title: &str) {
        ui.strong(self.step(title));
        ui.add(bar(self.fraction()));
        let amounts = self.amounts();
        if !amounts.is_empty() {
            ui.weak(amounts);
        }
        if self.feedback.download.is_some() || self.feedback.output_write.is_some() {
            let latest = self.history.back().copied().unwrap_or_default();
            ui.horizontal(|ui| {
                legend(ui, NETWORK_COLOR, &format!("Network {}", speed(latest[0])));
                ui.add_space(12.0);
                legend(ui, DISK_COLOR, &format!("Disk {}", speed(latest[1])));
            });
            self.graph(ui);
        }
    }

    /// One line, for the status bar.
    pub fn show_compact(&self, ui: &mut egui::Ui, title: &str) {
        ui.add(bar(self.fraction()).text(self.step(title)));
    }

    fn graph(&self, ui: &mut egui::Ui) {
        let size = egui::vec2(ui.available_width(), 56.0);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        let top = self
            .history
            .iter()
            .flatten()
            .copied()
            .fold(1_000_000.0, f64::max);
        let step = rect.width() / (SAMPLES - 1) as f32;
        let start = SAMPLES - self.history.len();
        for (series, color) in [(0, NETWORK_COLOR), (1, DISK_COLOR)] {
            let points: Vec<_> = self
                .history
                .iter()
                .enumerate()
                .map(|(index, sample)| {
                    let x = rect.left() + (start + index) as f32 * step;
                    let y =
                        rect.bottom() - 2.0 - (sample[series] / top) as f32 * (rect.height() - 4.0);
                    egui::pos2(x, y)
                })
                .collect();
            if points.len() > 1 {
                painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, color)));
            }
        }
    }
}

fn bar(fraction: Option<f32>) -> egui::ProgressBar {
    match fraction {
        Some(fraction) => egui::ProgressBar::new(fraction).show_percentage(),
        None => egui::ProgressBar::new(0.0).animate(true),
    }
}

fn legend(ui: &mut egui::Ui, color: egui::Color32, text: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, color);
    ui.label(text);
}

/// A stream's recent speed; a finished stream is idle.
fn speed_of(stream: &Option<Throughput>) -> f64 {
    stream
        .as_ref()
        .filter(|stream| !stream.finished)
        .and_then(|stream| stream.recent_bytes_per_second)
        .unwrap_or(0.0)
}

/// Sizes in decimal units, as file managers on both systems show them.
pub fn size(bytes: u64) -> String {
    let bytes = bytes as f64;
    if bytes >= 1e9 {
        format!("{:.2} GB", bytes / 1e9)
    } else {
        format!("{:.0} MB", bytes / 1e6)
    }
}

fn speed(bytes_per_second: f64) -> String {
    format!("{:.1} MB/s", bytes_per_second / 1e6)
}

fn time_left(seconds: f64) -> String {
    let seconds = seconds.ceil() as u64;
    if seconds < 60 {
        format!("about {seconds} s left")
    } else {
        format!("about {} min left", seconds.div_ceil(60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_times_read_naturally() {
        assert_eq!(size(992_912_801), "993 MB");
        assert_eq!(size(1_292_440_678), "1.29 GB");
        assert_eq!(speed(18_240_000.0), "18.2 MB/s");
        assert_eq!(time_left(42.2), "about 43 s left");
        assert_eq!(time_left(61.0), "about 2 min left");
    }
}
