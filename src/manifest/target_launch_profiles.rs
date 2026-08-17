use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::{ExternalProductDependencyRequirement, FORMAT_VERSION, TargetLaunchAction};
use crate::{Error, Result};

pub const TARGET_LAUNCH_PROFILES_DOCUMENT_TYPE: &str = "gutch_target_launch_profiles";
pub const TARGET_LAUNCH_PROFILES_FILE: &str = "target_launch_profiles.json";
pub const DEFAULT_TARGET_LAUNCH_PROFILE_ID: &str = "default-launch";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetLaunchProfilesDocument {
    pub document_type: String,
    pub document_version: String,
    pub default_target_launch_profile_id: String,
    pub target_launch_profiles: BTreeMap<String, TargetLaunchProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetLaunchProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_description: Option<String>,
    pub launch_actions: Vec<TargetLaunchAction>,
    pub external_dependencies: Vec<ExternalProductDependencyRequirement>,
    pub assigned_target_ids: Vec<String>,
}

impl Default for TargetLaunchProfilesDocument {
    fn default() -> Self {
        let mut target_launch_profiles = BTreeMap::new();
        target_launch_profiles.insert(
            DEFAULT_TARGET_LAUNCH_PROFILE_ID.to_string(),
            TargetLaunchProfile {
                profile_description: Some(
                    "Launch configuration automatically assigned to newly inserted targets."
                        .to_string(),
                ),
                launch_actions: Vec::new(),
                external_dependencies: Vec::new(),
                assigned_target_ids: Vec::new(),
            },
        );
        Self {
            document_type: TARGET_LAUNCH_PROFILES_DOCUMENT_TYPE.to_string(),
            document_version: FORMAT_VERSION.to_string(),
            default_target_launch_profile_id: DEFAULT_TARGET_LAUNCH_PROFILE_ID.to_string(),
            target_launch_profiles,
        }
    }
}

impl TargetLaunchProfilesDocument {
    pub fn validate(&self) -> Result<()> {
        if self.document_type != TARGET_LAUNCH_PROFILES_DOCUMENT_TYPE {
            return Err(Error::manifest(
                "target launch profiles document_type is unsupported",
            ));
        }
        if self.document_version != FORMAT_VERSION {
            return Err(Error::manifest(
                "target launch profiles document_version is unsupported",
            ));
        }
        if !self
            .target_launch_profiles
            .contains_key(&self.default_target_launch_profile_id)
        {
            return Err(Error::manifest(
                "default target launch profile does not exist",
            ));
        }

        let mut target_assignments = HashMap::new();
        for (profile_id, profile) in &self.target_launch_profiles {
            validate_profile_id(profile_id)?;
            if profile
                .profile_description
                .as_deref()
                .is_some_and(|description| description.trim().is_empty())
            {
                return Err(Error::manifest(format!(
                    "target launch profile '{profile_id}' has an empty description"
                )));
            }
            super::target::validate_runtime_actions(&profile.launch_actions, false)?;
            super::validate_external_product_dependency_requirements(
                &profile.external_dependencies,
            )?;
            for target_id in &profile.assigned_target_ids {
                validate_target_id(target_id)?;
                if let Some(previous_profile_id) =
                    target_assignments.insert(target_id.as_str(), profile_id.as_str())
                {
                    return Err(Error::manifest(format!(
                        "target '{target_id}' is assigned to both '{previous_profile_id}' and '{profile_id}'"
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn validate_repository_targets(&self, repository_target_ids: &[String]) -> Result<()> {
        self.validate()?;
        let repository_target_ids = repository_target_ids.iter().collect::<HashSet<_>>();
        let assigned_target_ids = self
            .target_launch_profiles
            .values()
            .flat_map(|profile| profile.assigned_target_ids.iter())
            .collect::<HashSet<_>>();
        if repository_target_ids == assigned_target_ids {
            return Ok(());
        }
        let missing = repository_target_ids
            .difference(&assigned_target_ids)
            .next()
            .map(|value| value.as_str());
        let unknown = assigned_target_ids
            .difference(&repository_target_ids)
            .next()
            .map(|value| value.as_str());
        Err(Error::manifest(match (missing, unknown) {
            (Some(target_id), _) => format!(
                "repository target '{target_id}' is not assigned to a target launch profile"
            ),
            (_, Some(target_id)) => {
                format!("target launch profiles reference unknown target '{target_id}'")
            }
            _ => "target launch profile assignments do not match repository targets".to_string(),
        }))
    }

    pub fn assigned_profile_id(&self, target_id: &str) -> Option<&str> {
        self.target_launch_profiles
            .iter()
            .find(|(_, profile)| {
                profile
                    .assigned_target_ids
                    .iter()
                    .any(|assigned_target_id| assigned_target_id == target_id)
            })
            .map(|(profile_id, _)| profile_id.as_str())
    }
}

pub fn validate_profile_id(profile_id: &str) -> Result<()> {
    let mut characters = profile_id.chars();
    let valid = profile_id.len() <= 64
        && matches!(characters.next(), Some(first) if first.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        });
    if valid {
        Ok(())
    } else {
        Err(Error::manifest(format!(
            "invalid target launch profile id '{profile_id}'"
        )))
    }
}

pub fn validate_target_id(target_id: &str) -> Result<()> {
    if target_id.len() == 64
        && target_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(Error::manifest(format!(
            "invalid BLAKE3 target id '{target_id}'"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target_id(character: char) -> String {
        std::iter::repeat_n(character, 64).collect()
    }

    #[test]
    fn default_document_has_one_empty_default_profile() {
        let document = TargetLaunchProfilesDocument::default();

        assert!(document.validate().is_ok());
        assert_eq!(
            document.default_target_launch_profile_id,
            DEFAULT_TARGET_LAUNCH_PROFILE_ID
        );
        assert_eq!(document.target_launch_profiles.len(), 1);
        assert!(
            document.target_launch_profiles[DEFAULT_TARGET_LAUNCH_PROFILE_ID]
                .assigned_target_ids
                .is_empty()
        );
    }

    #[test]
    fn rejects_a_target_assigned_to_multiple_profiles() {
        let duplicate_target_id = target_id('a');
        let mut document = TargetLaunchProfilesDocument::default();
        document
            .target_launch_profiles
            .get_mut(DEFAULT_TARGET_LAUNCH_PROFILE_ID)
            .unwrap()
            .assigned_target_ids
            .push(duplicate_target_id.clone());
        document.target_launch_profiles.insert(
            "alternate-launch".into(),
            TargetLaunchProfile {
                profile_description: None,
                launch_actions: Vec::new(),
                external_dependencies: Vec::new(),
                assigned_target_ids: vec![duplicate_target_id],
            },
        );

        assert!(document.validate().is_err());
    }

    #[test]
    fn requires_assignments_to_match_repository_targets() {
        let target_id = target_id('b');
        let document = TargetLaunchProfilesDocument::default();

        assert!(
            document
                .validate_repository_targets(std::slice::from_ref(&target_id))
                .is_err()
        );
    }

    #[test]
    fn assigned_target_ids_serialize_last_in_each_profile() {
        let json = serde_json::to_string_pretty(&TargetLaunchProfilesDocument::default()).unwrap();
        let description = json.find("profile_description").unwrap();
        let actions = json.find("launch_actions").unwrap();
        let dependencies = json.find("external_dependencies").unwrap();
        let assignments = json.find("assigned_target_ids").unwrap();

        assert!(description < actions && actions < dependencies && dependencies < assignments);
    }
}
