//! Gutch Launcher Sample: installs official Blender builds side by side from
//! a public Gutch repository, then launches, verifies, repairs, moves and
//! uninstalls them. Not affiliated with the Blender Foundation.
//!
//! Run with `cargo run --release -p gutch-launcher-sample`. See README.md.

mod app;
mod library;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([860.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Gutch Launcher Sample",
        options,
        Box::new(|_| Ok(Box::new(app::App::new()))),
    )
}
