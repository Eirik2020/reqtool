//! What `reqtool check` and `reqtool report` print: compact, one fact per
//! line, for agents first.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::configuration::Configuration;
use crate::items::Resolver;
use crate::project::Project;
use crate::schema::{Method, Requirement};
use crate::status::{self, Evidence, Status};
use crate::trace::Import;

/// The whole project: a count per status, then one line per requirement.
pub fn summary(project: &Project, evidence: &dyn Evidence) -> String {
    let statuses = status::compute(&project.requirements, evidence);
    let mut counts: BTreeMap<Status, usize> = BTreeMap::new();
    for status in statuses.values() {
        *counts.entry(*status).or_default() += 1;
    }
    let mut out = format!("{} requirements", project.requirements.len());
    for (status, count) in &counts {
        let _ = write!(out, ", {count} {}", status.name());
    }
    let unmethoded = project
        .requirements
        .values()
        .filter(|r| has_no_method(r))
        .count();
    if unmethoded > 0 {
        let _ = write!(out, "; {unmethoded} with no verification method yet");
    }
    out.push('\n');
    out.push_str(&left_out(project));
    for requirement in project.requirements.values() {
        let gap = if has_no_method(requirement) {
            "  (no method)"
        } else if (0..requirement.verification.entries.len())
            .any(|entry| !evidence.reviews(requirement, entry).is_empty())
        {
            "  (stands on a review)"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "{:<16} {:<10} {}{gap}",
            requirement.id,
            statuses[&requirement.id].name(),
            requirement.title
        );
    }
    out
}

/// One line per entry of `every_file_traced_except`: how many source files it
/// leaves out, and how many of those their own marker keeps safety-critical.
pub fn left_out(project: &Project) -> String {
    let mut out = String::new();
    for (prefix, files) in project.left_out() {
        let marked = files
            .iter()
            .filter(|path| project.trace.files[**path].critical)
            .count();
        let _ = write!(
            out,
            "every_file_traced leaves out {prefix}: {} file(s)",
            files.len()
        );
        if marked > 0 {
            let _ = write!(out, ", {marked} kept safety-critical by their own marker");
        }
        out.push('\n');
    }
    out
}

/// A requirement with no entries: nothing verifies it yet.
fn has_no_method(requirement: &Requirement) -> bool {
    requirement.verification.entries.is_empty()
}

/// One requirement by ID, a governing requirement's children, or a source
/// file, by repository-relative path or by module path, with the
/// requirements it declares.
pub fn check(project: &Project, target: &str, evidence: &dyn Evidence) -> Result<String, String> {
    let statuses = status::compute(&project.requirements, evidence);
    if let Some(requirement) = project.requirements.get(target) {
        return Ok(detail(project, requirement, &statuses, evidence));
    }
    if project.root(target).is_some() {
        let children = project.children(target);
        let mut out = format!(
            "{target} is a governing requirement; {} derive from it\n",
            children.len()
        );
        for child in children {
            let _ = writeln!(
                out,
                "{:<16} {:<10} {}",
                child.id,
                statuses[&child.id].name(),
                child.title
            );
        }
        return Ok(out);
    }
    let path = if project.trace.files.contains_key(target) {
        target.to_owned()
    } else {
        let resolved = Resolver::new(&project.workspace.crates).resolve(target)?;
        project
            .trace
            .locate(&project.repository, &resolved.file)
            .ok_or_else(|| format!("{target}: its file is in no module tree"))?
    };
    let file = &project.trace.files[&path];
    let mut out = format!(
        "{path}  {}\n",
        if file.critical {
            "safety-critical"
        } else {
            "not safety-critical"
        }
    );
    if let Some(prefix) = project.left_out_by(&path) {
        let _ = writeln!(
            out,
            "  every_file_traced leaves it out ({prefix}){}",
            if file.critical {
                ", and its own marker keeps it safety-critical"
            } else {
                ""
            }
        );
    }
    let files = file
        .imports
        .values()
        .filter(|import| **import == Import::File)
        .count();
    let _ = writeln!(
        out,
        "  imports: {files} file(s), {} external crate(s)",
        file.imports.len() - files
    );
    if file.requirements.is_empty() {
        out.push_str("  declares no requirement\n");
        if file.critical && file.defines_code {
            out.push_str("  a safety-critical file must declare the requirements it implements\n");
        }
        return Ok(out);
    }
    let _ = writeln!(out, "  declares: {}", file.requirements.join(", "));
    for id in &file.requirements {
        if let Some(requirement) = project.requirements.get(id) {
            out.push_str(&detail(project, requirement, &statuses, evidence));
        }
    }
    Ok(out)
}

