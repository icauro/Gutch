//! Gutch Launcher Sample: installs official Blender builds side by side from
//! a public Gutch repository, then launches, verifies, repairs, moves and
//! uninstalls them. Not affiliated with the Blender Foundation.
//!
//! Run with `cargo run --release -p gutch-launcher-sample`. See README.md.

// Release builds open no console window on Windows; debug builds keep it
// for panic messages.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod library;

const TITLE: &str = "Gutch Launcher Sample";

fn main() -> eframe::Result {
    let library = match library::Library::for_current_user(&app::App::bucket_url()) {
        Ok(library) => library,
        Err(error) => {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title(TITLE)
                .set_description(format!("Cannot start: {error}."))
                .show();
            std::process::exit(1);
        }
    };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([860.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        TITLE,
        options,
        Box::new(|_| Ok(Box::new(app::App::new(library)))),
    )
}
