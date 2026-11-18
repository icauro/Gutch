//! The window. Downloads and disk work run on a worker thread; the window
//! reads progress and the result from a channel.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use eframe::egui;
use gutch::api::{CancelFlag, OperationControl, ProgressEvent};
use launcher_catalog::{Catalog, CatalogEntry, Os};

use crate::library::{self, DEFAULT_BUCKET_URL, Installed, Library, Result, State};
use crate::progress::{self, Progress};

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

/// What a job works on, so its row can show the progress.
#[derive(Clone, PartialEq)]
enum Subject {
    Catalog,
    /// A published build, by snapshot.
    Entry(String),
    /// An installed version, by folder.
    Install(PathBuf),
}

struct Job {
    title: &'static str,
    subject: Subject,
    cancel: CancelFlag,
    updates: Receiver<Update>,
    progress: Progress,
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
        subject: Subject,
        expected_bytes: Option<u64>,
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
            subject,
            cancel,
            updates,
            progress: Progress::new(expected_bytes),
        });
    }

    fn refresh(&mut self, ctx: &egui::Context) {
        let os = self.os;
        self.spawn(ctx, "Refresh", Subject::Catalog, None, move |library, _| {
            let catalog = library.fetch_catalog()?;
            let count = os.map_or(0, |os| catalog.for_os(os).count());
            Ok(Done {
                message: format!("{count} versions published for this system"),
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
                Update::Progress(event) => job.progress.record(event),
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

    fn installed_section(&mut self, ui: &mut egui::Ui, idle: bool) {
        ui.heading("Installed");
        let versions_dir = self.library.versions_dir();
        ui.horizontal(|ui| {
            ui.weak(format!("New versions go to {}", versions_dir.display()));
            if ui.small_button("Open folder").clicked() {
                let result = std::fs::create_dir_all(&versions_dir)
                    .map_err(|error| format!("cannot create {}: {error}", versions_dir.display()))
                    .and_then(|()| library::open_folder(&versions_dir))
                    .map(|()| String::new());
                if result.is_err() {
                    self.report(result);
                }
            }
        });
        ui.add_space(4.0);
        if self.state.installs.is_empty() {
            card(ui, |ui| {
                ui.label("Nothing installed yet. Install a version below to get started.");
            });
            return;
        }
        let mut action = None;
        for installed in &self.state.installs {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&installed.version).size(18.0).strong());
                            self.install_tags(ui, installed);
                        });
                        ui.weak(installed.path.display().to_string());
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_enabled_ui(idle, |ui| {
                            ui.menu_button("More…", |ui| {
                                for (name, tip) in [
                                    ("Open folder", "Show the files."),
                                    ("Verify", "Compare the files with the published build."),
                                    ("Repair", "Restore missing and changed files."),
                                    ("Move…", "Move the folder somewhere else."),
                                    ("Uninstall", "Delete the folder."),
                                ] {
                                    if ui.button(name).on_hover_text(tip).clicked() {
                                        action = Some((name, installed.clone()));
                                        ui.close();
                                    }
                                }
                            });
                            if ui.add(primary_button("Launch")).clicked() {
                                action = Some(("Launch", installed.clone()));
                            }
                        });
                    });
                });
                self.job_view(ui, &Subject::Install(installed.path.clone()));
            });
        }
        if let Some((name, installed)) = action {
            self.run_action(ui.ctx(), name, installed);
        }
    }

    /// "Update available" when a newer patch of the same series is published
    /// and not installed; "No longer published" when the build was retired.
    fn install_tags(&self, ui: &mut egui::Ui, installed: &Installed) {
        let Some(catalog) = &self.catalog else {
            return;
        };
        let published = catalog
            .versions
            .iter()
            .any(|entry| entry.snapshot_id == installed.snapshot_id);
        let update = catalog
            .for_os(installed.os)
            .filter(|entry| series(&entry.version) == series(&installed.version))
            .filter(|entry| {
                launcher_catalog::compare_versions(&entry.version, &installed.version).is_gt()
            })
            .find(|entry| {
                !self
                    .state
                    .installs
                    .iter()
                    .any(|other| other.snapshot_id == entry.snapshot_id)
            });
        if let Some(entry) = update {
            let color = ui.visuals().warn_fg_color;
            tag(ui, &format!("Update available: {}", entry.version), color);
        } else if !published {
            let color = ui.visuals().weak_text_color();
            tag(ui, "No longer published", color)
                .on_hover_text("It still runs, but a repair may need a newer version.");
        }
    }

    /// The progress of a job on `subject`, if one runs.
    fn job_view(&self, ui: &mut egui::Ui, subject: &Subject) {
        let Some(job) = self.job.as_ref().filter(|job| &job.subject == subject) else {
            return;
        };
        ui.separator();
        job.progress.show(ui, job.title);
        if ui
            .button("Cancel")
            .on_hover_text("Stop as soon as it is safe to.")
            .clicked()
        {
            job.cancel.cancel();
        }
    }

    fn run_action(&mut self, ctx: &egui::Context, name: &str, installed: Installed) {
        match name {
            "Open folder" => {
                let result = library::open_folder(&installed.path).map(|()| String::new());
                if result.is_err() {
                    self.report(result);
                }
            }
            "Launch" => {
                let result =
                    library::launch(&installed).map(|()| format!("started {}", installed.version));
                self.report(result);
            }
            "Verify" => self.spawn(
                ctx,
                "Verify",
                Subject::Install(installed.path.clone()),
                None,
                move |library, control| {
                    let issues = library.verify(&installed, control)?;
                    Ok(Done {
                        message: format!("{}: {issues}", installed.version),
                        change: Change::None,
                    })
                },
            ),
            "Repair" => {
                let catalog = self.catalog.clone().unwrap_or_default();
                self.spawn(
                    ctx,
                    "Repair",
                    Subject::Install(installed.path.clone()),
                    None,
                    move |library, control| {
                        Ok(Done {
                            message: format!(
                                "{}: {}",
                                installed.version,
                                library.repair(&catalog, &installed, control)?
                            ),
                            change: Change::None,
                        })
                    },
                );
            }
            "Move…" => {
                let Some(parent) = rfd::FileDialog::new()
                    .set_title("Move to which folder?")
                    .pick_folder()
                else {
                    return;
                };
                self.spawn(
                    ctx,
                    "Move",
                    Subject::Install(installed.path.clone()),
                    None,
                    move |_, _| {
                        let new = library::move_folder(&installed.path, &parent)?;
                        Ok(Done {
                            message: format!("{} is now in {}", installed.version, new.display()),
                            change: Change::Move(installed.path, new),
                        })
                    },
                );
            }
            _ => self.confirm_uninstall = Some(installed),
        }
    }

    fn available_section(&mut self, ui: &mut egui::Ui, idle: bool) {
        ui.heading("Available");
        let Some(os) = self.os else {
            ui.label("Builds are published for Windows and Linux only.");
            return;
        };
        let Some(catalog) = &self.catalog else {
            ui.label("Press Refresh to read the published builds.");
            return;
        };
        let latest = catalog
            .for_os(os)
            .map(|entry| entry.version.as_str())
            .max_by(|left, right| launcher_catalog::compare_versions(left, right));
        let mut install = None;
        for entry in catalog.for_os(os) {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&entry.version).size(18.0).strong());
                            if latest == Some(entry.version.as_str()) {
                                let color = ui.visuals().hyperlink_color;
                                tag(ui, "Latest", color);
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.weak(format!("{} on disk", progress::size(entry.size)));
                            ui.weak("·");
                            ui.hyperlink_to("Release notes", release_notes(&entry.version));
                        });
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let installed = self
                            .state
                            .installs
                            .iter()
                            .any(|installed| installed.snapshot_id == entry.snapshot_id);
                        if installed {
                            ui.weak("Installed");
                            return;
                        }
                        ui.add_enabled_ui(idle, |ui| {
                            ui.menu_button("▼", |ui| {
                                if ui
                                    .button("Install to…")
                                    .on_hover_text("Pick the folder to install into.")
                                    .clicked()
                                {
                                    install = Some((entry.clone(), true));
                                    ui.close();
                                }
                            });
                            if ui.add(primary_button("Install")).clicked() {
                                install = Some((entry.clone(), false));
                            }
                        });
                    });
                });
                self.job_view(ui, &Subject::Entry(entry.snapshot_id.clone()));
            });
        }
        if let Some((entry, pick)) = install {
            self.install(ui.ctx(), entry, pick);
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
        if let Some(free) = library::free_space(&folder)
            && free < entry.size
        {
            self.report(Err(format!(
                "Install {}: it needs {} and only {} is free on that disk",
                entry.version,
                progress::size(entry.size),
                progress::size(free)
            )));
            return;
        }
        let catalog = self.catalog.clone().unwrap_or_default();
        let reuse = self.state.newest(entry.os).cloned();
        let subject = Subject::Entry(entry.snapshot_id.clone());
        self.spawn(
            ctx,
            "Install",
            subject,
            Some(entry.size),
            move |library, control| {
                let installed =
                    library.install(&catalog, &entry, &folder, reuse.as_ref(), control)?;
                Ok(Done {
                    message: format!("{} installed in {}", entry.version, folder.display()),
                    change: Change::Add(installed),
                })
            },
        );
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
                let subject = Subject::Install(installed.path.clone());
                self.spawn(ctx, "Uninstall", subject, None, move |library, control| {
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
                // A row shows its own job with a Cancel button.
                if job.subject == Subject::Catalog
                    && ui
                        .button("Cancel")
                        .on_hover_text("Stop as soon as it is safe to.")
                        .clicked()
                {
                    job.cancel.cancel();
                }
                job.progress.show_compact(ui, job.title);
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
                if ui.add_enabled(idle, egui::Button::new("Refresh")).clicked() {
                    self.refresh(ui.ctx());
                }
                if ui.button("About").clicked() {
                    self.show_about = true;
                }
            });
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.installed_section(ui, idle);
                ui.add_space(12.0);
                self.available_section(ui, idle);
            });
        });
        let ctx = ui.ctx().clone();
        self.uninstall_dialog(&ctx);
        self.about_dialog(&ctx);
    }
}

/// A row's frame, as wide as the window.
fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(12))
        .corner_radius(6)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
    ui.add_space(6.0);
}

fn primary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).size(16.0).strong())
        .min_size(egui::vec2(96.0, 32.0))
}

/// A small coloured label beside a version.
fn tag(ui: &mut egui::Ui, text: &str, color: egui::Color32) -> egui::Response {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).small().color(color))
        })
        .response
}

/// The release series, such as `5.2` for `5.2.4`.
fn series(version: &str) -> String {
    version.split('.').take(2).collect::<Vec<_>>().join(".")
}

/// Blender's release notes for a version's series.
fn release_notes(version: &str) -> String {
    format!(
        "https://www.blender.org/download/releases/{}/",
        series(version).replace('.', "-")
    )
}
