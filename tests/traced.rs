//! `every_file_traced` with `every_file_traced_except`, through the real tool:
//! a library that owes tracing in every file, and bench crates beside it that
//! it leaves out.

use std::path::Path;
use std::process::{Command, Output};

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn edit(path: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains(from), "{from} in {}", path.display());
    write(path, &text.replacen(from, to, 1));
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

const SETTINGS: &str = r#"name = "my-hal"
requirements = "requirements"
roots = "goals.md"
root_prefix = "G"
evidence = "evidence"
boards = "hil"
every_file_traced = true
"#;

const LIBRARY: &str = "//! Requirements: HAL-001

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

const BENCH: &str = "#[path = \"../../common/shared.rs\"]
mod shared;

fn main() {
    let _ = shared::twice(my_hal::answer());
}
";

const SHARED: &str = "pub fn twice(value: u32) -> u32 {
    value * 2
}
";

/// The shared bench file, by the path its `#[path]` attribute reaches it at.
const SHARED_PATH: &str = "hil/bench-a/src/../../common/shared.rs";

/// A library at the root, and one bench crate under `hil/` that depends on it
/// and pulls in a file shared between benches.
fn repository(root: &Path, except: Option<&str>) {
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"my-hal\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    );
    write(&root.join("src/lib.rs"), LIBRARY);
    let settings = match except {
        Some(except) => format!("{SETTINGS}every_file_traced_except = {except}\n"),
        None => SETTINGS.to_owned(),
    };
    write(&root.join("reqtool.toml"), &settings);
    write(&root.join("goals.md"), "1. **G1 - Goal.** A goal.\n");
    write(
        &root.join("requirements/hal.yaml"),
        "schema_version: 2
requirements:
  - id: HAL-001
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
    let bench = root.join("hil/bench-a");
    write(
        &bench.join("Cargo.toml"),
        "[package]\nname = \"bench-a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nmy-hal = { path = \"../..\" }\n\n[workspace]\n",
    );
    write(&bench.join("src/main.rs"), BENCH);
    write(&root.join("hil/common/shared.rs"), SHARED);
    write(&root.join(".gitignore"), "target\n");
    for directory in [root, &bench] {
        let built = Command::new("cargo")
            .args(["generate-lockfile", "--offline"])
            .current_dir(directory)
            .output()
            .unwrap();
        assert!(built.status.success(), "{built:?}");
    }
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "first"]);
}

fn set_except(root: &Path, except: &str) {
    write(
        &root.join("reqtool.toml"),
        &format!("{SETTINGS}every_file_traced_except = {except}\n"),
    );
}

#[test]
fn benches_are_left_out_and_the_library_stays_traced() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    repository(root, None);

    // With no exception, every file owes tracing, the benches' as well.
    let everything = tool(root, &["validate"]);
    assert!(!everything.status.success());
    let found = text(&everything);
    assert!(
        found.contains("hil/bench-a/src/main.rs: is safety-critical"),
        "{found}"
    );
    assert!(
        found.contains(&format!("{SHARED_PATH}: is safety-critical")),
        "{found}"
    );
    assert!(!found.contains("src/lib.rs: "), "{found}");

    set_except(root, r#"["hil"]"#);
    let valid = tool(root, &["validate"]);
    assert!(valid.status.success(), "{}", text(&valid));
    assert!(
        text(&valid).contains("every_file_traced leaves out hil: 2 file(s)\n"),
        "{}",
        text(&valid)
    );
    let summary = tool(root, &["check"]);
    assert!(summary.status.success(), "{}", text(&summary));
    assert!(text(&summary).contains("every_file_traced leaves out hil: 2 file(s)"));
    let bench = text(&tool(root, &["check", "hil/bench-a/src/main.rs"]));
    assert!(
        bench.contains("not safety-critical") && bench.contains("leaves it out (hil)"),
        "{bench}"
    );
    let library = text(&tool(root, &["check", "src/lib.rs"]));
    assert!(
        library.starts_with("src/lib.rs  safety-critical") && !library.contains("leaves it out"),
        "{library}"
    );

    // The library still owes tracing in every file.
    edit(
        &root.join("src/lib.rs"),
        "pub fn answer",
        "pub mod extra;\n\npub fn answer",
    );
    write(&root.join("src/extra.rs"), "pub fn more() {}\n");
    let undeclared = tool(root, &["validate"]);
    assert!(!undeclared.status.success());
    let found = text(&undeclared);
    assert!(
        found.contains("src/extra.rs: is safety-critical, and declares no requirement"),
        "{found}"
    );
    assert!(!found.contains("hil/"), "{found}");
}

