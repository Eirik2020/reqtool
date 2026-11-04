//! Reviewed diff analyses of dependency bumps: a review carries evidence
//! recorded with a crate at one version forward to another, and nothing else.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use reqtool::config::Config;
use reqtool::configuration::{Configuration, Configurations};
use reqtool::evidence::{self, Current, Outcome, Recorded, RunRequest, Timestamp};
use reqtool::library::NoLibraries;
use reqtool::project::Project;
use reqtool::review::{self, Request};
use reqtool::schema::Method;
use reqtool::status::{Evidence, Status};

const TEST: &str = "gadget::tests::answers";
const BENCH: &str = "procedures.md#Bench";
const HOST: usize = 0;
const BOARD: usize = 1;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn edit(path: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains(from), "{from} in {}", path.display());
    std::fs::write(path, text.replacen(from, to, 1)).unwrap();
}

fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .current_dir(directory)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn settings(name: &str, maintainers: &str) -> String {
    format!(
        r#"name = "{name}"
requirements = "requirements"
roots = "goals.md"
root_prefix = "G"
evidence = "evidence"
boards = "firmware"
{maintainers}
"#
    )
}

fn requirement(id: &str, test: &str) -> String {
    format!(
        "schema_version: 2
requirements:
  - id: {id}
    title: The answer is right
    statement: The gadget shall give the right answer.
    rationale: A test needs one.
    derived_from: [G1]
    verification:
      entries:
        - level: unit
          environment: host
          method:
            test: {test}
        - level: target
          environment: development
          method:
            procedure: {BENCH}
"
    )
}

const SOURCE: &str = "//! Requirements: CORE-001
//! Safety-critical.

use dep::Thing;
use other::Item;

pub fn answer() -> u32 {
    42
}

#[cfg(test)]
mod tests {
    #[test]
    fn answers() {
        assert_eq!(super::answer(), 42);
    }
}
";

const LOCK: &str = r#"version = 4

[[package]]
name = "dep"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "gadget"
version = "0.1.0"
dependencies = [
 "dep",
 "other",
]

[[package]]
name = "other"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;

/// A crate importing two pinned registry crates, one requirement on it with a
/// host test and a board procedure, and an analysis to review. Nothing here is
/// built: the board's configuration comes from [`Built`].
fn project() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let path = root.path();
    write(
        &path.join("reqtool.toml"),
        &settings("fixture", r#"maintainers = ["maintainer"]"#),
    );
    write(&path.join("goals.md"), "1. **G1 - Goal.** A goal.\n");
    write(
        &path.join("Cargo.toml"),
        "[package]\nname = \"gadget\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [package.metadata.reqtool]\ncritical = true\n\n\
         [dependencies]\ndep = \"=1.0.0\"\nother = \"=2.0.0\"\n\n[workspace]\n",
    );
    write(&path.join("Cargo.lock"), LOCK);
    write(&path.join("src/lib.rs"), SOURCE);
    write(
        &path.join("firmware/demo/Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    );
    write(&path.join("procedures.md"), "# Bench\n\nRun it.\n");
    write(
        &path.join("requirements/core.yaml"),
        &requirement("CORE-001", TEST),
    );
    write(
        &path.join("analysis.md"),
        "# dep 1.0.0 to 1.0.1\n\nNothing the evidence relies on changed.\n",
    );
    git(path, &["init", "-q"]);
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "fixture"]);
    root
}

/// What a board's build would report: the declaring file and the external
/// crate versions the lock file holds now.
struct Built {
    repository: PathBuf,
}

