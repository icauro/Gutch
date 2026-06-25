use std::path::Path;

use dialoguer::{Confirm, Select, theme::ColorfulTheme};

use gutch as g;

pub(super) fn run_repo_config_edit(theme: &ColorfulTheme, repo: &Path) -> g::Result<()> {
    let (repo_paths, mut repo_manifest) = g::load_gutch_repo_manifest(repo)?;

    loop {
        super::refresh_terminal()?;
        super::print_active_repo_info_table(repo)?;
        println!();
        print_repo_config_summary(&repo_manifest);
        println!();

        let actions = [
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
            .map_err(super::dialoguer_to_manifest_error)?;

        match selection {
            0 => add_or_update_tag_definition(theme, &mut repo_manifest)?,
            1 => remove_tag_definition(theme, &mut repo_manifest)?,
            2 => edit_repo_profiles(theme, &mut repo_manifest)?,
            3 => {
                if super::prompt_approval_action(
                    theme,
                    "Reset target schema to built-in defaults?",
                )? {
                    repo_manifest.target_schema = Some(g::manifest::TargetTagSchema::default());
                }
            }
            4 => {
                println!();
                println!("This will write:");
                println!("  {}", repo_paths.repo_manifest_path.display());
                println!();
                if super::prompt_approval_action(theme, "Save repo config changes?")? {
                    g::write_gutch_repo_manifest(&repo_paths, &repo_manifest)?;
                    println!(
                        "updated repo config: {}",
                        repo_paths.repo_manifest_path.display()
                    );
                    return Ok(());
                }
            }
            5 => return Err(g::Error::manifest("interactive command cancelled")),
            _ => unreachable!(),
        }
    }
}

fn print_repo_config_summary(repo_manifest: &g::manifest::gutch_repo::GutchRepoManifest) {
    println!("Repo config summary");
    println!("  name              {}", repo_manifest.name);
    println!(
        "  remote_repo_id    {}",
        if repo_manifest.remote_repo_id.is_empty() {
            "(not linked)"
        } else {
            &repo_manifest.remote_repo_id
        }
    );
    println!(
        "  chunk profile     {}",
        super::artifact_sizing_str(repo_manifest.chunk_sizing_profile.preset)
    );
    println!(
        "  bundle profile    {}",
        super::bundle_sizing_str(repo_manifest.bundle_sizing_profile.preset)
    );
    println!(
        "  compression       {}",
        repo_manifest.compression_profile.preset.cli_name()
    );

    let schema = repo_manifest.target_schema.clone().unwrap_or_default();
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
        .map_err(super::dialoguer_to_manifest_error)?;

    let existing_definition = if selection < existing_names.len() {
        schema.target_tags.get(selection).cloned()
    } else {
        None
    };

    let name = super::required_string(
        theme,
        "Tag name",
        existing_definition
            .as_ref()
            .map(|definition| definition.name.as_str())
            .unwrap_or(""),
    )?;
    let kind = super::prompt_target_tag_kind(
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
        .map_err(super::dialoguer_to_manifest_error)?;
    let values = super::csv_list_input(
        theme,
        "Allowed values (value[alias1|alias2], comma-separated, optional)",
        existing_definition
            .as_ref()
            .map(render_tag_values)
            .unwrap_or_default()
            .as_str(),
    )?;
    let inference_prefixes = super::csv_list_input(
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
    let default_value = super::optional_string_input(
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
        .map_err(super::dialoguer_to_manifest_error)?;
    let removed = schema.target_tags.remove(selection);
    println!("Removed tag definition '{}'.", removed.name);
    Ok(())
}

fn edit_repo_profiles(
    theme: &ColorfulTheme,
    repo_manifest: &mut g::manifest::gutch_repo::GutchRepoManifest,
) -> g::Result<()> {
    repo_manifest.chunk_sizing_profile =
        g::manifest::ChunkSizingProfile::for_preset(super::prompt_chunk_profile(theme)?);
    repo_manifest.bundle_sizing_profile =
        g::manifest::BundleSizingProfile::for_preset(super::prompt_bundle_profile(theme)?);
    repo_manifest.compression_profile =
        g::manifest::CompressionProfile::for_preset(super::prompt_compression_profile(theme)?);
    Ok(())
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
