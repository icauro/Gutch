//! The window. Every bucket or disk operation runs on a worker thread; the
//! window reads its progress and result from a channel.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use eframe::egui;
use gutch::api::{
    CancelFlag, OperationControl, ProgressEvent, RemoteRepoSummary, RemoteSnapshot, Result,
};

use crate::ops::Remote;
use crate::store::{S3Settings, S3Store};

type S3Remote = Remote<S3Store>;

/// What a finished job hands back to the window.
#[derive(Default)]
struct Outcome {
    message: String,
    repos: Option<Vec<RemoteRepoSummary>>,
    /// Replaces the snapshot list of the selected repository.
    snapshots: Option<Vec<RemoteSnapshot>>,
    /// The snapshot now in the destination folder.
    installed_snapshot_id: Option<String>,
}

enum Update {
    Progress(ProgressEvent),
    Done(Result<Outcome>),
}

struct Job {
    title: &'static str,
    cancel: CancelFlag,
    updates: Receiver<Update>,
    last_event: Option<ProgressEvent>,
}

pub struct App {
    settings: S3Settings,
    cache_dir: String,
    remote: Option<Arc<S3Remote>>,

    repos: Vec<RemoteRepoSummary>,
    repo_id: Option<String>,
    new_scope: String,
    new_name: String,

    snapshots: Vec<RemoteSnapshot>,
    snapshot_id: Option<String>,

    source: Option<PathBuf>,
    tags: String,
    destination: Option<PathBuf>,
    /// The snapshot already in `destination`, so an install only changes what differs.
    installed_snapshot_id: Option<String>,

    job: Option<Job>,
    status: String,
    status_is_error: bool,
}

impl App {
    /// Fills the connection from the usual AWS variables. The secret is never saved.
    pub fn new() -> Self {
        let variable = |name: &str| std::env::var(name).unwrap_or_default();
        let region = Some(variable("AWS_REGION"))
            .filter(|region| !region.is_empty())
            .unwrap_or_else(|| "us-east-1".into());
        Self {
            settings: S3Settings {
                endpoint: Some(variable("AWS_ENDPOINT_URL"))
                    .filter(|endpoint| !endpoint.is_empty())
                    .unwrap_or_else(|| format!("https://s3.{region}.amazonaws.com")),
                region,
                bucket: String::new(),
                prefix: "gutch".into(),
                path_style: false,
                access_key_id: variable("AWS_ACCESS_KEY_ID"),
                secret_access_key: variable("AWS_SECRET_ACCESS_KEY"),
            },
            cache_dir: std::env::temp_dir()
                .join("gutch-s3-app")
                .display()
                .to_string(),
            remote: None,
            repos: Vec::new(),
            repo_id: None,
            new_scope: "team".into(),
            new_name: String::new(),
            snapshots: Vec::new(),
            snapshot_id: None,
            source: None,
            tags: "version=1.0".into(),
            destination: None,
            installed_snapshot_id: None,
            job: None,
            status: "Fill in the bucket and connect.".into(),
            status_is_error: false,
        }
    }