#[test]
fn a_prefix_matches_a_shared_file_where_it_sits() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    repository(root, Some(r#"["hil/common"]"#));
    let output = tool(root, &["validate"]);
    assert!(!output.status.success());
    let found = text(&output);
    assert!(
        found.contains("every_file_traced leaves out hil/common: 1 file(s)"),
        "{found}"
    );
    assert!(
        found.contains("hil/bench-a/src/main.rs: is safety-critical"),
        "{found}"
    );
    assert!(!found.contains(&format!("{SHARED_PATH}: ")), "{found}");
}

#[test]
fn a_file_marked_safety_critical_keeps_its_marker() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    repository(root, Some(r#"["hil"]"#));
    write(
        &root.join("hil/common/shared.rs"),
        &format!("//! Safety-critical.\n\n{SHARED}"),
    );
    let output = tool(root, &["validate"]);
    assert!(!output.status.success());
    let found = text(&output);
    assert!(
        found.contains("every_file_traced leaves out hil: 2 file(s), 1 kept safety-critical by their own marker"),
        "{found}"
    );
    assert!(
        found.contains(&format!(
            "{SHARED_PATH}: is safety-critical, and declares no requirement"
        )),
        "{found}"
    );
    assert!(!found.contains("hil/bench-a/src/main.rs: "), "{found}");
    let file = text(&tool(root, &["check", SHARED_PATH]));
    assert!(
        file.contains("leaves it out (hil), and its own marker keeps it safety-critical"),
        "{file}"
    );
}

#[test]
fn an_exception_must_leave_out_something_and_stay_inside() {
    for (except, expected) in [
        (r#"["benches"]"#, r#""benches" leaves out no source file"#),
        (r#"["hil/bench-a/src/bin"]"#, "leaves out no source file"),
        (r#"["/hil"]"#, "is absolute"),
        (r#"["../hil"]"#, "climbs out with `..`"),
        (r#"["./hil"]"#, "has a `.` component"),
        (r#"["hil/*"]"#, "is a pattern"),
        (r#"["hil", "hil"]"#, "is listed twice"),
        (
            r#"["hil", "hil/common"]"#,
            "leaves out only what an earlier entry does",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        repository(root, Some(except));
        for command in [&["validate"][..], &["check"][..]] {
            let output = tool(root, command);
            assert!(!output.status.success(), "{except}: {}", text(&output));
            assert!(
                text(&output).contains("reqtool.toml: every_file_traced_except: ")
                    && text(&output).contains(expected),
                "{except}: {}",
                text(&output)
            );
        }
    }

    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    repository(root, Some(r#"["hil"]"#));
    edit(
        &root.join("reqtool.toml"),
        "every_file_traced = true",
        "every_file_traced = false",
    );
    let output = tool(root, &["validate"]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("every_file_traced_except is set, but every_file_traced is not"),
        "{}",
        text(&output)
    );
}

#[test]
fn leaving_files_out_changes_no_evidence() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    repository(root, None);
    // Tracing findings do not stop a test run, so evidence is recorded with
    // every file traced.
    let ran = tool(root, &["test"]);
    assert!(ran.status.success(), "{}", text(&ran));
    let before = text(&tool(root, &["check", "HAL-001"]));
    assert!(before.contains("HAL-001  verified"), "{before}");
    let records = std::fs::read_dir(root.join("evidence")).unwrap().count();

    set_except(root, r#"["hil"]"#);
    assert!(tool(root, &["validate"]).status.success());
    assert_eq!(text(&tool(root, &["check", "HAL-001"])), before);
    let again = tool(root, &["test"]);
    assert!(
        text(&again).contains("nothing new to record"),
        "the same run covers the same identities: {}",
        text(&again)
    );
    assert_eq!(
        std::fs::read_dir(root.join("evidence")).unwrap().count(),
        records
    );
}
