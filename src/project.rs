//! Loading the requirement files and checking them against the roots, each
//! other, and the code.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::gates::{self, Gate};
use crate::items::Resolver;
use crate::review::{self, Reviews};
use crate::roots::{self, Root};
use crate::schema::{
    Entry, Environment, Hazard, Level, Method, Requirement, RequirementFile, SCHEMA_VERSION,
};
use crate::trace::{Import, Trace};
use crate::workspace::{self, Workspace};

/// One thing wrong, at a file or a requirement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Problem {
    pub location: String,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.location, self.message)
    }
}

pub struct Project {
    pub repository: PathBuf,
    pub config: Config,
    pub roots: Vec<Root>,
    pub requirements: BTreeMap<String, Requirement>,
    /// The file each requirement came from, relative to the repository.
    pub sources: BTreeMap<String, String>,
    pub workspace: Workspace,
    /// Every source file: identities, declarations and imports.
    pub trace: Trace,
    pub gates: Vec<Gate>,
    /// The reviewed diff analyses that hold.
    pub reviews: Reviews,
    /// The source files `every_file_traced_except` leaves out, each with the
    /// entry it sits under.
    pub left_out: BTreeMap<String, String>,
}

impl Project {
    /// Loads everything; returns the problems found alongside what loaded.
    /// An `Err` means the repository itself could not be read.
    pub fn load(repository: &Path) -> Result<(Self, Vec<Problem>), String> {
        let config = Config::load(repository)?;
        let roots = roots::load(repository, &config)?;
        let workspace = workspace::discover(repository, &config)?;
        let mut problems = Vec::new();
        let mut requirements = BTreeMap::new();
        let mut sources = BTreeMap::new();

        let gates_file = config.gates();
        for file in requirement_files(repository, &config.requirements, &gates_file) {
            let relative = relative(repository, &file);
            let parsed = std::fs::read_to_string(&file)
                .map_err(|error| error.to_string())
                .and_then(|text| {
                    serde_saphyr::from_str::<RequirementFile>(&text)
                        .map_err(|error| error.to_string())
                });
            let parsed = match parsed {
                Ok(parsed) => parsed,
                Err(message) => {
                    problems.push(Problem {
                        location: relative,
                        message,
                    });
                    continue;
                }
            };
            if parsed.schema_version != SCHEMA_VERSION {
                problems.push(Problem {
                    location: relative,
                    message: format!(
                        "schema_version {} is not {SCHEMA_VERSION}",
                        parsed.schema_version
                    ),
                });
                continue;
            }
            for requirement in parsed.requirements {
                let location = format!("{relative}: {}", requirement.id);
                if let Err(message) = check_id(&requirement.id) {
                    problems.push(Problem { location, message });
                    continue;
                }
                if roots.iter().any(|root| root.id == requirement.id) {
                    problems.push(Problem {
                        location,
                        message: "reuses a governing requirement's ID".into(),
                    });
                    continue;
                }
                if let Some(first) = sources.get(&requirement.id) {
                    problems.push(Problem {
                        location,
                        message: format!("duplicate ID, first defined in {first}"),
                    });
                    continue;
                }
                sources.insert(requirement.id.clone(), relative.clone());
                requirements.insert(requirement.id.clone(), requirement);
            }
        }

        let gates = match gates::load(repository, &gates_file) {
            Ok(gates) => gates,
            Err(message) => {
                problems.push(Problem {
                    location: gates_file.clone(),
                    message,
                });
                Vec::new()
            }
        };
        let mut trace = Trace::build(repository, &workspace)?;
        let left_out = left_out_files(repository, &config, &workspace, &trace);
        problems.extend(left_out_problems(&config, &left_out));
        if config.every_file_traced {
            for (path, file) in &mut trace.files {
                if !left_out.contains_key(path) {
                    file.critical = true;
                }
            }
        }
        problems.extend(trace.problems.iter().map(|message| Problem {
            location: "source".to_owned(),
            message: message.clone(),
        }));
        let (reviews, refused) = review::load(repository, &config);
        problems.extend(refused);
        let project = Self {
            repository: repository.to_path_buf(),
            config,
            roots,
            requirements,
            sources,
            workspace,
            trace,
            gates,
            reviews,
            left_out,
        };
        problems.extend(project.validate());
        problems.extend(
            gates::validate(&project.gates, &project.requirements)
                .into_iter()
                .map(|message| Problem {
                    location: gates_file.clone(),
                    message,
                }),
        );
        Ok((project, problems))
    }

