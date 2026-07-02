use std::path::{Path, PathBuf};

use dialoguer::{Confirm, Select, theme::ColorfulTheme};

use gutch as g;

#[derive(Clone)]
pub(super) enum RepoSelectionAction {
    CreateRepo,
    OpenRepo(PathBuf),
    ShowAllTrackedRepos,
    EnterCustomRepoPath,
    Exit,
}

#[derive(Clone)]
pub(super) enum RepoWorkAction {
    TargetInsert,
    TargetExtract,
    TargetInstallApply,
    TargetInstallVerify,
    RepoCheck,
    RepoVerify,
    RepoSyncUp,
    Advanced,
    SwitchRepo,
    Exit,
}

#[derive(Clone)]
pub(super) enum AdvancedAction {
    RepoConfigEdit,
    RepoLink,
    RepoUnlink,
    SetTargetSourcesPath,
    SyncTargetSources,
    Back,
}

pub(super) fn prompt_repo_selection(theme: &ColorfulTheme) -> g::Result<RepoSelectionAction> {
    let config = g::config::load_config()?;
    const PAGE_SIZE: usize = 10;
    let total = config.repos.len();
    let truncated = total > PAGE_SIZE;
    let visible_repos = if truncated { PAGE_SIZE } else { total };

    loop {
        super::refresh_terminal()?;
        super::print_last_ran_command();
        super::print_landing_info_table(&config);

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
            .map_err(super::dialoguer_to_manifest_error)?;

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

pub(super) fn prompt_all_tracked_repos(
    theme: &ColorfulTheme,
    config: &g::config::GutchConfig,
) -> g::Result<PathBuf> {
    super::refresh_terminal()?;
    super::print_last_ran_command();

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
        .map_err(super::dialoguer_to_manifest_error)?;
    if selection == labels.len().saturating_sub(1) {
        return Err(g::Error::manifest("interactive command cancelled"));
    }
    Ok(PathBuf::from(&config.repos[selection].path))
}

pub(super) fn custom_repo_path_input(theme: &ColorfulTheme) -> g::Result<PathBuf> {
    super::path_or_browse_input(theme, "Repo path", ".")
}

pub(super) fn resolve_active_repo_from_selection(
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
                    .map_err(super::dialoguer_to_manifest_error)?
            {
                return Ok(None);
            }
            Ok(None)
        }
        Ok(_) => {
            g::ensure_gutch_repo_tracked(&repo.display().to_string())?;
            Ok(Some(repo))
        }
    }
}

pub(super) fn prompt_repo_work_action(
    theme: &ColorfulTheme,
    repo: &Path,
) -> g::Result<RepoWorkAction> {
    super::refresh_terminal()?;
    super::print_active_repo_info_table(repo)?;

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
        ("- Check repo targets", RepoWorkAction::RepoCheck),
        ("- Verify repo bundle integrity", RepoWorkAction::RepoVerify),
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
        .map_err(super::dialoguer_to_manifest_error)?;
    Ok(actions[selection].1.clone())
}

pub(super) fn prompt_advanced_action(
    theme: &ColorfulTheme,
    repo: &Path,
) -> g::Result<AdvancedAction> {
    super::refresh_terminal()?;
    super::print_active_repo_info_table(repo)?;

    let actions = [
        (
            "- Sync targets from sources folder",
            AdvancedAction::SyncTargetSources,
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
            "- Set target sources folder",
            AdvancedAction::SetTargetSourcesPath,
        ),
        ("- Back", AdvancedAction::Back),
    ];
    let labels: Vec<&str> = actions.iter().map(|(label, _)| *label).collect();
    let selection = Select::with_theme(theme)
        .with_prompt(format!("Advanced — repo: {}", repo.display()))
        .items(&labels)
        .default(0)
        .interact()
        .map_err(super::dialoguer_to_manifest_error)?;
    Ok(actions[selection].1.clone())
}

fn render_tracked_repo_label(index: usize, repo: &g::config::TrackedRepo) -> String {
    let sources_marker = if repo.target_sources_path.is_some() {
        " [sources]"
    } else {
        ""
    };
    format!("- [Repo-{}] {}{}", index, repo.path, sources_marker)
}
