//! Requirements and verification tracing for Rust projects.
//!
//! Requirements are YAML, each derived from a project's roots or another
//! requirement. Source files declare the requirements they implement, and
//! evidence records what ran against which file's hash. Status is computed from
//! the evidence, never stored. `reqtool.toml` holds a project's layout.

pub mod config;
pub mod configuration;
pub mod details;
pub mod evidence;
pub mod gates;
pub mod identity;
pub mod items;
pub mod library;
pub mod lint;
pub mod project;
pub mod report;
pub mod roots;
pub mod runner;
pub mod schema;
pub mod status;
pub mod trace;
pub mod workspace;

use std::path::{Path, PathBuf};

/// The project containing `start`: the nearest ancestor with a `reqtool.toml`.
pub fn find_repository(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|candidate| candidate.join(config::FILE).is_file())
        .map(Path::to_path_buf)
}
