use std::collections::BTreeMap;
use std::path::PathBuf;

use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};

use crate::{RepoCommands, TargetCommands, TargetInstallCommands};
use gutch as g;

#[derive(Clone)]
struct InteractiveRuntimeOptions {
    memory_mode: g::MemoryMode,
    max_memory_mib: Option<u64>,
    jobs: Option<usize>,
}

#[derive(Clone)]
enum RepoSelectionAction {
    CreateRepo,
    SelectRepo,
    Exit,
}

#[derive(Clone)]
enum RepoWorkAction {
    TargetInsert,
    TargetExport,
    TargetInstallApply,
    TargetInstallVerify,
    RepoConfigEdit,
    SwitchRepo,
    Exit,
}

pub(crate) fn run() -> g::Result<()> {
    let theme = ColorfulTheme::default();
    let mut active_repo: Option<PathBuf> = None;

    loop {
        if active_repo.is_none() {
            match prompt_repo_selection(&theme)? {
                RepoSelectionAction::Exit => return Ok(()),
                RepoSelectionAction::CreateRepo => {
                    let path = run_repo_create(&theme)?;
                    if Confirm::with_theme(&theme)
                        .with_prompt("Work on this repo?")
                        .default(true)
                        .interact()
                        .map_err(dialoguer_to_manifest_error)?
                    {
                        active_repo = Some(path);
                    }
                }
                RepoSelectionAction::SelectRepo => {
                    let repo = prompt_select_tracked_repo(&theme)?;
                    if let Err(err) = g::load_content_repo_manifest(&repo) {
                        println!(
                            "not a valid content repo at {}: {}",
                            repo.display(),
                            err
                        );
                    } else {
                        println!("selected repo at {}", repo.display());
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
                    run_target_insert(&theme, repo)?;
                }
                RepoWorkAction::TargetExport => {
                    run_target_export(&theme, repo)?;
                }
                RepoWorkAction::TargetInstallApply => {
                    run_target_install_apply(&theme, repo)?;
                }
                RepoWorkAction::TargetInstallVerify => {
                    run_target_install_verify(&theme, repo)?;
                }
                RepoWorkAction::RepoConfigEdit => {
                    run_repo_config_edit(&theme, repo)?;
                }
            }
        }
    }
}

fn prompt_repo_selection(theme: &ColorfulTheme) -> g::Result<RepoSelectionAction> {
    let actions = [
        ("Create a content repo", RepoSelectionAction::CreateRepo),
        ("Select an existing repo", RepoSelectionAction::SelectRepo),
        ("Exit", RepoSelectionAction::Exit),
    ];
    let labels: Vec<&str> = actions.iter().map(|(label, _)| *label).collect();
    let selection = Select::with_theme(theme)
        .with_prompt("Gutch interactive — no repo selected")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;
    Ok(actions[selection].1.clone())
}

fn prompt_select_tracked_repo(theme: &ColorfulTheme) -> g::Result<std::path::PathBuf> {
    let config = g::config::load_config()?;

    if config.repos.is_empty() {
        return path_input(theme, "Repo path", ".");
    }

    let mut labels: Vec<String> = config
        .repos
        .iter()
        .map(|repo| format!("{}  ({})", repo.path, repo.content_repo_id))
        .collect();
    labels.push("Enter a custom path...".to_string());

    let selection = Select::with_theme(theme)
        .with_prompt("Select a tracked repo")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;

    if selection < config.repos.len() {
        return Ok(std::path::PathBuf::from(&config.repos[selection].path));
    }

    path_input(theme, "Repo path", ".")
}

fn prompt_repo_work_action(
    theme: &ColorfulTheme,
    repo: &std::path::Path,
) -> g::Result<RepoWorkAction> {
    let actions = [
        ("Insert target from local files", RepoWorkAction::TargetInsert),
        ("Export target to directory", RepoWorkAction::TargetExport),
        (
            "Apply target install from local repo",
            RepoWorkAction::TargetInstallApply,
        ),
        ("Verify installed target", RepoWorkAction::TargetInstallVerify),
        ("Edit repo config and tag heuristics", RepoWorkAction::RepoConfigEdit),
        ("Switch to a different repo", RepoWorkAction::SwitchRepo),
        ("Exit", RepoWorkAction::Exit),
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

#[derive(Clone)]
enum TaggingTemplate {
    Standard,
    Minimal,
}

fn run_repo_create(theme: &ColorfulTheme) -> g::Result<PathBuf> {
    let path = path_input(theme, "Repo path", ".")?;

    let template = prompt_tagging_template(theme)?;

    let preview = format!("gutch repo init --path {}", shell_arg_path(&path));
    confirm_preview(theme, &preview)?;

    crate::cli::repo::run(RepoCommands::Init {
        path: path.clone(),
        schema: None,
    })?;

    let (repo_paths, mut repo_manifest) = g::load_content_repo_manifest(&path)?;
    repo_manifest.target_schema = Some(match template {
        TaggingTemplate::Standard => g::manifest::TargetTagSchema::default(),
        TaggingTemplate::Minimal => minimal_tagging_template(),
    });
    g::write_content_repo_manifest(&repo_paths, &repo_manifest)?;

    let template_name = match template {
        TaggingTemplate::Standard => "standard (version, os, arch, tier)",
        TaggingTemplate::Minimal => "minimal (version only)",
    };
    println!("Applied tagging template: {}", template_name);
    println!("You can edit this later via 'Edit repo config'.");

    Ok(path)
}

fn prompt_tagging_template(theme: &ColorfulTheme) -> g::Result<TaggingTemplate> {
    let descriptions = [
        (
            "Standard — version, os, arch, tier. Suitable for most apps.",
            TaggingTemplate::Standard,
        ),
        (
            "Minimal — version only. For non-OS, non-platform-dependent assets (Python, JAR, PNG, JPG, blend files, etc.).",
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
            values: Vec::new(),
            aliases: BTreeMap::new(),
            default_value: None,
        }],
        target_id_order: vec!["version".to_string()],
        on_incomplete: g::manifest::IncompleteInferenceBehavior::Error,
    }
}

fn run_repo_config_edit(theme: &ColorfulTheme, repo: &std::path::Path) -> g::Result<()> {
    let (repo_paths, mut repo_manifest) = g::load_content_repo_manifest(repo)?;

    loop {
        println!();
        print_repo_config_summary(&repo_manifest);
        println!();

        let actions = [
            "Set incomplete inference behavior",
            "Add or update tag definition",
            "Remove tag definition",
            "Edit target id order",
            "Edit sizing and compression defaults",
            "Reset target schema to built-in defaults",
            "Save and exit",
            "Cancel",
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
            3 => edit_target_id_order(theme, &mut repo_manifest)?,
            4 => edit_repo_profiles(theme, &mut repo_manifest)?,
            5 => {
                if Confirm::with_theme(theme)
                    .with_prompt("Reset target schema to built-in defaults?")
                    .default(false)
                    .interact()
                    .map_err(dialoguer_to_manifest_error)?
                {
                    repo_manifest.target_schema = Some(g::manifest::TargetTagSchema::default());
                }
            }
            6 => {
                println!();
                println!("This will write:");
                println!("  {}", repo_paths.repo_manifest_path.display());
                println!();
                if Confirm::with_theme(theme)
                    .with_prompt("Save repo config changes?")
                    .default(true)
                    .interact()
                    .map_err(dialoguer_to_manifest_error)?
                {
                    g::write_content_repo_manifest(&repo_paths, &repo_manifest)?;
                    println!(
                        "updated repo config: {}",
                        repo_paths.repo_manifest_path.display()
                    );
                    return Ok(());
                }
            }
            7 => return Err(g::Error::manifest("interactive command cancelled")),
            _ => unreachable!(),
        }
    }
}

fn run_target_insert(theme: &ColorfulTheme, repo: &std::path::Path) -> g::Result<()> {
    let dir = path_input(theme, "Input directory or file", ".")?;
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
        memory_mode: runtime.memory_mode,
        max_memory_mib: runtime.max_memory_mib,
        jobs: runtime.jobs,
        summary_out: None,
        stats_json_out: None,
    })
}

