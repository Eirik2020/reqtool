//! The requirement file format, as `serde` types.
//!
//! Every type refuses unknown fields, so a misspelt or invented field is an
//! error rather than silently ignored. There is deliberately no `status`
//! field anywhere: status is computed from evidence. Which code implements
//! a requirement is declared in the code.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The only schema version this tool reads.
pub const SCHEMA_VERSION: u32 = 2;

/// One requirement file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequirementFile {
    pub schema_version: u32,
    pub requirements: Vec<Requirement>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    /// `<AREA>-<NNN>`, stable and never reused.
    pub id: String,
    pub title: String,
    /// The "shall" statement.
    pub statement: String,
    pub rationale: String,
    /// The project's roots or parent requirement IDs.
    pub derived_from: Vec<String>,
    pub verification: Verification,
}

/// A requirement is verified by its own entries; none means nothing
/// verifies it yet.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verification {
    #[serde(default)]
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub level: Level,
    pub environment: Environment,
    /// What can go wrong while this runs, which decides who may run it.
    #[serde(default)]
    pub hazard: Hazard,
    /// What must hold for a pass to count, as `name: state`. A run record must
    /// state each one.
    #[serde(default)]
    pub preconditions: BTreeMap<String, String>,
    pub method: Method,
}

/// Verification levels, ordered by cost and risk.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Reasoned or computed, with nothing run.
    Analysis,
    /// One unit, by a host test.
    Unit,
    /// Against a simulation of the hardware.
    Simulation,
    /// On the target hardware, alone.
    Target,
    /// The integrated system, in a controlled setting.
    System,
    /// The system in real use.
    Field,
}

/// Where a verification runs.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    Host,
    /// A development board standing in for the product.
    Development,
    /// The product's own hardware.
    Product,
}

/// What can go wrong while a verification runs.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Hazard {
    /// Nothing: an agent may run and record it.
    #[default]
    None,
    /// It could damage expensive equipment: a person runs it.
    Equipment,
    /// It could hurt someone: a person runs it.
    Personnel,
}

/// What runs the verification: a Rust test found by parsing, a written
/// procedure named by `path#heading`, or a library's requirement named by
/// `crate:ID`, whose evidence this entry inherits.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    Test(String),
    Procedure(String),
    Library(String),
}

impl Level {
    /// Levels run on hardware, whose evidence belongs to one board.
    pub const fn on_target(self) -> bool {
        matches!(self, Self::Target | Self::System | Self::Field)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Analysis => "analysis",
            Self::Unit => "unit",
            Self::Simulation => "simulation",
            Self::Target => "target",
            Self::System => "system",
            Self::Field => "field",
        }
    }
}

impl Environment {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Development => "development",
            Self::Product => "product",
        }
    }
}

impl Hazard {
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Equipment => "equipment",
            Self::Personnel => "personnel",
        }
    }
}
