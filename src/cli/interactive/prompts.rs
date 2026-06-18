use std::path::{Path, PathBuf};

use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};

use gutch as g;

#[derive(Clone)]
pub(super) enum TaggingTemplate {
    Standard,
    Minimal,
}

pub(super) fn path_or_browse_input(
    theme: &ColorfulTheme,
    prompt: &str,
    default: &str,
) -> g::Result<PathBuf> {
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

pub(super) fn optional_path_or_browse_input(
    theme: &ColorfulTheme,
    prompt: &str,
    default: &str,
) -> g::Result<Option<PathBuf>> {
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

pub(super) fn path_or_browse_input_file(
    theme: &ColorfulTheme,
    prompt: &str,
    default: &str,
) -> g::Result<PathBuf> {
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

pub(super) fn prompt_tagging_template(theme: &ColorfulTheme) -> g::Result<TaggingTemplate> {
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

pub(super) fn minimal_tagging_template() -> g::manifest::TargetTagSchema {
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

pub(super) fn prompt_target_selection(
    theme: &ColorfulTheme,
    repo: &Path,
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
                super::display::render_target_tag_summary(&target.target_tags, target_schema),
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

pub(super) fn confirm_output_path_overwrite(
    theme: &ColorfulTheme,
    path: &Path,
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

    let confirmed = super::prompt_approval_action(
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

pub(super) fn prompt_runtime_options(
    theme: &ColorfulTheme,
) -> g::Result<super::InteractiveRuntimeOptions> {
    if !Confirm::with_theme(theme)
        .with_prompt("Configure runtime memory and worker options?")
        .default(false)
        .interact()
        .map_err(dialoguer_to_manifest_error)?
    {
        return Ok(super::InteractiveRuntimeOptions {
            memory_mode: g::MemoryMode::Auto,
            max_memory_mib: None,
            jobs: None,
        });
    }

    let memory_mode = prompt_memory_mode(theme)?;
    let max_memory_mib = optional_u64_input(theme, "Max memory MiB override", "")?;
    let jobs = optional_usize_input(theme, "Worker count override", "")?;
    Ok(super::InteractiveRuntimeOptions {
        memory_mode,
        max_memory_mib,
        jobs,
    })
}

pub(super) fn prompt_memory_mode(theme: &ColorfulTheme) -> g::Result<g::MemoryMode> {
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

pub(super) fn prompt_chunk_profile(
    theme: &ColorfulTheme,
) -> g::Result<g::manifest::ArtifactSizingPreset> {
    let options = [
        ("small", g::manifest::ArtifactSizingPreset::Small),
        ("balanced", g::manifest::ArtifactSizingPreset::Balanced),
        ("large", g::manifest::ArtifactSizingPreset::Large),
    ];
    let labels: Vec<&str> = options.iter().map(|(label, _)| *label).collect();
    let selection = Select::with_theme(theme)
        .with_prompt("Chunk profile")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(dialoguer_to_manifest_error)?;
    Ok(options[selection].1.clone())
}

pub(super) fn prompt_bundle_profile(
    theme: &ColorfulTheme,
) -> g::Result<g::manifest::BundleSizingPreset> {
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

pub(super) fn prompt_target_tag_kind(
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

pub(super) fn required_string(
    theme: &ColorfulTheme,
    prompt: &str,
    default: &str,
) -> g::Result<String> {
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

pub(super) fn optional_string_input(
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

pub(super) fn path_input(theme: &ColorfulTheme, prompt: &str, default: &str) -> g::Result<PathBuf> {
    required_string(theme, prompt, default).map(PathBuf::from)
}

pub(super) fn optional_path_input(
    theme: &ColorfulTheme,
    prompt: &str,
    default: &str,
) -> g::Result<Option<PathBuf>> {
    optional_string_input(theme, prompt, default).map(|value| value.map(PathBuf::from))
}

pub(super) fn optional_u64_input(
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

pub(super) fn optional_usize_input(
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

pub(super) fn prompt_i32_input(
    theme: &ColorfulTheme,
    prompt: &str,
    default: &str,
) -> g::Result<i32> {
    let value = required_string(theme, prompt, default)?;
    value
        .parse::<i32>()
        .map_err(|_| g::Error::manifest(format!("invalid number for {}: {}", prompt, value)))
}

pub(super) fn optional_tag_list(theme: &ColorfulTheme) -> g::Result<Vec<String>> {
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

pub(super) fn csv_list_input(
    theme: &ColorfulTheme,
    prompt: &str,
    default: &str,
) -> g::Result<Vec<String>> {
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

pub(super) fn confirm_preview(theme: &ColorfulTheme, preview: &str) -> g::Result<()> {
    println!();
    println!("Equivalent command:");
    println!("  {}", preview);
    println!();
    let confirmed = super::prompt_approval_action(theme, "Run this command now?")?;
    if confirmed {
        super::set_last_ran_command(preview.to_string());
        Ok(())
    } else {
        Err(g::Error::manifest("interactive command cancelled"))
    }
}

pub(super) fn runtime_preview_suffix(runtime: &super::InteractiveRuntimeOptions) -> String {
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

pub(super) fn tags_preview_suffix(tags: &[String]) -> String {
    if tags.is_empty() {
        return String::new();
    }
    tags.iter()
        .map(|tag| format!(" --tag {}", shell_arg(tag)))
        .collect::<String>()
}

pub(super) fn artifact_sizing_str(profile: g::manifest::ArtifactSizingPreset) -> &'static str {
    match profile {
        g::manifest::ArtifactSizingPreset::Small => "small",
        g::manifest::ArtifactSizingPreset::Balanced => "balanced",
        g::manifest::ArtifactSizingPreset::Large => "large",
    }
}

pub(super) fn bundle_sizing_str(profile: g::manifest::BundleSizingPreset) -> &'static str {
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

pub(super) fn shell_arg(value: &str) -> String {
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

pub(super) fn shell_arg_path(path: &Path) -> String {
    shell_arg(&path.display().to_string())
}

pub(super) fn trim_to_option(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(super) fn dialoguer_to_manifest_error(err: dialoguer::Error) -> g::Error {
    g::Error::manifest(format!("interactive prompt failed: {}", err))
}
