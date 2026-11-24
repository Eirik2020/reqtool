//! A library's requirements and evidence, reached through the build.
//!
//! A project maps a requirement onto a library's by crate and ID, and inherits
//! that requirement's status. Cargo says which copy of the crate this project
//! builds, so the status is the one the pinned revision's own evidence gives,
//! computed there as that library would compute it. Nothing is stored here.
//!
//! The links also carry a requirement's own evidence across a move of the
//! library's git commit: evidence recorded at the old commit stands at the
//! one the build uses while every requirement it links in that library keeps
//! its claim between the two commits and is verified at the new one. The old
//! commit's requirements are read from the library's own git history, where
//! cargo checked it out; a commit not found there carries nothing.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use serde::Deserialize;

use crate::config::FILE;
use crate::configuration::{Cargo, same_form};
use crate::evidence::{self, Current, Recorded};
use crate::identity;
use crate::project::{Problem, Project, check_id};
use crate::schema::{Method, Requirement, RequirementFile, SCHEMA_VERSION};
use crate::status::{self, Status};

/// Splits `crate:ID`.
pub fn parse(link: &str) -> Result<(&str, &str), String> {
    let (name, id) = link
        .split_once(':')
        .filter(|(name, _)| !name.is_empty())
        .ok_or_else(|| format!("{link}: a library link is crate:ID"))?;
    check_id(id).map_err(|message| format!("{link}: {message}"))?;
    Ok((name, id))
}

/// Where mapped requirements get their status: the libraries the build uses
/// ([`Resolved`]), or a fixed set in tests.
pub trait Libraries {
    fn status(&self, link: &str) -> Result<Status, String>;

    /// Whether evidence recorded with the library `package` at version `from`
    /// stands at `to`, the version this build uses, on the requirements `ids`
    /// of that library that a requirement links: each keeps its claim between
    /// the two versions and is verified at `to`. `Err` says why not.
    fn carry(&self, package: &str, from: &str, to: &str, ids: &[&str]) -> Result<(), String> {
        let _ = (from, to, ids);
        Err(format!("{package}: no libraries here"))
    }
}

/// No libraries: every link is an error. For a project that maps nothing.
pub struct NoLibraries;

impl Libraries for NoLibraries {
    fn status(&self, link: &str) -> Result<Status, String> {
        Err(format!("{link}: no libraries here"))
    }
}

/// What a requirement in `requirements` promises ([`identity::promise`]), or
/// `None` when there is no such requirement.
pub fn promise(requirements: &BTreeMap<String, Requirement>, id: &str) -> Option<String> {
    let requirement = requirements.get(id)?;
    // Everything derived from it, however deep, in ID order, as a claim has.
    let mut sets: BTreeMap<&str, &Requirement> = BTreeMap::new();
    let mut waiting = vec![id];
    while let Some(parent) = waiting.pop() {
        for child in requirements
            .values()
            .filter(|child| child.derived_from.iter().any(|from| from == parent))
        {
            if sets.insert(child.id.as_str(), child).is_none() {
                waiting.push(child.id.as_str());
            }
        }
    }
    let sets: Vec<&Requirement> = sets.into_values().collect();
    Some(identity::promise(requirement, &sets))
}

/// The commit a git dependency's version names: `<version>+<commit>`.
fn commit_of(version: &str) -> Option<String> {
    let version = same_form(version);
    let (_, commit) = version.split_once('+')?;
    (commit.len() >= 7 && commit.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| commit.to_owned())
}

