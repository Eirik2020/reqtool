//! Configurations: the parts one board's image is built
//! from, and how they are put together.
//!
//! A configuration is a board's firmware app with a feature set. Its identity
//! says which car this is, not what state each part is in:
//! - which source files are compiled in, by path, from the dep-info cargo writes
//!   for each crate, and every file a build script reruns on, except under
//!   `.git`;
//! - the content of everything that assembles the parts: the app's own source,
//!   which holds its scheduling table (priorities and bindings), and every
//!   file that is not Rust source, such as its manifest, `.cargo/config.toml`
//!   and linker map;
//! - every environment variable the compiler read, such as `DEFMT_LOG`,
//!   except those a build script sets, such as the git revision;
//! - the version of every external crate compiled in.
//!
//! The identity only tells one board's records apart (a repeat at the same
//! identity adds nothing); status never compares it. A part's content is not
//! in it: evidence names the requirements a run covered, and stands while the
//! files declaring them are unchanged, whatever else the image holds. The
//! compiler is deliberately left out: differences between compiler versions are
//! negligible outside safety assurance, which this project does not claim.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::identity;
use crate::trace::relative;
use crate::workspace::Workspace;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub board: String,
    pub features: Vec<String>,
    /// Repository-relative path to identity.
    pub files: BTreeMap<String, String>,
    /// Package to version.
    pub externals: BTreeMap<String, String>,
    /// Environment variables the compiler read, to their values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environment: BTreeMap<String, String>,
}

impl Configuration {
    pub fn identity(&self) -> String {
        let mut text = String::new();
        let _ = writeln!(text, "board {}", self.board);
        for feature in &self.features {
            let _ = writeln!(text, "feature {feature}");
        }
        let app_source = format!("/{}/src/", self.board);
        for (path, identity) in &self.files {
            if path.ends_with(".rs") && !path.contains(&app_source) {
                let _ = writeln!(text, "part {path}");
            } else {
                let _ = writeln!(text, "file {path} {identity}");
            }
        }
        for (package, version) in &self.externals {
            let _ = writeln!(text, "extern {package} {}", same_form(version));
        }
        for (name, value) in &self.environment {
            let _ = writeln!(text, "env {name}={value}");
        }
        identity::sha256(text.as_bytes())
    }

    /// The files whose identity differs from `other`'s, or that only one of
    /// the two compiles in.
    pub fn differences(&self, other: &Self) -> Vec<String> {
        let paths: BTreeSet<&String> = self.files.keys().chain(other.files.keys()).collect();
        paths
            .into_iter()
            .filter(|path| self.files.get(*path) != other.files.get(*path))
            .cloned()
            .collect()
    }
}

/// Where configurations come from: cargo ([`Cargo`]), or a fixed set in
/// tests.
pub trait Configurations {
    fn configuration(&self, board: &str, features: &[String]) -> Result<Rc<Configuration>, String>;

    /// The repository's source files a host test build compiles: those a host
    /// test can reach. The rest, such as task definitions, are target-only.
    fn host_files(&self) -> Result<Rc<BTreeSet<String>>, String>;
}

/// A board and its sorted features.
type Key = (String, Vec<String>);

/// A version in the one form both sides of evidence use. A git dependency
/// pinned to an exact `rev` is `<version>+<commit>`; records written before the
/// two sides agreed hold `<version>+rev=<commit>`, which compares as the same
/// version, so evidence on disk keeps its meaning. A registry version cannot
/// hold `=`, so nothing else is touched.
pub fn same_form(version: &str) -> String {
    version.replace("+rev=", "+")
}

/// Configurations from `cargo check`, one run per board and feature set.
pub struct Cargo<'a> {
    repository: &'a Path,
    workspace: &'a Workspace,
    cache: RefCell<HashMap<Key, Rc<Configuration>>>,
    host_files: RefCell<Option<Rc<BTreeSet<String>>>>,
    target_dir: Option<PathBuf>,
}

