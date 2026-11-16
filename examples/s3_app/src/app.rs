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

use crate::cloudflare::{self, Account, R2SignIn};
use crate::ops::{Remote, check_bucket, explain};
use crate::provider::{self, Provider, R2_JURISDICTIONS};
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
    /// A snapshot and its files' paths and sizes.
    files: Option<(String, Vec<(String, u64)>)>,
    /// A Cloudflare sign-in, with the bucket to select.
    r2: Option<(R2SignIn, Option<String>)>,
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

/// Cloudflare R2 sign-in state. The API token stays in memory.
#[derive(Default)]
struct R2 {
    token: String,
    account_id: String,
    jurisdiction: String,
    accounts: Vec<Account>,
    buckets: Vec<String>,
    new_bucket: String,
}

pub struct App {
    provider: Provider,
    /// The raw fields; a provider preset replaces some of them on use.
    settings: S3Settings,
    r2: R2,
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
    /// The listed snapshot and its files, shown while that snapshot is selected.
    files: Option<(String, Vec<(String, u64)>)>,

    job: Option<Job>,
    status: String,
    status_is_error: bool,
}

/// What the app remembers between runs, in eframe's settings file
/// (`app.ron` in the platform's app data folder). The secret key and the
/// Cloudflare token are left out on purpose: they come from
/// `AWS_SECRET_ACCESS_KEY` and `CLOUDFLARE_API_TOKEN`, or are typed each run.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)]
struct Saved {
    provider: Provider,
    endpoint: String,
    region: String,
    bucket: String,
    prefix: String,
    path_style: bool,
    access_key_id: String,
    r2_account_id: String,
    r2_jurisdiction: String,
    cache_dir: String,
    new_scope: String,
    tags: String,
    source: Option<PathBuf>,
    destination: Option<PathBuf>,
}

impl Default for Saved {
    /// The first run starts from the usual AWS variables.
    fn default() -> Self {
        let region = variable("AWS_REGION").unwrap_or_else(|| "us-east-1".into());
        let endpoint = variable("AWS_ENDPOINT_URL");
        Self {
            provider: if endpoint.is_some() {
                Provider::Other
            } else {
                Provider::Aws
            },
            endpoint: endpoint.unwrap_or_else(|| provider::aws(&region).endpoint),
            region,
            bucket: String::new(),
            prefix: "gutch".into(),
            path_style: false,
            access_key_id: String::new(),
            r2_account_id: String::new(),
            r2_jurisdiction: "default".into(),
            cache_dir: default_cache_dir().display().to_string(),
            new_scope: "team".into(),
            tags: "version=1.0".into(),
            source: None,
            destination: None,
        }
    }
}

const SAVED_KEY: &str = "settings";

impl App {
    /// Restores the last run's settings. The key pair from the environment,
    /// when set, wins over the saved key ID so the two always match.
    pub fn new(storage: Option<&dyn eframe::Storage>) -> Self {
        let saved: Saved = storage
            .and_then(|storage| eframe::get_value(storage, SAVED_KEY))
            .unwrap_or_default();
        Self {
            provider: saved.provider,
            settings: S3Settings {
                endpoint: saved.endpoint,
                region: saved.region,
                bucket: saved.bucket,
                prefix: saved.prefix,
                path_style: saved.path_style,
                access_key_id: variable("AWS_ACCESS_KEY_ID").unwrap_or(saved.access_key_id),
                secret_access_key: variable("AWS_SECRET_ACCESS_KEY").unwrap_or_default(),
            },
            r2: R2 {
                token: variable("CLOUDFLARE_API_TOKEN").unwrap_or_default(),
                account_id: saved.r2_account_id,
                jurisdiction: saved.r2_jurisdiction,
                ..Default::default()
            },
            cache_dir: saved.cache_dir,
            remote: None,
            repos: Vec::new(),
            repo_id: None,
            new_scope: saved.new_scope,
            new_name: String::new(),
            snapshots: Vec::new(),
            snapshot_id: None,
            source: saved.source,
            tags: saved.tags,
            destination: saved.destination,
            installed_snapshot_id: None,
            files: None,
            job: None,
            status: "Pick the provider, fill in the bucket and connect.".into(),
            status_is_error: false,
        }
    }

