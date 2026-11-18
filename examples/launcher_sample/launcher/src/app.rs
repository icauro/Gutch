//! The window. Downloads and disk work run on a worker thread; the window
//! reads progress and the result from a channel.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use eframe::egui;
use gutch::api::{CancelFlag, OperationControl, ProgressEvent};
use launcher_catalog::{Catalog, CatalogEntry, Os};

use crate::library::{self, DEFAULT_BUCKET_URL, Installed, Library, Result, State};

const GUTCH_URL: &str = "https://github.com/icauro/Gutch";

/// What a finished job changes.
enum Change {
    None,
    Catalog(Catalog),
    Add(Installed),
    /// The install at the old path moved.
    Move(PathBuf, PathBuf),
    /// The install at this path is gone.
    Remove(PathBuf),
}

struct Done {
    message: String,
    change: Change,
}

enum Update {
    Progress(ProgressEvent),
    Done(Result<Done>),
}

struct Job {
    title: &'static str,
    cancel: CancelFlag,
    updates: Receiver<Update>,
    last_event: Option<ProgressEvent>,
}

pub struct App {
    library: Library,
    os: Option<Os>,
    state: State,
    catalog: Option<Catalog>,
    job: Option<Job>,
    status: String,
    status_is_error: bool,
    /// The install waiting for the user to confirm its removal.
    confirm_uninstall: Option<Installed>,
    show_about: bool,
}

impl App {
    /// The bucket comes from `LAUNCHER_BUCKET_URL`, or the sample's own.
    pub fn bucket_url() -> String {
        std::env::var("LAUNCHER_BUCKET_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BUCKET_URL.to_owned())
    }

    pub fn new(library: Library) -> Self {
        let (state, status, status_is_error) = match library.load_state() {
            Ok(state) => (state, String::new(), false),
            Err(error) => (State::default(), error, true),
        };
        Self {
            library,
            os: Os::current(),
            state,
            catalog: None,
            job: None,
            status,
            status_is_error,
            confirm_uninstall: None,
            show_about: false,
        }
    }

    /// Runs `work` on a worker thread with progress reporting and a cancel flag.
    fn spawn(
        &mut self,
        ctx: &egui::Context,
        title: &'static str,
        work: impl FnOnce(&Library, OperationControl) -> Result<Done> + Send + 'static,
    ) {
        let (sender, updates) = channel();
        let cancel = CancelFlag::default();
        let progress_sender = sender.clone();
        let progress_ctx = ctx.clone();
        let control = OperationControl::default()
            .on_progress(move |event| {
                let _ = progress_sender.send(Update::Progress(event));
                progress_ctx.request_repaint();
            })
            .with_cancel_flag(&cancel);
        let library = self.library.clone();
        let done_ctx = ctx.clone();
        thread::spawn(move || {
            let _ = sender.send(Update::Done(work(&library, control)));
            done_ctx.request_repaint();
        });
        self.job = Some(Job {
            title,
            cancel,
            updates,
            last_event: None,
        });
    }

    fn refresh(&mut self, ctx: &egui::Context) {
        self.spawn(ctx, "Refresh", |library, _| {
            let catalog = library.fetch_catalog()?;
            Ok(Done {
                message: format!("{} builds published", catalog.versions.len()),
                change: Change::Catalog(catalog),
            })
        });
    }

    fn receive_updates(&mut self) {
        let Some(job) = &mut self.job else {
            return;
        };
        let mut finished = None;
        for update in job.updates.try_iter() {
            match update {
                Update::Progress(event) => job.last_event = Some(event),
                Update::Done(result) => finished = Some(result),
            }
        }
        let Some(result) = finished else {
            return;
        };
        let title = job.title;
        self.job = None;
        match result {
            Ok(done) => {
                self.apply(done.change);
                self.report(Ok(format!("{title}: {}", done.message)));
            }
            Err(error) => self.report(Err(format!("{title}: {error}"))),
        }
    }

    fn apply(&mut self, change: Change) {
        let installs = &mut self.state.installs;
        match change {
            Change::None => return,
            Change::Catalog(catalog) => {
                self.catalog = Some(catalog);
                return;
            }
            Change::Add(installed) => installs.push(installed),
            Change::Move(old, new) => {
                if let Some(installed) = installs.iter_mut().find(|item| item.path == old) {
                    installed.path = new;
                }
            }
            Change::Remove(path) => installs.retain(|installed| installed.path != path),
        }
        installs.sort_by(|left, right| {
            launcher_catalog::compare_versions(&right.version, &left.version)
        });
        if let Err(error) = self.library.save_state(&self.state) {
            self.report(Err(error));
        }
    }

    fn report(&mut self, result: Result<String>) {
        self.status_is_error = result.is_err();
        self.status = result.unwrap_or_else(|error| error);
    }

    fn installed_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Installed");
        if self.state.installs.is_empty() {
            ui.label("Nothing installed yet.");
            return;
        }
        let published = |installed: &Installed| {
            self.catalog.as_ref().is_none_or(|catalog| {
                catalog
                    .versions
                    .iter()
                    .any(|entry| entry.snapshot_id == installed.snapshot_id)
            })
        };
        let mut action = None;
        egui::Grid::new("installed")
            .num_columns(3)
            .striped(true)
            .show(ui, |ui| {
                for installed in &self.state.installs {
                    let label = ui.strong(&installed.version);
                    if !published(installed) {
                        label.on_hover_text(
                            "No longer published. It still runs, but a repair may \
                             need a newer version.",
                        );
                    }
                    ui.label(installed.path.display().to_string());
                    ui.horizontal(|ui| {
                        for (name, tip) in [
                            ("Launch", "Start this version."),
                            ("Verify", "Compare the files with the published build."),
                            ("Repair", "Restore missing and changed files."),
                            ("Move…", "Move the folder somewhere else."),
                            ("Uninstall", "Delete the folder."),
                        ] {
                            if ui.button(name).on_hover_text(tip).clicked() {
                                action = Some((name, installed.clone()));
                            }
                        }
                    });
                    ui.end_row();
                }
            });
        if let Some((name, installed)) = action {
            self.run_action(ui.ctx(), name, installed);
        }
    }

