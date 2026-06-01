use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use dialoguer::console::Term;
use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};

use crate::{RepoCommands, TargetCommands, TargetInstallCommands};
use gutch as g;

#[derive(Clone)]
struct InteractiveRuntimeOptions {
    memory_mode: g::MemoryMode,
    max_memory_mib: Option<u64>,
    jobs: Option<usize>,
}

static LAST_RAN_COMMAND: OnceLock<Mutex<Option<String>>> = OnceLock::new();

#[derive(Clone)]
enum RepoSelectionAction {
    CreateRepo,
    OpenRepo(PathBuf),
    ShowAllTrackedRepos,
    EnterCustomRepoPath,
    Exit,
}

#[derive(Clone)]
enum RepoWorkAction {
    TargetInsert,
    TargetExtract,
    TargetInstallApply,
    TargetInstallVerify,
    RepoInspect,
    RepoSyncUp,
    Advanced,
    SwitchRepo,
    Exit,
}

#[derive(Clone)]
enum AdvancedAction {
    RepoConfigEdit,
    RepoLink,
    RepoUnlink,
    SetTargetLookupPath,
    DiscoverTargets,
    Back,
}

pub(crate) fn run() -> g::Result<()> {
    let theme = ColorfulTheme::default();
    let mut active_repo: Option<PathBuf> = None;

    loop {
        if active_repo.is_none() {
            match prompt_repo_selection(&theme)? {
                RepoSelectionAction::Exit => return Ok(()),
                RepoSelectionAction::CreateRepo => {
                    let path = match run_repo_create(&theme) {
                        Ok(path) => path,
                        Err(err) => {
                            handle_action_error(err)?;
                            continue;
                        }
                    };
                    if prompt_approval_action(&theme, "Work on this repo?")? {
                        print_repo_summary(&path)?;
                        active_repo = Some(path);
                    }
                }
                RepoSelectionAction::OpenRepo(repo) => {
                    if let Some(repo) = resolve_active_repo_from_selection(&theme, repo, false)? {
                        active_repo = Some(repo);
                    }
                }
                RepoSelectionAction::ShowAllTrackedRepos => {
                    let repo = match prompt_all_tracked_repos(&theme, &g::config::load_config()?) {
                        Ok(repo) => repo,
                        Err(err) => {
                            handle_action_error(err)?;
                            continue;
                        }
                    };
                    if let Some(repo) = resolve_active_repo_from_selection(&theme, repo, false)? {
                        active_repo = Some(repo);
                    }
                }
                RepoSelectionAction::EnterCustomRepoPath => {
                    let repo = match custom_repo_path_input(&theme) {
                        Ok(repo) => repo,
                        Err(err) => {
                            handle_action_error(err)?;
                            continue;
                        }
                    };
                    if let Some(repo) = resolve_active_repo_from_selection(&theme, repo, true)? {
                        active_repo = Some(repo);
                    }
                }
            }
        } else {
            let repo = active_repo.as_ref().unwrap();
            match prompt_repo_work_action(&theme, repo)? {
                RepoWorkAction::Exit => return Ok(()),
                RepoWorkAction::SwitchRepo => {
                    active_repo = None;
                }
                RepoWorkAction::TargetInsert => {
                    if let Err(err) = run_target_insert(&theme, repo) {
                        handle_action_error(err)?;
                    }
                }
                RepoWorkAction::TargetExtract => {
                    if let Err(err) = run_target_extract(&theme, repo) {
                        handle_action_error(err)?;
                    }
                }
                RepoWorkAction::TargetInstallApply => {
                    if let Err(err) = run_target_install_apply(&theme, repo) {
                        handle_action_error(err)?;
                    }
                }
                RepoWorkAction::TargetInstallVerify => {
                    if let Err(err) = run_target_install_verify(&theme, repo) {
                        handle_action_error(err)?;
                    }
                }
                RepoWorkAction::RepoInspect => {
                    run_repo_inspect(repo)?;
                }
                RepoWorkAction::RepoSyncUp => {
                    if let Err(err) = run_interactive_repo_sync_up(&theme, repo) {
                        handle_action_error(err)?;
                    }
                }
                RepoWorkAction::Advanced => loop {
                    match prompt_advanced_action(&theme, repo)? {
                        AdvancedAction::Back => break,
                        AdvancedAction::RepoConfigEdit => {
                            if let Err(err) = run_repo_config_edit(&theme, repo) {
                                handle_action_error(err)?;
                            }
                        }
                        AdvancedAction::RepoLink => {
                            if let Err(err) = run_interactive_repo_link(&theme, repo) {
                                handle_action_error(err)?;
                            }
                        }
                        AdvancedAction::RepoUnlink => {
                            if let Err(err) = run_interactive_repo_unlink(&theme, repo) {
                                handle_action_error(err)?;
                            }
                        }
                        AdvancedAction::SetTargetLookupPath => {
                            if let Err(err) = run_set_target_lookup_path(&theme, repo) {
                                handle_action_error(err)?;
                            }
                        }
                        AdvancedAction::DiscoverTargets => {
                            if let Err(err) = run_discover_targets(&theme, repo) {
                                handle_action_error(err)?;
                            }
                        }
                    }
                },
            }
        }
    }
}

fn handle_action_error(err: g::Error) -> g::Result<()> {
    if is_cancelled(&err) {
        return Ok(());
    }
    Err(err)
}

fn is_cancelled(err: &g::Error) -> bool {
    matches!(err, g::Error::Manifest(msg) if msg.contains("cancelled"))
}

fn last_ran_command_store() -> &'static Mutex<Option<String>> {
    LAST_RAN_COMMAND.get_or_init(|| Mutex::new(None))
}

fn set_last_ran_command(command: impl Into<String>) {
    if let Ok(mut last_ran_command) = last_ran_command_store().lock() {
        *last_ran_command = Some(command.into());
    }
}

fn print_last_ran_command() {
    let Ok(last_ran_command) = last_ran_command_store().lock() else {
        return;
    };
    if let Some(last_ran_command) = last_ran_command.as_deref() {
        println!("Last ran command: {}", last_ran_command);
        println!();
    }
}

fn refresh_terminal() -> g::Result<()> {
    Term::stdout()
        .clear_screen()
        .map_err(|err| g::Error::manifest(format!("failed to refresh terminal: {}", err)))
}

fn prompt_approval_action(theme: &ColorfulTheme, prompt: impl AsRef<str>) -> g::Result<bool> {
    let actions = [("- Approve", true), ("- Cancel", false)];
    let labels: Vec<&str> = actions.iter().map(|(label, _)| *label).collect();
    let selection = Select::with_theme(theme)
        .with_prompt(prompt.as_ref())
        .items(&labels)
        .default(1)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;
    Ok(actions[selection].1)
}