    fn saved(&self) -> Saved {
        let settings = &self.settings;
        Saved {
            provider: self.provider,
            endpoint: settings.endpoint.clone(),
            region: settings.region.clone(),
            bucket: settings.bucket.clone(),
            prefix: settings.prefix.clone(),
            path_style: settings.path_style,
            access_key_id: settings.access_key_id.clone(),
            r2_account_id: self.r2.account_id.clone(),
            r2_jurisdiction: self.r2.jurisdiction.clone(),
            cache_dir: self.cache_dir.clone(),
            new_scope: self.new_scope.clone(),
            tags: self.tags.clone(),
            source: self.source.clone(),
            destination: self.destination.clone(),
        }
    }

    /// The S3 settings the chosen provider stands for. With a Cloudflare
    /// token, the R2 secret key is the token's SHA-256.
    fn resolved_settings(&self) -> S3Settings {
        let mut settings = self.settings.clone();
        let preset = match self.provider {
            Provider::Aws => provider::aws(&settings.region),
            Provider::Ovh => provider::ovh(&settings.region),
            Provider::Minio => provider::minio(&settings.endpoint),
            Provider::CloudflareR2 => {
                if !self.r2.token.trim().is_empty() {
                    settings.secret_access_key = cloudflare::s3_secret(&self.r2.token);
                }
                provider::r2(&self.r2.account_id, &self.r2.jurisdiction)
            }
            Provider::Other => return settings,
        };
        settings.endpoint = preset.endpoint;
        settings.region = preset.region;
        settings.path_style = preset.path_style;
        settings
    }

    /// Switching to Other keeps the last preset's values to edit by hand.
    fn pick_provider(&mut self, picked: Provider) {
        if picked == self.provider {
            return;
        }
        let resolved = self.resolved_settings();
        self.settings.endpoint = resolved.endpoint;
        self.settings.path_style = resolved.path_style;
        self.settings.region = match picked {
            Provider::Aws | Provider::Minio => "us-east-1".into(),
            Provider::Ovh => "gra".into(),
            Provider::CloudflareR2 | Provider::Other => resolved.region,
        };
        if picked == Provider::Minio {
            self.settings.endpoint = "http://localhost:9000".into();
        }
        self.provider = picked;
    }

    /// Runs `work` on a worker thread with progress reporting and a cancel flag.
    fn spawn(
        &mut self,
        ctx: &egui::Context,
        title: &'static str,
        work: impl FnOnce(OperationControl) -> Result<Outcome> + Send + 'static,
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
        let done_ctx = ctx.clone();
        thread::spawn(move || {
            let _ = sender.send(Update::Done(work(control)));
            done_ctx.request_repaint();
        });
        self.job = Some(Job {
            title,
            cancel,
            updates,
            last_event: None,
        });
    }

