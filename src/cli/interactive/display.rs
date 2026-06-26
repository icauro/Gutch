use std::path::Path;

use gutch as g;

pub(super) fn print_landing_info_table(config: &g::config::GutchConfig) {
    let tracked_repo_count = config.repos.len();
    let sources_path_count = config
        .repos
        .iter()
        .filter(|repo| repo.target_sources_path.is_some())
        .count();

    let rows = vec![
        ("tracked repos", tracked_repo_count.to_string()),
        ("target sources folders", sources_path_count.to_string()),
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
        format!(
            "product {} / repo {}",
            repo_manifest.product_id, repo_manifest.remote_repo_id
        )
    } else {
        "not linked".to_string()
    };
    let sources_value = tracked
        .and_then(|tracked_repo| tracked_repo.target_sources_path.as_deref())
        .unwrap_or("-");

    let rows = vec![
        ("name", repo_manifest.name),
        ("path", repo.display().to_string()),
        ("linked", linked_value),
        ("target sources", sources_value.to_string()),
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

pub(super) fn render_target_tag_summary(
    target_tags: &g::manifest::TargetTagSet,
    target_schema: Option<&g::manifest::TargetTagSchema>,
) -> String {
    target_tags.to_ordered_strings(target_schema).join(", ")
}

pub(super) fn print_repo_summary(repo: &Path) -> g::Result<()> {
    let status = g::inspect_gutch_repo(repo)?;
    let config = g::config::load_config()?;
    let tracked = config.find_repo(&repo.display().to_string());

    println!();
    println!("  path:     {}", status.repo_path);
    println!("  name:     {}", status.name);
    println!(
        "  linked:   {}",
        if status.remote_repo_id.is_empty() {
            "no".to_string()
        } else {
            format!(
                "product {} / repo {}",
                status.product_id, status.remote_repo_id
            )
        }
    );
    println!("  targets:  {}", status.targets.len());
    println!(
        "  bundles:  {} ({})",
        status.bundle_count,
        format_bytes(status.bundle_bytes)
    );

    if let Some(tracked) = tracked {
        if let Some(sources) = &tracked.target_sources_path {
            println!("  sources:  {}", sources);
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