fn prompt_repo_selection(theme: &ColorfulTheme) -> g::Result<RepoSelectionAction> {
    let config = g::config::load_config()?;
    const PAGE_SIZE: usize = 10;
    let total = config.repos.len();
    let truncated = total > PAGE_SIZE;
    let visible_repos = if truncated { PAGE_SIZE } else { total };

    loop {
        refresh_terminal()?;
        print_last_ran_command();
        print_landing_info_table(&config);

        let mut labels: Vec<String> = config
            .repos
            .iter()
            .enumerate()
            .take(visible_repos)
            .map(|(index, repo)| render_tracked_repo_label(index, repo))
            .collect();
        if truncated {
            labels.push(format!("- Show all {} tracked repos...", total));
        }
        labels.push("- Create a gutch repo".to_string());
        labels.push("- Enter a custom repo path...".to_string());
        labels.push("- Exit".to_string());

        let selection = Select::with_theme(theme)
            .with_prompt("Gutch interactive — no repo selected")
            .items(&labels)
            .default(0)
            .interact()
            .map_err(dialoguer_to_manifest_error)?;

        if selection < visible_repos {
            let repo = &config.repos[selection];
            return Ok(RepoSelectionAction::OpenRepo(PathBuf::from(&repo.path)));
        }
        let mut next_index = visible_repos;
        if truncated {
            if selection == next_index {
                return Ok(RepoSelectionAction::ShowAllTrackedRepos);
            }
            next_index += 1;
        }
        if selection == next_index {
            return Ok(RepoSelectionAction::CreateRepo);
        }
        if selection == next_index + 1 {
            return Ok(RepoSelectionAction::EnterCustomRepoPath);
        }
        return Ok(RepoSelectionAction::Exit);
    }
}

fn prompt_all_tracked_repos(
    theme: &ColorfulTheme,
    config: &g::config::GutchConfig,
) -> g::Result<std::path::PathBuf> {
    refresh_terminal()?;
    print_last_ran_command();

    let labels: Vec<String> = config
        .repos
        .iter()
        .enumerate()
        .map(|(index, repo)| render_tracked_repo_label(index, repo))
        .collect();
    let mut labels = labels;
    labels.push("- Back".to_string());
    let selection = Select::with_theme(theme)
        .with_prompt("Select a tracked repo")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;
    if selection == labels.len().saturating_sub(1) {
        return Err(g::Error::manifest("interactive command cancelled"));
    }
    Ok(std::path::PathBuf::from(&config.repos[selection].path))
}

fn custom_repo_path_input(theme: &ColorfulTheme) -> g::Result<std::path::PathBuf> {
    path_or_browse_input(theme, "Repo path", ".")
}

fn render_tracked_repo_label(index: usize, repo: &g::config::TrackedRepo) -> String {
    let linked_prefix = match &repo.linked {
        Some(linked) => format!(
            "[linked to {}/{}]",
            linked.product_id, linked.remote_repo_slug
        ),
        None => "[not linked]".to_string(),
    };
    let lookup_marker = if repo.target_lookup_path.is_some() {
        " [lookup]"
    } else {
        ""
    };
    format!(
        "- [Repo-{}] {} | {}{}",
        index, linked_prefix, repo.path, lookup_marker
    )
}

fn print_landing_info_table(config: &g::config::GutchConfig) {
    let tracked_repo_count = config.repos.len();
    let linked_repo_count = config
        .repos
        .iter()
        .filter(|repo| repo.linked.is_some())
        .count();
    let lookup_path_count = config
        .repos
        .iter()
        .filter(|repo| repo.target_lookup_path.is_some())
        .count();

    let rows = vec![
        ("tracked repos", tracked_repo_count.to_string()),
        ("linked repos", linked_repo_count.to_string()),
        ("lookup paths", lookup_path_count.to_string()),
    ];

    println!();
    print_ascii_key_value_table(&rows);
    println!();
}

fn print_active_repo_info_table(repo: &std::path::Path) -> g::Result<()> {
    print_last_ran_command();

    let repo_path = repo.display().to_string();
    let config = g::config::load_config()?;
    let tracked = config.find_repo(&repo_path);
    let (_, repo_manifest) = g::load_gutch_repo_manifest(repo)?;

    let linked_value = match tracked.and_then(|tracked_repo| tracked_repo.linked.as_ref()) {
        Some(linked) => format!("{} / {}", linked.product_id, linked.remote_repo_slug),
        None => "not linked".to_string(),
    };
    let lookup_value = tracked
        .and_then(|tracked_repo| tracked_repo.target_lookup_path.as_deref())
        .unwrap_or("-");

    let rows = vec![
        ("repo id", repo_manifest.gutch_repo_id),
        ("path", repo.display().to_string()),
        ("linked", linked_value),
        ("lookup path", lookup_value.to_string()),
    ];

    println!();
    print_ascii_key_value_table(&rows);
    println!();
    Ok(())
}

fn print_ascii_key_value_table(rows: &[(&str, String)]) {
    let key_width = rows.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
    let value_width = rows.iter().map(|(_, value)| value.len()).max().unwrap_or(0);

    println!(
        "+-{}-+-{}-+",
        "-".repeat(key_width),
        "-".repeat(value_width)
    );
    for (key, value) in rows {
        println!(
            "| {:<key_width$} | {:<value_width$} |",
            key,
            value,
            key_width = key_width,
            value_width = value_width
        );
    }
    println!(
        "+-{}-+-{}-+",
        "-".repeat(key_width),
        "-".repeat(value_width)
    );
}

