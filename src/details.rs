//! What a run record says about how the run was done: who ran it, on which
//! image, in what conditions, with which artifacts and observations. Board
//! evidence must carry it. An agent may run what has no hazard; a person runs
//! the rest, and a pass counts only with its entry's preconditions met.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::evidence::{Outcome, RunResult};
use crate::identity::hex;
use crate::schema::Hazard;

/// Artifacts live under the project's artifact directory; records hold only
/// their path, size and hash, never their content.

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Details {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<Execution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<Candidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Conditions>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<CommandRun>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub measurements: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_conditions_triggered: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Execution {
    pub performed_by: Performer,
    #[serde(default)]
    pub user_confirmed: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Performer {
    Agent,
    User,
    UserWithAgentAssistance,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub firmware_sha256: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_triple: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub configuration: serde_json::Map<String, serde_json::Value>,
}

/// The state things were in during a run, as `name: state`, and what was used.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Conditions {
    #[serde(flatten)]
    pub states: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub equipment: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRun {
    pub command: String,
    pub exit_code: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub id: String,
    pub kind: ArtifactKind,
    /// Repository-relative, under the project's artifact directory.
    pub path: String,
    /// Filled in by `reqtool record` from the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    Analysis,
    CommandOutput,
    Configuration,
    Firmware,
    Image,
    Log,
    Other,
    Report,
    Trace,
}

impl Details {
    /// Hashes every artifact from its file, refusing a path outside the
    /// project's artifact directory,
    /// a missing file, or a stated hash that does not match.
    pub fn seal_artifacts(&mut self, repository: &Path) -> Result<(), String> {
        let mut ids = std::collections::BTreeSet::new();
        for artifact in &self.artifacts {
            if !valid_artifact_id(&artifact.id) {
                return Err(format!(
                    "artifact ID {:?} must be lowercase words joined by hyphens, led by a letter, at most 64 characters",
                    artifact.id
                ));
            }
            if !ids.insert(artifact.id.as_str()) {
                return Err(format!("artifact ID {} is used twice", artifact.id));
            }
        }
        for artifact in &mut self.artifacts {
            let (bytes, sha256) = hash_artifact(repository, &artifact.path)?
                .ok_or_else(|| format!("artifact {} does not exist", artifact.path))?;
            if artifact
                .sha256
                .as_ref()
                .is_some_and(|stated| *stated != sha256)
            {
                return Err(format!(
                    "artifact {}: the stated SHA-256 does not match the file",
                    artifact.path
                ));
            }
            artifact.bytes = Some(bytes);
            artifact.sha256 = Some(sha256);
        }
        Ok(())
    }

    /// For a record read back: an artifact present on this machine must
    /// still match its hash. An absent one is fine; logs stay local.
    pub fn verify_artifacts(&self, repository: &Path) -> Result<(), String> {
        for artifact in &self.artifacts {
            if let Some((_, sha256)) = hash_artifact(repository, &artifact.path)?
                && artifact.sha256.as_deref() != Some(sha256.as_str())
            {
                return Err(format!(
                    "artifact {} no longer matches its recorded SHA-256",
                    artifact.path
                ));
            }
        }
        Ok(())
    }
}

fn valid_artifact_id(id: &str) -> bool {
    id.len() <= 64
        && id
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn hash_artifact(repository: &Path, path: &str) -> Result<Option<(u64, String)>, String> {
    let relative = Path::new(path);
    let inside = relative
        .components()
        .all(|part| matches!(part, Component::Normal(_)));
    let root = Config::load(repository)?.artifacts;
    if !inside || !relative.starts_with(&root) {
        return Err(format!(
            "artifact {path} must be a relative path under {root}/"
        ));
    }
    let full = repository.join(relative);
    if !full.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(&full).map_err(|e| format!("artifact {path}: {e}"))?;
    Ok(Some((bytes.len() as u64, hex(&Sha256::digest(&bytes)))))
}

/// The rules for any run: a pass never carries a triggered stop condition.
/// Evidence on hardware says who ran it and on which image. A hazardous entry
/// is run by a person who confirms it, and a pass counts only where the record
/// shows its entry's preconditions held.
pub fn check(results: &[RunResult], details: Option<&Details>) -> Result<(), String> {
    let passes = results.iter().any(|result| result.result == Outcome::Pass);
    if let Some(details) = details
        && passes
        && !details.stop_conditions_triggered.is_empty()
    {
        return Err("a pass cannot include triggered stop conditions".into());
    }
    let on_hardware: Vec<&RunResult> = results
        .iter()
        .filter(|result| result.level.on_target())
        .collect();
    if on_hardware.is_empty() {
        return Ok(());
    }
    let details = details.ok_or("evidence on hardware needs details: execution")?;
    let execution = details
        .execution
        .as_ref()
        .ok_or("evidence on hardware needs execution")?;
    // Evidence follows files, not images: the image is optional, and only
    // checked for form when given.
    if let Some(candidate) = details.candidate.as_ref() {
        let sha = &candidate.firmware_sha256;
        if sha.len() != 64
            || !sha
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err("a candidate image's SHA-256 is 64 lowercase hex digits".into());
        }
    }
    for result in on_hardware {
        if result.hazard != Hazard::None {
            if execution.performed_by == Performer::Agent {
                return Err(format!(
                    "{}: hazard {} is run by a person, never by an agent alone",
                    result.requirement,
                    result.hazard.name()
                ));
            }
            if !execution.user_confirmed {
                return Err(format!(
                    "{}: hazard {} needs the person's confirmation",
                    result.requirement,
                    result.hazard.name()
                ));
            }
        }
        if result.result == Outcome::Pass {
            for (name, state) in &result.preconditions {
                let held = details
                    .conditions
                    .as_ref()
                    .and_then(|conditions| conditions.states.get(name));
                if held != Some(state) {
                    return Err(format!(
                        "{}: a pass needs {name} to be {state}, and the record says {}",
                        result.requirement,
                        held.map_or("nothing", String::as_str)
                    ));
                }
            }
        }
    }
    Ok(())
}
