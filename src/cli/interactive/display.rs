use std::path::Path;

use gutch as g;

pub(super) fn print_landing_info_table(config: &g::config::GutchConfig) {
    let tracked_repo_count = config.repos.len();
    let lookup_path_count = config
        .repos
        .iter()
        .filter(|repo| repo.target_lookup_path.is_some())
        .count();

    let rows = vec![
        ("tracked repos", tracked_repo_count.to_string()),
        ("lookup paths", lookup_path_count.to_string()),
    ];

    println!();
    print_ascii_key_value_table(&rows);
    println!();
}

pub(super) fn print_active_repo_info_table(repo: &Path) -> g::Result<()> {
    super::print_last_ran_command();

    let repo_path = repo.display().to_string();
    let config = g::config::load_config()?;
    let tracked = config.find_repo(&repo_path);
    let (_, repo_manifest) = g::load_gutch_repo_manifest(repo)?;

    let linked_value = if repo_manifest.is_linked() {
        format!("product {} / repo {}", repo_manifest.product_id, repo_manifest.remote_repo_id)
    } else {
        "not linked".to_string()
    };
    let lookup_value = tracked
        .and_then(|tracked_repo| tracked_repo.target_lookup_path.as_deref())
        .unwrap_or("-");

    let rows = vec![
        ("name", repo_manifest.name),
        ("path", repo.display().to_string()),
        ("linked", linked_value),
        ("lookup path", lookup_value.to_string()),
    ];

    println!();
    print_ascii_key_value_table(&rows);
    println!();
    Ok(())
}

pub(super) fn print_ascii_key_value_table(rows: &[(&str, String)]) {
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

pub(super) fn print_ascii_table(headers: &[&str], rows: &[Vec<String>]) {
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

pub(super) fn display_table_value(value: &str) -> String {
    if value.is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

pub(super) fn format_tag_column_name(schema: &g::manifest::TargetTagSchema, name: &str) -> String {
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

pub(super) fn relative_candidate_path(lookup_path: &Path, candidate_path: &str) -> String {
    let candidate_path = Path::new(candidate_path);
    candidate_path
        .strip_prefix(lookup_path)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| candidate_path.display().to_string())
}

pub(super) fn render_target_tag_summary(
    target_tags: &g::manifest::TargetTagSet,
    target_schema: Option<&g::manifest::TargetTagSchema>,
) -> String {
    target_tags.to_ordered_strings(target_schema).join(", ")
}

pub(super) fn tag_definition_value(
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

pub(super) fn print_repo_summary(repo: &Path) -> g::Result<()> {
    let status = g::inspect_gutch_repo(repo)?;
    let config = g::config::load_config()?;
    let tracked = config.find_repo(&repo.display().to_string());

    println!();
    println!("  path:     {}", status.repo_path);
    println!("  name:     {}", status.name);
    println!("  linked:   {}", if status.remote_repo_id.is_empty() { "no".to_string() } else { format!("product {} / repo {}", status.product_id, status.remote_repo_id) });
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
    }
    println!();
    Ok(())
}

pub(super) fn print_repo_inspect(repo: &Path) -> g::Result<()> {
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

pub(super) fn format_bytes(bytes: u64) -> String {
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

pub(super) fn print_discovered_targets(
    lookup_path: &Path,
    target_schema: &g::manifest::TargetTagSchema,
    candidates: &[&g::SourceTargetInstallCandidate],
) {
    println!();
    if !candidates.is_empty() {
        println!("Uninserted:");
        let schema_headers: Vec<String> = target_schema
            .target_tags
            .iter()
            .map(|definition| format_tag_column_name(target_schema, &definition.name))
            .collect();
        let rows: Vec<Vec<String>> = candidates
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
                row.push(relative_candidate_path(lookup_path, &candidate.path));
                row
            })
            .collect();
        let mut headers: Vec<&str> = schema_headers.iter().map(String::as_str).collect();
        headers.push("path");
        print_ascii_table(&headers, &rows);
    }
}