fn print_ascii_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers.iter().map(|header| header.len()).collect();
    for row in rows {
        for (index, value) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(value.len());
            }
        }
    }

    let separator = format!(
        "+-{}-+",
        widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<_>>()
            .join("-+-")
    );

    println!("{}", separator);
    println!(
        "| {} |",
        headers
            .iter()
            .enumerate()
            .map(|(index, header)| format!("{:<width$}", header, width = widths[index]))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    println!("{}", separator);
    for row in rows {
        println!(
            "| {} |",
            row.iter()
                .enumerate()
                .map(|(index, value)| format!("{:<width$}", value, width = widths[index]))
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }
    println!("{}", separator);
}

fn display_table_value(value: &str) -> String {
    if value.is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

fn format_tag_column_name(schema: &g::manifest::TargetTagSchema, name: &str) -> String {
    match schema
        .find_definition(name)
        .map(|definition| &definition.kind)
    {
        Some(g::manifest::TargetTagKind::Plain) => name.to_string(),
        Some(g::manifest::TargetTagKind::Scoped) => format!("{}:", name),
        Some(g::manifest::TargetTagKind::Exclusive) => format!("{}::", name),
        None => name.to_string(),
    }
}

fn relative_candidate_path(lookup_path: &std::path::Path, candidate_path: &str) -> String {
    let candidate_path = std::path::Path::new(candidate_path);
    candidate_path
        .strip_prefix(lookup_path)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| candidate_path.display().to_string())
}

fn resolve_active_repo_from_selection(
    theme: &ColorfulTheme,
    repo: PathBuf,
    allow_retry: bool,
) -> g::Result<Option<PathBuf>> {
    match g::load_gutch_repo_manifest(&repo) {
        Err(err) => {
            println!("Not a valid gutch repo at {}: {}", repo.display(), err);
            if allow_retry
                && Confirm::with_theme(theme)
                    .with_prompt("Try another path?")
                    .default(true)
                    .interact()
                    .map_err(dialoguer_to_manifest_error)?
            {
                return Ok(None);
            }
            Ok(None)
        }
        Ok(_) => {
            let mut config = g::config::load_config()?;
            let repo_path_str = repo.display().to_string();
            if config.find_repo(&repo_path_str).is_none() {
                config.upsert_repo(g::config::TrackedRepo {
                    path: repo_path_str,
                    target_lookup_path: None,
                    linked: None,
                });
                g::config::save_config(&config)?;
            }
            Ok(Some(repo))
        }
    }
}

fn path_or_browse_input(
    theme: &ColorfulTheme,
    prompt: &str,
    default: &str,
) -> g::Result<std::path::PathBuf> {
    let prompt_text = format!("{} — how to select?", prompt);
    let actions = [
        ("- Browse folders...", 0),
        ("- Type path manually", 1),
        ("- Back", 2),
    ];
    let labels: Vec<&str> = actions.iter().map(|(label, _)| *label).collect();
    let selection = Select::with_theme(theme)
        .with_prompt(&prompt_text)
        .items(&labels)
        .default(0)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;

    match actions[selection].1 {
        0 => match rfd::FileDialog::new().pick_folder() {
            Some(path) => Ok(path),
            None => Err(g::Error::manifest("interactive command cancelled")),
        },
        1 => path_input(theme, prompt, default),
        2 => Err(g::Error::manifest("interactive command cancelled")),
        _ => unreachable!(),
    }
}

fn optional_path_or_browse_input(
    theme: &ColorfulTheme,
    prompt: &str,
    default: &str,
) -> g::Result<Option<std::path::PathBuf>> {
    let prompt_text = format!("{} — how to select?", prompt);
    let actions = [
        ("- Browse folders...", 0),
        ("- Type path manually", 1),
        ("- Skip (leave empty)", 2),
        ("- Back", 3),
    ];
    let labels: Vec<&str> = actions.iter().map(|(label, _)| *label).collect();
    let selection = Select::with_theme(theme)
        .with_prompt(&prompt_text)
        .items(&labels)
        .default(0)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;

    match actions[selection].1 {
        0 => match rfd::FileDialog::new().pick_folder() {
            Some(path) => Ok(Some(path)),
            None => Err(g::Error::manifest("interactive command cancelled")),
        },
        1 => optional_path_input(theme, prompt, default),
        2 => Ok(None),
        3 => Err(g::Error::manifest("interactive command cancelled")),
        _ => unreachable!(),
    }
}

fn path_or_browse_input_file(
    theme: &ColorfulTheme,
    prompt: &str,
    default: &str,
) -> g::Result<std::path::PathBuf> {
    let prompt_text = format!("{} — how to select?", prompt);
    let actions = [
        ("- Browse folders...", 0),
        ("- Browse files...", 1),
        ("- Type path manually", 2),
        ("- Back", 3),
    ];
    let labels: Vec<&str> = actions.iter().map(|(label, _)| *label).collect();
    let selection = Select::with_theme(theme)
        .with_prompt(&prompt_text)
        .items(&labels)
        .default(0)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;

    match actions[selection].1 {
        0 => match rfd::FileDialog::new().pick_folder() {
            Some(path) => Ok(path),
            None => Err(g::Error::manifest("interactive command cancelled")),
        },
        1 => match rfd::FileDialog::new().pick_file() {
            Some(path) => Ok(path),
            None => Err(g::Error::manifest("interactive command cancelled")),
        },
        2 => path_input(theme, prompt, default),
        3 => Err(g::Error::manifest("interactive command cancelled")),
        _ => unreachable!(),
    }
}

fn prompt_repo_work_action(
    theme: &ColorfulTheme,
    repo: &std::path::Path,
) -> g::Result<RepoWorkAction> {
    refresh_terminal()?;
    print_active_repo_info_table(repo)?;

    let actions = [
        ("- Insert target from path", RepoWorkAction::TargetInsert),
        ("- Extract target to path", RepoWorkAction::TargetExtract),
        (
            "- Install target to path",
            RepoWorkAction::TargetInstallApply,
        ),
        (
            "- Verify target install",
            RepoWorkAction::TargetInstallVerify,
        ),
        ("- Inspect repo targets", RepoWorkAction::RepoInspect),
        ("- Sync-up to remote repo", RepoWorkAction::RepoSyncUp),
        ("- Advanced...", RepoWorkAction::Advanced),
        ("- Switch to a different repo", RepoWorkAction::SwitchRepo),
        ("- Exit", RepoWorkAction::Exit),
    ];
    let labels: Vec<&str> = actions.iter().map(|(label, _)| *label).collect();
    let selection = Select::with_theme(theme)
        .with_prompt(format!("Working on repo: {}", repo.display()))
        .items(&labels)
        .default(0)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;
    Ok(actions[selection].1.clone())
}

fn prompt_advanced_action(
    theme: &ColorfulTheme,
    repo: &std::path::Path,
) -> g::Result<AdvancedAction> {
    refresh_terminal()?;
    print_active_repo_info_table(repo)?;

    let actions = [
        (
            "- Discover and insert targets from lookup path",
            AdvancedAction::DiscoverTargets,
        ),
        (
            "- Edit repo config and tag inference",
            AdvancedAction::RepoConfigEdit,
        ),
        ("- Link to remote product repo", AdvancedAction::RepoLink),
        (
            "- Unlink from remote product repo",
            AdvancedAction::RepoUnlink,
        ),
        (
            "- Set target lookup path",
            AdvancedAction::SetTargetLookupPath,
        ),
        ("- Back", AdvancedAction::Back),
    ];
    let labels: Vec<&str> = actions.iter().map(|(label, _)| *label).collect();
    let selection = Select::with_theme(theme)
        .with_prompt(format!("Advanced — repo: {}", repo.display()))
        .items(&labels)
        .default(0)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;
    Ok(actions[selection].1.clone())
}

#[derive(Clone)]
enum TaggingTemplate {
    Standard,
    Minimal,
}

fn run_repo_create(theme: &ColorfulTheme) -> g::Result<PathBuf> {
    let cwd = std::env::current_dir()
        .map_err(|err| g::Error::manifest(format!("failed to get current directory: {}", err)))?;
    let cwd_with_slash = format!("{}_", cwd.display()).replace("_", std::path::MAIN_SEPARATOR_STR);

    let path = path_or_browse_input(theme, "Repo path", &cwd_with_slash)?;

    if path.exists() && path.is_dir() {
        let contents: Vec<String> = std::fs::read_dir(&path)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if entry.path().is_dir() {
                            format!("{}/", name)
                        } else {
                            name
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !contents.is_empty() {
            println!();
            println!(
                "WARNING: '{}' already exists with contents:",
                path.display()
            );
            for item in &contents {
                println!("  {}", item);
            }
            let proceed =
                prompt_approval_action(theme, "Existing contents may be overwritten. Continue?")?;
            if !proceed {
                return Err(g::Error::manifest("repo creation cancelled"));
            }
        }
    }

    let template = prompt_tagging_template(theme)?;

    let preview = format!("gutch repo init --path {}", shell_arg_path(&path));
    confirm_preview(theme, &preview)?;

    crate::cli::repo::run(RepoCommands::Init {
        path: path.clone(),
        schema: None,
    })?;

    let (repo_paths, mut repo_manifest) = g::load_gutch_repo_manifest(&path)?;
    repo_manifest.target_schema = Some(match template {
        TaggingTemplate::Standard => g::manifest::TargetTagSchema::default(),
        TaggingTemplate::Minimal => minimal_tagging_template(),
    });
    g::write_gutch_repo_manifest(&repo_paths, &repo_manifest)?;

    let template_name = match template {
        TaggingTemplate::Standard => "standard (version::, os::, arch::, tier::)",
        TaggingTemplate::Minimal => "minimal (version:: only)",
    };
    println!("Applied tagging template: {}", template_name);
    println!("You can edit this later via 'Edit repo config'.");

    Ok(path)
}

fn prompt_tagging_template(theme: &ColorfulTheme) -> g::Result<TaggingTemplate> {
    let descriptions = [
        (
            "Standard — version::, os::, arch::, tier::. Suitable for most apps.",
            TaggingTemplate::Standard,
        ),
        (
            "Minimal — version:: only. For non-OS, non-platform-dependent assets (Python, JAR, PNG, JPG, blend files, etc.).",
            TaggingTemplate::Minimal,
        ),
    ];
    let labels: Vec<&str> = descriptions.iter().map(|(label, _)| *label).collect();
    let selection = Select::with_theme(theme)
        .with_prompt("Tagging template")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;
    Ok(descriptions[selection].1.clone())
}

fn minimal_tagging_template() -> g::manifest::TargetTagSchema {
    g::manifest::TargetTagSchema {
        target_tags: vec![g::manifest::TargetTagDefinition {
            name: "version".to_string(),
            kind: g::manifest::TargetTagKind::Exclusive,
            required: true,
            inference_prefixes: vec!["-".to_string(), "-v".to_string()],
            values: Vec::new(),
            default_value: None,
        }],
        on_incomplete: g::manifest::IncompleteInferenceBehavior::Error,
    }
}

fn run_repo_config_edit(theme: &ColorfulTheme, repo: &std::path::Path) -> g::Result<()> {
    let (repo_paths, mut repo_manifest) = g::load_gutch_repo_manifest(repo)?;

    loop {
        refresh_terminal()?;
        print_active_repo_info_table(repo)?;
        println!();
        print_repo_config_summary(&repo_manifest);
        println!();

        let actions = [
            "- Set incomplete inference behavior",
            "- Add or update tag definition",
            "- Remove tag definition",
            "- Edit sizing and compression defaults",
            "- Reset target schema to built-in defaults",
            "- Save and exit",
            "- Back",
        ];
        let selection = Select::with_theme(theme)
            .with_prompt("Repo config editor")
            .items(&actions)
            .default(0)
            .interact()
            .map_err(dialoguer_to_manifest_error)?;

        match selection {
            0 => edit_incomplete_behavior(theme, &mut repo_manifest)?,
            1 => add_or_update_tag_definition(theme, &mut repo_manifest)?,
            2 => remove_tag_definition(theme, &mut repo_manifest)?,
            3 => edit_repo_profiles(theme, &mut repo_manifest)?,
            4 => {
                if prompt_approval_action(theme, "Reset target schema to built-in defaults?")? {
                    repo_manifest.target_schema = Some(g::manifest::TargetTagSchema::default());
                }
            }
            5 => {
                println!();
                println!("This will write:");
                println!("  {}", repo_paths.repo_manifest_path.display());
                println!();
                if prompt_approval_action(theme, "Save repo config changes?")? {
                    g::write_gutch_repo_manifest(&repo_paths, &repo_manifest)?;
                    println!(
                        "updated repo config: {}",
                        repo_paths.repo_manifest_path.display()
                    );
                    return Ok(());
                }
            }
            6 => return Err(g::Error::manifest("interactive command cancelled")),
            _ => unreachable!(),
        }
    }
}

fn run_target_insert(theme: &ColorfulTheme, repo: &std::path::Path) -> g::Result<()> {
    let dir = path_or_browse_input_file(theme, "Input directory or file", ".")?;
    let version = required_string(theme, "Version", "1.0.0")?;
    let platform = required_string(theme, "Platform", "windows")?;
    let architecture = required_string(theme, "Architecture", "x86_64")?;
    let tier = required_string(theme, "Tier", "base")?;
    let tags = optional_tag_list(theme)?;
    let chunk_profile = prompt_chunk_profile(theme)?;
    let bundle_profile = prompt_bundle_profile(theme)?;
    let runtime = prompt_runtime_options(theme)?;
    let preview = format!(
        "gutch target insert --dir {} --repo {} --version {} --platform {} --architecture {} --tier {} --chunk-profile {} --bundle-profile {}{}{}",
        shell_arg_path(&dir),
        shell_arg_path(repo),
        shell_arg(&version),
        shell_arg(&platform),
        shell_arg(&architecture),
        shell_arg(&tier),
        artifact_sizing_str(chunk_profile),
        bundle_sizing_str(bundle_profile),
        tags_preview_suffix(&tags),
        runtime_preview_suffix(&runtime),
    );
    confirm_preview(theme, &preview)?;

    crate::cli::target::run(TargetCommands::Insert {
        dir,
        repo: repo.to_path_buf(),
        version: Some(version),
        platform: Some(platform),
        architecture: Some(architecture),
        tier: Some(tier),
        tags,
        chunk_profile,
        bundle_profile,
        zstd_level: 1,
        memory_mode: runtime.memory_mode,
        max_memory_mib: runtime.max_memory_mib,
        jobs: runtime.jobs,
        summary_out: None,
        stats_json_out: None,
    })
}

fn run_target_extract(theme: &ColorfulTheme, repo: &std::path::Path) -> g::Result<()> {
    let Some(target) = prompt_target_selection(theme, repo, "Select target to extract")? else {
        return Err(g::Error::manifest("interactive command cancelled"));
    };
    let output = path_or_browse_input(theme, "Output directory", "./extracted-target")?;
    confirm_output_path_overwrite(theme, &output, "Extract output path")?;
    let runtime = prompt_runtime_options(theme)?;
    let preview = format!(
        "gutch target extract --repo {} --target {} --output {}{}",
        shell_arg_path(repo),
        shell_arg(&target),
        shell_arg_path(&output),
        runtime_preview_suffix(&runtime),
    );
    confirm_preview(theme, &preview)?;

    crate::cli::target::run(TargetCommands::Extract {
        repo: repo.to_path_buf(),
        target,
        output,
        memory_mode: runtime.memory_mode,
        max_memory_mib: runtime.max_memory_mib,
        jobs: runtime.jobs,
        summary_out: None,
        stats_json_out: None,
    })
}

fn run_target_install_apply(theme: &ColorfulTheme, repo: &std::path::Path) -> g::Result<()> {
    let target = required_string(theme, "Target id", "")?;
    let dir = path_or_browse_input(theme, "Install directory", "./install")?;
    let staging_dir = if Confirm::with_theme(theme)
        .with_prompt("Specify a custom staging directory?")
        .default(false)
        .interact()
        .map_err(dialoguer_to_manifest_error)?
    {
        optional_path_or_browse_input(theme, "Staging directory", "")?
    } else {
        None
    };
    let source_target = optional_string_input(theme, "Source target id", "")?;
    let source_dir = if source_target.is_some() {
        optional_path_or_browse_input(theme, "Source install directory", "")?
    } else {
        None
    };
    let runtime = prompt_runtime_options(theme)?;
    let preview = format!(
        "gutch target_install apply --repo {} --target {} --dir {}{}{}{}",
        shell_arg_path(repo),
        shell_arg(&target),
        shell_arg_path(&dir),
        staging_dir
            .as_ref()
            .map(|path| format!(" --staging-dir {}", shell_arg_path(path)))
            .unwrap_or_default(),
        source_target
            .as_ref()
            .map(|value| format!(" --source-target {}", shell_arg(value)))
            .unwrap_or_default(),
        source_dir
            .as_ref()
            .map(|path| format!(" --source-dir {}", shell_arg_path(path)))
            .unwrap_or_default()
            + &runtime_preview_suffix(&runtime),
    );
    confirm_preview(theme, &preview)?;

    crate::cli::target_install::run(TargetInstallCommands::Apply {
        repo: repo.to_path_buf(),
        target,
        dir,
        staging_dir,
        source_target,
        source_dir,
        memory_mode: runtime.memory_mode,
        max_memory_mib: runtime.max_memory_mib,
        jobs: runtime.jobs,
        summary_out: None,
        stats_json_out: None,
    })
}

fn run_target_install_verify(theme: &ColorfulTheme, repo: &std::path::Path) -> g::Result<()> {
    let target = required_string(theme, "Target id", "")?;
    let dir = path_or_browse_input(theme, "Install directory", "./install")?;
    let runtime = prompt_runtime_options(theme)?;
    let preview = format!(
        "gutch target_install verify --repo {} --target {} --dir {}{}",
        shell_arg_path(repo),
        shell_arg(&target),
        shell_arg_path(&dir),
        runtime_preview_suffix(&runtime),
    );
    confirm_preview(theme, &preview)?;

    crate::cli::target_install::run(TargetInstallCommands::Verify {
        repo: repo.to_path_buf(),
        target,
        dir,
        memory_mode: runtime.memory_mode,
        max_memory_mib: runtime.max_memory_mib,
        jobs: runtime.jobs,
        summary_out: None,
        stats_json_out: None,
    })
}

fn prompt_target_selection(
    theme: &ColorfulTheme,
    repo: &std::path::Path,
    prompt: &str,
) -> g::Result<Option<String>> {
    let status = g::inspect_gutch_repo(repo)?;
    let target_schema = status.target_schema.as_ref();
    if status.targets.is_empty() {
        println!("No targets found in repo.");
        return Ok(None);
    }

    let mut labels: Vec<String> = status
        .targets
        .iter()
        .map(|target| {
            format!(
                "- {} | tags={} | files={} | missing_bundles={}",
                target.target_id,
                render_target_tag_summary(&target.target_tags, target_schema),
                target.file_count,
                target.missing_bundle_count
            )
        })
        .collect();
    labels.push("- Back".to_string());

    let selection = Select::with_theme(theme)
        .with_prompt(prompt)
        .items(&labels)
        .default(0)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;

    if selection == labels.len().saturating_sub(1) {
        return Ok(None);
    }

    Ok(Some(status.targets[selection].target_id.clone()))
}

fn confirm_output_path_overwrite(
    theme: &ColorfulTheme,
    path: &std::path::Path,
    label: &str,
) -> g::Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let path_has_contents = if path.is_dir() {
        std::fs::read_dir(path)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
    } else {
        true
    };
    if !path_has_contents {
        return Ok(());
    }

    let confirmed = prompt_approval_action(
        theme,
        format!(
            "{} '{}' already exists and may be overwritten. Continue?",
            label,
            path.display()
        ),
    )?;

    if confirmed {
        Ok(())
    } else {
        Err(g::Error::manifest("interactive command cancelled"))
    }
}

fn print_repo_config_summary(repo_manifest: &g::manifest::gutch_repo::GutchRepoManifest) {
    println!("Repo config summary");
    println!("  gutch repo      {}", repo_manifest.gutch_repo_id);
    println!(
        "  chunk profile     {}",
        artifact_sizing_str(repo_manifest.chunk_sizing_profile.preset)
    );
    println!(
        "  bundle profile    {}",
        bundle_sizing_str(repo_manifest.bundle_sizing_profile.preset)
    );
    println!(
        "  compression       {} / zstd level {}",
        repo_manifest.compression_profile.compression_unit,
        repo_manifest.compression_profile.zstd_level
    );

    let schema = repo_manifest.target_schema.clone().unwrap_or_default();
    println!(
        "  on incomplete     {}",
        incomplete_behavior_str(schema.on_incomplete)
    );
    println!("  tag definitions   {}", schema.target_tags.len());
    for definition in &schema.target_tags {
        let values = if definition.values.is_empty() {
            "-".to_string()
        } else {
            definition
                .values
                .iter()
                .map(render_tag_value_definition)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let inference_prefixes = if definition.inference_prefixes.is_empty() {
            "-".to_string()
        } else {
            definition.inference_prefixes.join(", ")
        };
        let default_value = definition.default_value.as_deref().unwrap_or("-");
        println!(
            "    - {} [{}] required={} prefixes={} default={} values={}",
            definition.name,
            target_tag_kind_str(definition.kind.clone()),
            definition.required,
            inference_prefixes,
            default_value,
            values
        );
    }
}

fn edit_incomplete_behavior(
    theme: &ColorfulTheme,
    repo_manifest: &mut g::manifest::gutch_repo::GutchRepoManifest,
) -> g::Result<()> {
    let schema = ensure_target_schema(repo_manifest);
    let options = [
        ("error", g::manifest::IncompleteInferenceBehavior::Error),
        ("warn", g::manifest::IncompleteInferenceBehavior::Warn),
    ];
    let labels: Vec<&str> = options.iter().map(|(label, _)| *label).collect();
    let default_index = match schema.on_incomplete {
        g::manifest::IncompleteInferenceBehavior::Error => 0,
        g::manifest::IncompleteInferenceBehavior::Warn => 1,
    };
    let selection = Select::with_theme(theme)
        .with_prompt("On incomplete inference")
        .items(&labels)
        .default(default_index)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;
    schema.on_incomplete = options[selection].1.clone();
    Ok(())
}

fn add_or_update_tag_definition(
    theme: &ColorfulTheme,
    repo_manifest: &mut g::manifest::gutch_repo::GutchRepoManifest,
) -> g::Result<()> {
    let schema = ensure_target_schema(repo_manifest);
    let existing_names: Vec<String> = schema
        .target_tags
        .iter()
        .map(|definition| definition.name.clone())
        .collect();
    let mut choices: Vec<String> = existing_names.clone();
    choices.push("Create new definition".to_string());
    let selection = Select::with_theme(theme)
        .with_prompt("Choose a tag definition to edit")
        .items(&choices)
        .default(choices.len().saturating_sub(1))
        .interact()
        .map_err(dialoguer_to_manifest_error)?;

    let existing_definition = if selection < existing_names.len() {
        schema.target_tags.get(selection).cloned()
    } else {
        None
    };

    let name = required_string(
        theme,
        "Tag name",
        existing_definition
            .as_ref()
            .map(|definition| definition.name.as_str())
            .unwrap_or(""),
    )?;
    let kind = prompt_target_tag_kind(
        theme,
        existing_definition
            .as_ref()
            .map(|definition| definition.kind.clone())
            .unwrap_or(g::manifest::TargetTagKind::Exclusive),
    )?;
    let required = Confirm::with_theme(theme)
        .with_prompt("Required?")
        .default(
            existing_definition
                .as_ref()
                .is_some_and(|definition| definition.required),
        )
        .interact()
        .map_err(dialoguer_to_manifest_error)?;
    let values = csv_list_input(
        theme,
        "Allowed values (value[alias1|alias2], comma-separated, optional)",
        existing_definition
            .as_ref()
            .map(render_tag_values)
            .unwrap_or_default()
            .as_str(),
    )?;
    let inference_prefixes = csv_list_input(
        theme,
        "Inference prefixes (comma-separated, optional)",
        existing_definition
            .as_ref()
            .map(|definition| definition.inference_prefixes.join(", "))
            .unwrap_or_else(|| match kind {
                g::manifest::TargetTagKind::Exclusive if name == "version" => "-, -v".to_string(),
                _ => "-".to_string(),
            })
            .as_str(),
    )?;
    let default_value = optional_string_input(
        theme,
        "Default value (optional)",
        existing_definition
            .as_ref()
            .and_then(|definition| definition.default_value.clone())
            .unwrap_or_default()
            .as_str(),
    )?;

    let new_definition = g::manifest::TargetTagDefinition {
        name: name.clone(),
        kind,
        required,
        inference_prefixes,
        values: parse_tag_value_definitions(&values)?,
        default_value,
    };

    if let Some(position) = schema
        .target_tags
        .iter()
        .position(|definition| definition.name == name)
    {
        schema.target_tags[position] = new_definition;
    } else {
        schema.target_tags.push(new_definition);
    }

    Ok(())
}

fn remove_tag_definition(
    theme: &ColorfulTheme,
    repo_manifest: &mut g::manifest::gutch_repo::GutchRepoManifest,
) -> g::Result<()> {
    let schema = ensure_target_schema(repo_manifest);
    if schema.target_tags.is_empty() {
        println!("No tag definitions to remove.");
        return Ok(());
    }

    let labels: Vec<String> = schema
        .target_tags
        .iter()
        .map(|definition| definition.name.clone())
        .collect();
    let selection = Select::with_theme(theme)
        .with_prompt("Choose a tag definition to remove")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;
    let removed = schema.target_tags.remove(selection);
    println!("Removed tag definition '{}'.", removed.name);
    Ok(())
}

fn edit_repo_profiles(
    theme: &ColorfulTheme,
    repo_manifest: &mut g::manifest::gutch_repo::GutchRepoManifest,
) -> g::Result<()> {
    repo_manifest.chunk_sizing_profile =
        g::manifest::ChunkSizingProfile::for_preset(prompt_chunk_profile(theme)?);
    repo_manifest.bundle_sizing_profile =
        g::manifest::BundleSizingProfile::for_preset(prompt_bundle_profile(theme)?);
    repo_manifest.compression_profile.compression_unit =
        g::manifest::COMPRESSION_UNIT_BUNDLE.to_string();
    repo_manifest.compression_profile.zstd_level = prompt_i32_input(
        theme,
        "Zstd compression level",
        repo_manifest
            .compression_profile
            .zstd_level
            .to_string()
            .as_str(),
    )?;
    Ok(())
}

fn prompt_runtime_options(theme: &ColorfulTheme) -> g::Result<InteractiveRuntimeOptions> {
    if !Confirm::with_theme(theme)
        .with_prompt("Configure runtime memory and worker options?")
        .default(false)
        .interact()
        .map_err(dialoguer_to_manifest_error)?
    {
        return Ok(InteractiveRuntimeOptions {
            memory_mode: g::MemoryMode::Auto,
            max_memory_mib: None,
            jobs: None,
        });
    }

    let memory_mode = prompt_memory_mode(theme)?;
    let max_memory_mib = optional_u64_input(theme, "Max memory MiB override", "")?;
    let jobs = optional_usize_input(theme, "Worker count override", "")?;
    Ok(InteractiveRuntimeOptions {
        memory_mode,
        max_memory_mib,
        jobs,
    })
}

fn prompt_memory_mode(theme: &ColorfulTheme) -> g::Result<g::MemoryMode> {
    let modes = [
        ("auto", g::MemoryMode::Auto),
        ("low", g::MemoryMode::Low),
        ("balanced", g::MemoryMode::Balanced),
        ("high", g::MemoryMode::High),
    ];
    let labels: Vec<&str> = modes.iter().map(|(label, _)| *label).collect();
    let selection = Select::with_theme(theme)
        .with_prompt("Memory mode")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;
    Ok(modes[selection].1)
}

fn prompt_chunk_profile(theme: &ColorfulTheme) -> g::Result<g::manifest::ArtifactSizingPreset> {
    let options = [
        ("small", g::manifest::ArtifactSizingPreset::Small),
        ("balanced", g::manifest::ArtifactSizingPreset::Balanced),
        ("large", g::manifest::ArtifactSizingPreset::Large),
    ];
    let labels: Vec<&str> = options.iter().map(|(label, _)| *label).collect();
    let selection = Select::with_theme(theme)
        .with_prompt("Chunk profile")
        .items(&labels)
        .default(1)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;
    Ok(options[selection].1.clone())
}

fn prompt_bundle_profile(theme: &ColorfulTheme) -> g::Result<g::manifest::BundleSizingPreset> {
    let options = [
        ("auto", g::manifest::BundleSizingPreset::Auto),
        ("tiny", g::manifest::BundleSizingPreset::Tiny),
        ("small", g::manifest::BundleSizingPreset::Small),
        ("balanced", g::manifest::BundleSizingPreset::Balanced),
        ("large", g::manifest::BundleSizingPreset::Large),
        ("huge", g::manifest::BundleSizingPreset::Huge),
        ("massive", g::manifest::BundleSizingPreset::Massive),
    ];
    let labels: Vec<&str> = options.iter().map(|(label, _)| *label).collect();
    let selection = Select::with_theme(theme)
        .with_prompt("Bundle profile")
        .items(&labels)
        .default(3)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;
    Ok(options[selection].1.clone())
}

fn prompt_target_tag_kind(
    theme: &ColorfulTheme,
    current: g::manifest::TargetTagKind,
) -> g::Result<g::manifest::TargetTagKind> {
    let options = [
        ("plain", g::manifest::TargetTagKind::Plain),
        ("scoped", g::manifest::TargetTagKind::Scoped),
        ("exclusive", g::manifest::TargetTagKind::Exclusive),
    ];
    let labels: Vec<&str> = options.iter().map(|(label, _)| *label).collect();
    let default_index = match current {
        g::manifest::TargetTagKind::Plain => 0,
        g::manifest::TargetTagKind::Scoped => 1,
        g::manifest::TargetTagKind::Exclusive => 2,
    };
    let selection = Select::with_theme(theme)
        .with_prompt("Tag kind")
        .items(&labels)
        .default(default_index)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;
    Ok(options[selection].1.clone())
}

fn required_string(theme: &ColorfulTheme, prompt: &str, default: &str) -> g::Result<String> {
    Input::with_theme(theme)
        .with_prompt(prompt)
        .with_initial_text(default)
        .validate_with(|value: &String| -> std::result::Result<(), &str> {
            if value.trim().is_empty() {
                Err("value is required")
            } else {
                Ok(())
            }
        })
        .interact_text()
        .map(|value: String| value.trim().to_string())
        .map_err(dialoguer_to_manifest_error)
}

fn optional_string_input(
    theme: &ColorfulTheme,
    prompt: &str,
    default: &str,
) -> g::Result<Option<String>> {
    Input::with_theme(theme)
        .with_prompt(prompt)
        .with_initial_text(default)
        .allow_empty(true)
        .interact_text()
        .map(trim_to_option)
        .map_err(dialoguer_to_manifest_error)
}

fn path_input(theme: &ColorfulTheme, prompt: &str, default: &str) -> g::Result<PathBuf> {
    required_string(theme, prompt, default).map(PathBuf::from)
}

fn optional_path_input(
    theme: &ColorfulTheme,
    prompt: &str,
    default: &str,
) -> g::Result<Option<PathBuf>> {
    optional_string_input(theme, prompt, default).map(|value| value.map(PathBuf::from))
}

fn optional_u64_input(
    theme: &ColorfulTheme,
    prompt: &str,
    default: &str,
) -> g::Result<Option<u64>> {
    let value: String = Input::with_theme(theme)
        .with_prompt(prompt)
        .with_initial_text(default)
        .allow_empty(true)
        .interact_text()
        .map_err(dialoguer_to_manifest_error)?;
    match trim_to_option(value) {
        Some(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|_| g::Error::manifest(format!("invalid number for {}: {}", prompt, value))),
        None => Ok(None),
    }
}

fn optional_usize_input(
    theme: &ColorfulTheme,
    prompt: &str,
    default: &str,
) -> g::Result<Option<usize>> {
    let value: String = Input::with_theme(theme)
        .with_prompt(prompt)
        .with_initial_text(default)
        .allow_empty(true)
        .interact_text()
        .map_err(dialoguer_to_manifest_error)?;
    match trim_to_option(value) {
        Some(value) => value
            .parse::<usize>()
            .map(Some)
            .map_err(|_| g::Error::manifest(format!("invalid number for {}: {}", prompt, value))),
        None => Ok(None),
    }
}

fn prompt_i32_input(theme: &ColorfulTheme, prompt: &str, default: &str) -> g::Result<i32> {
    let value = required_string(theme, prompt, default)?;
    value
        .parse::<i32>()
        .map_err(|_| g::Error::manifest(format!("invalid number for {}: {}", prompt, value)))
}

fn optional_tag_list(theme: &ColorfulTheme) -> g::Result<Vec<String>> {
    let raw = optional_string_input(theme, "Extra tags (comma-separated, optional)", "")?;
    Ok(raw
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default())
}

fn csv_list_input(theme: &ColorfulTheme, prompt: &str, default: &str) -> g::Result<Vec<String>> {
    let raw = optional_string_input(theme, prompt, default)?;
    Ok(raw
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default())
}

fn confirm_preview(theme: &ColorfulTheme, preview: &str) -> g::Result<()> {
    println!();
    println!("Equivalent command:");
    println!("  {}", preview);
    println!();
    let confirmed = prompt_approval_action(theme, "Run this command now?")?;
    if confirmed {
        set_last_ran_command(preview.to_string());
        Ok(())
    } else {
        Err(g::Error::manifest("interactive command cancelled"))
    }
}

fn runtime_preview_suffix(runtime: &InteractiveRuntimeOptions) -> String {
    let mut preview = String::new();
    if runtime.memory_mode != g::MemoryMode::Auto {
        preview.push_str(&format!(
            " --memory-mode {}",
            shell_arg(runtime.memory_mode.as_str())
        ));
    }
    if let Some(max_memory_mib) = runtime.max_memory_mib {
        preview.push_str(&format!(" --max-memory-mib {}", max_memory_mib));
    }
    if let Some(jobs) = runtime.jobs {
        preview.push_str(&format!(" --jobs {}", jobs));
    }
    preview
}

fn tags_preview_suffix(tags: &[String]) -> String {
    if tags.is_empty() {
        return String::new();
    }
    tags.iter()
        .map(|tag| format!(" --tag {}", shell_arg(tag)))
        .collect::<String>()
}

fn artifact_sizing_str(profile: g::manifest::ArtifactSizingPreset) -> &'static str {
    match profile {
        g::manifest::ArtifactSizingPreset::Small => "small",
        g::manifest::ArtifactSizingPreset::Balanced => "balanced",
        g::manifest::ArtifactSizingPreset::Large => "large",
    }
}

fn bundle_sizing_str(profile: g::manifest::BundleSizingPreset) -> &'static str {
    match profile {
        g::manifest::BundleSizingPreset::Auto => "auto",
        g::manifest::BundleSizingPreset::Tiny => "tiny",
        g::manifest::BundleSizingPreset::Small => "small",
        g::manifest::BundleSizingPreset::Balanced => "balanced",
        g::manifest::BundleSizingPreset::Large => "large",
        g::manifest::BundleSizingPreset::Huge => "huge",
        g::manifest::BundleSizingPreset::Massive => "massive",
    }
}

