//! A project's settings, from `reqtool.toml` at its root: where it keeps its
//! requirements, roots and evidence, and how its code is built and traced.

use std::path::{Component, Path, PathBuf};

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
    /// What `every_file_traced` leaves out: repository-relative path prefixes,
    /// matched by whole components, such as a directory of bench crates. A file
    /// left out is safety-critical only if its own module doc says so.
    #[serde(default)]
    pub every_file_traced_except: Vec<String>,
    /// Directories of retired documents, which no active procedure may point into.
    #[serde(default)]
    pub retired: Vec<String>,
    /// The feature sets host tests are built and run with, each in turn. A test
    /// a set does not compile is simply not in that run.
    #[serde(default = "default_test_features")]
    pub test_features: Vec<Vec<String>>,
    /// The directory holding reviewed diff analyses of dependency bumps; by
    /// default `reviews` in `requirements`.
    pub reviews: Option<String>,
    /// Who may review a diff analysis. Anyone may write one; with no
    /// maintainers, no review holds.
    #[serde(default)]
    pub maintainers: Vec<String>,
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

    /// The entry of `every_file_traced_except` that leaves a source file out,
    /// matched against the file's path with `.` and `..` resolved, so a file a
    /// `#[path]` attribute reaches is matched where it sits.
    pub fn left_out_by(&self, path: &str) -> Option<&str> {
        self.every_file_traced_except
            .iter()
            .map(String::as_str)
            .find(|prefix| leaves_out(prefix, path))
    }

    /// What is wrong with `every_file_traced_except` on its own, before any
    /// file is looked at.
    pub fn except_problems(&self) -> Vec<String> {
        let key = "every_file_traced_except";
        let mut problems = Vec::new();
        if !self.every_file_traced && !self.every_file_traced_except.is_empty() {
            problems.push(format!(
                "{key} is set, but every_file_traced is not, so it leaves out nothing"
            ));
        }
        for (index, prefix) in self.every_file_traced_except.iter().enumerate() {
            if let Some(reason) = prefix_problem(prefix) {
                problems.push(format!(
                    "{key}: {prefix:?} {reason}; give a path prefix relative to the repository"
                ));
            } else if self.every_file_traced_except[..index].contains(prefix) {
                problems.push(format!("{key}: {prefix:?} is listed twice"));
            }
        }
        problems
    }

    pub fn reviews(&self) -> String {
        self.reviews
            .clone()
            .unwrap_or_else(|| format!("{}/reviews", self.requirements))
    }
}

/// Why a prefix is not a plain repository-relative path, if it is not.
fn prefix_problem(prefix: &str) -> Option<&'static str> {
    if prefix.is_empty() {
        return Some("is empty");
    }
    if prefix.starts_with('/') || Path::new(prefix).is_absolute() {
        return Some("is absolute");
    }
    if prefix.contains('\\') {
        return Some("separates components with a backslash");
    }
    if prefix.contains(['*', '?', '[', ']', '{', '}']) {
        return Some("is a pattern, and only path prefixes are matched");
    }
    let components: Vec<&str> = prefix
        .strip_suffix('/')
        .unwrap_or(prefix)
        .split('/')
        .collect();
    if components.iter().any(|component| component.is_empty()) {
        return Some("has an empty component");
    }
    if components.contains(&"..") {
        return Some("climbs out with `..`");
    }
    if components.contains(&".") {
        return Some("has a `.` component");
    }
    None
}

/// Whether one prefix covers a source file's path, `.` and `..` resolved.
pub fn leaves_out(prefix: &str, path: &str) -> bool {
    lexical(path).starts_with(prefix)
}

/// A relative path with `.` and `..` resolved by its text alone.
fn lexical(path: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
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
        assert!(config.every_file_traced_except.is_empty());
        assert!(config.except_problems().is_empty());
        assert_eq!(config.boards, None);
        assert_eq!(config.reviews(), "req/reviews");
        assert!(config.maintainers.is_empty());
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

    fn traced_except(prefixes: &[&str]) -> Config {
        let mut config: Config = toml::from_str(
            r#"name = "x"
               requirements = "req"
               roots = "goals.md"
               root_prefix = "G"
               evidence = "runs"
               every_file_traced = true"#,
        )
        .unwrap();
        config.every_file_traced_except = prefixes.iter().map(|p| (*p).to_owned()).collect();
        config
    }

    #[test]
    fn a_prefix_leaves_out_whole_components() {
        let config = traced_except(&["benches", "tools/gen.rs"]);
        assert!(config.except_problems().is_empty());
        assert_eq!(config.left_out_by("benches/a/src/main.rs"), Some("benches"));
        assert_eq!(config.left_out_by("tools/gen.rs"), Some("tools/gen.rs"));
        assert_eq!(config.left_out_by("benchesx/src/main.rs"), None);
        assert_eq!(config.left_out_by("src/benches/mod.rs"), None);
        assert_eq!(config.left_out_by("src/lib.rs"), None);
    }

    #[test]
    fn a_prefix_matches_where_a_file_sits_once_dots_are_resolved() {
        let config = traced_except(&["benches/common/"]);
        assert!(config.except_problems().is_empty());
        assert_eq!(
            config.left_out_by("benches/a/src/../../common/shared.rs"),
            Some("benches/common/")
        );
        assert_eq!(config.left_out_by("benches/a/src/main.rs"), None);
        assert_eq!(config.left_out_by("benches/common/../../src/lib.rs"), None);
    }

    #[test]
    fn a_prefix_must_be_a_plain_relative_path() {
        for (prefix, reason) in [
            ("", "is empty"),
            ("/benches", "is absolute"),
            ("benches\\a", "backslash"),
            ("benches/*", "is a pattern"),
            ("**/benches", "is a pattern"),
            ("benches//a", "empty component"),
            ("../benches", "climbs out"),
            ("benches/../src", "climbs out"),
            ("./benches", "`.` component"),
        ] {
            let problems = traced_except(&[prefix]).except_problems();
            assert!(
                problems.len() == 1 && problems[0].contains(reason),
                "{prefix:?}: {problems:?}"
            );
        }
        let problems = traced_except(&["benches", "benches"]).except_problems();
        assert!(
            problems.len() == 1 && problems[0].contains("listed twice"),
            "{problems:?}"
        );
    }

    #[test]
    fn an_exception_needs_every_file_traced() {
        let mut config = traced_except(&["benches"]);
        config.every_file_traced = false;
        let problems = config.except_problems();
        assert!(
            problems.len() == 1 && problems[0].contains("every_file_traced is not"),
            "{problems:?}"
        );
    }
}
