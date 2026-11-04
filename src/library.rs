//! A library's requirements and evidence, reached through the build.
//!
//! A project maps a requirement onto a library's by crate and ID, and inherits
//! that requirement's status. Cargo says which copy of the crate this project
//! builds, so the status is the one the pinned revision's own evidence gives,
//! computed there as that library would compute it. Nothing is stored here.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use serde::Deserialize;

use crate::configuration::Cargo;
use crate::evidence::{self, Current, Recorded};
use crate::project::{Problem, Project, check_id};
use crate::schema::Method;
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
}

/// No libraries: every link is an error. For a project that maps nothing.
pub struct NoLibraries;

impl Libraries for NoLibraries {
    fn status(&self, link: &str) -> Result<Status, String> {
        Err(format!("{link}: no libraries here"))
    }
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
}

impl Resolved {
    pub fn new(repository: &Path) -> Self {
        Self {
            repository: repository.to_path_buf(),
            chain: Vec::new(),
            roots: RefCell::new(None),
            projects: RefCell::new(BTreeMap::new()),
            statuses: RefCell::new(BTreeMap::new()),
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

impl Libraries for Resolved {
    fn status(&self, link: &str) -> Result<Status, String> {
        let (name, id) = parse(link)?;
        self.statuses(name)?
            .get(id)
            .copied()
            .ok_or_else(|| format!("{name} presents no requirement {id}"))
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
