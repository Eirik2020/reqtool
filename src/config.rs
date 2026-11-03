//! A project's settings, from `reqtool.toml` at its root: where it keeps its
//! requirements, roots and evidence, and how its code is built and traced.

use std::path::Path;

use serde::Deserialize;

/// The file that marks a project's root.
pub const FILE: &str = "reqtool.toml";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The name other projects link to this one's requirements by, normally
    /// its crate's: `my-hal:UART-012`.
    pub name: String,
    /// The directory holding requirement YAML, searched at every depth.
    pub requirements: String,
    /// The gates file; by default `gates.yaml` in `requirements`.
    pub gates: Option<String>,
    /// The Markdown file holding the project's roots, one per line as
    /// `1. **G1 - Title.** ...`.
    pub roots: String,
    /// What each root's ID starts with: the `G` of `G1`.
    pub root_prefix: String,
    /// The directory evidence records are written to.
    pub evidence: String,
    /// The directory a board run's artifacts must sit under.
    #[serde(default = "default_artifacts")]
    pub artifacts: String,
    /// The directory holding one firmware crate per board, named after it.
    pub boards: Option<String>,
    /// `[package.metadata.<this>] critical = true` marks a crate as traced.
    #[serde(default = "default_critical_metadata")]
    pub critical_metadata: String,
    /// Every source file owes tracing, with no per-file marker: for a project
    /// that has no notion of what is safety-critical.
    #[serde(default)]
    pub every_file_traced: bool,
    /// Directories of retired documents, which no active procedure may point into.
    #[serde(default)]
    pub retired: Vec<String>,
    /// The feature sets host tests are built and run with, each in turn. A test
    /// a set does not compile is simply not in that run.
    #[serde(default = "default_test_features")]
    pub test_features: Vec<Vec<String>>,
}

fn default_artifacts() -> String {
    "logs".to_owned()
}

fn default_critical_metadata() -> String {
    "reqtool".to_owned()
}

fn default_test_features() -> Vec<Vec<String>> {
    vec![Vec::new()]
}

impl Config {
    pub fn load(repository: &Path) -> Result<Self, String> {
        let path = repository.join(FILE);
        let text = std::fs::read_to_string(&path).map_err(|error| format!("{FILE}: {error}"))?;
        toml::from_str(&text).map_err(|error| format!("{FILE}: {error}"))
    }

    pub fn gates(&self) -> String {
        self.gates
            .clone()
            .unwrap_or_else(|| format!("{}/gates.yaml", self.requirements))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_required_settings_are_required() {
        let config: Config = toml::from_str(
            r#"name = "x"
               requirements = "req"
               roots = "goals.md"
               root_prefix = "G"
               evidence = "runs""#,
        )
        .unwrap();
        assert_eq!(config.gates(), "req/gates.yaml");
        assert_eq!(config.artifacts, "logs");
        assert_eq!(config.critical_metadata, "reqtool");
        assert_eq!(config.test_features, [Vec::<String>::new()]);
        assert!(!config.every_file_traced);
        assert_eq!(config.boards, None);
    }

    #[test]
    fn an_unknown_setting_is_refused() {
        let text = r#"name = "x"
            requirements = "req"
            roots = "goals.md"
            root_prefix = "G"
            evidence = "runs"
            evidance = "typo""#;
        assert!(toml::from_str::<Config>(text).is_err());
    }
}