    pub fn root(&self, id: &str) -> Option<&Root> {
        self.roots.iter().find(|root| root.id == id)
    }

    /// Requirements that name `parent` in `derived_from`.
    pub fn children(&self, parent: &str) -> Vec<&Requirement> {
        self.requirements
            .values()
            .filter(|r| r.derived_from.iter().any(|p| p == parent))
            .collect()
    }

    /// The per-file rules, at the `analysis` level:
    /// a safety-critical file that defines code declares a requirement
    ///, and imports only safety-critical files and pinned
    /// external crates.
    pub fn trace_findings(&self) -> Vec<Problem> {
        let mut problems = Vec::new();
        for (path, file) in &self.trace.files {
            if !file.critical {
                continue;
            }
            let mut problem = |message: String| {
                problems.push(Problem {
                    location: path.clone(),
                    message,
                })
            };
            if file.defines_code && file.requirements.is_empty() {
                problem("is safety-critical, and declares no requirement it implements".into());
            }
            for (key, import) in &file.imports {
                match import {
                    Import::File => {
                        if self
                            .trace
                            .files
                            .get(key)
                            .is_some_and(|target| !target.critical)
                        {
                            problem(format!(
                                "is safety-critical, and imports {key}, which is not"
                            ));
                        }
                    }
                    Import::External { pinned: false } => problem(format!(
                        "is safety-critical, and imports {}, which its manifest does not pin to one version",
                        key.trim_start_matches("extern:")
                    )),
                    Import::External { pinned: true } => {}
                }
            }
        }
        problems
    }

    /// What `every_file_traced_except` leaves out: each entry, with the files
    /// it leaves out.
    pub fn left_out(&self) -> Vec<(&str, Vec<&str>)> {
        if !self.config.every_file_traced {
            return Vec::new();
        }
        self.config
            .every_file_traced_except
            .iter()
            .map(|entry| {
                let files = self
                    .left_out
                    .iter()
                    .filter(|(_, by)| *by == entry)
                    .map(|(path, _)| path.as_str())
                    .collect();
                (entry.as_str(), files)
            })
            .collect()
    }

    /// The entry that leaves a source file out, if one does.
    pub fn left_out_by(&self, path: &str) -> Option<&str> {
        self.left_out.get(path).map(String::as_str)
    }

    fn location(&self, id: &str) -> String {
        format!("{}: {id}", self.sources.get(id).map_or("?", String::as_str))
    }

    fn validate(&self) -> Vec<Problem> {
        let mut problems = Vec::new();
        let resolver = Resolver::new(&self.workspace.crates);
        for requirement in self.requirements.values() {
            let location = self.location(&requirement.id);
            let mut problem = |message: String| {
                problems.push(Problem {
                    location: location.clone(),
                    message,
                })
            };
            for (field, value) in [
                ("title", &requirement.title),
                ("statement", &requirement.statement),
                ("rationale", &requirement.rationale),
            ] {
                if value.trim().is_empty() {
                    problem(format!("{field} is empty"));
                }
            }
            if requirement.derived_from.is_empty() {
                problem(
                    "derived_from is empty; every requirement traces to a root or a parent".into(),
                );
            }
            for parent in &requirement.derived_from {
                if parent == &requirement.id {
                    problem("derives from itself".into());
                } else if self.root(parent).is_none() && !self.requirements.contains_key(parent) {
                    problem(format!("derives from unknown {parent}"));
                }
            }
            for (index, entry) in requirement.verification.entries.iter().enumerate() {
                for message in
                    entry_problems(entry, &resolver, &self.repository, &self.config.retired)
                {
                    problem(format!("entry {}: {message}", index + 1));
                }
            }
        }
        for (path, file) in &self.trace.files {
            let mut seen = BTreeSet::new();
            for id in &file.requirements {
                let location = path.clone();
                if !seen.insert(id) {
                    problems.push(Problem {
                        location,
                        message: format!("declares {id} twice"),
                    });
                } else if !self.requirements.contains_key(id) {
                    problems.push(Problem {
                        location,
                        message: format!("declares unknown requirement {id}"),
                    });
                }
            }
        }
        problems.extend(self.cycles());
        problems
    }

