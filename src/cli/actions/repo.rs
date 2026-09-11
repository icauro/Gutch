use crate::cli::common::prompt_yes_no;
use std::path::Path;

pub(crate) fn init_repo(path: &Path, force: bool) -> crate::api::Result<()> {
    let assessment = crate::api::assess_repo_init_path(path)?;
    let mut proceed_force = force;
    if assessment.requires_confirmation && !proceed_force {
        if !confirm_repo_init_stdio(&assessment)? {
            return Err(crate::api::Error::manifest("repo creation cancelled"));
        }
        proceed_force = true;
    }

    let repo_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("base");
    crate::api::initialize_gutch_repo_with_options(
        path,
        crate::api::RepoInitOptions {
            force: proceed_force,
        },
    )?;

    println!("created gutch repo '{}' at {}", repo_name, path.display());
    Ok(())
}

pub(crate) fn rename_repo(repo: &Path, name: &str) -> crate::api::Result<()> {
    crate::api::update_gutch_repo_name(repo, name)?;
    println!("renamed repo '{}' to '{}'", repo.display(), name.trim());
    Ok(())
}

fn confirm_repo_init_stdio(
    assessment: &crate::api::RepoInitAssessment,
) -> crate::api::Result<bool> {
    println!();
    for risk in &assessment.risks {
        println!("{}", risk.message);
    }
    if assessment.path_kind == crate::api::RepoInitPathKind::ExistingRepo {
        if let Some(name) = assessment.repo_name.as_deref() {
            println!("  existing repo: {name}");
        }
        if let Some(snapshot_count) = assessment.snapshot_count {
            println!("  snapshots: {snapshot_count}");
        }
        if let Some(bundle_count) = assessment.bundle_count {
            println!("  bundles: {bundle_count}");
        }
    } else if assessment.path_kind == crate::api::RepoInitPathKind::NonEmpty {
        println!("  entries: {}", assessment.entry_count);
    }

    prompt_yes_no(
        "Delete existing Gutch repo files and create a new repo?",
        false,
    )
}