impl<'a> Cargo<'a> {
    pub fn new(repository: &'a Path, workspace: &'a Workspace) -> Self {
        Self {
            repository,
            workspace,
            cache: RefCell::new(HashMap::new()),
            host_files: RefCell::new(None),
            target_dir: None,
        }
    }

    /// Builds under `directory`, for a repository that isn't this project's own.
    #[must_use]
    pub fn with_target_dir(mut self, directory: PathBuf) -> Self {
        self.target_dir = Some(directory);
        self
    }

    fn cargo(&self) -> Command {
        let mut command = Command::new("cargo");
        // The project's own `rust-toolchain.toml` decides, not whichever
        // toolchain is running this tool.
        command
            .env_remove("RUSTUP_TOOLCHAIN")
            .env_remove("RUSTC")
            .env_remove("CARGO");
        if let Some(directory) = &self.target_dir {
            command.env("CARGO_TARGET_DIR", directory);
        }
        command
    }
}

impl Configurations for Cargo<'_> {
    fn host_files(&self) -> Result<Rc<BTreeSet<String>>, String> {
        if let Some(found) = self.host_files.borrow().as_ref() {
            return Ok(Rc::clone(found));
        }
        let config = Config::load(self.repository)?;
        let host = crate::runner::host_triple(self.repository)?;
        let mut files = BTreeSet::new();
        for features in &config.test_features {
            let output = self
                .cargo()
                .args([
                    "check",
                    "--workspace",
                    "--tests",
                    "--locked",
                    "--quiet",
                    "--message-format=json",
                    "--target",
                    &host,
                    "--features",
                    &features.join(","),
                ])
                .current_dir(self.repository)
                .output()
                .map_err(|error| format!("host test build: cannot run cargo: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "host test build: cargo check failed, so what host tests can reach is unknown:\n{}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            let compiled =
                from_messages(&String::from_utf8_lossy(&output.stdout), self.repository)?;
            files.extend(
                compiled
                    .files
                    .iter()
                    .filter(|source| source.starts_with(self.repository))
                    .map(|source| relative(self.repository, source)),
            );
        }
        let files = Rc::new(files);
        *self.host_files.borrow_mut() = Some(Rc::clone(&files));
        Ok(files)
    }

    fn configuration(&self, board: &str, features: &[String]) -> Result<Rc<Configuration>, String> {
        let mut features = features.to_vec();
        features.sort();
        features.dedup();
        let key = (board.to_owned(), features.clone());
        if let Some(found) = self.cache.borrow().get(&key) {
            return Ok(Rc::clone(found));
        }
        let app = self
            .workspace
            .boards
            .get(board)
            .ok_or_else(|| format!("no firmware app for board {board}"))?;
        let mut command = self.cargo();
        command
            .args(["check", "--locked", "--quiet", "--message-format=json"])
            .current_dir(&app.directory);
        if !features.is_empty() {
            command.arg("--features").arg(features.join(","));
        }
        let output = command
            .output()
            .map_err(|error| format!("{board}: cannot run cargo: {error}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: Vec<&str> = stderr.lines().rev().take(15).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            return Err(format!(
                "{board}: cargo check failed, so its configuration is unknown:\n{}",
                tail.join("\n")
            ));
        }
        let compiled = from_messages(&String::from_utf8_lossy(&output.stdout), &app.directory)?;
        let mut sources: BTreeSet<PathBuf> = compiled.files;
        sources.insert(app.directory.join("Cargo.toml"));
        let config = app.directory.join(".cargo/config.toml");
        if config.is_file() {
            sources.insert(config);
        }
        let mut files = BTreeMap::new();
        for source in sources {
            if !source.starts_with(self.repository)
                || source
                    .components()
                    .any(|part| part.as_os_str() == "target" || part.as_os_str() == ".git")
            {
                continue;
            }
            files.insert(relative(self.repository, &source), identity::file(&source)?);
        }
        let configuration = Rc::new(Configuration {
            board: board.to_owned(),
            features,
            files,
            externals: compiled.externals,
            environment: compiled.environment,
        });
        self.cache
            .borrow_mut()
            .insert(key, Rc::clone(&configuration));
        Ok(configuration)
    }
}