impl Configurations for Built {
    fn configuration(&self, board: &str, features: &[String]) -> Result<Rc<Configuration>, String> {
        let lock: toml::Table =
            toml::from_str(&std::fs::read_to_string(self.repository.join("Cargo.lock")).unwrap())
                .unwrap();
        let externals = lock["package"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|package| package.get("source").is_some())
            .map(|package| {
                (
                    package["name"].as_str().unwrap().to_owned(),
                    package["version"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        let files = ["src/lib.rs", "firmware/demo/Cargo.toml"]
            .into_iter()
            .map(|file| {
                (
                    file.to_owned(),
                    reqtool::identity::file(&self.repository.join(file)).unwrap(),
                )
            })
            .collect();
        Ok(Rc::new(Configuration {
            board: board.to_owned(),
            features: features.to_vec(),
            files,
            externals,
            environment: BTreeMap::new(),
        }))
    }

    fn host_files(&self) -> Result<Rc<BTreeSet<String>>, String> {
        Ok(Rc::new(["src/lib.rs".to_owned()].into()))
    }
}

fn load(repository: &Path) -> Project {
    let (project, problems) = Project::load(repository).unwrap();
    assert!(problems.is_empty(), "{problems:#?}");
    project
}

fn problems(repository: &Path) -> String {
    let (_, problems) = Project::load(repository).unwrap();
    problems
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn bench_details() -> reqtool::details::Details {
    serde_json::from_str(
        r#"{"execution": {"performed_by": "user", "user_confirmed": true},
            "candidate": {"firmware_sha256": "0000000000000000000000000000000000000000000000000000000000000000"}}"#,
    )
    .unwrap()
}

/// Records the host test and the board procedure, each with `outcome`.
fn record(repository: &Path, outcome: Outcome, second: u64) {
    let project = load(repository);
    let built = Built {
        repository: repository.to_path_buf(),
    };
    let current = Current {
        project: &project,
        trace: &project.trace,
        configurations: &built,
        libraries: &NoLibraries,
        reviews: &project.reviews,
    };
    for (index, (method, board)) in [
        (Method::Test(TEST.into()), None),
        (Method::Procedure(BENCH.into()), Some("demo")),
    ]
    .into_iter()
    .enumerate()
    {
        let now = Timestamp::from_unix(1_792_195_200 + second * 10 + index as u64);
        let request = RunRequest {
            runs: vec![(method, outcome)],
            board,
            features: &[],
            failed: &[],
            note: None,
            supersedes: &[],
            ruling: None,
            details: board.map(|_| bench_details()),
        };
        let record = evidence::build(&current, &request, now).unwrap();
        evidence::write(repository, &record, &now).unwrap();
    }
}

/// Each entry's status, and the reviewed steps it stands on.
fn statuses(repository: &Path) -> [(Status, Vec<String>); 2] {
    let project = load(repository);
    let built = Built {
        repository: repository.to_path_buf(),
    };
    let current = Current {
        project: &project,
        trace: &project.trace,
        configurations: &built,
        libraries: &NoLibraries,
        reviews: &project.reviews,
    };
    let records = evidence::load(repository).unwrap();
    let recorded = Recorded::new(&current, &records, None).unwrap();
    let requirement = &project.requirements["CORE-001"];
    [HOST, BOARD].map(|entry| {
        (
            recorded.entry(requirement, entry),
            recorded.reviews(requirement, entry),
        )
    })
}

fn status(repository: &Path) -> [Status; 2] {
    statuses(repository).map(|(status, _)| status)
}

/// What `reqtool check CORE-001` prints.
fn check(repository: &Path) -> String {
    let project = load(repository);
    let built = Built {
        repository: repository.to_path_buf(),
    };
    let current = Current {
        project: &project,
        trace: &project.trace,
        configurations: &built,
        libraries: &NoLibraries,
        reviews: &project.reviews,
    };
    let records = evidence::load(repository).unwrap();
    let recorded = Recorded::new(&current, &records, None).unwrap();
    reqtool::report::check(&project, "CORE-001", &recorded).unwrap()
        + &reqtool::report::summary(&project, &recorded)
}

fn bump(repository: &Path, package: &str, from: &str, to: &str) {
    edit(
        &repository.join("Cargo.lock"),
        &format!("name = \"{package}\"\nversion = \"{from}\""),
        &format!("name = \"{package}\"\nversion = \"{to}\""),
    );
}

fn request<'a>(package: &'a str, from: &'a str, to: &'a str) -> Request<'a> {
    Request {
        package,
        from,
        to,
        analysis: "analysis.md",
        analysed_by: "contributor",
        reviewed_by: "maintainer",
    }
}

fn review(repository: &Path, request: &Request<'_>) -> Result<PathBuf, String> {
    let config = Config::load(repository)?;
    let now = Timestamp::from_unix(1_800_000_000);
    let review = review::create(repository, &config, request, &now)?;
    review::write(repository, &config, &review)
}

#[test]
fn a_bump_flags_without_a_review() {
    let repo = project();
    let path = repo.path();
    record(path, Outcome::Pass, 1);
    assert_eq!(status(path), [Status::Verified, Status::Verified]);

    bump(path, "dep", "1.0.0", "1.0.1");
    assert_eq!(status(path), [Status::Stale, Status::Stale]);
}

#[test]
fn a_review_restores_it() {
    let repo = project();
    let path = repo.path();
    record(path, Outcome::Pass, 1);
    bump(path, "dep", "1.0.0", "1.0.1");
    review(path, &request("dep", "1.0.0", "1.0.1")).unwrap();

    let step = "dep 1.0.0 to 1.0.1, reviewed by maintainer";
    for (status, steps) in statuses(path) {
        assert_eq!(status, Status::Verified);
        assert_eq!(steps, [step]);
    }
    let shown = check(path);
    assert!(
        shown.contains(&format!("stands on a review: {step}")),
        "{shown}"
    );
    assert!(shown.contains("(stands on a review)"), "{shown}");

    // Evidence at the current version needs no review, and says so.
    record(path, Outcome::Pass, 2);
    for (status, steps) in statuses(path) {
        assert_eq!(status, Status::Verified);
        assert!(steps.is_empty(), "{steps:?}");
    }
    assert!(!check(path).contains("review"));
}

#[test]
fn the_wrong_pair_or_the_wrong_direction_does_not_bridge() {
    let wrong_pair = |package: &str, from: &str, to: &str| {
        let repo = project();
        let path = repo.path();
        record(path, Outcome::Pass, 1);
        bump(path, "dep", "1.0.0", "1.0.1");
        review(path, &request(package, from, to)).unwrap();
        assert_eq!(
            status(path),
            [Status::Stale, Status::Stale],
            "{package} {from} to {to}"
        );
    };
    wrong_pair("dep", "1.0.0", "1.0.2");
    wrong_pair("dep", "0.9.0", "1.0.1");
    wrong_pair("other", "1.0.0", "1.0.1");

    // Evidence at 1.0.1, and the crate goes back to 1.0.0.
    let repo = project();
    let path = repo.path();
    bump(path, "dep", "1.0.0", "1.0.1");
    record(path, Outcome::Pass, 1);
    bump(path, "dep", "1.0.1", "1.0.0");
    review(path, &request("dep", "1.0.0", "1.0.1")).unwrap();
    assert_eq!(status(path), [Status::Stale, Status::Stale]);
}

#[test]
fn reviews_chain() {
    let repo = project();
    let path = repo.path();
    record(path, Outcome::Pass, 1);
    bump(path, "dep", "1.0.0", "1.0.2");
    review(path, &request("dep", "1.0.0", "1.0.1")).unwrap();
    assert_eq!(status(path), [Status::Stale, Status::Stale], "half a chain");

    review(path, &request("dep", "1.0.1", "1.0.2")).unwrap();
    for (status, steps) in statuses(path) {
        assert_eq!(status, Status::Verified);
        assert_eq!(steps.len(), 2, "{steps:?}");
    }
}

#[test]
fn a_changed_file_or_claim_stays_stale_under_a_review() {
    let repo = project();
    let path = repo.path();
    record(path, Outcome::Pass, 1);
    bump(path, "dep", "1.0.0", "1.0.1");
    review(path, &request("dep", "1.0.0", "1.0.1")).unwrap();
    edit(&path.join("src/lib.rs"), "    42\n}", "    43\n}");
    assert_eq!(status(path), [Status::Stale, Status::Stale]);

    let repo = project();
    let path = repo.path();
    record(path, Outcome::Pass, 1);
    bump(path, "dep", "1.0.0", "1.0.1");
    review(path, &request("dep", "1.0.0", "1.0.1")).unwrap();
    edit(
        &path.join("requirements/core.yaml"),
        "right answer",
        "right answer quickly",
    );
    assert_eq!(status(path), [Status::Stale, Status::Stale]);

    // Another crate moving as well is a difference the review does not cover.
    let repo = project();
    let path = repo.path();
    record(path, Outcome::Pass, 1);
    bump(path, "dep", "1.0.0", "1.0.1");
    bump(path, "other", "2.0.0", "2.0.1");
    review(path, &request("dep", "1.0.0", "1.0.1")).unwrap();
    assert_eq!(status(path), [Status::Stale, Status::Stale]);
}

#[test]
fn a_failing_result_stays_failed() {
    let repo = project();
    let path = repo.path();
    record(path, Outcome::Fail, 1);
    assert_eq!(status(path), [Status::Failed, Status::Failed]);
    bump(path, "dep", "1.0.0", "1.0.1");
    assert_eq!(
        status(path),
        [Status::Unverified, Status::Unverified],
        "without a review, the fail no longer counts"
    );

    review(path, &request("dep", "1.0.0", "1.0.1")).unwrap();
    for (status, steps) in statuses(path) {
        assert_eq!(
            status,
            Status::Failed,
            "the review carries the fail forward"
        );
        assert_eq!(steps.len(), 1);
    }
    record(path, Outcome::Pass, 2);
    assert_eq!(
        status(path),
        [Status::Failed, Status::Failed],
        "a pass with nothing changed does not clear it"
    );
}

#[test]
fn only_a_maintainer_may_review() {
    let repo = project();
    let path = repo.path();
    let refused = review(
        path,
        &Request {
            reviewed_by: "contributor",
            ..request("dep", "1.0.0", "1.0.1")
        },
    )
    .unwrap_err();
    assert!(refused.contains("not a maintainer"), "{refused}");
    assert!(!path.join("requirements/reviews").exists());

    // A review written by hand is refused the same way, and does not count.
    record(path, Outcome::Pass, 1);
    bump(path, "dep", "1.0.0", "1.0.1");
    review(path, &request("dep", "1.0.0", "1.0.1")).unwrap();
    let file = path.join("requirements/reviews/dep__1.0.0__1.0.1.json");
    edit(
        &file,
        "\"reviewed_by\": \"maintainer\"",
        "\"reviewed_by\": \"contributor\"",
    );
    assert!(
        problems(path).contains("not a maintainer"),
        "{}",
        problems(path)
    );
    let (project, _) = Project::load(path).unwrap();
    assert!(project.reviews.is_empty());

    // With no maintainers, no review holds.
    edit(
        &file,
        "\"reviewed_by\": \"contributor\"",
        "\"reviewed_by\": \"maintainer\"",
    );
    assert_eq!(problems(path), "");
    write(&path.join("reqtool.toml"), &settings("fixture", ""));
    assert!(
        problems(path).contains("no maintainers"),
        "{}",
        problems(path)
    );
    let refused = review(path, &request("dep", "1.0.0", "1.0.2")).unwrap_err();
    assert!(refused.contains("no maintainers"), "{refused}");
}

#[test]
fn a_missing_or_changed_analysis_is_refused() {
    let repo = project();
    let path = repo.path();
    for (analysis, expected) in [
        ("missing.md", "missing.md"),
        ("../analysis.md", "not a repository-relative path"),
        ("/etc/hostname", "not a repository-relative path"),
    ] {
        let refused = review(
            path,
            &Request {
                analysis,
                ..request("dep", "1.0.0", "1.0.1")
            },
        )
        .unwrap_err();
        assert!(refused.contains(expected), "{refused}");
    }

    review(path, &request("dep", "1.0.0", "1.0.1")).unwrap();
    assert_eq!(problems(path), "");
    edit(&path.join("analysis.md"), "Nothing", "Little");
    assert!(
        problems(path).contains("analysis analysis.md has changed since it was reviewed"),
        "{}",
        problems(path)
    );
    std::fs::remove_file(path.join("analysis.md")).unwrap();
    assert!(
        problems(path).contains("analysis analysis.md:"),
        "{}",
        problems(path)
    );
}

#[test]
fn a_review_bridges_a_real_step_and_is_written_once() {
    let repo = project();
    let path = repo.path();
    for (from, to) in [("1.0.0", "1.0.0"), ("0.1.0+rev=abc", "0.1.0+abc")] {
        let refused = review(path, &request("dep", from, to)).unwrap_err();
        assert!(refused.contains("same version"), "{refused}");
    }
    review(path, &request("dep", "1.0.0", "1.0.1")).unwrap();
    let again = review(path, &request("dep", "1.0.0", "1.0.1")).unwrap_err();
    assert!(again.contains("exists already"), "{again}");
}