/// `git -C directory args`: its output, or why it failed.
fn git(directory: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// A repository-relative path in the one form git lists: no `.` or empty
/// components.
fn normal(path: &str) -> String {
    path.split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// A library's requirements as they stood at `commit`, read from the git
/// history of the checkout at `repository`: its `reqtool.toml` and every
/// requirement file under the directory it names, as [`Project::load`] reads
/// them. Anything unreadable is an error, so it carries nothing.
pub fn requirements_at(
    repository: &Path,
    package: &str,
    commit: &str,
) -> Result<BTreeMap<String, Requirement>, String> {
    let top = PathBuf::from(git(repository, &["rev-parse", "--show-toplevel"])?.trim());
    let prefix = git(repository, &["rev-parse", "--show-prefix"])?
        .trim()
        .to_owned();
    let found = git(
        repository,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{commit}^{{commit}}"),
        ],
    )
    .map_err(|_| format!("{commit} is not in the library's git history here"))?;
    let sha = found.trim();
    let show = |path: &str| git(&top, &["show", &format!("{sha}:{path}")]);
    let settings: toml::Table = toml::from_str(&show(&normal(&format!("{prefix}/{FILE}")))?)
        .map_err(|error| format!("{FILE} at {commit}: {error}"))?;
    let setting = |key: &str| settings.get(key).and_then(toml::Value::as_str);
    if setting("name") != Some(package) {
        return Err(format!("the {FILE} at {commit} is not {package}'s"));
    }
    let directory = setting("requirements")
        .ok_or_else(|| format!("the {FILE} at {commit} names no requirements"))?;
    let gates =
        setting("gates").map_or_else(|| format!("{directory}/gates.yaml"), ToString::to_string);
    let gates = normal(&format!("{prefix}/{gates}"));
    let under = normal(&format!("{prefix}/{directory}"));
    let listing = git(
        &top,
        &[
            "ls-tree",
            "-r",
            "-z",
            "--name-only",
            sha,
            "--",
            if under.is_empty() { "." } else { &under },
        ],
    )?;
    let mut files: Vec<&str> = listing
        .split('\0')
        .filter(|path| {
            (path.ends_with(".yaml") || path.ends_with(".yml"))
                && *path != gates
                && !path.split('/').any(|part| part == "target")
        })
        .collect();
    files.sort_unstable();
    let mut requirements = BTreeMap::new();
    for file in files {
        let parsed: RequirementFile = serde_saphyr::from_str(&show(file)?)
            .map_err(|error| format!("{file} at {commit}: {error}"))?;
        if parsed.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "{file} at {commit}: schema_version {} is not {SCHEMA_VERSION}",
                parsed.schema_version
            ));
        }
        for requirement in parsed.requirements {
            let id = requirement.id.clone();
            if requirements.insert(id.clone(), requirement).is_some() {
                return Err(format!("{id} is defined twice at {commit}"));
            }
        }
    }
    Ok(requirements)
}

/// Each package's manifest directories, by name.
type Directories = BTreeMap<String, Vec<PathBuf>>;

/// The libraries a project's build resolves.
pub struct Resolved {
    repository: PathBuf,
    /// The crates being resolved, outermost first, so a loop is refused.
    chain: Vec<String>,
    roots: RefCell<Option<Rc<Directories>>>,
    projects: RefCell<BTreeMap<String, Rc<Project>>>,
    statuses: RefCell<BTreeMap<String, Rc<BTreeMap<String, Status>>>>,
    /// A library's requirements at an earlier commit, by crate and commit.
    earlier: RefCell<BTreeMap<(String, String), Earlier>>,
}

type Earlier = Result<Rc<BTreeMap<String, Requirement>>, String>;

impl Resolved {
    pub fn new(repository: &Path) -> Self {
        Self {
            repository: repository.to_path_buf(),
            chain: Vec::new(),
            roots: RefCell::new(None),
            projects: RefCell::new(BTreeMap::new()),
            statuses: RefCell::new(BTreeMap::new()),
            earlier: RefCell::new(BTreeMap::new()),
        }
    }

    /// Every link in `project` that does not name a requirement its library
    /// presents, at the level the entry claims.
    pub fn problems(&self, project: &Project) -> Vec<Problem> {
        let mut problems = Vec::new();
        for requirement in project.requirements.values() {
            for entry in &requirement.verification.entries {
                let Method::Library(link) = &entry.method else {
                    continue;
                };
                let found = parse(link).and_then(|(name, id)| {
                    let library = self.project(name)?;
                    let target = library
                        .requirements
                        .get(id)
                        .ok_or_else(|| format!("{name} presents no requirement {id}"))?;
                    if target
                        .verification
                        .entries
                        .iter()
                        .any(|theirs| theirs.level == entry.level)
                    {
                        Ok(())
                    } else {
                        Err(format!(
                            "{link} has no entry at level {}",
                            entry.level.name()
                        ))
                    }
                });
                if let Err(message) = found {
                    problems.push(Problem {
                        location: format!(
                            "{}: {}",
                            project
                                .sources
                                .get(&requirement.id)
                                .map_or("?", String::as_str),
                            requirement.id
                        ),
                        message,
                    });
                }
            }
        }
        problems
    }