fn run_target_export(theme: &ColorfulTheme, repo: &std::path::Path) -> g::Result<()> {
    let target = required_string(theme, "Target id", "")?;
    let output = path_input(theme, "Output directory", "./exported-target")?;
    let runtime = prompt_runtime_options(theme)?;
    let preview = format!(
        "gutch target export --repo {} --target {} --output {}{}",
        shell_arg_path(repo),
        shell_arg(&target),
        shell_arg_path(&output),
        runtime_preview_suffix(&runtime),
    );
    confirm_preview(theme, &preview)?;

    crate::cli::target::run(TargetCommands::Export {
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
    let dir = path_input(theme, "Install directory", "./install")?;
    let staging_dir = if Confirm::with_theme(theme)
        .with_prompt("Specify a custom staging directory?")
        .default(false)
        .interact()
        .map_err(dialoguer_to_manifest_error)?
    {
        optional_path_input(theme, "Staging directory", "")?
    } else {
        None
    };
    let source_target = optional_string_input(theme, "Source target id", "")?;
    let source_dir = if source_target.is_some() {
        optional_path_input(theme, "Source install directory", "")?
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
    let dir = path_input(theme, "Install directory", "./install")?;
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

fn print_repo_config_summary(repo_manifest: &g::manifest::content_repo::ContentRepoManifest) {
    println!("Repo config summary");
    println!("  content repo      {}", repo_manifest.content_repo_id);
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
    println!("  on incomplete     {}", incomplete_behavior_str(schema.on_incomplete));
    println!("  target id order   {}", schema.target_id_order.join(", "));
    println!("  tag definitions   {}", schema.target_tags.len());
    for definition in &schema.target_tags {
        let aliases = if definition.aliases.is_empty() {
            "-".to_string()
        } else {
            definition
                .aliases
                .iter()
                .map(|(alias, canonical)| format!("{alias}={canonical}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let values = if definition.values.is_empty() {
            "-".to_string()
        } else {
            definition.values.join(", ")
        };
        let default_value = definition.default_value.as_deref().unwrap_or("-");
        println!(
            "    - {} [{}] required={} default={} values={} aliases={}",
            definition.name,
            target_tag_kind_str(definition.kind.clone()),
            definition.required,
            default_value,
            values,
            aliases
        );
    }
}

fn edit_incomplete_behavior(
    theme: &ColorfulTheme,
    repo_manifest: &mut g::manifest::content_repo::ContentRepoManifest,
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
    repo_manifest: &mut g::manifest::content_repo::ContentRepoManifest,
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
        .default(existing_definition.as_ref().is_some_and(|definition| definition.required))
        .interact()
        .map_err(dialoguer_to_manifest_error)?;
    let values = csv_list_input(
        theme,
        "Allowed values (comma-separated, optional)",
        existing_definition
            .as_ref()
            .map(|definition| definition.values.join(", "))
            .unwrap_or_default()
            .as_str(),
    )?;
    let aliases = alias_map_input(
        theme,
        "Aliases (comma-separated alias=canonical, optional)",
        existing_definition
            .as_ref()
            .map(render_alias_map)
            .unwrap_or_default()
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
        values,
        aliases,
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
    repo_manifest: &mut g::manifest::content_repo::ContentRepoManifest,
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
    schema.target_id_order.retain(|name| name != &removed.name);
    println!("Removed tag definition '{}'.", removed.name);
    Ok(())
}

fn edit_target_id_order(
    theme: &ColorfulTheme,
    repo_manifest: &mut g::manifest::content_repo::ContentRepoManifest,
) -> g::Result<()> {
    let schema = ensure_target_schema(repo_manifest);
    let current = schema.target_id_order.join(", ");
    let order = csv_list_input(
        theme,
        "Target id order (comma-separated tag names)",
        current.as_str(),
    )?;
    schema.target_id_order = order;
    Ok(())
}

fn edit_repo_profiles(
    theme: &ColorfulTheme,
    repo_manifest: &mut g::manifest::content_repo::ContentRepoManifest,
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
        repo_manifest.compression_profile.zstd_level.to_string().as_str(),
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
    let raw = optional_string_input(
        theme,
        "Extra tags (comma-separated, optional)",
        "",
    )?;
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

fn alias_map_input(
    theme: &ColorfulTheme,
    prompt: &str,
    default: &str,
) -> g::Result<BTreeMap<String, String>> {
    let raw = optional_string_input(theme, prompt, default)?;
    let mut aliases = BTreeMap::new();
    let Some(raw) = raw else {
        return Ok(aliases);
    };

    for entry in raw.split(',').map(str::trim).filter(|entry| !entry.is_empty()) {
        let Some((alias, canonical)) = entry.split_once('=') else {
            return Err(g::Error::manifest(format!(
                "invalid alias entry '{}'. Use alias=canonical",
                entry
            )));
        };
        let alias = alias.trim();
        let canonical = canonical.trim();
        if alias.is_empty() || canonical.is_empty() {
            return Err(g::Error::manifest(format!(
                "invalid alias entry '{}'. Use alias=canonical",
                entry
            )));
        }
        aliases.insert(alias.to_string(), canonical.to_string());
    }

    Ok(aliases)
}

fn confirm_preview(theme: &ColorfulTheme, preview: &str) -> g::Result<()> {
    println!();
    println!("Equivalent command:");
    println!("  {}", preview);
    println!();
    let confirmed = Confirm::with_theme(theme)
        .with_prompt("Run this command now?")
        .default(true)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;
    if confirmed {
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
    repo_manifest: &mut g::manifest::content_repo::ContentRepoManifest,
) -> &mut g::manifest::TargetTagSchema {
    repo_manifest
        .target_schema
        .get_or_insert_with(g::manifest::TargetTagSchema::default)
}

fn render_alias_map(definition: &g::manifest::TargetTagDefinition) -> String {
    definition
        .aliases
        .iter()
        .map(|(alias, canonical)| format!("{alias}={canonical}"))
        .collect::<Vec<_>>()
        .join(", ")
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