/// What cargo's JSON messages say was compiled.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Compiled {
    /// Every file in a crate's dep-info, and every file a build script
    /// reruns on.
    pub files: BTreeSet<PathBuf>,
    /// Registry and git packages, to their versions.
    pub externals: BTreeMap<String, String>,
    /// Environment variables the compiler read and no build script set.
    pub environment: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Message {
    reason: String,
    #[serde(default)]
    package_id: String,
    #[serde(default)]
    filenames: Vec<PathBuf>,
    #[serde(default)]
    out_dir: Option<PathBuf>,
    /// For a build script: the variables it set for the compiler.
    #[serde(default)]
    env: Vec<(String, String)>,
}

/// Notes one compiled version of a package. A tree can hold several versions of
/// a package, and cargo reports them in no fixed order, so they are kept sorted:
/// either version changing has to show.
fn add_version(externals: &mut BTreeMap<String, String>, name: String, version: &str) {
    let versions = externals.entry(name).or_default();
    let mut all: BTreeSet<&str> = versions.split(", ").filter(|v| !v.is_empty()).collect();
    all.insert(version);
    *versions = all.into_iter().collect::<Vec<_>>().join(", ");
}

/// Reads cargo's messages; `base` is the workspace root, which relative
/// dep-info paths start from.
pub fn from_messages(stdout: &str, base: &Path) -> Result<Compiled, String> {
    let mut compiled = Compiled::default();
    let mut set_by_scripts = BTreeSet::new();
    for line in stdout.lines().filter(|line| line.starts_with('{')) {
        let message: Message =
            serde_json::from_str(line).map_err(|e| format!("cargo message: {e}"))?;
        match message.reason.as_str() {
            "compiler-artifact" => {
                if let Some((name, version)) = external(&message.package_id) {
                    add_version(&mut compiled.externals, name, &version);
                    continue;
                }
                for artifact in &message.filenames {
                    let Some(dep_info) = dep_info_of(artifact) else {
                        continue;
                    };
                    let text = std::fs::read_to_string(&dep_info)
                        .map_err(|e| format!("{}: {e}", dep_info.display()))?;
                    compiled.files.extend(
                        dep_info_files(&text)
                            .into_iter()
                            .map(|file| base.join(file)),
                    );
                    compiled.environment.extend(dep_info_environment(&text));
                }
            }
            "build-script-executed" => {
                set_by_scripts.extend(message.env.iter().map(|(name, _)| name.clone()));
                let (Some(out_dir), Some(package)) =
                    (&message.out_dir, local_directory(&message.package_id))
                else {
                    continue;
                };
                let output = out_dir.with_file_name("output");
                if let Ok(text) = std::fs::read_to_string(&output) {
                    compiled.files.extend(rerun_files(&text, &package));
                }
            }
            _ => {}
        }
    }
    compiled
        .environment
        .retain(|name, _| !set_by_scripts.contains(name));
    Ok(compiled)
}

/// `# env-dep:NAME=value` lines, as rustc writes them into dep-info.
pub fn dep_info_environment(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.strip_prefix("# env-dep:"))
        .map(|dependency| match dependency.split_once('=') {
            Some((name, value)) => (name.to_owned(), value.to_owned()),
            None => (dependency.to_owned(), String::new()),
        })
        .collect()
}

/// A path package's directory, from its ID:
/// `path+file:///dir#name@version`.
fn local_directory(package_id: &str) -> Option<PathBuf> {
    let rest = package_id.strip_prefix("path+file://")?;
    let directory = rest
        .split_once('#')
        .map_or(rest, |(directory, _)| directory);
    Some(PathBuf::from(directory))
}