    fn run_action(&mut self, ctx: &egui::Context, name: &str, installed: Installed) {
        match name {
            "Launch" => {
                let result =
                    library::launch(&installed).map(|()| format!("started {}", installed.version));
                self.report(result);
            }
            "Verify" => self.spawn(ctx, "Verify", move |library, control| {
                let issues = library.verify(&installed, control)?;
                Ok(Done {
                    message: format!("{}: {issues}", installed.version),
                    change: Change::None,
                })
            }),
            "Repair" => {
                let catalog = self.catalog.clone().unwrap_or_default();
                self.spawn(ctx, "Repair", move |library, control| {
                    Ok(Done {
                        message: format!(
                            "{}: {}",
                            installed.version,
                            library.repair(&catalog, &installed, control)?
                        ),
                        change: Change::None,
                    })
                });
            }
            "Move…" => {
                let Some(parent) = rfd::FileDialog::new()
                    .set_title("Move to which folder?")
                    .pick_folder()
                else {
                    return;
                };
                self.spawn(ctx, "Move", move |_, _| {
                    let new = library::move_folder(&installed.path, &parent)?;
                    Ok(Done {
                        message: format!("{} is now in {}", installed.version, new.display()),
                        change: Change::Move(installed.path, new),
                    })
                });
            }
            _ => self.confirm_uninstall = Some(installed),
        }
    }

