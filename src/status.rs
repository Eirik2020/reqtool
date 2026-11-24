//! Computed status. Nothing here is ever stored.

use std::collections::BTreeMap;

use crate::schema::Requirement;

/// Ordered from best to worst, so the worst of several is their maximum.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Status {
    /// A pass exists whose identities match the current code.
    Verified,
    /// A pass exists, but what it covered has changed since.
    Stale,
    /// No evidence at all.
    Unverified,
    /// A fail stands against the current code.
    Failed,
}

impl Status {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Stale => "stale",
            Self::Unverified => "unverified",
            Self::Failed => "failed",
        }
    }
}

/// A target-level entry's status on one board, with the tier its counting
/// evidence ran at: the newest counting pass's stamp, or the board's tier in
/// `reqtool.toml` for a record from before tiers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoardStatus {
    pub board: String,
    pub status: Status,
    pub tier: Option<String>,
}

/// Where entry statuses come from: the evidence records
/// ([`crate::evidence::Recorded`]), or nothing at all ([`NoEvidence`]).
pub trait Evidence {
    fn entry(&self, requirement: &Requirement, entry: usize) -> Status;

    /// For a target-level entry, its status on each board with evidence.
    fn boards(&self, _requirement: &Requirement, _entry: usize) -> Vec<BoardStatus> {
        Vec::new()
    }

    /// Host-buildable files declaring this requirement that no test passing
    /// now has exercised.
    fn owed_files(&self, _requirement: &Requirement) -> Vec<String> {
        Vec::new()
    }

    /// Files declaring this requirement that neither a counting host test
    /// exercised nor a counting board run compiled in: its board entries
    /// cannot stand while one is left.
    fn owed_board_files(&self, _requirement: &Requirement) -> Vec<String> {
        Vec::new()
    }

    /// Files declaring this requirement that no host test can reach, so only
    /// target-level evidence can judge them.
    fn target_only_files(&self, _requirement: &Requirement) -> Vec<String> {
        Vec::new()
    }

    /// The reviewed steps an entry's status stands on, where it is not the
    /// status the same evidence gives without reviews.
    fn reviews(&self, _requirement: &Requirement, _entry: usize) -> Vec<String> {
        Vec::new()
    }
}

pub struct NoEvidence;

impl Evidence for NoEvidence {
    fn entry(&self, _: &Requirement, _: usize) -> Status {
        Status::Unverified
    }
}

/// Requirement statuses from entry statuses: the worst of a requirement's own
/// entries, and `unverified` without any. Entries naming the same part count
/// as one: the part is proven once, on any one of its carriers, so they give
/// their best status, or `failed` if any of them failed. A parent is never
/// proved through its children, nor a child through its parent's run.
pub fn compute(
    requirements: &BTreeMap<String, Requirement>,
    evidence: &dyn Evidence,
) -> BTreeMap<String, Status> {
    requirements
        .values()
        .map(|requirement| {
            let mut alone = Vec::new();
            let mut parts: BTreeMap<&str, Vec<Status>> = BTreeMap::new();
            for (index, entry) in requirement.verification.entries.iter().enumerate() {
                let status = evidence.entry(requirement, index);
                match &entry.part {
                    Some(part) => parts.entry(part).or_default().push(status),
                    None => alone.push(status),
                }
            }
            let status = alone
                .into_iter()
                .chain(parts.into_values().map(carrier_status))
                .max()
                .unwrap_or(Status::Unverified);
            (requirement.id.clone(), status)
        })
        .collect()
}