    /// Like `spawn`, for work on the connected bucket.
    fn start(
        &mut self,
        ctx: &egui::Context,
        title: &'static str,
        work: impl FnOnce(&S3Remote, OperationControl) -> Result<Outcome> + Send + 'static,
    ) {
        let Some(remote) = self.remote.clone() else {
            return;
        };
        self.spawn(ctx, title, move |control| work(&remote, control));
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
                if outcome.files.is_some() {
                    self.files = outcome.files;
                }
                if let Some((signed_in, bucket)) = outcome.r2 {
                    self.signed_in(signed_in, bucket);
                }
            }
            Err(error) => {
                self.status = format!("{title} failed: {}", explain(&error.to_string()));
                self.status_is_error = true;
            }
        }
    }

    fn signed_in(&mut self, signed_in: R2SignIn, bucket: Option<String>) {
        self.r2.account_id = signed_in.account_id;
        self.r2.accounts = signed_in.accounts;
        self.r2.buckets = signed_in.buckets.unwrap_or_default();
        self.settings.access_key_id = signed_in.access_key_id;
        self.settings.secret_access_key = signed_in.secret_access_key;
        if let Some(bucket) = bucket {
            self.settings.bucket = bucket;
        } else if !self.r2.buckets.contains(&self.settings.bucket)
            && let Some(first) = self.r2.buckets.first()
        {
            self.settings.bucket = first.clone();
        }
    }

    fn connect(&mut self, ctx: &egui::Context) {
        match S3Store::new(&self.resolved_settings()) {
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

    fn test_connection(&mut self, ctx: &egui::Context) {
        match S3Store::new(&self.resolved_settings()) {
            Ok(store) => self.spawn(ctx, "Test connection", move |_| {
                Ok(Outcome {
                    message: check_bucket(&store)?,
                    ..Default::default()
                })
            }),
            Err(error) => {
                self.status = format!("Test connection failed: {error}");
                self.status_is_error = true;
            }
        }
    }

    /// Signs in to Cloudflare; with `new_bucket`, creates that bucket first.
    fn r2_sign_in(&mut self, ctx: &egui::Context, new_bucket: Option<String>) {
        let token = self.r2.token.clone();
        let account_id = self.r2.account_id.clone();
        let jurisdiction = self.r2.jurisdiction.clone();
        let title = if new_bucket.is_some() {
            "Create bucket"
        } else {
            "Sign in to Cloudflare"
        };
        self.spawn(ctx, title, move |_| {
            if let Some(name) = &new_bucket {
                cloudflare::create_bucket(&token, &account_id, &jurisdiction, name)?;
            }
            let signed_in = cloudflare::sign_in(&token, &account_id, &jurisdiction)?;
            let message = match &signed_in.buckets {
                Some(buckets) => format!(
                    "account {}; {} buckets; S3 keys derived from the token",
                    signed_in.account_id,
                    buckets.len()
                ),
                None => format!(
                    "account {}; the token may not list buckets, so type the bucket name",
                    signed_in.account_id
                ),
            };
            Ok(Outcome {
                message,
                r2: Some((signed_in, new_bucket)),
                ..Default::default()
            })
        });
    }

    fn connection_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Bucket");
        egui::Grid::new("connection").num_columns(2).show(ui, |ui| {
            ui.label("Provider")
                .on_hover_text("Where the bucket is. The app fills in the rest of the address.");
            let mut picked = self.provider;
            egui::ComboBox::from_id_salt("provider")
                .selected_text(picked.label())
                .show_ui(ui, |ui| {
                    for provider in Provider::ALL {
                        ui.selectable_value(&mut picked, provider, provider.label());
                    }
                });
            ui.end_row();
            self.pick_provider(picked);

            match self.provider {
                Provider::Aws => region_field(ui, &mut self.settings.region, "eu-central-1"),
                Provider::Ovh => region_field(ui, &mut self.settings.region, "gra, sbg or de"),
                Provider::Minio => field(
                    ui,
                    "Endpoint",
                    "The MinIO server's address, for example http://localhost:9000.",
                    egui::TextEdit::singleline(&mut self.settings.endpoint),
                ),
                Provider::CloudflareR2 => self.r2_fields(ui),
                Provider::Other => raw_fields(ui, &mut self.settings),
            }

            self.bucket_field(ui);
            let settings = &mut self.settings;
            field(
                ui,
                "Key prefix",
                "The folder in the bucket that holds Gutch's objects, such as gutch. \
                 Leave it empty to use the bucket root.",
                egui::TextEdit::singleline(&mut settings.prefix),
            );
            if self.provider != Provider::CloudflareR2 {
                field(
                    ui,
                    "Access key ID",
                    "The ID of an access key that may read, write, list and delete objects \
                     below the prefix. Filled in from AWS_ACCESS_KEY_ID when it is set.",
                    egui::TextEdit::singleline(&mut settings.access_key_id),
                );
                field(
                    ui,
                    "Secret key",
                    "The access key's secret. It is kept in memory only and never saved; \
                     set AWS_SECRET_ACCESS_KEY to avoid typing it each time.",
                    egui::TextEdit::singleline(&mut settings.secret_access_key).password(true),
                );
            }
            field(
                ui,
                "Local cache",
                "Where the app keeps a local copy of each repository it opens. \
                 Captures go into it before upload, and installs read from it. \
                 Deleting it is safe: the app downloads again what it needs.",
                egui::TextEdit::singleline(&mut self.cache_dir),
            );
        });
        ui.horizontal(|ui| {
            if ui
                .button("Connect")
                .on_hover_text("Sign in to the bucket and list its repositories.")
                .clicked()
            {
                self.connect(ui.ctx());
            }
            if ui
                .button("Test connection")
                .on_hover_text(
                    "Check that the keys may list, write and delete objects below the prefix, \
                     and that the bucket supports conditional writes. \
                     Writes one small test object and deletes it.",
                )
                .clicked()
            {
                self.test_connection(ui.ctx());
            }
        });
    }

    fn r2_fields(&mut self, ui: &mut egui::Ui) {
        let r2 = &mut self.r2;
        field(
            ui,
            "API token",
            "A Cloudflare API token with the Workers R2 Storage Write permission. \
             The app derives the S3 keys from it. It is kept in memory only and never \
             saved; set CLOUDFLARE_API_TOKEN to avoid pasting it each time.",
            egui::TextEdit::singleline(&mut r2.token).password(true),
        );
        ui.label("");
        ui.hyperlink_to("Create token", cloudflare::TOKEN_PAGE)
            .on_hover_text(
                "Open the Cloudflare dashboard. Under R2, choose Manage API tokens, \
                 create a token with Admin Read & Write, and copy the token value.",
            );
        ui.end_row();
        ui.label("Account")
            .on_hover_text("Found by Sign in. You can also paste the account ID from the R2 page.");
        if r2.accounts.len() > 1 {
            let selected = r2
                .accounts
                .iter()
                .find(|account| account.id == r2.account_id)
                .map_or(r2.account_id.clone(), |account| account.name.clone());
            egui::ComboBox::from_id_salt("r2_account")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for account in &r2.accounts {
                        ui.selectable_value(&mut r2.account_id, account.id.clone(), &account.name);
                    }
                });
        } else {
            ui.text_edit_singleline(&mut r2.account_id);
        }
        ui.end_row();
        ui.label("Jurisdiction")
            .on_hover_text("Buckets created for the EU or FedRAMP live at their own address.");
        egui::ComboBox::from_id_salt("r2_jurisdiction")
            .selected_text(r2.jurisdiction.as_str())
            .show_ui(ui, |ui| {
                for jurisdiction in R2_JURISDICTIONS {
                    ui.selectable_value(&mut r2.jurisdiction, jurisdiction.into(), jurisdiction);
                }
            });
        ui.end_row();
        ui.label("");
        if ui
            .button("Sign in")
            .on_hover_text("Find the account and its buckets, and derive the S3 keys.")
            .clicked()
        {
            self.r2_sign_in(ui.ctx(), None);
        }
        ui.end_row();
    }

    fn bucket_field(&mut self, ui: &mut egui::Ui) {
        let tooltip = "The bucket that holds the repositories.";
        if self.provider == Provider::CloudflareR2 && !self.r2.buckets.is_empty() {
            ui.label("Bucket").on_hover_text(tooltip);
            egui::ComboBox::from_id_salt("bucket")
                .selected_text(self.settings.bucket.as_str())
                .show_ui(ui, |ui| {
                    for bucket in &self.r2.buckets {
                        ui.selectable_value(&mut self.settings.bucket, bucket.clone(), bucket);
                    }
                });
            ui.end_row();
        } else {
            field(
                ui,
                "Bucket",
                tooltip,
                egui::TextEdit::singleline(&mut self.settings.bucket),
            );
        }
        if self.provider == Provider::CloudflareR2 {
            ui.label("New bucket")
                .on_hover_text("Create an R2 bucket with this name and select it.");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.r2.new_bucket)
                        .desired_width(140.0)
                        .hint_text("lowercase-name"),
                );
                let name = self.r2.new_bucket.trim().to_owned();
                if ui
                    .add_enabled(!name.is_empty(), egui::Button::new("Create"))
                    .clicked()
                {
                    self.r2.new_bucket.clear();
                    self.r2_sign_in(ui.ctx(), Some(name));
                }
            });
            ui.end_row();
        }
    }

    fn repos_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Repositories");
        let mut opened = None;
        for repo in &self.repos {
            let selected = self.repo_id.as_deref() == Some(&repo.repository_id);
            if ui
                .selectable_label(selected, &repo.repository_id)
                .on_hover_text("Open this repository and list its snapshots.")
                .clicked()
            {
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
            )
            .on_hover_text(
                "Groups repositories, for example by team or product. \
                 The repository ID becomes SCOPE--NAME.",
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.new_name)
                    .hint_text("name")
                    .desired_width(110.0),
            )
            .on_hover_text("The repository's name within the scope.");
        });
        if ui
            .button("Create")
            .on_hover_text("Create an empty repository in the bucket.")
            .clicked()
        {
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
        self.files = None;
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
                        ui.strong("Snapshot").on_hover_text(
                            "A snapshot is a captured folder. Its ID is a hash of the \
                             content, so the same content always has the same ID.",
                        );
                        ui.strong("Tags");
                        ui.strong("Created");
                        ui.end_row();
                        for snapshot in &self.snapshots {
                            let id = &snapshot.snapshot_id;
                            let selected = self.snapshot_id.as_ref() == Some(id);
                            if ui
                                .selectable_label(selected, short(id))
                                .on_hover_text(id)
                                .clicked()
                            {
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
            if ui
                .button("Source folder…")
                .on_hover_text("The folder to capture, with everything in it.")
                .clicked()
            {
                self.source = rfd::FileDialog::new().pick_folder().or(self.source.take());
            }
            ui.label(path_label(&self.source));
        });
        ui.horizontal(|ui| {
            ui.label("Tags");
            ui.text_edit_singleline(&mut self.tags).on_hover_text(
                "key=value pairs that describe the snapshot, separated by spaces, \
                 for example version=1.2 os=windows.",
            );
        });
        let ready = self.source.is_some();
        if ui
            .add_enabled(ready, egui::Button::new("Capture and push"))
            .on_hover_text(
                "Capture the folder as a snapshot and upload it. Only content the \
                 bucket does not already have is uploaded.",
            )
            .on_disabled_hover_text("Pick a source folder first.")
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
            if ui
                .button("Destination folder…")
                .on_hover_text("The folder to install into, verify or repair.")
                .clicked()
            {
                self.destination = rfd::FileDialog::new()
                    .pick_folder()
                    .or(self.destination.take());
            }
            ui.label(path_label(&self.destination));
        });
        ui.horizontal(|ui| {
            ui.label("Already installed there").on_hover_text(
                "The snapshot the destination holds now. Installing a newer snapshot \
                 then keeps the files that did not change and only writes the rest. \
                 Choose nothing for an empty folder.",
            );
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
            if ui
                .button("Install")
                .on_hover_text(
                    "Make the destination match the selected snapshot, downloading \
                     what is missing from the bucket.",
                )
                .clicked()
            {
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
            if ui
                .button("Verify")
                .on_hover_text(
                    "Check the destination against the selected snapshot and count \
                     missing, changed (corrupt) and extra files. Changes nothing.",
                )
                .clicked()
            {
                let (repo_id, snapshot_id, destination) =
                    (repo_id.clone(), snapshot_id.clone(), destination.clone());
                self.start(ui.ctx(), "Verify", move |remote, control| {
                    Ok(Outcome {
                        message: remote.verify(&repo_id, &snapshot_id, &destination, control)?,
                        ..Default::default()
                    })
                });
            }
            if ui
                .button("Repair")
                .on_hover_text(
                    "Restore missing and changed files from the selected snapshot. \
                     Files the snapshot does not have are left alone.",
                )
                .clicked()
            {
                self.start(ui.ctx(), "Repair", move |remote, control| {
                    Ok(Outcome {
                        message: remote.repair(&repo_id, &snapshot_id, &destination, control)?,
                        ..Default::default()
                    })
                });
            }
        });
    }

    fn files_section(&mut self, ui: &mut egui::Ui, repo_id: &str) {
        ui.heading("Files of the selected snapshot");
        let Some(snapshot_id) = self.snapshot_id.clone() else {
            ui.label("Select a snapshot.");
            return;
        };
        if ui
            .button("List files")
            .on_hover_text(
                "Download this snapshot's file list, without any file data, into \
                 the local cache and list its files.",
            )
            .clicked()
        {
            let (repo_id, snapshot_id) = (repo_id.to_owned(), snapshot_id.clone());
            self.start(ui.ctx(), "List files", move |remote, control| {
                let files = remote.files(&repo_id, &snapshot_id, control)?;
                Ok(Outcome {
                    message: format!("{} has {} files", short(&snapshot_id), files.len()),
                    files: Some((snapshot_id, files)),
                    ..Default::default()
                })
            });
        }
        let Some((_, files)) = self.files.as_ref().filter(|(id, _)| *id == snapshot_id) else {
            return;
        };
        let mut save = None;
        egui::ScrollArea::vertical()
            .id_salt("files")
            .max_height(200.0)
            .show(ui, |ui| {
                egui::Grid::new("files")
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        for (path, size) in files {
                            ui.label(path);
                            ui.label(format!("{size} bytes"));
                            if ui
                                .button("Save a copy…")
                                .on_hover_text(
                                    "Write this one file anywhere on your computer. Only \
                                     the data blocks holding it are downloaded, and the file \
                                     is checked against its hashes. Nothing is installed.",
                                )
                                .clicked()
                            {
                                save = Some(path.clone());
                            }
                            ui.end_row();
                        }
                    });
            });
        let Some(path) = save else {
            return;
        };
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let Some(target) = rfd::FileDialog::new().set_file_name(name).save_file() else {
            return;
        };
        let repo_id = repo_id.to_owned();
        self.start(ui.ctx(), "Save", move |remote, control| {
            Ok(Outcome {
                message: remote.save_file(&repo_id, &snapshot_id, &path, &target, control)?,
                ..Default::default()
            })
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        if let Some(job) = &self.job {
            ui.horizontal(|ui| {
                if ui
                    .button("Cancel")
                    .on_hover_text("Stop the operation as soon as it can stop safely.")
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
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SAVED_KEY, &self.saved());
    }

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
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_enabled_ui(idle, |ui| {
                    self.snapshots_table(ui);
                    ui.separator();
                    self.capture_section(ui, &repo_id);
                    ui.separator();
                    self.install_section(ui, &repo_id);
                    ui.separator();
                    self.files_section(ui, &repo_id);
                });
            });
        });
    }
}