/// `name` and `version` of a registry or git package ID; `None` for a path
/// package.
pub(crate) fn external(package_id: &str) -> Option<(String, String)> {
    if package_id.starts_with("path+") {
        return None;
    }
    let (url, fragment) = package_id.rsplit_once('#')?;
    let (source, reference) = url.split_once('?').unwrap_or((url, ""));
    // Cargo leaves the name out where the URL's last segment already says it.
    let (name, version) = fragment
        .rsplit_once('@')
        .or_else(|| Some((source.rsplit('/').next()?, fragment)))?;
    // A version doesn't move when a git dependency's revision does. An exact
    // `rev` is the commit, as the lock file records it; any other reference
    // keeps its text.
    let version = match (
        package_id.starts_with("git+"),
        reference.strip_prefix("rev="),
    ) {
        (true, Some(commit)) => format!("{version}+{commit}"),
        (true, None) => format!("{version}+{reference}"),
        (false, _) => version.to_owned(),
    };
    Some((name.to_owned(), version))
}

/// The `.d` file beside a `lib<name>-<hash>.rmeta` (or `.rlib`, or a binary).
fn dep_info_of(artifact: &Path) -> Option<PathBuf> {
    let stem = artifact.file_stem()?.to_string_lossy().into_owned();
    let stem = stem.strip_prefix("lib").unwrap_or(&stem).to_owned();
    let candidate = artifact.with_file_name(format!("{stem}.d"));
    candidate.is_file().then_some(candidate)
}

/// The dependencies on the first line of a Makefile-style dep-info file:
/// `target: a b\ c`, with spaces in names escaped.
pub fn dep_info_files(text: &str) -> BTreeSet<PathBuf> {
    let Some(first) = text.lines().next() else {
        return BTreeSet::new();
    };
    let Some((_, list)) = first.split_once(": ") else {
        return BTreeSet::new();
    };
    let mut files = BTreeSet::new();
    let mut current = String::new();
    let mut chars = list.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&' ') => {
                current.push(' ');
                chars.next();
            }
            ' ' => {
                if !current.is_empty() {
                    files.insert(PathBuf::from(std::mem::take(&mut current)));
                }
            }
            other => current.push(other),
        }
    }
    if !current.is_empty() {
        files.insert(PathBuf::from(current));
    }
    files
}

