//! A desktop app that keeps Gutch repositories in an S3-compatible bucket:
//! create a repository there, capture a folder as a snapshot and push it,
//! then install, update, verify or repair a local copy of any snapshot.
//!
//! Run with `cargo run --release` in this folder. See README.md.

mod app;
mod cloudflare;
mod ops;
mod provider;
mod store;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([1000.0, 680.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Gutch S3",
        options,
        Box::new(|creation| Ok(Box::new(app::App::new(creation.storage)))),
    )
}