    fn available_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Available");
        let Some(os) = self.os else {
            ui.label("Builds are published for Windows and Linux only.");
            return;
        };
        let Some(catalog) = &self.catalog else {
            ui.label("Press Refresh to read the published builds.");
            return;
        };
        let mut install = None;
        egui::Grid::new("available")
            .num_columns(4)
            .striped(true)
            .show(ui, |ui| {
                for entry in catalog.for_os(os) {
                    ui.strong(&entry.version);
                    ui.label(format!("{:.0} MB", entry.size as f64 / 1_000_000.0));
                    ui.label(entry.published_at.get(..10).unwrap_or(""));
                    let installed = self
                        .state
                        .installs
                        .iter()
                        .any(|installed| installed.snapshot_id == entry.snapshot_id);
                    if installed {
                        ui.label("installed");
                    } else {
                        ui.horizontal(|ui| {
                            if ui.button("Install").clicked() {
                                install = Some((entry.clone(), None));
                            }
                            if ui
                                .button("Install to…")
                                .on_hover_text("Pick the folder to install into.")
                                .clicked()
                            {
                                install = Some((entry.clone(), Some(())));
                            }
                        });
                    }
                    ui.end_row();
                }
            });
        if let Some((entry, pick)) = install {
            self.install(ui.ctx(), entry, pick.is_some());
        }
    }

    fn install(&mut self, ctx: &egui::Context, entry: CatalogEntry, pick: bool) {
        let folder = if pick {
            let Some(parent) = rfd::FileDialog::new()
                .set_title("Install into which folder?")
                .pick_folder()
            else {
                return;
            };
            parent.join(format!("blender-{}", entry.version))
        } else {
            self.library.default_folder(&entry)
        };
        let catalog = self.catalog.clone().unwrap_or_default();
        let reuse = self.state.newest(entry.os).cloned();
        self.spawn(ctx, "Install", move |library, control| {
            let installed = library.install(&catalog, &entry, &folder, reuse.as_ref(), control)?;
            Ok(Done {
                message: format!("{} installed in {}", entry.version, folder.display()),
                change: Change::Add(installed),
            })
        });
    }

    fn uninstall_dialog(&mut self, ctx: &egui::Context) {
        let Some(installed) = self.confirm_uninstall.clone() else {
            return;
        };
        let mut answer = None;
        egui::Window::new("Uninstall")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!(
                    "Delete {} and everything in {}?",
                    installed.version,
                    installed.path.display()
                ));
                ui.horizontal(|ui| {
                    if ui.button("Uninstall").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Keep").clicked() {
                        answer = Some(false);
                    }
                });
            });
        match answer {
            Some(true) => {
                self.confirm_uninstall = None;
                self.spawn(ctx, "Uninstall", move |library, control| {
                    library.uninstall(&installed, control)?;
                    Ok(Done {
                        message: format!("{} removed", installed.version),
                        change: Change::Remove(installed.path),
                    })
                });
            }
            Some(false) => self.confirm_uninstall = None,
            None => {}
        }
    }

    fn about_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("About")
            .open(&mut self.show_about)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.strong("Gutch Launcher Sample");
                ui.label(
                    "An example of the Gutch library: it installs several versions \
                     side by side and downloads only what changed between them.",
                );
                ui.hyperlink(GUTCH_URL);
                ui.separator();
                ui.label(
                    "The builds are official Blender releases, unchanged, from \
                     download.blender.org and its mirrors. Blender is free \
                     software under the GNU GPL; its source is at blender.org.",
                );
                ui.strong("Not affiliated with or endorsed by the Blender Foundation.");
                ui.label("Blender is a trademark of the Blender Foundation.");
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        if let Some(job) = &self.job {
            ui.horizontal(|ui| {
                if ui
                    .button("Cancel")
                    .on_hover_text("Stop as soon as it is safe to.")
                    .clicked()
                {
                    job.cancel.cancel();
                }
                let (fraction, text) = match &job.last_event {
                    Some(event) => (progress_fraction(event), event.message.clone()),
                    None => (None, format!("{}…", job.title)),
                };
                let bar = match fraction {
                    Some(fraction) => egui::ProgressBar::new(fraction).show_percentage(),
                    None => egui::ProgressBar::new(0.0).animate(true),
                };
                ui.add(bar.text(text));
            });
        } else if self.status_is_error {
            ui.colored_label(ui.visuals().error_fg_color, &self.status);
        } else {
            ui.label(&self.status);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.catalog.is_none() && self.job.is_none() && !self.status_is_error {
            self.refresh(ui.ctx());
        }
        self.receive_updates();
        // One job at a time keeps the cache and the state simple.
        let idle = self.job.is_none() && self.confirm_uninstall.is_none();
        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Gutch Launcher Sample");
                ui.add_enabled_ui(idle, |ui| {
                    if ui.button("Refresh").clicked() {
                        self.refresh(ui.ctx());
                    }
                });
                if ui.button("About").clicked() {
                    self.show_about = true;
                }
            });
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_enabled_ui(idle, |ui| {
                    self.installed_section(ui);
                    ui.separator();
                    self.available_section(ui);
                });
            });
        });
        let ctx = ui.ctx().clone();
        self.uninstall_dialog(&ctx);
        self.about_dialog(&ctx);
    }
}

/// Bytes when the phase moves data, otherwise items.
fn progress_fraction(event: &ProgressEvent) -> Option<f32> {
    if event.total_bytes > 0 {
        Some(event.transferred_bytes as f32 / event.total_bytes as f32)
    } else if event.total > 0 {
        Some(event.current as f32 / event.total as f32)
    } else {
        None
    }
}