fn incomplete_behavior_str(behavior: g::manifest::IncompleteInferenceBehavior) -> &'static str {
    match behavior {
        g::manifest::IncompleteInferenceBehavior::Warn => "warn",
        g::manifest::IncompleteInferenceBehavior::Error => "error",
    }
}

fn target_tag_kind_str(kind: g::manifest::TargetTagKind) -> &'static str {
    match kind {
        g::manifest::TargetTagKind::Plain => "plain",
        g::manifest::TargetTagKind::Scoped => "scoped",
        g::manifest::TargetTagKind::Exclusive => "exclusive",
    }
}

fn ensure_target_schema(
    repo_manifest: &mut g::manifest::gutch_repo::GutchRepoManifest,
) -> &mut g::manifest::TargetTagSchema {
    repo_manifest
        .target_schema
        .get_or_insert_with(g::manifest::TargetTagSchema::default)
}

fn render_tag_values(definition: &g::manifest::TargetTagDefinition) -> String {
    definition
        .values
        .iter()
        .map(render_tag_value_definition)
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_tag_value_definition(value_definition: &g::manifest::TargetTagValueDefinition) -> String {
    if value_definition.aliases.is_empty() {
        value_definition.value.clone()
    } else {
        format!(
            "{}[{}]",
            value_definition.value,
            value_definition.aliases.join("|")
        )
    }
}

fn parse_tag_value_definitions(
    raw_values: &[String],
) -> g::Result<Vec<g::manifest::TargetTagValueDefinition>> {
    raw_values
        .iter()
        .map(|entry| {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                return Err(g::Error::manifest("tag value entry cannot be empty"));
            }

            if let Some((value, alias_block)) = trimmed.split_once('[') {
                let value = value.trim();
                let alias_block = alias_block.trim();
                if value.is_empty() || !alias_block.ends_with(']') {
                    return Err(g::Error::manifest(format!(
                        "invalid tag value entry '{}'. Use value or value[alias1|alias2]",
                        entry
                    )));
                }
                let aliases = alias_block[..alias_block.len() - 1]
                    .split('|')
                    .map(str::trim)
                    .filter(|alias| !alias.is_empty())
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>();
                return Ok(g::manifest::TargetTagValueDefinition {
                    value: value.to_string(),
                    aliases,
                });
            }

            Ok(g::manifest::TargetTagValueDefinition {
                value: trimmed.to_string(),
                aliases: Vec::new(),
            })
        })
        .collect()
}