/// One labelled row of a grid. The tooltip shows on both label and field.
fn region_field(ui: &mut egui::Ui, region: &mut String, example: &str) {
    field(
        ui,
        "Region",
        &format!("The bucket's region, for example {example}."),
        egui::TextEdit::singleline(region),
    );
}

/// Every S3 setting, for providers without a preset.
fn raw_fields(ui: &mut egui::Ui, settings: &mut S3Settings) {
    field(
        ui,
        "Endpoint",
        "The S3 service address, for example https://s3.example.com.",
        egui::TextEdit::singleline(&mut settings.endpoint),
    );
    field(
        ui,
        "Region",
        "The bucket's region. Most servers without regions accept us-east-1.",
        egui::TextEdit::singleline(&mut settings.region),
    );
    ui.label("");
    ui.checkbox(&mut settings.path_style, "Path-style URLs")
        .on_hover_text(
            "Address objects as endpoint/bucket/key instead of bucket.endpoint/key. \
             Turn it on for servers without bucket subdomains.",
        );
    ui.end_row();
}

fn field(ui: &mut egui::Ui, label: &str, tooltip: &str, widget: impl egui::Widget) {
    ui.label(label).on_hover_text(tooltip);
    ui.add(widget).on_hover_text(tooltip);
    ui.end_row();
}

fn variable(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// The platform's cache folder: `%LOCALAPPDATA%` on Windows,
/// `~/Library/Caches` on macOS, `$XDG_CACHE_HOME` or `~/.cache` on Linux.
fn default_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Gutch S3")
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
