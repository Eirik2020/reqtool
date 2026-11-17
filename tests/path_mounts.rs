//! A directory mounted by `#[path]` owns its children, as rustc treats it: a
//! test in `timer/pwm.rs` resolves under every name `timer/mod.rs` is mounted
//! as.

use std::path::Path;
use std::process::{Command, Output};

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn validate(directory: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reqtool"))
        .arg("validate")
        .current_dir(directory)
        .output()
        .unwrap()
}

const PWM: &str = "//! Requirements: PWM-001

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

#[test]
fn a_child_of_a_path_mounted_directory_resolves_under_both_names() {
    let root = tempfile::tempdir().unwrap();
    let library = root.path();
    write(
        &library.join("Cargo.toml"),
        "[package]\nname = \"my-hal\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    );
    write(
        &library.join("src/lib.rs"),
        "//! Requirements: PWM-001\n\n\
         #[path = \"timer/mod.rs\"]\npub mod timer_v1;\n\
         #[path = \"timer/mod.rs\"]\npub mod timer_v3;\n",
    );
    write(
        &library.join("src/timer/mod.rs"),
        "//! Requirements: PWM-001\n\nmod pwm;\npub use pwm::answer;\n",
    );
    write(&library.join("src/timer/pwm.rs"), PWM);
    write(
        &library.join("reqtool.toml"),
        "name = \"my-hal\"\nrequirements = \"requirements\"\nroots = \"goals.md\"\n\
         root_prefix = \"G\"\nevidence = \"evidence\"\n",
    );
    write(&library.join("goals.md"), "1. **G1 - Goal.** A goal.\n");
    write(
        &library.join("requirements/pwm.yaml"),
        "schema_version: 2
requirements:
  - id: PWM-001
    title: The answer is right
    statement: The library shall give the right answer.
    rationale: A test needs one.
    derived_from: [G1]
    verification:
      entries:
        - level: unit
          environment: host
          method:
            test: my_hal::timer_v1::pwm::tests::answers
        - level: unit
          environment: host
          method:
            test: my_hal::timer_v3::pwm::tests::answers
",
    );
    let built = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(library)
        .output()
        .unwrap();
    assert!(built.status.success(), "{built:?}");

    let output = validate(library);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{text}");
    assert!(!text.contains("has no source file"), "{text}");
}