/// `cargo:rerun-if-changed=` (or `cargo::`) paths in a build script's output,
/// relative to its package.
pub fn rerun_files(text: &str, package: &Path) -> BTreeSet<PathBuf> {
    text.lines()
        .filter_map(|line| {
            line.strip_prefix("cargo::rerun-if-changed=")
                .or_else(|| line.strip_prefix("cargo:rerun-if-changed="))
        })
        .map(|path| package.join(path.trim()))
        .filter(|path| path.is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_external_crate_is_named_from_any_package_id() {
        assert_eq!(
            external("registry+https://github.com/rust-lang/crates.io-index#defmt@1.2.0"),
            Some(("defmt".into(), "1.2.0".into()))
        );
        assert_eq!(external("path+file:///r/crates/a#0.1.0"), None);
        assert_eq!(external("path+file:///r/crates/a#b@0.1.0"), None);
    }

    #[test]
    fn a_git_crate_is_identified_by_its_revision() {
        let at = |revision: &str| {
            external(&format!(
                "git+https://github.com/o/my-hal?rev={revision}#0.1.0"
            ))
        };
        assert_eq!(at("abc"), Some(("my-hal".into(), "0.1.0+abc".into())));
        assert_ne!(at("abc"), at("def"));
        assert_eq!(
            external("git+https://github.com/o/repo?rev=abc#my-hal@0.1.0"),
            Some(("my-hal".into(), "0.1.0+abc".into()))
        );
        assert_eq!(
            external("git+https://github.com/o/my-hal?branch=main#0.1.0"),
            Some(("my-hal".into(), "0.1.0+branch=main".into())),
            "a reference that is not an exact revision keeps its text"
        );
    }

    #[test]
    fn a_recorded_revision_in_the_older_form_is_the_same_version() {
        let configuration = |version: &str| Configuration {
            board: "b".into(),
            features: vec![],
            files: BTreeMap::new(),
            externals: [("my-hal".to_owned(), version.to_owned())].into(),
            environment: BTreeMap::new(),
        };
        assert_eq!(
            configuration("0.1.0+rev=abc").identity(),
            configuration("0.1.0+abc").identity()
        );
        assert_ne!(
            configuration("0.1.0+rev=abc").identity(),
            configuration("0.1.0+def").identity()
        );
        assert_ne!(
            configuration("0.1.0+branch=main").identity(),
            configuration("0.1.0+main").identity()
        );
        assert_eq!(same_form("0.6.1, 0.7.0+rev=abc"), "0.6.1, 0.7.0+abc");
    }

    #[test]
    fn dep_info_lists_the_first_rules_dependencies() {
        let text =
            "/t/deps/a-1.d: /r/src/lib.rs /r/src/my\\ file.rs /r/memory.x\n\n/r/src/lib.rs:\n";
        let files: Vec<String> = dep_info_files(text)
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(files, ["/r/memory.x", "/r/src/lib.rs", "/r/src/my file.rs"]);
    }

    #[test]
    fn environment_the_compiler_read_is_kept_unless_a_build_script_set_it() {
        let text = "a.d: src/main.rs\n\n# env-dep:DEFMT_LOG=info\n# env-dep:FWSP_GIT_REV=abc\n";
        let read = dep_info_environment(text);
        assert_eq!(read.len(), 2);
        assert_eq!(read["DEFMT_LOG"], "info");
    }

    #[test]
    fn every_version_of_a_package_counts_in_any_order() {
        let mut one = BTreeMap::new();
        add_version(&mut one, "embedded-io".into(), "0.6.1");
        add_version(&mut one, "embedded-io".into(), "0.7.1");
        let mut other = BTreeMap::new();
        add_version(&mut other, "embedded-io".into(), "0.7.1");
        add_version(&mut other, "embedded-io".into(), "0.6.1");
        add_version(&mut other, "embedded-io".into(), "0.6.1");
        assert_eq!(one, other);
        assert_eq!(one["embedded-io"], "0.6.1, 0.7.1");
    }

    #[test]
    fn package_ids_tell_external_from_local() {
        assert_eq!(
            external("registry+https://github.com/rust-lang/crates.io-index#heapless@0.8.0"),
            Some(("heapless".into(), "0.8.0".into()))
        );
        assert_eq!(external("path+file:///r/crates/a#0.1.0"), None);
        assert_eq!(
            local_directory("path+file:///r/crates/a#0.1.0"),
            Some(PathBuf::from("/r/crates/a"))
        );
    }

    #[test]
    fn a_configuration_names_its_parts_and_holds_what_assembles_them() {
        let base = Configuration {
            board: "b".into(),
            features: vec![],
            files: [
                ("crates/a.rs".to_owned(), "1".to_owned()),
                ("firmware/b/src/main.rs".to_owned(), "1".to_owned()),
                ("firmware/b/memory.x".to_owned(), "1".to_owned()),
            ]
            .into(),
            externals: BTreeMap::new(),
            environment: BTreeMap::new(),
        };
        let with_file = |path: &str| {
            let mut other = base.clone();
            other.files.insert(path.into(), "2".into());
            other
        };
        let new_part_content = with_file("crates/a.rs");
        assert_eq!(base.identity(), new_part_content.identity(), "the same car");
        assert_eq!(base.differences(&new_part_content), ["crates/a.rs"]);
        assert_ne!(
            base.identity(),
            with_file("crates/added.rs").identity(),
            "another part"
        );
        assert_ne!(
            base.identity(),
            with_file("firmware/b/src/main.rs").identity()
        );
        assert_ne!(base.identity(), with_file("firmware/b/memory.x").identity());
        let mut featured = base.clone();
        featured.features.push("x".into());
        assert_ne!(base.identity(), featured.identity());
        let mut bumped = base.clone();
        bumped.externals.insert("heapless".into(), "0.9.0".into());
        assert_ne!(base.identity(), bumped.identity());
    }
}