/// One part's status from its carriers': the best of them, since any one
/// carrier proves the part. A fail on any carrier still stands: only changed
/// code and a pass, or a ruling, clear one.
fn carrier_status(carriers: Vec<Status>) -> Status {
    if carriers.contains(&Status::Failed) {
        return Status::Failed;
    }
    carriers.into_iter().min().unwrap_or(Status::Unverified)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Entry, Environment, Hazard, Level, Method, Verification};

    fn requirement(id: &str, parents: &[&str], entries: usize) -> Requirement {
        Requirement {
            id: id.into(),
            title: id.into(),
            statement: String::new(),
            rationale: String::new(),
            derived_from: parents.iter().map(|parent| (*parent).into()).collect(),
            verification: Verification {
                entries: (0..entries)
                    .map(|_| Entry {
                        level: Level::Unit,
                        environment: Environment::Host,
                        hazard: Hazard::None,
                        preconditions: std::collections::BTreeMap::new(),
                        part: None,
                        method: Method::Test("t".into()),
                    })
                    .collect(),
            },
        }
    }

    struct Fixed(BTreeMap<(String, usize), Status>);

    impl Evidence for Fixed {
        fn entry(&self, requirement: &Requirement, entry: usize) -> Status {
            self.0
                .get(&(requirement.id.clone(), entry))
                .copied()
                .unwrap_or(Status::Unverified)
        }
    }

    fn graph(list: Vec<Requirement>) -> BTreeMap<String, Requirement> {
        list.into_iter().map(|r| (r.id.clone(), r)).collect()
    }

    fn evidence(entries: &[(&str, usize, Status)]) -> Fixed {
        Fixed(
            entries
                .iter()
                .map(|(id, entry, status)| (((*id).into(), *entry), *status))
                .collect(),
        )
    }

    #[test]
    fn without_evidence_everything_is_unverified() {
        let requirements = graph(vec![
            requirement("A-001", &["R2"], 2),
            requirement("A-002", &["A-001"], 0),
        ]);
        let statuses = compute(&requirements, &NoEvidence);
        assert!(
            statuses
                .values()
                .all(|status| *status == Status::Unverified)
        );
    }

    #[test]
    fn a_requirement_takes_its_worst_entry() {
        let requirements = graph(vec![requirement("A-001", &["R2"], 2)]);
        let fixed = evidence(&[("A-001", 0, Status::Verified), ("A-001", 1, Status::Stale)]);
        assert_eq!(compute(&requirements, &fixed)["A-001"], Status::Stale);
        let fixed = evidence(&[("A-001", 0, Status::Verified), ("A-001", 1, Status::Failed)]);
        assert_eq!(compute(&requirements, &fixed)["A-001"], Status::Failed);
    }

    #[test]
    fn carriers_of_one_part_stand_on_any_one_of_them() {
        let mut both = requirement("A-001", &["R2"], 3);
        both.verification.entries[1].part = Some("uart".into());
        both.verification.entries[2].part = Some("uart".into());
        let requirements = graph(vec![both]);
        let fixed = evidence(&[
            ("A-001", 0, Status::Verified),
            ("A-001", 1, Status::Verified),
            ("A-001", 2, Status::Stale),
        ]);
        assert_eq!(compute(&requirements, &fixed)["A-001"], Status::Verified);
        let fixed = evidence(&[
            ("A-001", 0, Status::Verified),
            ("A-001", 1, Status::Stale),
            ("A-001", 2, Status::Unverified),
        ]);
        assert_eq!(compute(&requirements, &fixed)["A-001"], Status::Stale);
        // The entry outside the part still has to stand on its own.
        let fixed = evidence(&[
            ("A-001", 0, Status::Stale),
            ("A-001", 1, Status::Verified),
            ("A-001", 2, Status::Verified),
        ]);
        assert_eq!(compute(&requirements, &fixed)["A-001"], Status::Stale);
        // A fail on one carrier is not hidden by a pass on another.
        let fixed = evidence(&[
            ("A-001", 0, Status::Verified),
            ("A-001", 1, Status::Verified),
            ("A-001", 2, Status::Failed),
        ]);
        assert_eq!(compute(&requirements, &fixed)["A-001"], Status::Failed);
    }

    #[test]
    fn different_parts_each_need_a_carrier() {
        let mut two = requirement("A-001", &["R2"], 2);
        two.verification.entries[0].part = Some("v2".into());
        two.verification.entries[1].part = Some("v4".into());
        let requirements = graph(vec![two]);
        let fixed = evidence(&[("A-001", 0, Status::Verified), ("A-001", 1, Status::Stale)]);
        assert_eq!(compute(&requirements, &fixed)["A-001"], Status::Stale);
    }

    #[test]
    fn verified_children_do_not_verify_a_parent() {
        let requirements = graph(vec![
            requirement("P-001", &["R7"], 0),
            requirement("C-001", &["P-001"], 1),
        ]);
        let fixed = evidence(&[("C-001", 0, Status::Verified)]);
        let statuses = compute(&requirements, &fixed);
        assert_eq!(statuses["C-001"], Status::Verified);
        assert_eq!(statuses["P-001"], Status::Unverified);
    }
}