fn detail(
    project: &Project,
    requirement: &Requirement,
    statuses: &BTreeMap<String, Status>,
    evidence: &dyn Evidence,
) -> String {
    let mut out = format!(
        "{}  {}  {}\n  statement: {}\n",
        requirement.id,
        statuses[&requirement.id].name(),
        requirement.title,
        requirement
            .statement
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    );
    let parents: Vec<String> = requirement
        .derived_from
        .iter()
        .map(|parent| match project.root(parent) {
            Some(root) => format!("{parent} ({})", root.title),
            None => parent.clone(),
        })
        .collect();
    let _ = writeln!(out, "  derived from: {}", parents.join(", "));
    let declaring = project.trace.declaring(&requirement.id);
    if !declaring.is_empty() {
        let _ = writeln!(out, "  declared by: {}", declaring.join(", "));
    }
    let owed = evidence.owed_files(requirement);
    if !owed.is_empty() {
        let _ = writeln!(out, "  no passing test exercises: {}", owed.join(", "));
    }
    let target_only = evidence.target_only_files(requirement);
    if !target_only.is_empty() {
        let _ = writeln!(out, "  judged on target only: {}", target_only.join(", "));
    }
    let children: Vec<&str> = project
        .children(&requirement.id)
        .iter()
        .map(|child| child.id.as_str())
        .collect();
    if !children.is_empty() {
        let _ = writeln!(out, "  children: {}", children.join(", "));
    }
    out.push_str("  verification:\n");
    if has_no_method(requirement) {
        out.push_str("    no verification method yet\n");
    }
    for (index, entry) in requirement.verification.entries.iter().enumerate() {
        let method = match &entry.method {
            Method::Test(path) => format!("test {path}"),
            Method::Procedure(reference) => format!("procedure {reference}"),
            Method::Library(link) => format!("library {link}"),
        };
        let _ = writeln!(
            out,
            "    {:<10} {:<11} {:<9} {:<10} {method}",
            entry.level.name(),
            entry.environment.name(),
            entry.hazard.name(),
            evidence.entry(requirement, index).name()
        );
        for (board, status) in evidence.boards(requirement, index) {
            let _ = writeln!(out, "      {board}: {}", status.name());
        }
        for step in evidence.reviews(requirement, index) {
            let _ = writeln!(out, "      stands on a review: {step}");
        }
    }
    out
}

/// Maturity for one configuration: every requirement declared by
/// the safety-critical files it compiles in, with its status there, and the
/// safety-critical files that declare nothing. Requirements declared only by
/// files it compiles in that `every_file_traced_except` leaves out are listed
/// apart, so none drops out of sight.
pub fn maturity(
    project: &Project,
    configuration: &Configuration,
    evidence: &dyn Evidence,
) -> String {
    let statuses = status::compute(&project.requirements, evidence);
    let critical: Vec<(&String, &crate::trace::SourceFile)> = project
        .trace
        .files
        .iter()
        .filter(|(path, file)| file.critical && configuration.files.contains_key(*path))
        .collect();
    let mut declared: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut silent = Vec::new();
    for (path, file) in &critical {
        if file.requirements.is_empty() {
            silent.push(path.as_str());
        }
        for id in &file.requirements {
            declared.entry(id.as_str()).or_default().push(path.as_str());
        }
    }
    let mut counts: BTreeMap<Status, usize> = BTreeMap::new();
    for id in declared.keys() {
        if let Some(status) = statuses.get(*id) {
            *counts.entry(*status).or_default() += 1;
        }
    }
    let features = if configuration.features.is_empty() {
        "default features".to_owned()
    } else {
        configuration.features.join(", ")
    };
    let mut out = format!(
        "{} with {features}: {} files compiled in, {} safety-critical; configuration {}\n",
        configuration.board,
        configuration.files.len(),
        critical.len(),
        &configuration.identity()[..16]
    );
    let _ = write!(out, "{} requirements declared", declared.len());
    for (status, count) in &counts {
        let _ = write!(out, ", {count} {}", status.name());
    }
    out.push('\n');
    for (id, files) in &declared {
        let status = statuses.get(*id).map_or("unknown", |status| status.name());
        let _ = writeln!(out, "{id:<16} {status:<10} {}", files.join(", "));
    }
    let mut apart: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (path, file) in &project.trace.files {
        if file.critical
            || project.left_out_by(path).is_none()
            || !configuration.files.contains_key(path)
        {
            continue;
        }
        for id in &file.requirements {
            if !declared.contains_key(id.as_str()) {
                apart.entry(id.as_str()).or_default().push(path.as_str());
            }
        }
    }
    if !apart.is_empty() {
        let _ = writeln!(
            out,
            "{} requirement(s) declared only by files every_file_traced leaves out:",
            apart.len()
        );
        for (id, files) in &apart {
            let status = statuses.get(*id).map_or("unknown", |status| status.name());
            let _ = writeln!(out, "{id:<16} {status:<10} {}", files.join(", "));
        }
    }
    if !silent.is_empty() {
        let _ = writeln!(
            out,
            "{} safety-critical file(s) declare no requirement:",
            silent.len()
        );
        for path in silent {
            let _ = writeln!(out, "  {path}");
        }
    }
    out
}
