use std::path::{Path, PathBuf};

use dialoguer::{Confirm, theme::ColorfulTheme};

use crate::cli::TargetCommands;
use crate::cli::actions;
use gutch as g;

pub(super) fn run_repo_create(theme: &ColorfulTheme) -> g::Result<PathBuf> {
    let cwd = std::env::current_dir()
        .map_err(|err| g::Error::manifest(format!("failed to get current directory: {}", err)))?;
    let cwd_with_slash = format!("{}_", cwd.display()).replace("_", std::path::MAIN_SEPARATOR_STR);

    let path = super::path_or_browse_input(theme, "Repo path", &cwd_with_slash)?;
    let assessment = g::assess_repo_init_path(&path)?;
    let mut force = false;

    if assessment.requires_confirmation {
        println!();
        println!("Repo init requires confirmation for '{}':", path.display());
        for risk in &assessment.risks {
            println!("  {}", risk.message);
        }
        if assessment.path_kind == g::RepoInitPathKind::ExistingRepo {
            if let Some(name) = assessment.repo_name.as_deref() {
                println!("  existing repo: {}", name);
            }
            if let Some(target_count) = assessment.target_count {
                println!("  targets: {}", target_count);
            }
            if let Some(bundle_count) = assessment.bundle_count {
                println!("  bundles: {}", bundle_count);
            }
        } else if assessment.path_kind == g::RepoInitPathKind::NonEmpty {
            println!("  entries: {}", assessment.entry_count);
        }

        let proceed = Confirm::with_theme(theme)
            .with_prompt("Delete existing Gutch repo files and create a new repo?")
            .default(false)
            .interact()
            .map_err(super::dialoguer_to_manifest_error)?;
        if !proceed {
            return Err(g::Error::manifest("repo creation cancelled"));
        }
        force = true;
    }

    let template = super::prompt_tagging_template(theme)?;
    let preview = if force {
        format!(
            "plctl gutch repo init --path {} --force",
            super::shell_arg_path(&path)
        )
    } else {
        format!(
            "plctl gutch repo init --path {}",
            super::shell_arg_path(&path)
        )
    };
    super::confirm_preview(theme, &preview)?;

    actions::repo::init_repo(&path, None, force)?;

    let (repo_paths, mut repo_manifest) = g::load_gutch_repo_manifest(&path)?;
    repo_manifest.target_schema = Some(match template {
        super::TaggingTemplate::Standard => g::manifest::TargetTagSchema::default(),
        super::TaggingTemplate::Minimal => super::minimal_tagging_template(),
    });
    g::write_gutch_repo_manifest(&repo_paths, &repo_manifest)?;

    let template_name = match template {
        super::TaggingTemplate::Standard => "standard (version::, os::, arch::, access::)",
        super::TaggingTemplate::Minimal => "minimal (version:: only)",
    };
    println!("Applied tagging template: {}", template_name);
    println!("You can edit this later via 'Edit repo config'.");

    Ok(path)
}

pub(super) fn run_target_insert(theme: &ColorfulTheme, repo: &Path) -> g::Result<()> {
    let dir = super::path_or_browse_input_file(theme, "Input directory or file", ".")?;
    let inferred = g::preview_source_target_install(repo, &dir)?;
    let (_, repo_manifest) = g::load_gutch_repo_manifest(repo)?;
    println!(
        "Inferred from folder name '{}': {}",
        inferred.folder_name,
        super::display::render_target_tag_summary(
            &inferred.target_tags,
            repo_manifest.target_schema.as_ref(),
        )
    );
    let tags = super::optional_tag_list(theme)?;
    let chunk_profile = super::prompt_chunk_profile(theme)?;
    let bundle_profile = super::prompt_bundle_profile(theme)?;
    let compression_profile = super::prompt_compression_profile(theme)?;
    let runtime = super::prompt_runtime_options(theme)?;
    let explicit_tags = g::parse_cli_tags(&tags)?;
    let insert_plan = g::plan_target_insert(repo, &dir, &explicit_tags)?;
    let preview = format!(
        "plctl gutch target insert --dir {} --repo {} --chunk-profile {} --bundle-profile {} --compression-profile {}{}{}\n{}",
        super::shell_arg_path(&dir),
        super::shell_arg_path(repo),
        super::artifact_sizing_str(chunk_profile),
        super::bundle_sizing_str(bundle_profile),
        compression_profile.cli_name(),
        super::tags_preview_suffix(&tags),
        super::runtime_preview_suffix(&runtime),
        g::describe_target_insert_plan(&insert_plan),
    );
    super::confirm_preview(theme, &preview)?;

    crate::cli::target::run(TargetCommands::Insert {
        dir,
        repo: repo.to_path_buf(),
        tags,
        chunk_profile,
        bundle_profile,
        compression_profile,
        storage_mode: g::manifest::ArtifactStorageMode::Bundled,
        memory_mode: runtime.memory_mode,
        max_memory_mib: runtime.max_memory_mib,
        jobs: runtime.jobs,
        summary_out: None,
        stats_json_out: None,
        yes: true,
    })
}

