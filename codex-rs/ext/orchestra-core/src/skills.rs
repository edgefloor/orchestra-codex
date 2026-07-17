use crate::ResolvedSkill;
use crate::SkillIdentity;
use crate::SkillRequirement;
use crate::SkillToolDependency;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillManifest {
    pub entries: BTreeMap<String, SkillManifestEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillManifestEntry {
    pub identity: SkillIdentity,
    pub instructions: SkillArtifact,
    pub resources: BTreeMap<String, SkillArtifact>,
    pub tool_dependencies: Vec<SkillToolDependency>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillArtifact {
    pub path: PathBuf,
    pub sha256: String,
}

pub(crate) struct PreparedSkills {
    pub manifest: SkillManifest,
    pub sha256: String,
    pub files: BTreeMap<PathBuf, Vec<u8>>,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum SkillError {
    #[error("skill requirement name `{0}` must be nonempty and contain no whitespace")]
    InvalidName(String),
    #[error("duplicate skill requirement `{0}`")]
    DuplicateRequirement(String),
    #[error("duplicate skill requirement `{0}` has conflicting declarations")]
    ConflictingRequirement(String),
    #[error("skill `{skill}` requires undeclared skill `{required}`")]
    UndeclaredRequirement { skill: String, required: String },
    #[error("skill requirement cycle includes `{0}`")]
    Cycle(String),
    #[error("skill `{skill}` declares unsafe resource path `{resource}`")]
    UnsafeResource { skill: String, resource: String },
    #[error("native host did not resolve required skill `{0}`")]
    Missing(String),
    #[error("native host returned unexpected skill `{0}`")]
    Unexpected(String),
    #[error("native host returned duplicate skill `{0}`")]
    Duplicate(String),
    #[error("skill requirements `{first}` and `{second}` resolve to the same installed identity")]
    DuplicateIdentity { first: String, second: String },
    #[error("resolved skill `{skill}` omitted resource `{resource}`")]
    MissingResource { skill: String, resource: String },
    #[error("resolved skill `{skill}` returned undeclared resource `{resource}`")]
    UnexpectedResource { skill: String, resource: String },
    #[error("resolved skill `{0}` instructions are not UTF-8")]
    NonUtf8Instructions(String),
    #[error("skill snapshot digest mismatch: expected {expected}, found {actual}")]
    DigestMismatch { expected: String, actual: String },
    #[error("skill snapshot artifact changed or is missing: {0}")]
    ArtifactChanged(String),
}

pub(crate) fn collect_requirements(
    requirement_groups: impl Iterator<Item = Vec<SkillRequirement>>,
) -> Result<BTreeMap<String, SkillRequirement>, SkillError> {
    let mut all: BTreeMap<String, SkillRequirement> = BTreeMap::new();
    for group in requirement_groups {
        let names: BTreeSet<_> = group.iter().map(|skill| skill.name.as_str()).collect();
        if names.len() != group.len() {
            let mut seen = BTreeSet::new();
            let duplicate = group
                .iter()
                .find(|skill| !seen.insert(skill.name.as_str()))
                .map(|skill| skill.name.clone())
                .unwrap();
            return Err(SkillError::DuplicateRequirement(duplicate));
        }
        for skill in &group {
            if skill.name.is_empty() || skill.name.chars().any(char::is_whitespace) {
                return Err(SkillError::InvalidName(skill.name.clone()));
            }
            for required in &skill.requires {
                if !names.contains(required.as_str()) {
                    return Err(SkillError::UndeclaredRequirement {
                        skill: skill.name.clone(),
                        required: required.clone(),
                    });
                }
            }
        }
        for mut skill in group {
            skill.requires.sort();
            skill.requires.dedup();
            skill.resources.sort();
            skill.resources.dedup();
            if skill.resources.iter().any(|path| !safe_relative(path)) {
                let resource = skill
                    .resources
                    .iter()
                    .find(|path| !safe_relative(path))
                    .cloned()
                    .unwrap();
                return Err(SkillError::UnsafeResource {
                    skill: skill.name,
                    resource,
                });
            }
            match all.get_mut(&skill.name) {
                Some(existing) if existing.requires != skill.requires => {
                    return Err(SkillError::ConflictingRequirement(skill.name));
                }
                Some(existing) => {
                    existing.resources.extend(skill.resources);
                    existing.resources.sort();
                    existing.resources.dedup();
                }
                None => {
                    all.insert(skill.name.clone(), skill);
                }
            }
        }
    }
    detect_cycles(&all)?;
    Ok(all)
}

pub(crate) fn prepare_skills(
    requirements: &BTreeMap<String, SkillRequirement>,
    resolved: Vec<ResolvedSkill>,
) -> Result<PreparedSkills, SkillError> {
    let mut by_requirement = BTreeMap::new();
    for skill in resolved {
        let name = skill.requirement.clone();
        if !requirements.contains_key(&name) {
            return Err(SkillError::Unexpected(name));
        }
        if by_requirement.insert(name.clone(), skill).is_some() {
            return Err(SkillError::Duplicate(name));
        }
    }
    let mut entries = BTreeMap::new();
    let mut files = BTreeMap::new();
    let mut identities = BTreeMap::new();
    for (index, (name, requirement)) in requirements.iter().enumerate() {
        let skill = by_requirement
            .remove(name)
            .ok_or_else(|| SkillError::Missing(name.clone()))?;
        let identity_key = (
            skill.identity.canonical_name.clone(),
            skill.identity.source_kind,
            skill.identity.source_locator.clone(),
            skill.identity.plugin_id.clone(),
        );
        if let Some(first) = identities.insert(identity_key, name.clone()) {
            return Err(SkillError::DuplicateIdentity {
                first,
                second: name.clone(),
            });
        }
        std::str::from_utf8(&skill.instructions)
            .map_err(|_| SkillError::NonUtf8Instructions(name.clone()))?;
        for resource in &requirement.resources {
            if !skill.resources.contains_key(resource) {
                return Err(SkillError::MissingResource {
                    skill: name.clone(),
                    resource: resource.clone(),
                });
            }
        }
        for resource in skill.resources.keys() {
            if !requirement.resources.contains(resource) {
                return Err(SkillError::UnexpectedResource {
                    skill: name.clone(),
                    resource: resource.clone(),
                });
            }
        }
        let directory = PathBuf::from(format!("evidence/skills/{index}-{}", safe_name(name)));
        let instruction_path = directory.join("SKILL.md");
        let instructions = artifact(&instruction_path, &skill.instructions);
        files.insert(instruction_path, skill.instructions);
        let mut resources = BTreeMap::new();
        for (resource, bytes) in skill.resources {
            let path = directory.join("resources").join(&resource);
            resources.insert(resource, artifact(&path, &bytes));
            files.insert(path, bytes);
        }
        entries.insert(
            name.clone(),
            SkillManifestEntry {
                identity: skill.identity,
                instructions,
                resources,
                tool_dependencies: skill.tool_dependencies,
            },
        );
    }
    let manifest = SkillManifest { entries };
    let sha256 = digest_json(&manifest);
    Ok(PreparedSkills {
        manifest,
        sha256,
        files,
    })
}

pub(crate) fn verify_and_load(
    root: &Path,
    manifest: &SkillManifest,
    expected_sha256: &str,
) -> Result<BTreeMap<String, String>, SkillError> {
    let actual = digest_json(manifest);
    if actual != expected_sha256 {
        return Err(SkillError::DigestMismatch {
            expected: expected_sha256.into(),
            actual,
        });
    }
    let mut instructions = BTreeMap::new();
    for (name, entry) in &manifest.entries {
        let bytes = verify_artifact(root, &entry.instructions)?;
        let text =
            String::from_utf8(bytes).map_err(|_| SkillError::NonUtf8Instructions(name.clone()))?;
        instructions.insert(name.clone(), text);
        for artifact in entry.resources.values() {
            verify_artifact(root, artifact)?;
        }
    }
    Ok(instructions)
}

fn verify_artifact(root: &Path, artifact: &SkillArtifact) -> Result<Vec<u8>, SkillError> {
    let bytes = std::fs::read(root.join(&artifact.path))
        .map_err(|_| SkillError::ArtifactChanged(artifact.path.display().to_string()))?;
    if digest(&bytes) != artifact.sha256 {
        return Err(SkillError::ArtifactChanged(
            artifact.path.display().to_string(),
        ));
    }
    Ok(bytes)
}

fn detect_cycles(requirements: &BTreeMap<String, SkillRequirement>) -> Result<(), SkillError> {
    fn visit(
        name: &str,
        requirements: &BTreeMap<String, SkillRequirement>,
        visiting: &mut BTreeSet<String>,
        done: &mut BTreeSet<String>,
    ) -> Result<(), SkillError> {
        if done.contains(name) {
            return Ok(());
        }
        if !visiting.insert(name.into()) {
            return Err(SkillError::Cycle(name.into()));
        }
        if let Some(requirement) = requirements.get(name) {
            for required in &requirement.requires {
                visit(required, requirements, visiting, done)?;
            }
        }
        visiting.remove(name);
        done.insert(name.into());
        Ok(())
    }
    let mut done = BTreeSet::new();
    for name in requirements.keys() {
        visit(name, requirements, &mut BTreeSet::new(), &mut done)?;
    }
    Ok(())
}

fn safe_relative(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('\\')
        && !Path::new(value).is_absolute()
        && Path::new(value)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn safe_name(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn artifact(path: &Path, bytes: &[u8]) -> SkillArtifact {
    SkillArtifact {
        path: path.into(),
        sha256: digest(bytes),
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn digest_json(value: &impl Serialize) -> String {
    digest(&serde_json::to_vec(value).expect("skill manifest serializes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requirement(name: &str, requires: &[&str], resources: &[&str]) -> SkillRequirement {
        SkillRequirement {
            name: name.into(),
            requires: requires.iter().map(|value| (*value).into()).collect(),
            resources: resources.iter().map(|value| (*value).into()).collect(),
        }
    }

    fn resolved(name: &str) -> ResolvedSkill {
        ResolvedSkill {
            requirement: name.into(),
            identity: SkillIdentity {
                canonical_name: name.into(),
                source_kind: crate::SkillSourceKind::User,
                source_locator: format!("/skills/{name}/SKILL.md"),
                plugin_id: None,
            },
            instructions: format!("instructions for {name}").into_bytes(),
            resources: BTreeMap::new(),
            tool_dependencies: vec![],
        }
    }

    #[test]
    fn requires_complete_acyclic_closure_and_safe_resources() {
        let valid = collect_requirements(
            [vec![
                requirement("implement", &["tdd"], &[]),
                requirement("tdd", &[], &["references/testing.md"]),
            ]]
            .into_iter(),
        )
        .unwrap();
        assert_eq!(valid.len(), 2);

        assert!(matches!(
            collect_requirements([vec![requirement("implement", &["tdd"], &[])]].into_iter()),
            Err(SkillError::UndeclaredRequirement { .. })
        ));
        assert!(matches!(
            collect_requirements(
                [vec![
                    requirement("one", &["two"], &[]),
                    requirement("two", &["one"], &[]),
                ]]
                .into_iter()
            ),
            Err(SkillError::Cycle(_))
        ));
        assert!(matches!(
            collect_requirements([vec![requirement("unsafe", &[], &["../secret"])]].into_iter()),
            Err(SkillError::UnsafeResource { .. })
        ));
        assert!(matches!(
            collect_requirements([vec![requirement("unsafe", &[], &["..\\secret"])]].into_iter()),
            Err(SkillError::UnsafeResource { .. })
        ));
        assert!(matches!(
            collect_requirements([vec![requirement("bad name", &[], &[])]].into_iter()),
            Err(SkillError::InvalidName(_))
        ));
    }

    #[test]
    fn resolved_snapshot_is_complete_and_order_independent() {
        let requirements = BTreeMap::from([
            ("implement".into(), requirement("implement", &["tdd"], &[])),
            ("tdd".into(), requirement("tdd", &[], &[])),
        ]);
        let one =
            prepare_skills(&requirements, vec![resolved("implement"), resolved("tdd")]).unwrap();
        let two =
            prepare_skills(&requirements, vec![resolved("tdd"), resolved("implement")]).unwrap();
        assert_eq!(one.sha256, two.sha256);
        assert!(matches!(
            prepare_skills(&requirements, vec![resolved("implement")]),
            Err(SkillError::Missing(name)) if name == "tdd"
        ));
    }
}
