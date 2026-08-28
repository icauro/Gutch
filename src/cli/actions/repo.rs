use crate::cli::common::load_optional_rules;
use gutch as g;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub fn init_repo(path: &Path, schema: Option<&PathBuf>, force: bool) -> g::Result<()> {
    let assessment = g::assess_repo_init_path(path)?;
    let mut proceed_force = force;
    if assessment.requires_confirmation && !proceed_force {
        if !confirm_repo_init_stdio(&assessment)? {
            return Err(g::Error::manifest("repo creation cancelled"));
        }
        proceed_force = true;
    }

    let repo_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("base");
    g::initialize_gutch_repo_with_options(
        path,
        load_optional_rules(&schema.cloned())?,
        g::RepoInitOptions {
            force: proceed_force,
        },
    )?;

    println!("created gutch repo '{}' at {}", repo_name, path.display());
    Ok(())
}

pub fn rename_repo(repo: &Path, name: &str) -> g::Result<()> {
    g::update_gutch_repo_name(repo, name)?;
    println!("renamed repo '{}' to '{}'", repo.display(), name.trim());
    Ok(())
}

fn confirm_repo_init_stdio(assessment: &g::RepoInitAssessment) -> g::Result<bool> {
    println!();
    for risk in &assessment.risks {
        println!("{}", risk.message);
    }
    if assessment.path_kind == g::RepoInitPathKind::ExistingRepo {
        if let Some(name) = assessment.repo_name.as_deref() {
            println!("  existing repo: {name}");
        }
        if let Some(snapshot_count) = assessment.snapshot_count {
            println!("  snapshots: {snapshot_count}");
        }
        if let Some(bundle_count) = assessment.bundle_count {
            println!("  bundles: {bundle_count}");
        }
    } else if assessment.path_kind == g::RepoInitPathKind::NonEmpty {
        println!("  entries: {}", assessment.entry_count);
    }

    prompt_yes_no(
        "Delete existing Gutch repo files and create a new repo?",
        false,
    )
}

fn prompt_yes_no(prompt: &str, default_yes: bool) -> g::Result<bool> {
    loop {
        let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
        print!("{prompt} {suffix}: ");
        io::stdout()
            .flush()
            .map_err(|err| g::Error::manifest(format!("io error: {err}")))?;
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|err| g::Error::manifest(format!("io error: {err}")))?;
        let trimmed = line.trim().to_ascii_lowercase();
        if trimmed.is_empty() {
            return Ok(default_yes);
        }
        if trimmed == "y" || trimmed == "yes" {
            return Ok(true);
        }
        if trimmed == "n" || trimmed == "no" {
            return Ok(false);
        }
        eprintln!("enter y or n");
    }
}