pub(super) fn run_target_extract(theme: &ColorfulTheme, repo: &Path) -> g::Result<()> {
    let Some(target) = super::prompt_target_selection(theme, repo, "Select target to extract")?
    else {
        return Err(g::Error::manifest("interactive command cancelled"));
    };
    let output = super::path_or_browse_input(theme, "Output directory", "./extracted-target")?;
    super::confirm_output_path_overwrite(theme, &output, "Extract output path")?;
    let runtime = super::prompt_runtime_options(theme)?;
    let preview = format!(
        "plctl gutch target extract --repo {} --target {} --output {}{}",
        super::shell_arg_path(repo),
        super::shell_arg(&target),
        super::shell_arg_path(&output),
        super::runtime_preview_suffix(&runtime),
    );
    super::confirm_preview(theme, &preview)?;

    crate::cli::target::run(TargetCommands::Extract {
        repo: repo.to_path_buf(),
        target,
        output,
        memory_mode: runtime.memory_mode,
        max_memory_mib: runtime.max_memory_mib,
        jobs: runtime.jobs,
        restore_verify: true,
        summary_out: None,
        stats_json_out: None,
    })
}

pub(super) fn run_target_install_apply(theme: &ColorfulTheme, repo: &Path) -> g::Result<()> {
    let target = super::required_string(theme, "Target id", "")?;
    let dir = super::path_or_browse_input(theme, "Install directory", "./install")?;
    let staging_dir = if Confirm::with_theme(theme)
        .with_prompt("Specify a custom staging directory?")
        .default(false)
        .interact()
        .map_err(super::dialoguer_to_manifest_error)?
    {
        super::optional_path_or_browse_input(theme, "Staging directory", "")?
    } else {
        None
    };
    let source_target = super::optional_string_input(theme, "Source target id", "")?;
    let source_dir = if source_target.is_some() {
        super::optional_path_or_browse_input(theme, "Source install directory", "")?
    } else {
        None
    };
    let runtime = super::prompt_runtime_options(theme)?;
    let preview = format!(
        "plctl gutch target_install apply --repo {} --target {} --dir {}{}{}{}",
        super::shell_arg_path(repo),
        super::shell_arg(&target),
        super::shell_arg_path(&dir),
        staging_dir
            .as_ref()
            .map(|path| format!(" --staging-dir {}", super::shell_arg_path(path)))
            .unwrap_or_default(),
        source_target
            .as_ref()
            .map(|value| format!(" --source-target {}", super::shell_arg(value)))
            .unwrap_or_default(),
        source_dir
            .as_ref()
            .map(|path| format!(" --source-dir {}", super::shell_arg_path(path)))
            .unwrap_or_default()
            + &super::runtime_preview_suffix(&runtime),
    );
    super::confirm_preview(theme, &preview)?;

    actions::target_install::apply_install(actions::target_install::ApplyCommand {
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

pub(super) fn run_target_install_verify(theme: &ColorfulTheme, repo: &Path) -> g::Result<()> {
    let target = super::required_string(theme, "Target id", "")?;
    let dir = super::path_or_browse_input(theme, "Install directory", "./install")?;
    let runtime = super::prompt_runtime_options(theme)?;
    let preview = format!(
        "plctl gutch target_install verify --repo {} --target {} --dir {}{}",
        super::shell_arg_path(repo),
        super::shell_arg(&target),
        super::shell_arg_path(&dir),
        super::runtime_preview_suffix(&runtime),
    );
    super::confirm_preview(theme, &preview)?;

    actions::target_install::verify_install(actions::target_install::VerifyCommand {
        repo: repo.to_path_buf(),
        target,
        dir,
        verify_mode: g::VerifyMode::Fast,
        memory_mode: runtime.memory_mode,
        max_memory_mib: runtime.max_memory_mib,
        jobs: runtime.jobs,
        summary_out: None,
        stats_json_out: None,
    })
}

pub(super) fn run_interactive_repo_link(repo: &Path) -> g::Result<()> {
    let endpoint = g::remote::auth::default_backend_url();
    let session_id = crate::cli::common::session_id_for_cli(None)?;
    super::set_last_ran_command(format!(
        "plctl gutch repo link --repo {} --endpoint {} --session-id {}",
        super::shell_arg_path(repo),
        super::shell_arg(&endpoint),
        super::shell_arg(&session_id)
    ));
    crate::cli::repo::run(crate::cli::RepoCommands::Link {
        repo: repo.to_path_buf(),
        endpoint,
        session_id: Some(session_id),
        product_id: None,
    })
}

pub(super) fn run_interactive_repo_unlink(theme: &ColorfulTheme, repo: &Path) -> g::Result<()> {
    let confirmed = super::prompt_approval_action(theme, "Unlink this repo from remote?")?;
    if !confirmed {
        return Ok(());
    }
    super::set_last_ran_command(format!(
        "plctl gutch repo unlink --repo {}",
        super::shell_arg_path(repo)
    ));
    actions::repo::unlink_repo(repo, false, None, None)
}

pub(super) fn run_interactive_repo_sync_up(repo: &Path) -> g::Result<()> {
    let endpoint = g::remote::auth::default_backend_url();
    let session_id = crate::cli::common::session_id_for_cli(None)?;
    super::set_last_ran_command(format!(
        "plctl gutch repo sync-up --repo {} --endpoint {} --session-id {}",
        super::shell_arg_path(repo),
        super::shell_arg(&endpoint),
        super::shell_arg(&session_id)
    ));
    let stats = actions::repo::sync_up_repo(repo.to_path_buf(), endpoint, Some(session_id))?;
    println!(
        "synced repo {} to product {} repo {}",
        stats.repo_root, stats.product_id, stats.remote_repo_id
    );
    println!(
        "repo manifest uploaded: {}, chunk index uploaded: {}",
        stats.repo_manifest_uploaded, stats.chunk_index_uploaded
    );
    println!(
        "target manifests uploaded: {}, bundles uploaded: {}, bundles skipped: {}",
        stats.target_manifest_uploaded_count,
        stats.bundle_uploaded_count,
        stats.bundle_skipped_count
    );
    Ok(())
}

pub(super) fn run_set_target_sources_path(theme: &ColorfulTheme, repo: &Path) -> g::Result<()> {
    let current = g::config::load_config()?
        .find_repo(&repo.display().to_string())
        .and_then(|r| r.target_sources_path.clone())
        .unwrap_or_default();

    let path = super::optional_path_or_browse_input(
        theme,
        "Target sources folder (directory with build outputs)",
        &current,
    )?;
    let path_str = path.as_ref().map(|p| p.display().to_string());

    let mut config = g::config::load_config()?;
    config.set_repo_target_sources_path(&repo.display().to_string(), path_str.clone());
    g::config::save_config(&config)?;

    match path_str {
        Some(p) => println!("target sources folder set to '{}'", p),
        None => println!("target sources folder cleared"),
    }
    Ok(())
}

pub(super) fn run_sync_target_sources(theme: &ColorfulTheme, repo: &Path) -> g::Result<()> {
    use crate::cli::actions::target_sources::{TargetSourcesSyncOptions, sync_target_sources};

    let sources = g::config::load_config()?
        .find_repo(&repo.display().to_string())
        .and_then(|r| r.target_sources_path.clone());

    let Some(sources) = sources else {
        println!("no target sources folder set. Use 'Set target sources folder' first.");
        return Ok(());
    };
    let sources_path = PathBuf::from(&sources);

    let plan = g::plan_target_sources_sync(repo, Some(&sources_path))?;
    if !plan.sources_ready {
        println!("target sources folder is not ready.");
        return Ok(());
    }
    if plan.candidate_count == 0 && plan.removable_count == 0 {
        println!("no target source folders found in '{}'", sources);
        return Ok(());
    }
    if !plan.has_changes() {
        println!("target sources folder and local repo already match.");
        return Ok(());
    }

    println!("target sources sync plan");
    println!("candidates: {}", plan.candidate_count);
    println!("new targets: {}", plan.insertable_count);
    println!("tag updates: {}", plan.tag_update_count);
    println!("same-tag replacements: {}", plan.replace_same_tags_count);
    println!("to refresh: {}", plan.refresh_count());
    println!("to prune: {}", plan.removable_count);
    for line in g::describe_target_sources_sync_plan(&plan) {
        println!("  {line}");
    }
    if plan.replace_same_tags_count > 0 {
        println!("  Patron installs pinned to replaced content ids become invalid until updated.");
    }

    let confirmed = super::prompt_approval_action(theme, "Sync target sources?")?;
    if !confirmed {
        return Ok(());
    }

    super::set_last_ran_command(format!(
        "plctl gutch repo target-sources sync --repo {} --root {} --yes",
        super::shell_arg_path(repo),
        super::shell_arg(&sources),
    ));

    sync_target_sources(
        repo,
        &sources_path,
        TargetSourcesSyncOptions {
            no_prune: false,
            yes: true,
        },
    )?;
    Ok(())
}
