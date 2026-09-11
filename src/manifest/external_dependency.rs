use serde::Deserialize;
use serde::Serialize;

use crate::Error;
use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalProductDependencyRequirement {
    pub dependency_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_version_exclusive: Option<String>,
}

pub(crate) fn validate_external_product_dependency_requirements(
    requirements: &[ExternalProductDependencyRequirement],
) -> Result<()> {
    let mut dependency_ids = std::collections::HashSet::new();
    for requirement in requirements {
        validate_dependency_id(&requirement.dependency_id)?;
        if !dependency_ids.insert(requirement.dependency_id.as_str()) {
            return Err(Error::manifest(format!(
                "external dependency '{}' is declared more than once",
                requirement.dependency_id
            )));
        }
        validate_version("minimum_version", requirement.minimum_version.as_deref())?;
        validate_version(
            "maximum_version_exclusive",
            requirement.maximum_version_exclusive.as_deref(),
        )?;
        if let (Some(minimum), Some(maximum)) = (
            requirement.minimum_version.as_deref(),
            requirement.maximum_version_exclusive.as_deref(),
        ) && compare_versions(minimum, maximum) != std::cmp::Ordering::Less
        {
            return Err(Error::manifest(
                "external dependency maximum_version_exclusive must be greater than minimum_version",
            ));
        }
    }
    Ok(())
}

fn validate_dependency_id(dependency_id: &str) -> Result<()> {
    let components = dependency_id.split('.').collect::<Vec<_>>();
    let is_valid = components.len() >= 3
        && components.iter().all(|component| {
            !component.is_empty()
                && component.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
                })
                && component
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_ascii_lowercase())
        });
    if !is_valid || dependency_id.len() > 160 {
        return Err(Error::manifest(format!(
            "invalid external dependency id '{dependency_id}'"
        )));
    }
    Ok(())
}

fn validate_version(field: &str, version: Option<&str>) -> Result<()> {
    let Some(version) = version else {
        return Ok(());
    };
    let is_valid = !version.is_empty()
        && version.len() <= 64
        && version.split('.').all(|component| {
            !component.is_empty()
                && component
                    .chars()
                    .all(|character| character.is_ascii_digit())
        });
    if !is_valid {
        return Err(Error::manifest(format!(
            "external dependency {field} is invalid"
        )));
    }
    Ok(())
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let left = left
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect::<Vec<_>>();
    let right = right
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect::<Vec<_>>();
    for index in 0..left.len().max(right.len()) {
        match left
            .get(index)
            .unwrap_or(&0)
            .cmp(right.get(index).unwrap_or(&0))
        {
            std::cmp::Ordering::Equal => {}
            ordering => return ordering,
        }
    }
    std::cmp::Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_reverse_dns_dependency_ids_and_versions() {
        let requirements = vec![ExternalProductDependencyRequirement {
            dependency_id: "org.python.python".into(),
            minimum_version: Some("3.11".into()),
            maximum_version_exclusive: Some("4.0".into()),
        }];

        assert!(validate_external_product_dependency_requirements(&requirements).is_ok());
    }

    #[test]
    fn rejects_duplicate_dependency_ids() {
        let requirement = ExternalProductDependencyRequirement {
            dependency_id: "org.python.python".into(),
            minimum_version: None,
            maximum_version_exclusive: None,
        };

        assert!(
            validate_external_product_dependency_requirements(&[requirement.clone(), requirement,])
                .is_err()
        );
    }

    #[test]
    fn rejects_invalid_or_reversed_version_ranges() {
        let reversed = ExternalProductDependencyRequirement {
            dependency_id: "org.python.python".into(),
            minimum_version: Some("4.0".into()),
            maximum_version_exclusive: Some("3.0".into()),
        };
        let non_numeric = ExternalProductDependencyRequirement {
            dependency_id: "org.python.python".into(),
            minimum_version: Some("3.x".into()),
            maximum_version_exclusive: None,
        };

        assert!(validate_external_product_dependency_requirements(&[reversed]).is_err());
        assert!(validate_external_product_dependency_requirements(&[non_numeric]).is_err());
    }
}
