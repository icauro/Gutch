use std::collections::HashSet;
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
            let proceed = super::prompt_approval_action(
                theme,
                "Existing contents may be overwritten. Continue?",
            )?;
            if !proceed {
                return Err(g::Error::manifest("repo creation cancelled"));
            }
        }
    }

    let template = super::prompt_tagging_template(theme)?;
    let preview = format!("gutch repo init --path {}", super::shell_arg_path(&path));
    super::confirm_preview(theme, &preview)?;

    actions::repo::init_repo(&path, None)?;

    let (repo_paths, mut repo_manifest) = g::load_gutch_repo_manifest(&path)?;
    repo_manifest.target_schema = Some(match template {
        super::TaggingTemplate::Standard => g::manifest::TargetTagSchema::default(),
        super::TaggingTemplate::Minimal => super::minimal_tagging_template(),
    });
    g::write_gutch_repo_manifest(&repo_paths, &repo_manifest)?;

    let template_name = match template {
        super::TaggingTemplate::Standard => "standard (version::, os::, arch::, tier::)",
        super::TaggingTemplate::Minimal => "minimal (version:: only)",
    };
    println!("Applied tagging template: {}", template_name);
    println!("You can edit this later via 'Edit repo config'.");

    Ok(path)
}

pub(super) fn run_target_insert(theme: &ColorfulTheme, repo: &Path) -> g::Result<()> {
    let dir = super::path_or_browse_input_file(theme, "Input directory or file", ".")?;
    let version = super::required_string(theme, "Version", "1.0.0")?;
    let platform = super::required_string(theme, "Platform", "windows")?;
    let architecture = super::required_string(theme, "Architecture", "x86_64")?;
    let tier = super::required_string(theme, "Tier", "base")?;
    let tags = super::optional_tag_list(theme)?;
    let chunk_profile = super::prompt_chunk_profile(theme)?;
    let bundle_profile = super::prompt_bundle_profile(theme)?;
    let runtime = super::prompt_runtime_options(theme)?;
    let preview = format!(
        "gutch target insert --dir {} --repo {} --version {} --platform {} --architecture {} --tier {} --chunk-profile {} --bundle-profile {}{}{}",
        super::shell_arg_path(&dir),
        super::shell_arg_path(repo),
        super::shell_arg(&version),
        super::shell_arg(&platform),
        super::shell_arg(&architecture),
        super::shell_arg(&tier),
        super::artifact_sizing_str(chunk_profile),
        super::bundle_sizing_str(bundle_profile),
        super::tags_preview_suffix(&tags),
        super::runtime_preview_suffix(&runtime),
    );
    super::confirm_preview(theme, &preview)?;

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

pub(super) fn run_target_extract(theme: &ColorfulTheme, repo: &Path) -> g::Result<()> {
    let Some(target) = super::prompt_target_selection(theme, repo, "Select target to extract")?
    else {
        return Err(g::Error::manifest("interactive command cancelled"));
    };
    let output = super::path_or_browse_input(theme, "Output directory", "./extracted-target")?;
    super::confirm_output_path_overwrite(theme, &output, "Extract output path")?;
    let runtime = super::prompt_runtime_options(theme)?;
    let preview = format!(
        "gutch target extract --repo {} --target {} --output {}{}",
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
        "gutch target_install apply --repo {} --target {} --dir {}{}{}{}",
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
        "gutch target_install verify --repo {} --target {} --dir {}{}",
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
        "gutch repo link --repo {} --endpoint {} --session-id {}",
        super::shell_arg_path(repo),
        super::shell_arg(&endpoint),
        super::shell_arg(&session_id)
    ));
    crate::cli::repo::run(crate::cli::RepoCommands::Link {
        repo: repo.to_path_buf(),
        endpoint,
        session_id: Some(session_id),
        product_id: None,
        gutch_repo_id: None,
        remote_repo_slug: None,
    })
}

pub(super) fn run_interactive_repo_unlink(theme: &ColorfulTheme, repo: &Path) -> g::Result<()> {
    let confirmed = super::prompt_approval_action(theme, "Unlink this repo from remote?")?;
    if !confirmed {
        return Ok(());
    }
    super::set_last_ran_command(format!(
        "gutch repo unlink --repo {}",
        super::shell_arg_path(repo)
    ));
    actions::repo::unlink_repo(repo)
}

pub(super) fn run_interactive_repo_sync_up(repo: &Path) -> g::Result<()> {
    let endpoint = g::remote::auth::default_backend_url();
    let session_id = crate::cli::common::session_id_for_cli(None)?;
    super::set_last_ran_command(format!(
        "gutch repo sync-up --repo {} --endpoint {} --session-id {}",
        super::shell_arg_path(repo),
        super::shell_arg(&endpoint),
        super::shell_arg(&session_id)
    ));
    let stats =
        actions::repo::sync_up_repo(repo.to_path_buf(), endpoint, None, None, Some(session_id))?;
    println!(
        "synced repo {} to product {} repo {}",
        stats.repo_root, stats.product_id, stats.gutch_repo_id
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

pub(super) fn run_set_target_lookup_path(theme: &ColorfulTheme, repo: &Path) -> g::Result<()> {
    let current = g::config::load_config()?
        .find_repo(&repo.display().to_string())
        .and_then(|r| r.target_lookup_path.clone())
        .unwrap_or_default();

    let path = super::optional_path_or_browse_input(
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

pub(super) fn run_discover_targets(theme: &ColorfulTheme, repo: &Path) -> g::Result<()> {
    let lookup = g::config::load_config()?
        .find_repo(&repo.display().to_string())
        .and_then(|r| r.target_lookup_path.clone());

    let Some(lookup) = lookup else {
        println!("no target lookup path set. Use 'Set target lookup path' first.");
        return Ok(());
    };
    let lookup_path = PathBuf::from(&lookup);

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

    super::display::print_discovered_targets(&lookup_path, &target_schema, &uninserted);
    if already_inserted > 0 {
        println!("Already inserted: {} target(s)", already_inserted);
    }

    if uninserted.is_empty() {
        println!("all {} candidate(s) already inserted.", candidates.len());
        return Ok(());
    }

    let confirmed = super::prompt_approval_action(
        theme,
        format!("Insert all {} uninserted targets?", uninserted.len()),
    )?;

    if !confirmed {
        return Ok(());
    }

    for candidate in &uninserted {
        println!("inserting {} ...", candidate.folder_name);
        super::set_last_ran_command(format!(
            "gutch target insert --dir {} --repo {} --version {} --platform {} --architecture {} --tier {} --chunk-profile balanced --bundle-profile balanced",
            super::shell_arg(&candidate.path),
            super::shell_arg_path(repo),
            super::shell_arg(&candidate.version_id),
            super::shell_arg(&candidate.platform),
            super::shell_arg(&candidate.architecture),
            super::shell_arg(&candidate.tier_id),
        ));
        crate::cli::target::run(TargetCommands::Insert {
            dir: PathBuf::from(&candidate.path),
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
