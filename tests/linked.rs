//! A requirement's links into a library carry its own evidence across a move
//! of the library's git commit: through the real tool, a library in its own
//! git repository with its own evidence, and a project pinning it by `rev`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use reqtool::configuration::Cargo;
use reqtool::evidence::{self, Current, Outcome, RunRequest, Timestamp};
use reqtool::library::Resolved;
use reqtool::project::Project;
use reqtool::schema::Method;

const BENCH: &str = "procedures.md#Bench";

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

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn tool(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reqtool"))
        .args(args)
        .current_dir(directory)
        .output()
        .unwrap()
}

fn cargo(directory: &Path, args: &[&str]) {
    let output = Command::new("cargo")
        .args(args)
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("RUSTC")
        .env_remove("CARGO")
        .current_dir(directory)
        .output()
        .unwrap();
    assert!(output.status.success(), "cargo {args:?}: {}", text(&output));
}

fn settings(name: &str) -> String {
    format!(
        r#"name = "{name}"
requirements = "requirements"
roots = "goals.md"
root_prefix = "G"
evidence = "evidence"
boards = "firmware"
"#
    )
}

const LIBRARY_SOURCE: &str = "//! Requirements: UART-012
//! Safety-critical.

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

fn uart(statement: &str) -> String {
    format!(
        "  - id: UART-012
    title: The answer is right
    statement: {statement}
    rationale: A test needs one.
    derived_from: [G1]
    verification:
      entries:
        - level: unit
          environment: host
          method:
            test: my_hal::tests::answers
"
    )
}

const UART_030: &str = "  - id: UART-030
    title: The answer is still right
    statement: The library shall give the right answer again.
    rationale: A test needs one.
    derived_from: [G1]
    verification:
      entries:
        - level: unit
          environment: host
          method:
            test: my_hal::tests::answers
";

fn catalog(requirements: &str) -> String {
    format!("schema_version: 2\nrequirements:\n{requirements}")
}

const STATEMENT: &str = "The library shall give the right answer.";

/// The app's requirements: APP-001 links the library, APP-002 does not, and
/// both are declared by files importing it.
fn app_requirements(links: &[&str]) -> String {
    let mut text = String::from(
        "schema_version: 2
requirements:
  - id: APP-001
    title: Twice the answer
    statement: The app shall give twice the library's answer.
    rationale: A test needs one.
    derived_from: [G1]
    verification:
      entries:
        - level: unit
          environment: host
          method:
            test: app::tests::doubles
        - level: target
          environment: development
          method:
            procedure: procedures.md#Bench
",
    );
    for link in links {
        text.push_str(&format!(
            "        - level: unit
          environment: host
          method:
            library: {link}
"
        ));
    }
    text.push_str(
        "  - id: APP-002
    title: Thrice the answer
    statement: The app shall give three times the library's answer.
    rationale: A test needs one.
    derived_from: [G1]
    verification:
      entries:
        - level: unit
          environment: host
          method:
            test: app::plain::tests::triples
",
    );
    text
}

/// A library at its first commit with its own evidence, and an app pinning it
/// there, with host evidence for both requirements and board evidence for
/// APP-001.
struct Fixture {
    _root: tempfile::TempDir,
    library: PathBuf,
    app: PathBuf,
    first: String,
    pinned: String,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("my-hal");
        write(
            &library.join("Cargo.toml"),
            "[package]\nname = \"my-hal\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
        );
        write(&library.join("src/lib.rs"), LIBRARY_SOURCE);
        write(&library.join("reqtool.toml"), &settings("my-hal"));
        write(&library.join("goals.md"), "1. **G1 - Goal.** A goal.\n");
        write(&library.join("README.md"), "# my-hal\n\nA library.\n");
        write(
            &library.join("requirements/uart.yaml"),
            &catalog(&uart(STATEMENT)),
        );
        write(&library.join(".gitignore"), "target\n");
        cargo(&library, &["generate-lockfile", "--offline"]);
        git(&library, &["init", "-q", "-b", "main"]);
        git(&library, &["add", "-A"]);
        git(&library, &["commit", "-qm", "first"]);
        let ran = tool(&library, &["test"]);
        assert!(ran.status.success(), "{}", text(&ran));
        git(&library, &["add", "-A"]);
        git(&library, &["commit", "-qm", "evidence"]);
        let first = git(&library, &["rev-parse", "HEAD"]);