    /// Runs `work` on a worker thread with progress reporting and a cancel flag.
    fn start(
        &mut self,
        ctx: &egui::Context,
        title: &'static str,
        work: impl FnOnce(&S3Remote, OperationControl) -> Result<Outcome> + Send + 'static,
    ) {
        let Some(remote) = self.remote.clone() else {
            return;
        };
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
        let done_ctx = ctx.clone();
        thread::spawn(move || {
            let _ = sender.send(Update::Done(work(&remote, control)));
            done_ctx.request_repaint();
        });
        self.job = Some(Job {
            title,
            cancel,
            updates,
            last_event: None,
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
            Ok(outcome) => {
                self.status = format!("{title}: {}", outcome.message);
                self.status_is_error = false;
                if let Some(repos) = outcome.repos {
                    self.repos = repos;
                }
                if let Some(snapshots) = outcome.snapshots {
                    self.snapshots = snapshots;
                }
                if outcome.installed_snapshot_id.is_some() {
                    self.installed_snapshot_id = outcome.installed_snapshot_id;
                }
            }
            Err(error) => {
                self.status = format!("{title} failed: {error}");
                self.status_is_error = true;
            }
        }
    }

    fn connect(&mut self, ctx: &egui::Context) {
        match S3Store::new(&self.settings) {
            Ok(store) => {
                let cache_dir = PathBuf::from(self.cache_dir.trim());
                self.remote = Some(Arc::new(Remote::new(store, cache_dir)));
                self.repo_id = None;
                self.snapshots.clear();
                self.start(ctx, "Connect", |remote, _| {
                    let repos = remote.repos()?;
                    Ok(Outcome {
                        message: format!("{} repositories", repos.len()),
                        repos: Some(repos),
                        ..Default::default()
                    })
                });
            }
            Err(error) => {
                self.status = format!("Connect failed: {error}");
                self.status_is_error = true;
            }
        }
    }

    fn connection_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Bucket");
        egui::Grid::new("connection").num_columns(2).show(ui, |ui| {
            let settings = &mut self.settings;
            ui.label("Endpoint");
            ui.text_edit_singleline(&mut settings.endpoint);
            ui.end_row();
            ui.label("Region");
            ui.text_edit_singleline(&mut settings.region);
            ui.end_row();
            ui.label("Bucket");
            ui.text_edit_singleline(&mut settings.bucket);
            ui.end_row();
            ui.label("Key prefix");
            ui.text_edit_singleline(&mut settings.prefix);
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut settings.path_style, "Path-style URLs (MinIO)");
            ui.end_row();
            ui.label("Access key ID");
            ui.text_edit_singleline(&mut settings.access_key_id);
            ui.end_row();
            ui.label("Secret key");
            ui.add(egui::TextEdit::singleline(&mut settings.secret_access_key).password(true));
            ui.end_row();
            ui.label("Local cache");
            ui.text_edit_singleline(&mut self.cache_dir);
            ui.end_row();
        });
        if ui.button("Connect").clicked() {
            self.connect(ui.ctx());
        }
    }

    fn repos_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Repositories");
        let mut opened = None;
        for repo in &self.repos {
            let selected = self.repo_id.as_deref() == Some(&repo.repository_id);
            if ui.selectable_label(selected, &repo.repository_id).clicked() {
                opened = Some(repo.repository_id.clone());
            }
        }
        if let Some(repo_id) = opened {
            self.open_repo(ui.ctx(), repo_id);
        }
        ui.separator();
        ui.label("New repository");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_scope)
                    .hint_text("scope")
                    .desired_width(70.0),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.new_name)
                    .hint_text("name")
                    .desired_width(110.0),
            );
        });
        if ui.button("Create").clicked() {
            let (scope, name) = (self.new_scope.clone(), self.new_name.clone());
            self.start(ui.ctx(), "Create", move |remote, control| {
                let repo_id = remote.create_repo(&scope, &name, control)?;
                Ok(Outcome {
                    message: format!("created {repo_id}"),
                    repos: Some(remote.repos()?),
                    ..Default::default()
                })
            });
        }
    }

    fn open_repo(&mut self, ctx: &egui::Context, repo_id: String) {
        self.repo_id = Some(repo_id.clone());
        self.snapshot_id = None;
        self.installed_snapshot_id = None;
        self.snapshots.clear();
        self.start(ctx, "Open", move |remote, _| {
            let snapshots = remote.snapshots(&repo_id)?;
            Ok(Outcome {
                message: format!("{repo_id} has {} snapshots", snapshots.len()),
                snapshots: Some(snapshots),
                ..Default::default()
            })
        });
    }

    fn snapshots_table(&mut self, ui: &mut egui::Ui) {
        ui.heading("Snapshots");
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                egui::Grid::new("snapshots")
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("Snapshot");
                        ui.strong("Tags");
                        ui.strong("Created");
                        ui.end_row();
                        for snapshot in &self.snapshots {
                            let id = &snapshot.snapshot_id;
                            let selected = self.snapshot_id.as_ref() == Some(id);
                            if ui.selectable_label(selected, short(id)).clicked() {
                                self.snapshot_id = Some(id.clone());
                            }
                            ui.label(snapshot.tags.join(" "));
                            ui.label(&snapshot.created_at);
                            ui.end_row();
                        }
                    });
            });
    }

    fn capture_section(&mut self, ui: &mut egui::Ui, repo_id: &str) {
        ui.heading("Capture and push");
        ui.horizontal(|ui| {
            if ui.button("Source folder…").clicked() {
                self.source = rfd::FileDialog::new().pick_folder().or(self.source.take());
            }
            ui.label(path_label(&self.source));
        });
        ui.horizontal(|ui| {
            ui.label("Tags");
            ui.text_edit_singleline(&mut self.tags);
        });
        let ready = self.source.is_some();
        if ui
            .add_enabled(ready, egui::Button::new("Capture and push"))
            .clicked()
        {
            let (repo_id, tags) = (repo_id.to_owned(), self.tags.clone());
            let source = self.source.clone().unwrap_or_default();
            self.start(ui.ctx(), "Capture", move |remote, control| {
                Ok(Outcome {
                    message: remote.capture_and_push(&repo_id, &source, &tags, control)?,
                    snapshots: Some(remote.snapshots(&repo_id)?),
                    ..Default::default()
                })
            });
        }
    }

    fn install_section(&mut self, ui: &mut egui::Ui, repo_id: &str) {
        ui.heading("Install the selected snapshot");
        ui.horizontal(|ui| {
            if ui.button("Destination folder…").clicked() {
                self.destination = rfd::FileDialog::new()
                    .pick_folder()
                    .or(self.destination.take());
            }
            ui.label(path_label(&self.destination));
        });
        ui.horizontal(|ui| {
            ui.label("Already installed there");
            egui::ComboBox::from_id_salt("installed")
                .selected_text(
                    self.installed_snapshot_id
                        .as_deref()
                        .map_or("nothing", short),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.installed_snapshot_id, None, "nothing");
                    for snapshot in &self.snapshots {
                        let id = snapshot.snapshot_id.clone();
                        let label = short(&id).to_owned();
                        ui.selectable_value(&mut self.installed_snapshot_id, Some(id), label);
                    }
                });
        });
        let (Some(snapshot_id), Some(destination)) =
            (self.snapshot_id.clone(), self.destination.clone())
        else {
            ui.label("Select a snapshot and a destination.");
            return;
        };
        let repo_id = repo_id.to_owned();
        ui.horizontal(|ui| {
            if ui.button("Install").clicked() {
                let installed = self.installed_snapshot_id.clone();
                let (repo_id, snapshot_id, destination) =
                    (repo_id.clone(), snapshot_id.clone(), destination.clone());
                self.start(ui.ctx(), "Install", move |remote, control| {
                    let message = remote.install(
                        &repo_id,
                        &snapshot_id,
                        &destination,
                        installed.as_deref(),
                        control,
                    )?;
                    Ok(Outcome {
                        message,
                        installed_snapshot_id: Some(snapshot_id),
                        ..Default::default()
                    })
                });
            }
            if ui.button("Verify").clicked() {
                let (repo_id, snapshot_id, destination) =
                    (repo_id.clone(), snapshot_id.clone(), destination.clone());
                self.start(ui.ctx(), "Verify", move |remote, control| {
                    Ok(Outcome {
                        message: remote.verify(&repo_id, &snapshot_id, &destination, control)?,
                        ..Default::default()
                    })
                });
            }
            if ui.button("Repair").clicked() {
                self.start(ui.ctx(), "Repair", move |remote, control| {
                    Ok(Outcome {
                        message: remote.repair(&repo_id, &snapshot_id, &destination, control)?,
                        ..Default::default()
                    })
                });
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        if let Some(job) = &self.job {
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
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
        self.receive_updates();
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        // One job at a time keeps the local cache simple.
        let idle = self.job.is_none();
        egui::Panel::left("side")
            .default_size(320.0)
            .show(ui, |ui| {
                ui.add_enabled_ui(idle, |ui| {
                    self.connection_panel(ui);
                    ui.separator();
                    self.repos_panel(ui);
                });
            });
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(repo_id) = self.repo_id.clone() else {
                ui.label("Connect, then pick or create a repository.");
                return;
            };
            ui.add_enabled_ui(idle, |ui| {
                self.snapshots_table(ui);
                ui.separator();
                self.capture_section(ui, &repo_id);
                ui.separator();
                self.install_section(ui, &repo_id);
            });
        });
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

/// Snapshot IDs are long hashes; the first 12 characters tell them apart.
fn short(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

fn path_label(path: &Option<PathBuf>) -> String {
    path.as_ref()
        .map_or_else(|| "none".into(), |path| path.display().to_string())
}
