use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use dialoguer::console::Term;
use dialoguer::{Select, theme::ColorfulTheme};

use gutch as g;

mod display;
mod menus;
mod prompts;
mod repo_config;
mod workflows;

use self::display::{print_active_repo_info_table, print_landing_info_table, print_repo_summary};
use self::menus::{
    AdvancedAction, RepoSelectionAction, RepoWorkAction, custom_repo_path_input,
    prompt_advanced_action, prompt_all_tracked_repos, prompt_repo_selection,
    prompt_repo_work_action, resolve_active_repo_from_selection,
};
use self::prompts::{
    TaggingTemplate, artifact_sizing_str, bundle_sizing_str, confirm_output_path_overwrite,
    confirm_preview, csv_list_input, dialoguer_to_manifest_error, minimal_tagging_template,
    optional_path_or_browse_input, optional_string_input, optional_tag_list, path_or_browse_input,
    path_or_browse_input_file, prompt_bundle_profile, prompt_chunk_profile, prompt_i32_input,
    prompt_runtime_options, prompt_tagging_template, prompt_target_selection,
    prompt_target_tag_kind, required_string, runtime_preview_suffix, shell_arg, shell_arg_path,
    tags_preview_suffix,
};
use self::repo_config::run_repo_config_edit;
use self::workflows::{
    run_discover_targets, run_interactive_repo_link, run_interactive_repo_sync_up,
    run_interactive_repo_unlink, run_repo_create, run_set_target_lookup_path, run_target_extract,
    run_target_insert, run_target_install_apply, run_target_install_verify,
};

#[derive(Clone)]
struct InteractiveRuntimeOptions {
    memory_mode: g::MemoryMode,
    max_memory_mib: Option<u64>,
    jobs: Option<usize>,
}

static LAST_RAN_COMMAND: OnceLock<Mutex<Option<String>>> = OnceLock::new();

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
                    display::print_repo_inspect(repo)?;
                }
                RepoWorkAction::RepoSyncUp => {
                    if let Err(err) = run_interactive_repo_sync_up(repo) {
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
                            if let Err(err) = run_interactive_repo_link(repo) {
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