    /// Each package's manifest directories, as cargo resolves them here.
    fn roots(&self) -> Result<Rc<Directories>, String> {
        if let Some(found) = self.roots.borrow().as_ref() {
            return Ok(Rc::clone(found));
        }
        // A library behind a feature is still this project's: without this,
        // cargo leaves out whatever only a feature pulls in.
        let output = Command::new("cargo")
            .args([
                "metadata",
                "--format-version",
                "1",
                "--locked",
                "--all-features",
            ])
            .env_remove("RUSTUP_TOOLCHAIN")
            .env_remove("RUSTC")
            .env_remove("CARGO")
            .current_dir(&self.repository)
            .output()
            .map_err(|error| format!("cannot run cargo: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "cargo metadata failed, so the libraries are unknown:\n{}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let metadata: Metadata =
            serde_json::from_slice(&output.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
        let found = Rc::new(by_name(metadata));
        *self.roots.borrow_mut() = Some(Rc::clone(&found));
        Ok(found)
    }

    fn project(&self, name: &str) -> Result<Rc<Project>, String> {
        if let Some(found) = self.projects.borrow().get(name) {
            return Ok(Rc::clone(found));
        }
        if self.chain.iter().any(|seen| seen == name) {
            return Err(format!("{name}: libraries map onto each other in a loop"));
        }
        let roots = self.roots()?;
        let directory = match roots.get(name).map(Vec::as_slice) {
            None | Some([]) => return Err(format!("{name}: not a dependency of this project")),
            Some([one]) => one,
            Some(_) => return Err(format!("{name}: the build resolves several copies")),
        };
        let root = crate::find_repository(directory)
            .ok_or_else(|| format!("{name}: presents no requirements, having no reqtool.toml"))?;
        let (project, problems) = Project::load(&root).map_err(|e| format!("{name}: {e}"))?;
        if project.config.name != name {
            return Err(format!(
                "{name}: the reqtool.toml above it is {}'s",
                project.config.name
            ));
        }
        if let Some(problem) = problems.first() {
            return Err(format!(
                "{name}: its requirements have {} problem(s), first: {problem}",
                problems.len()
            ));
        }
        let project = Rc::new(project);
        self.projects
            .borrow_mut()
            .insert(name.to_owned(), Rc::clone(&project));
        Ok(project)
    }

    fn statuses(&self, name: &str) -> Result<Rc<BTreeMap<String, Status>>, String> {
        if let Some(found) = self.statuses.borrow().get(name) {
            return Ok(Rc::clone(found));
        }
        let project = self.project(name)?;
        // A library's copy belongs to cargo, so what is built to judge it goes
        // in this project's own target directory.
        let target = self.repository.join("target/reqtool-libraries").join(name);
        let cargo = Cargo::new(&project.repository, &project.workspace).with_target_dir(target);
        let mut chain = self.chain.clone();
        chain.push(name.to_owned());
        let libraries = Self {
            chain,
            ..Self::new(&project.repository)
        };
        let current = Current {
            project: &project,
            trace: &project.trace,
            configurations: &cargo,
            libraries: &libraries,
            reviews: &project.reviews,
        };
        let records = evidence::load(&project.repository).map_err(|e| format!("{name}: {e}"))?;
        let recorded =
            Recorded::new(&current, &records, None).map_err(|e| format!("{name}: {e}"))?;
        let statuses = Rc::new(status::compute(&project.requirements, &recorded));
        self.statuses
            .borrow_mut()
            .insert(name.to_owned(), Rc::clone(&statuses));
        Ok(statuses)
    }
}

impl Resolved {
    fn earlier(&self, package: &str, repository: &Path, commit: &str) -> Earlier {
        let key = (package.to_owned(), commit.to_owned());
        if let Some(found) = self.earlier.borrow().get(&key) {
            return found.clone();
        }
        let found = requirements_at(repository, package, commit).map(Rc::new);
        self.earlier.borrow_mut().insert(key, found.clone());
        found
    }
}

impl Libraries for Resolved {
    fn status(&self, link: &str) -> Result<Status, String> {
        let (name, id) = parse(link)?;
        self.statuses(name)?
            .get(id)
            .copied()
            .ok_or_else(|| format!("{name} presents no requirement {id}"))
    }

    fn carry(&self, package: &str, from: &str, to: &str, ids: &[&str]) -> Result<(), String> {
        let (Some(old), Some(new)) = (commit_of(from), commit_of(to)) else {
            return Err("only a move between git commits is carried by links".into());
        };
        let library = self.project(package)?;
        let built = git(&library.repository, &["rev-parse", "HEAD"])
            .map_err(|why| format!("its copy here is not a git checkout: {why}"))?;
        if !built.trim().starts_with(&new) {
            return Err(format!(
                "the build here uses {package} at {}, not {to}",
                built.trim()
            ));
        }
        let before = self.earlier(package, &library.repository, &old)?;
        let mut ids = ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            let now = promise(&library.requirements, id)
                .ok_or_else(|| format!("{package} presents no requirement {id} at {to}"))?;
            match promise(&before, id) {
                None => return Err(format!("{id} is not presented at {from}")),
                Some(then) if then != now => {
                    return Err(format!("{id} changed its claim since {from}"));
                }
                Some(_) => {}
            }
            let status = self.status(&format!("{package}:{id}"))?;
            if status != Status::Verified {
                return Err(format!("{id} is {} at {to}", status.name()));
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    manifest_path: PathBuf,
}

fn by_name(metadata: Metadata) -> Directories {
    let mut found: BTreeMap<String, BTreeSet<PathBuf>> = BTreeMap::new();
    for package in metadata.packages {
        if let Some(directory) = package.manifest_path.parent() {
            found
                .entry(package.name)
                .or_default()
                .insert(directory.to_path_buf());
        }
    }
    found
        .into_iter()
        .map(|(name, directories)| (name, directories.into_iter().collect()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_is_a_crate_and_an_id() {
        assert_eq!(parse("my-hal:UART-012"), Ok(("my-hal", "UART-012")));
        assert!(parse("UART-012").is_err());
        assert!(parse(":UART-012").is_err());
        assert!(parse("my-hal:uart").is_err());
    }

    #[test]
    fn only_a_git_version_names_a_commit() {
        assert_eq!(commit_of("0.1.0+0a1b2c3d").as_deref(), Some("0a1b2c3d"));
        assert_eq!(commit_of("0.1.0+rev=0a1b2c3d").as_deref(), Some("0a1b2c3d"));
        assert_eq!(commit_of("0.1.0"), None, "a registry version");
        assert_eq!(commit_of("0.1.0+main"), None, "a branch");
        assert_eq!(commit_of("0.1.0+0a1b"), None, "too short to name one");
    }

    #[test]
    fn a_promise_takes_in_everything_derived_from_it() {
        let catalog = |child: &str| -> BTreeMap<String, Requirement> {
            let text = format!(
                "schema_version: 2
requirements:
  - id: A-001
    title: t
    statement: The gadget shall answer.
    rationale: r
    derived_from: [G1]
    verification: {{}}
  - id: A-002
    title: t
    statement: {child}
    rationale: r
    derived_from: [A-001]
    verification: {{}}
  - id: A-003
    title: t
    statement: The grandchild shall hold.
    rationale: r
    derived_from: [A-002]
    verification: {{}}
"
            );
            serde_saphyr::from_str::<RequirementFile>(&text)
                .unwrap()
                .requirements
                .into_iter()
                .map(|requirement| (requirement.id.clone(), requirement))
                .collect()
        };
        let before = catalog("The part shall add.");
        let after = catalog("The part shall subtract.");
        assert_ne!(promise(&before, "A-001"), promise(&after, "A-001"));
        assert_eq!(promise(&before, "A-003"), promise(&after, "A-003"));
        assert_eq!(promise(&before, "A-009"), None);
    }

    #[test]
    fn two_copies_of_a_crate_stay_visible() {
        let metadata = Metadata {
            packages: vec![
                Package {
                    name: "a".into(),
                    manifest_path: "/one/Cargo.toml".into(),
                },
                Package {
                    name: "a".into(),
                    manifest_path: "/two/Cargo.toml".into(),
                },
            ],
        };
        assert_eq!(by_name(metadata)["a"].len(), 2);
    }
}