fn render_target_tag_summary(
    target_tags: &g::manifest::TargetTagSet,
    target_schema: Option<&g::manifest::TargetTagSchema>,
) -> String {
    target_tags.to_ordered_strings(target_schema).join(", ")
}

fn tag_definition_value(
    target_tags: &g::manifest::TargetTagSet,
    definition: &g::manifest::TargetTagDefinition,
) -> String {
    match definition.kind {
        g::manifest::TargetTagKind::Plain => {
            if target_tags.plain.contains(&definition.name) {
                definition.name.clone()
            } else {
                String::new()
            }
        }
        g::manifest::TargetTagKind::Scoped => target_tags
            .scoped
            .get(&definition.name)
            .map(|values| values.iter().cloned().collect::<Vec<_>>().join(", "))
            .unwrap_or_default(),
        g::manifest::TargetTagKind::Exclusive => target_tags
            .get_exclusive(&definition.name)
            .unwrap_or_default()
            .to_string(),
    }
}

fn shell_arg(value: &str) -> String {
    if value.is_empty() {
        "\"\"".to_string()
    } else if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '\\'))
    {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('"', "\\\""))
    }
}

fn shell_arg_path(path: &std::path::Path) -> String {
    shell_arg(&path.display().to_string())
}

fn trim_to_option(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn dialoguer_to_manifest_error(err: dialoguer::Error) -> g::Error {
    g::Error::manifest(format!("interactive prompt failed: {}", err))
}

fn print_repo_summary(repo: &std::path::Path) -> g::Result<()> {
    let status = g::inspect_gutch_repo(repo)?;
    let config = g::config::load_config()?;
    let tracked = config.find_repo(&repo.display().to_string());

    println!();
    println!("  path:     {}", status.repo_path);
    println!("  id:       {}", status.gutch_repo_id);
    println!("  targets:  {}", status.targets.len());
    println!(
        "  bundles:  {} ({})",
        status.bundle_count,
        format_bytes(status.bundle_bytes)
    );

    if let Some(tracked) = tracked {
        if let Some(lookup) = &tracked.target_lookup_path {
            println!("  lookup:   {}", lookup);
        }
        if let Some(linked) = &tracked.linked {
            println!(
                "  linked:   product {} | repo {} (id: {})",
                linked.product_id, linked.remote_repo_slug, linked.remote_repo_id
            );
        }
    }
    println!();
    Ok(())
}

fn run_repo_inspect(repo: &std::path::Path) -> g::Result<()> {
    let status = g::inspect_gutch_repo(repo)?;
    if !status.is_repo {
        println!("not a gutch repo.");
        return Ok(());
    }

    println!();
    for target in &status.targets {
        println!(
            "{} | tags={} | files={} | missing_bundles={}",
            target.target_id,
            render_target_tag_summary(&target.target_tags, status.target_schema.as_ref()),
            target.file_count,
            target.missing_bundle_count
        );
    }
    Ok(())
}

fn run_interactive_repo_link(_theme: &ColorfulTheme, repo: &std::path::Path) -> g::Result<()> {
    let endpoint = g::remote::auth::default_backend_url();
    let session_id = crate::cli::common::session_id_for_cli(None)?;
    set_last_ran_command(format!(
        "gutch repo link --repo {} --endpoint {} --session-id {}",
        shell_arg_path(repo),
        shell_arg(&endpoint),
        shell_arg(&session_id)
    ));
    crate::cli::repo::run(RepoCommands::Link {
        repo: repo.to_path_buf(),
        endpoint,
        session_id: Some(session_id),
        product_id: None,
        gutch_repo_id: None,
        remote_repo_slug: None,
    })
}

fn run_interactive_repo_unlink(theme: &ColorfulTheme, repo: &std::path::Path) -> g::Result<()> {
    let confirmed = prompt_approval_action(theme, "Unlink this repo from remote?")?;
    if !confirmed {
        return Ok(());
    }
    set_last_ran_command(format!("gutch repo unlink --repo {}", shell_arg_path(repo)));
    crate::cli::repo::run(RepoCommands::Unlink {
        repo: repo.to_path_buf(),
    })
}

fn run_interactive_repo_sync_up(_theme: &ColorfulTheme, repo: &std::path::Path) -> g::Result<()> {
    let endpoint = g::remote::auth::default_backend_url();
    let session_id = crate::cli::common::session_id_for_cli(None)?;
    set_last_ran_command(format!(
        "gutch repo sync-up --repo {} --endpoint {} --session-id {}",
        shell_arg_path(repo),
        shell_arg(&endpoint),
        shell_arg(&session_id)
    ));
    crate::cli::repo::run(RepoCommands::SyncUp {
        repo: repo.to_path_buf(),
        endpoint,
        product_id: None,
        gutch_repo_id: None,
        session_id: Some(session_id),
    })
}

fn run_set_target_lookup_path(theme: &ColorfulTheme, repo: &std::path::Path) -> g::Result<()> {
    let current = g::config::load_config()?
        .find_repo(&repo.display().to_string())
        .and_then(|r| r.target_lookup_path.clone())
        .unwrap_or_default();

    let path = optional_path_or_browse_input(
        theme,
        "Target lookup path (directory with build outputs)",
        &current,
    )?;
    let path_str = path.as_ref().map(|p| p.display().to_string());

    let mut config = g::config::load_config()?;
    config.set_repo_target_lookup_path(&repo.display().to_string(), path_str.clone());
    g::config::save_config(&config)?;

    match path_str {
        Some(p) => println!("target lookup path set to '{}'", p),
        None => println!("target lookup path cleared"),
    }
    Ok(())
}

fn run_discover_targets(theme: &ColorfulTheme, repo: &std::path::Path) -> g::Result<()> {
    let lookup = g::config::load_config()?
        .find_repo(&repo.display().to_string())
        .and_then(|r| r.target_lookup_path.clone());

    let Some(lookup) = lookup else {
        println!("no target lookup path set. Use 'Set target lookup path' first.");
        return Ok(());
    };
    let lookup_path = std::path::PathBuf::from(&lookup);

    let candidates = g::discover_source_target_installs(repo, &lookup_path)?;
    let (_, repo_manifest) = g::load_gutch_repo_manifest(repo)?;
    let target_schema = repo_manifest.target_schema.unwrap_or_default();
    if candidates.is_empty() {
        println!("no source target installs found in '{}'", lookup);
        return Ok(());
    }

    let status = g::inspect_gutch_repo(repo)?;
    let existing_ids: HashSet<String> =
        status.targets.iter().map(|t| t.target_id.clone()).collect();

    let uninserted: Vec<_> = candidates
        .iter()
        .filter(|c| !existing_ids.contains(&c.target_id))
        .collect();
    let already_inserted = candidates.len() - uninserted.len();

    println!();
    if !uninserted.is_empty() {
        println!("Uninserted:");
        let schema_headers: Vec<String> = target_schema
            .target_tags
            .iter()
            .map(|definition| format_tag_column_name(&target_schema, &definition.name))
            .collect();
        let rows: Vec<Vec<String>> = uninserted
            .iter()
            .map(|candidate| {
                let mut row: Vec<String> = target_schema
                    .target_tags
                    .iter()
                    .map(|definition| {
                        display_table_value(&tag_definition_value(
                            &candidate.target_tags,
                            definition,
                        ))
                    })
                    .collect();
                row.push(relative_candidate_path(&lookup_path, &candidate.path));
                row
            })
            .collect();
        let mut headers: Vec<&str> = schema_headers.iter().map(String::as_str).collect();
        headers.push("path");
        print_ascii_table(&headers, &rows);
    }
    if already_inserted > 0 {
        println!("Already inserted: {} target(s)", already_inserted);
    }

    if uninserted.is_empty() {
        println!("all {} candidate(s) already inserted.", candidates.len());
        return Ok(());
    }

    let confirmed = prompt_approval_action(
        theme,
        format!("Insert all {} uninserted targets?", uninserted.len()),
    )?;

    if !confirmed {
        return Ok(());
    }

    for candidate in &uninserted {
        println!("inserting {} ...", candidate.folder_name);
        set_last_ran_command(format!(
            "gutch target insert --dir {} --repo {} --version {} --platform {} --architecture {} --tier {} --chunk-profile balanced --bundle-profile balanced",
            shell_arg(&candidate.path),
            shell_arg_path(repo),
            shell_arg(&candidate.version_id),
            shell_arg(&candidate.platform),
            shell_arg(&candidate.architecture),
            shell_arg(&candidate.tier_id),
        ));
        crate::cli::target::run(TargetCommands::Insert {
            dir: std::path::PathBuf::from(&candidate.path),
            repo: repo.to_path_buf(),
            version: Some(candidate.version_id.clone()),
            platform: Some(candidate.platform.clone()),
            architecture: Some(candidate.architecture.clone()),
            tier: Some(candidate.tier_id.clone()),
            tags: Vec::new(),
            chunk_profile: g::manifest::ArtifactSizingPreset::Balanced,
            bundle_profile: g::manifest::BundleSizingPreset::Balanced,
            zstd_level: 1,
            memory_mode: g::MemoryMode::Auto,
            max_memory_mib: None,
            jobs: None,
            summary_out: None,
            stats_json_out: None,
        })?;
    }

    println!("inserted {} target(s).", uninserted.len());
    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{} B", bytes)
    }
}