        let app = root.path().join("app");
        let url = format!("file://{}", library.display());
        write(&app.join("reqtool.toml"), &settings("app"));
        write(&app.join("goals.md"), "1. **G1 - Goal.** A goal.\n");
        write(
            &app.join("Cargo.toml"),
            &format!(
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
                 [dependencies]\nmy-hal = {{ git = \"{url}\", rev = \"{first}\" }}\n\n[workspace]\n"
            ),
        );
        write(
            &app.join("src/lib.rs"),
            "//! Requirements: APP-001

pub mod plain;

use my_hal::answer;

pub fn twice() -> u32 {
    answer() * 2
}

#[cfg(test)]
mod tests {
    #[test]
    fn doubles() {
        assert_eq!(super::twice(), 84);
    }
}
",
        );
        write(
            &app.join("src/plain.rs"),
            "//! Requirements: APP-002

use my_hal::answer;

pub fn thrice() -> u32 {
    answer() * 3
}

#[cfg(test)]
mod tests {
    #[test]
    fn triples() {
        assert_eq!(super::thrice(), 126);
    }
}
",
        );
        write(
            &app.join("firmware/demo/Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\napp = { path = \"../..\" }\n\n[workspace]\n",
        );
        write(
            &app.join("firmware/demo/src/main.rs"),
            "fn main() {\n    let _ = app::twice();\n}\n",
        );
        write(&app.join("procedures.md"), "# Bench\n\nRun it.\n");
        write(
            &app.join("requirements/app.yaml"),
            &app_requirements(&["my-hal:UART-012"]),
        );
        write(&app.join(".gitignore"), "target\n");
        cargo(&app, &["generate-lockfile"]);
        cargo(&app.join("firmware/demo"), &["generate-lockfile"]);
        git(&app, &["init", "-q"]);
        git(&app, &["add", "-A"]);
        git(&app, &["commit", "-qm", "first"]);

        let ran = tool(&app, &["test"]);
        assert!(ran.status.success(), "{}", text(&ran));
        record_bench(&app);
        let fixture = Self {
            _root: root,
            library,
            app,
            first: first.clone(),
            pinned: first,
        };
        let shown = fixture.check();
        assert!(shown.contains("APP-001  verified"), "{shown}");
        assert!(shown.contains("demo: verified"), "{shown}");
        assert!(!shown.contains("across a move"), "{shown}");
        fixture
    }

    /// A commit on its own branch from `A`, made by `change`.
    fn commit(&self, branch: &str, change: impl FnOnce(&Path)) -> String {
        git(
            &self.library,
            &["checkout", "-q", "-b", branch, &self.first],
        );
        change(&self.library);
        git(&self.library, &["add", "-A"]);
        git(&self.library, &["commit", "-qm", branch]);
        let commit = git(&self.library, &["rev-parse", "HEAD"]);
        git(&self.library, &["checkout", "-q", "main"]);
        commit
    }

    /// Moves the app's pin, in its manifest and both lock files.
    fn pin(&mut self, commit: &str) {
        edit(&self.app.join("Cargo.toml"), &self.pinned, commit);
        cargo(&self.app, &["update", "-p", "my-hal"]);
        cargo(&self.app.join("firmware/demo"), &["update", "-p", "my-hal"]);
        self.pinned = commit.to_owned();
    }

    fn check(&self) -> String {
        let output = tool(&self.app, &["check"]);
        assert!(output.status.success(), "{}", text(&output));
        let mut shown = text(&output);
        for id in ["APP-001", "APP-002"] {
            let output = tool(&self.app, &["check", id]);
            assert!(output.status.success(), "{}", text(&output));
            shown.push_str(&text(&output));
        }
        shown
    }

    fn version(&self, commit: &str) -> String {
        format!("0.1.0+{commit}")
    }
}

/// Board evidence for APP-001, as a bench run records it.
fn record_bench(app: &Path) {
    let (project, problems) = Project::load(app).unwrap();
    assert!(problems.is_empty(), "{problems:#?}");
    let built = Cargo::new(&project.repository, &project.workspace);
    let libraries = Resolved::new(&project.repository);
    let current = Current {
        project: &project,
        trace: &project.trace,
        configurations: &built,
        libraries: &libraries,
        reviews: &project.reviews,
    };
    let now = Timestamp::from_unix(1_792_195_200);
    let details = serde_json::from_str(
        r#"{"execution": {"performed_by": "user", "user_confirmed": true},
            "candidate": {"firmware_sha256": "0000000000000000000000000000000000000000000000000000000000000000"}}"#,
    )
    .unwrap();
    let record = evidence::build(
        &current,
        &RunRequest {
            runs: vec![(Method::Procedure(BENCH.into()), Outcome::Pass)],
            board: Some("demo"),
            features: &[],
            failed: &[],
            note: None,
            supersedes: &[],
            ruling: None,
            details: Some(details),
        },
        now,
    )
    .unwrap();
    evidence::write(app, &record, &now).unwrap();
}

/// The lines of `shown` that name the entry or the move, squeezed.
fn line_with(shown: &str, needle: &str) -> Vec<String> {
    shown
        .lines()
        .filter(|line| line.contains(needle))
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

#[test]
fn a_move_its_links_keep_leaves_a_requirements_evidence_standing() {
    let mut fixture = Fixture::new();
    // A README, a comment and a doc comment change in the library, nothing
    // its evidence covers.
    let docs = fixture.commit("docs", |library| {
        write(
            &library.join("README.md"),
            "# my-hal\n\nA better library.\n",
        );
        edit(
            &library.join("src/lib.rs"),
            "pub fn answer",
            "/// The answer.\n// Why it is right.\npub fn answer",
        );
    });
    fixture.pin(&docs);
    let shown = fixture.check();
    let step = format!(
        "my-hal {} to {}, on its links UART-012",
        fixture.version(&fixture.first),
        fixture.version(&docs)
    );

    // APP-001 links UART-012, which keeps its claim and is verified at the
    // new commit: its host and board evidence stand, and say what on.
    assert!(shown.contains("APP-001  verified"), "{shown}");
    assert!(
        line_with(&shown, "Twice the answer")
            .contains(&"APP-001 verified Twice the answer (stands on its links)".to_owned()),
        "{shown}"
    );
    assert!(shown.contains("demo: verified"), "{shown}");
    assert_eq!(
        shown
            .matches(&format!("stands across a move: {step}"))
            .count(),
        2,
        "the host and the board entry: {shown}"
    );

    // APP-002 imports the same crate and links nothing in it: stale, as
    // before, and it says why.
    assert!(shown.contains("APP-002  stale"), "{shown}");
    assert_eq!(
        line_with(&shown, "not carried across a move"),
        [format!(
            "not carried across a move: my-hal {} to {}: it links none of my-hal's requirements",
            fixture.version(&fixture.first),
            fixture.version(&docs)
        )],
        "{shown}"
    );

    // Evidence at the new commit needs no links to stand, and says nothing.
    let ran = tool(&fixture.app, &["test"]);
    assert!(ran.status.success(), "{}", text(&ran));
    record_bench(&fixture.app);
    let shown = fixture.check();
    assert!(shown.contains("APP-001  verified"), "{shown}");
    assert!(shown.contains("APP-002  verified"), "{shown}");
    assert!(!shown.contains("across a move"), "{shown}");
}

#[test]
fn a_crate_the_project_links_nowhere_moves_as_before() {
    let mut fixture = Fixture::new();
    // The app maps nothing onto the library: no requirement links it.
    write(
        &fixture.app.join("requirements/app.yaml"),
        &app_requirements(&[]),
    );
    let shown = fixture.check();
    assert!(shown.contains("APP-001  verified"), "{shown}");

    let docs = fixture.commit("docs", |library| {
        write(
            &library.join("README.md"),
            "# my-hal\n\nA better library.\n",
        );
    });
    fixture.pin(&docs);
    let shown = fixture.check();
    assert!(shown.contains("APP-001  stale"), "{shown}");
    assert!(shown.contains("demo: stale"), "{shown}");
    assert!(shown.contains("APP-002  stale"), "{shown}");
    assert!(
        !shown.contains("across a move") && !shown.contains("on its links"),
        "nothing new is said: {shown}"
    );
}

#[test]
fn a_move_is_not_carried_unless_every_link_holds_at_both_commits() {
    let mut fixture = Fixture::new();
    let from = fixture.version(&fixture.first);
    let not_carried = |fixture: &mut Fixture, commit: &str, why: &str, status: &str| {
        fixture.pin(commit);
        let shown = fixture.check();
        assert!(shown.contains(&format!("APP-001  {status}")), "{shown}");
        assert!(shown.contains("demo: stale"), "{shown}");
        assert!(!shown.contains("stands across a move"), "{shown}");
        let expected = format!(
            "not carried across a move: my-hal {from} to {}: {why}",
            fixture.version(commit)
        );
        assert_eq!(
            line_with(&shown, &expected).len(),
            2,
            "the host and the board entry: {shown}"
        );
    };

    // The file declaring the linked requirement changes, and the library has
    // not reverified it: stale there, so it carries nothing.
    let source = fixture.commit("source", |library| {
        edit(&library.join("src/lib.rs"), "    42\n}", "    40 + 2\n}");
    });
    not_carried(
        &mut fixture,
        &source,
        &format!("UART-012 is stale at 0.1.0+{source}"),
        "stale",
    );

    // Unverified at the new commit: the library's evidence is gone.
    let unverified = fixture.commit("unverified", |library| {
        git(library, &["rm", "-rq", "evidence"]);
    });
    not_carried(
        &mut fixture,
        &unverified,
        &format!("UART-012 is unverified at 0.1.0+{unverified}"),
        "unverified",
    );

    // Failed at the new commit: APP-001 fails through its library entry, and
    // its own evidence does not stand.
    let failed = fixture.commit("failed", |library| {
        let ran = tool(
            library,
            &[
                "record",
                "--test",
                "my_hal::tests::answers",
                "--result",
                "fail",
            ],
        );
        assert!(ran.status.success(), "{}", text(&ran));
    });
    not_carried(
        &mut fixture,
        &failed,
        &format!("UART-012 is failed at 0.1.0+{failed}"),
        "failed",
    );

    // The linked requirement says something else at the new commit, though
    // the library verified it there.
    let changed = fixture.commit("changed", |library| {
        write(
            &library.join("requirements/uart.yaml"),
            &catalog(&uart("The library shall give the right answer at once.")),
        );
        let ran = tool(library, &["test"]);
        assert!(ran.status.success(), "{}", text(&ran));
    });
    not_carried(
        &mut fixture,
        &changed,
        &format!("UART-012 changed its claim since {from}"),
        "stale",
    );

    // What it sets on the layers below changes its claim too.
    let derived = fixture.commit("derived", |library| {
        write(
            &library.join("requirements/uart.yaml"),
            &catalog(&(uart(STATEMENT) + &UART_030.replace("[G1]", "[UART-012]"))),
        );
        edit(
            &library.join("src/lib.rs"),
            "Requirements: UART-012",
            "Requirements: UART-012, UART-030",
        );
        let ran = tool(library, &["test"]);
        assert!(ran.status.success(), "{}", text(&ran));
    });
    not_carried(
        &mut fixture,
        &derived,
        &format!("UART-012 changed its claim since {from}"),
        "stale",
    );

    // A linked requirement the old commit did not present.
    let added = fixture.commit("added", |library| {
        write(
            &library.join("requirements/uart.yaml"),
            &catalog(&(uart(STATEMENT) + UART_030)),
        );
        edit(
            &library.join("src/lib.rs"),
            "Requirements: UART-012",
            "Requirements: UART-012, UART-030",
        );
        let ran = tool(library, &["test"]);
        assert!(ran.status.success(), "{}", text(&ran));
    });
    write(
        &fixture.app.join("requirements/app.yaml"),
        &app_requirements(&["my-hal:UART-012", "my-hal:UART-030"]),
    );
    not_carried(
        &mut fixture,
        &added,
        &format!("UART-030 is not presented at {from}"),
        "stale",
    );
    write(
        &fixture.app.join("requirements/app.yaml"),
        &app_requirements(&["my-hal:UART-012"]),
    );

    // A linked requirement the new commit no longer presents: the link no
    // longer resolves, so nothing is judged at all.
    let removed = fixture.commit("removed", |library| {
        write(&library.join("requirements/uart.yaml"), &catalog(UART_030));
        edit(
            &library.join("src/lib.rs"),
            "Requirements: UART-012",
            "Requirements: UART-030",
        );
    });
    fixture.pin(&removed);
    let output = tool(&fixture.app, &["check", "APP-001"]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("presents no requirement UART-012"),
        "{}",
        text(&output)
    );
}