    /// Cycles through `derived_from`, which would make status undefined.
    fn cycles(&self) -> Vec<Problem> {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            Open,
            Done,
        }
        fn visit(
            project: &Project,
            id: &str,
            marks: &mut BTreeMap<String, Mark>,
            found: &mut Vec<Problem>,
        ) {
            match marks.get(id) {
                Some(Mark::Done) => return,
                Some(Mark::Open) => {
                    found.push(Problem {
                        location: project.location(id),
                        message: "derived_from forms a cycle".into(),
                    });
                    return;
                }
                None => {}
            }
            marks.insert(id.to_owned(), Mark::Open);
            for parent in &project.requirements[id].derived_from {
                if project.requirements.contains_key(parent) {
                    visit(project, parent, marks, found);
                }
            }
            marks.insert(id.to_owned(), Mark::Done);
        }
        let mut marks = BTreeMap::new();
        let mut found = Vec::new();
        for id in self.requirements.keys() {
            visit(self, id, &mut marks, &mut found);
        }
        found
    }
}

/// The source files `every_file_traced_except` leaves out, each with the entry
/// it sits under. A file is left out only when the real file, links resolved,
/// sits inside the repository under an entry, and so does the directory of
/// every package whose module tree reaches it, by any path. A file anything
/// else compiles in, or that cannot be placed, stays traced.
fn left_out_files(
    repository: &Path,
    config: &Config,
    workspace: &Workspace,
    trace: &Trace,
) -> BTreeMap<String, String> {
    let mut left_out = BTreeMap::new();
    if !config.every_file_traced || config.every_file_traced_except.is_empty() {
        return left_out;
    }
    let Ok(root) = repository.canonicalize() else {
        return left_out;
    };
    let inside = |path: &Path| {
        path.canonicalize()
            .ok()
            .and_then(|real| real.strip_prefix(&root).ok().map(Path::to_path_buf))
    };
    let real: BTreeMap<&str, Option<PathBuf>> = trace
        .files
        .keys()
        .map(|key| (key.as_str(), inside(&repository.join(key))))
        .collect();
    // Every package reaching each real file, through any of its paths.
    let mut reached: BTreeMap<&Path, BTreeSet<&str>> = BTreeMap::new();
    for (key, file) in &trace.files {
        if let Some(Some(real)) = real.get(key.as_str()) {
            reached
                .entry(real.as_path())
                .or_default()
                .extend(file.reached_from.iter().map(String::as_str));
        }
    }
    let package_left_out = |package: &str| {
        workspace
            .packages
            .get(package)
            .and_then(|found| inside(&found.directory))
            .is_some_and(|directory| config.covering(&directory).is_some())
    };
    for (key, real) in &real {
        let Some(real) = real else { continue };
        let Some(entry) = config.covering(real) else {
            continue;
        };
        let packages = &reached[real.as_path()];
        if !packages.is_empty() && packages.iter().all(|package| package_left_out(package)) {
            left_out.insert((*key).to_owned(), entry.to_owned());
        }
    }
    left_out
}

/// `every_file_traced_except` checked on its own and against what it leaves
/// out: an entry that leaves out no file says nothing, and is refused like
/// one that is malformed.
fn left_out_problems(config: &Config, left_out: &BTreeMap<String, String>) -> Vec<Problem> {
    let mut messages = config.except_problems();
    if messages.is_empty() && config.every_file_traced {
        for entry in &config.every_file_traced_except {
            if !left_out.values().any(|by| by == entry) {
                messages.push(format!(
                    "every_file_traced_except: {entry:?} leaves out no source file; a file is \
                     left out only when it and every package reaching it sit under an entry"
                ));
            }
        }
    }
    messages
        .into_iter()
        .map(|message| Problem {
            location: crate::config::FILE.to_owned(),
            message,
        })
        .collect()
}

