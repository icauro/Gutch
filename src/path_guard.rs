use std::path::{Component, Path, PathBuf};

use crate::{Error, Result};

pub(crate) fn resolve_manifest_relative_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let relative_path = Path::new(relative);
    if relative.trim().is_empty()
        || relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir
            )
        })
    {
        return Err(Error::PathError {
            path: root.join(relative_path),
            reason: format!(
                "manifest path must stay within the target root: '{}'",
                relative
            ),
        });
    }

    Ok(root.join(relative_path))
}
