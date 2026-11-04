//! A project mapping its requirements onto a library's, through the real tool:
//! the library records its own evidence, and the project inherits the status.

use std::path::Path;
use std::process::{Command, Output};

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn tool(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reqtool"))
        .args(args)
        .current_dir(directory)
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn git(directory: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .current_dir(directory)
        .output()
        .unwrap();
    assert!(status.status.success(), "git {args:?}: {status:?}");
}

fn settings(name: &str) -> String {
    format!(
        r#"name = "{name}"
requirements = "requirements"
roots = "goals.md"
root_prefix = "G"
evidence = "evidence"
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

fn project(link: &str, level: &str, environment: &str) -> String {
    format!(
        "schema_version: 2
requirements:
  - id: LINK-001
    title: Mapped onto the library
    statement: The link shall be maintained.
    rationale: A test needs one.
    derived_from: [G1]
    verification:
      entries:
        - level: {level}
          environment: {environment}
          method:
            library: {link}
"
    )
}

/// A library with one requirement and its test, and a project depending on it.
fn workspace(root: &Path, requirements: &str) {
    let library = root.join("my-hal");
    write(
        &library.join("Cargo.toml"),
        "[package]\nname = \"my-hal\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [package.metadata.reqtool]\ncritical = true\n\n[workspace]\n",
    );
    write(&library.join("src/lib.rs"), LIBRARY_SOURCE);
    write(&library.join("reqtool.toml"), &settings("my-hal"));
    write(&library.join("goals.md"), "1. **G1 - Goal.** A goal.\n");
    write(
        &library.join("requirements/uart.yaml"),
        "schema_version: 2
requirements:
  - id: UART-012
    title: The answer is right
    statement: The library shall give the right answer.
    rationale: A test needs one.
    derived_from: [G1]
    verification:
      entries:
        - level: unit
          environment: host
          method:
            test: my_hal::tests::answers
",
    );
    write(&library.join(".gitignore"), "target\n");

    let app = root.join("app");
    write(
        &app.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nmy-hal = { path = \"../my-hal\", optional = true }\n\n[workspace]\n",
    );
    write(&app.join("src/lib.rs"), "\n");
    write(&app.join("reqtool.toml"), &settings("app"));
    write(&app.join("goals.md"), "1. **G1 - Goal.** A goal.\n");
    write(&app.join("requirements/link.yaml"), requirements);
    for directory in [&library, &app] {
        let built = Command::new("cargo")
            .args(["generate-lockfile", "--offline"])
            .current_dir(directory)
            .output()
            .unwrap();
        assert!(built.status.success(), "{built:?}");
    }
    git(&library, &["init", "-q"]);
    git(&library, &["add", "."]);
    git(&library, &["commit", "-qm", "first"]);
}

fn status_of_link(app: &Path) -> String {
    let output = tool(app, &["check", "LINK-001"]);
    assert!(output.status.success(), "{}", text(&output));
    text(&output)
}

#[test]
fn a_mapped_requirement_follows_the_librarys_evidence() {
    let root = tempfile::tempdir().unwrap();
    workspace(root.path(), &project("my-hal:UART-012", "unit", "host"));
    let (library, app) = (root.path().join("my-hal"), root.path().join("app"));

    let valid = tool(&app, &["validate"]);
    assert!(valid.status.success(), "{}", text(&valid));
    assert!(status_of_link(&app).contains("unverified"));

    let ran = tool(&library, &["test"]);
    assert!(ran.status.success(), "{}", text(&ran));
    let found = status_of_link(&app);
    assert!(
        found.contains("library my-hal:UART-012") && found.contains("verified"),
        "{found}"
    );
    assert!(!found.contains("unverified"), "{found}");

    // What the library's evidence covered changes, and the project sees it.
    write(
        &library.join("src/lib.rs"),
        &LIBRARY_SOURCE.replace("42", "43"),
    );
    assert!(status_of_link(&app).contains("stale"));
}

#[test]
fn a_link_must_name_something_the_library_presents() {
    let refused = |requirements: &str, expected: &str| {
        let root = tempfile::tempdir().unwrap();
        workspace(root.path(), requirements);
        let output = tool(&root.path().join("app"), &["validate"]);
        assert!(!output.status.success());
        assert!(text(&output).contains(expected), "{}", text(&output));
    };
    refused(
        &project("my-hal:UART-999", "unit", "host"),
        "presents no requirement UART-999",
    );
    refused(
        &project("other-hal:UART-012", "unit", "host"),
        "not a dependency",
    );
    refused(
        &project("my-hal:UART-012", "target", "development"),
        "no entry at level target",
    );
    refused(&project("my-hal", "unit", "host"), "crate:ID");
}

#[test]
fn a_library_entry_states_no_hazard() {
    let root = tempfile::tempdir().unwrap();
    let requirements = project("my-hal:UART-012", "target", "development").replace(
        "          method:",
        "          hazard: equipment\n          preconditions:\n            power: off\n          method:",
    );
    workspace(root.path(), &requirements);
    let output = tool(&root.path().join("app"), &["validate"]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("states no hazard"),
        "{}",
        text(&output)
    );
}