fn entry_problems(
    entry: &Entry,
    resolver: &Resolver<'_>,
    repository: &Path,
    retired: &[String],
) -> Vec<String> {
    let mut problems = Vec::new();
    let environments: &[Environment] = match entry.level {
        Level::Analysis | Level::Unit | Level::Simulation => &[Environment::Host],
        Level::Target | Level::System => &[Environment::Development, Environment::Product],
        Level::Field => &[Environment::Product],
    };
    let level = entry.level.name();
    if !environments.contains(&entry.environment) {
        problems.push(format!(
            "environment {} does not fit level {level}",
            entry.environment.name()
        ));
    }
    if entry.hazard != Hazard::None && entry.preconditions.is_empty() {
        problems.push(format!(
            "hazard {} needs its preconditions stated",
            entry.hazard.name()
        ));
    }
    match &entry.method {
        Method::Test(path) => {
            if entry.level >= Level::Target {
                problems.push(format!(
                    "level {level} is verified by a procedure, not a host test"
                ));
            }
            match resolver.resolve(path) {
                Ok(resolved) if resolved.kind.is_test() => {}
                Ok(_) => problems.push(format!("{path} is not a #[test] function")),
                Err(message) => problems.push(format!("test does not resolve: {message}")),
            }
        }
        Method::Procedure(reference) => {
            if matches!(entry.level, Level::Unit) {
                problems.push("level unit is verified by a host test, not a procedure".into());
            }
            if let Err(message) = check_procedure(repository, retired, reference) {
                problems.push(message);
            }
        }
        Method::Library(link) => {
            if entry.hazard != Hazard::None || !entry.preconditions.is_empty() {
                problems.push("a library entry runs nothing here, so states no hazard".into());
            }
            if let Err(message) = crate::library::parse(link) {
                problems.push(message);
            }
        }
    }
    problems
}

/// `path#heading`: the file exists and has a Markdown heading with exactly
/// that text.
fn check_procedure(repository: &Path, retired: &[String], reference: &str) -> Result<(), String> {
    let (path, heading) = reference
        .split_once('#')
        .ok_or_else(|| format!("procedure {reference:?} is not path#heading"))?;
    if let Some(old) = retired.iter().find(|old| Path::new(path).starts_with(old)) {
        return Err(format!(
            "procedure {path} is under {old}, which is retired; an active procedure never points there"
        ));
    }
    let text = std::fs::read_to_string(repository.join(path))
        .map_err(|error| format!("procedure {path}: {error}"))?;
    let found = text.lines().any(|line| {
        let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
        hashes > 0 && line[hashes..].starts_with(' ') && line[hashes..].trim() == heading
    });
    if found {
        Ok(())
    } else {
        Err(format!("procedure {path} has no heading {heading:?}"))
    }
}

/// `<AREA>-<NNN>`: one or more upper-case segments, then three digits.
pub fn check_id(id: &str) -> Result<(), String> {
    let segments: Vec<&str> = id.split('-').collect();
    let Some((number, area)) = segments.split_last() else {
        return Err(format!("ID {id:?} is not <AREA>-<NNN>"));
    };
    let number_ok = number.len() == 3 && number.bytes().all(|byte| byte.is_ascii_digit());
    let area_ok = !area.is_empty()
        && area.iter().all(|segment| {
            segment
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_uppercase())
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        });
    if number_ok && area_ok {
        Ok(())
    } else {
        Err(format!("ID {id:?} is not <AREA>-<NNN>"))
    }
}

/// Every requirement file under `directory`, at any depth, in path order.
fn requirement_files(repository: &Path, directory: &str, gates: &str) -> Vec<PathBuf> {
    fn collect(directory: &Path, gates: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            if path.is_dir() {
                if path.file_name().is_none_or(|name| name != "target") {
                    collect(&path, gates, out);
                }
            } else if path
                .extension()
                .is_some_and(|extension| extension == "yaml" || extension == "yml")
                && path != gates
            {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    collect(
        &repository.join(directory),
        &repository.join(gates),
        &mut files,
    );
    files.sort();
    files
}

fn relative(repository: &Path, path: &Path) -> String {
    path.strip_prefix(repository)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::check_id;

    #[test]
    fn ids_are_area_and_three_digits() {
        for good in ["SAFE-001", "INPUT-012", "CMD-INPUT-001", "IO2-100"] {
            assert!(check_id(good).is_ok(), "{good}");
        }
        for bad in [
            "R2",
            "safe-001",
            "SAFE-1",
            "SAFE-0001",
            "-001",
            "SAFE_001",
            "1AB-001",
        ] {
            assert!(check_id(bad).is_err(), "{bad}");
        }
    }
}
