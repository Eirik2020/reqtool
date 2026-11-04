//! Reviewed diff analyses of dependency bumps.
//!
//! Bumping a pinned dependency flags the evidence that recorded the old
//! version. A review clears that without reverifying: someone writes an
//! analysis of what the bump changed, and a maintainer reviews it. A review
//! names one crate and one step, `from` to `to`, and means that evidence
//! recorded with that crate at `from` counts as if it were at `to`.
//!
//! - It is directional and chains: `A` to `B` and `B` to `C` carry evidence at
//!   `A` to `C`, and never `C` to `A`.
//! - It bridges only that crate's version. A changed file, a changed claim or
//!   any other difference in what evidence covered still stales it, and a
//!   recorded fail is carried forward like a pass, so it is never bridged away.
//! - It holds only while its analysis is unchanged and its reviewer is one of
//!   the project's maintainers.
//!
//! A review is one JSON file in the project's reviews directory, written once
//! and never overwritten.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::configuration::same_form;
use crate::evidence::{Timestamp, collect_json};
use crate::identity;
use crate::project::Problem;

pub const REVIEW_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    pub schema_version: u32,
    /// The package whose version the review bridges.
    #[serde(rename = "crate")]
    pub package: String,
    /// The version evidence was recorded at, in the form evidence records it:
    /// `<version>`, or `<version>+<commit>` for a git dependency.
    pub from: String,
    /// The version that evidence counts at.
    pub to: String,
    /// The written analysis, by repository-relative path.
    pub analysis: String,
    /// The analysis's SHA-256 when it was reviewed.
    pub analysis_sha256: String,
    pub analysed_by: String,
    /// A maintainer: only a maintainer may review an analysis.
    pub reviewed_by: String,
    pub reviewed_at_utc: String,
}

impl Review {
    /// One line naming the step and who reviewed it.
    pub fn describe(&self) -> String {
        format!(
            "{} {} to {}, reviewed by {}",
            self.package, self.from, self.to, self.reviewed_by
        )
    }

    /// The file it is written to: one per crate and step.
    pub fn file_name(&self) -> String {
        let raw = format!("{}__{}__{}", self.package, self.from, self.to);
        let slug: String = raw
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+') {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        format!("{slug}.json")
    }
}

/// The steps a chain of reviews stood on, each described.
pub type Bridges = BTreeSet<String>;

/// The reviews that hold.
#[derive(Clone, Debug, Default)]
pub struct Reviews {
    reviews: Vec<Review>,
}

impl Reviews {
    pub fn new(reviews: Vec<Review>) -> Self {
        Self { reviews }
    }

    pub fn is_empty(&self) -> bool {
        self.reviews.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Review> {
        self.reviews.iter()
    }

    /// These reviews and one more.
    #[must_use]
    pub fn with(&self, review: Review) -> Self {
        let mut reviews = self.reviews.clone();
        reviews.push(review);
        Self { reviews }
    }

    /// The shortest chain of reviews carrying `package` forward from `from` to
    /// `to`: empty when they are the same version, `None` when no chain does.
    pub fn chain(&self, package: &str, from: &str, to: &str) -> Option<Vec<&Review>> {
        let (from, to) = (same_form(from), same_form(to));
        if from == to {
            return Some(Vec::new());
        }
        let steps: Vec<(&Review, String, String)> = self
            .reviews
            .iter()
            .filter(|review| review.package == package)
            .map(|review| (review, same_form(&review.from), same_form(&review.to)))
            .collect();
        let mut reached: Vec<(String, Vec<&Review>)> = vec![(from.clone(), Vec::new())];
        let mut waiting = VecDeque::from([from]);
        while let Some(version) = waiting.pop_front() {
            let path = reached
                .iter()
                .find(|(seen, _)| *seen == version)
                .map(|(_, path)| path.clone())
                .unwrap_or_default();
            for (review, step_from, step_to) in &steps {
                if *step_from != version || reached.iter().any(|(seen, _)| seen == step_to) {
                    continue;
                }
                let mut longer = path.clone();
                longer.push(*review);
                if *step_to == to {
                    return Some(longer);
                }
                reached.push((step_to.clone(), longer));
                waiting.push_back(step_to.clone());
            }
        }
        None
    }
}

/// Pairs the external crate versions a record holds with the current ones, one
/// to one: the same version, or, with `reviews`, one a chain of reviews carries
/// forward to. The steps it stood on, or `None` when a version is left unpaired.
pub fn pair(
    recorded: &[(&str, &str)],
    now: &[(&str, &str)],
    reviews: Option<&Reviews>,
) -> Option<Bridges> {
    let mut open: Vec<Option<(&str, String)>> = now
        .iter()
        .map(|(package, version)| Some((*package, same_form(version))))
        .collect();
    let mut moved = Vec::new();
    for (package, version) in recorded {
        let version = same_form(version);
        match open
            .iter_mut()
            .find(|slot| slot.as_ref() == Some(&(*package, version.clone())))
        {
            Some(slot) => *slot = None,
            None => moved.push((*package, version)),
        }
    }
    let open: Vec<(&str, String)> = open.into_iter().flatten().collect();
    if moved.len() != open.len() {
        return None;
    }
    let mut bridges = Bridges::new();
    if moved.is_empty() {
        return Some(bridges);
    }
    let mut used = vec![false; open.len()];
    assign(&moved, &open, reviews?, &mut used, &mut bridges).then_some(bridges)
}

/// Pairs each moved version with an unused current one a chain reaches,
/// backtracking where an earlier choice leaves a later version unpaired.
fn assign(
    moved: &[(&str, String)],
    open: &[(&str, String)],
    reviews: &Reviews,
    used: &mut [bool],
    bridges: &mut Bridges,
) -> bool {
    let Some(((package, from), rest)) = moved.split_first() else {
        return true;
    };
    for (index, (candidate, to)) in open.iter().enumerate() {
        if used[index] || candidate != package {
            continue;
        }
        let Some(chain) = reviews.chain(package, from, to) else {
            continue;
        };
        let before = bridges.clone();
        bridges.extend(chain.iter().map(|review| review.describe()));
        used[index] = true;
        if assign(rest, open, reviews, used, bridges) {
            return true;
        }
        used[index] = false;
        *bridges = before;
    }
    false
}

/// Every review in the project's reviews directory: those that hold, and a
/// problem for each that does not.
pub fn load(repository: &Path, config: &Config) -> (Reviews, Vec<Problem>) {
    let directory = config.reviews();
    let mut files = Vec::new();
    collect_json(&repository.join(&directory), &mut files);
    files.sort();
    let mut reviews = Vec::new();
    let mut problems = Vec::new();
    for file in files {
        let location = file
            .strip_prefix(repository)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let parsed = std::fs::read_to_string(&file)
            .map_err(|error| error.to_string())
            .and_then(|text| {
                serde_json::from_str::<Review>(&text).map_err(|error| error.to_string())
            });
        let review = match parsed {
            Ok(review) => review,
            Err(message) => {
                problems.push(Problem { location, message });
                continue;
            }
        };
        let found = check(repository, config, &review);
        if found.is_empty() {
            reviews.push(review);
        }
        problems.extend(found.into_iter().map(|message| Problem {
            location: location.clone(),
            message,
        }));
    }
    (Reviews::new(reviews), problems)
}

/// Why a review does not hold, if it does not.
pub fn check(repository: &Path, config: &Config, review: &Review) -> Vec<String> {
    let mut problems = Vec::new();
    if review.schema_version != REVIEW_SCHEMA_VERSION {
        problems.push(format!(
            "schema_version {} is not {REVIEW_SCHEMA_VERSION}",
            review.schema_version
        ));
    }
    for (field, value) in [
        ("crate", &review.package),
        ("from", &review.from),
        ("to", &review.to),
        ("analysed_by", &review.analysed_by),
        ("reviewed_by", &review.reviewed_by),
    ] {
        if value.trim().is_empty() {
            problems.push(format!("{field} is empty"));
        }
    }
    if same_form(&review.from) == same_form(&review.to) {
        problems.push("from and to are the same version, so it bridges nothing".into());
    }
    if config.maintainers.is_empty() {
        problems.push("the project lists no maintainers, so no review holds".into());
    } else if !config.maintainers.contains(&review.reviewed_by) {
        problems.push(format!(
            "reviewed_by {} is not a maintainer: anyone may write an analysis, only a maintainer may review it",
            review.reviewed_by
        ));
    }
    match analysis_sha256(repository, &review.analysis) {
        Ok(hash) if hash == review.analysis_sha256 => {}
        Ok(_) => problems.push(format!(
            "analysis {} has changed since it was reviewed",
            review.analysis
        )),
        Err(message) => problems.push(message),
    }
    problems
}

/// The SHA-256 of an analysis, which must be a file inside the repository.
pub fn analysis_sha256(repository: &Path, analysis: &str) -> Result<String, String> {
    let path = Path::new(analysis);
    if analysis.is_empty()
        || !path
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!(
            "analysis {analysis:?} is not a repository-relative path"
        ));
    }
    let bytes = std::fs::read(repository.join(path))
        .map_err(|error| format!("analysis {analysis}: {error}"))?;
    Ok(identity::sha256(&bytes))
}

/// What a new review says, before it is written.
pub struct Request<'a> {
    pub package: &'a str,
    pub from: &'a str,
    pub to: &'a str,
    pub analysis: &'a str,
    pub analysed_by: &'a str,
    pub reviewed_by: &'a str,
}

/// A new review of the analysis as it is now, refused unless it would hold.
pub fn create(
    repository: &Path,
    config: &Config,
    request: &Request<'_>,
    now: &Timestamp,
) -> Result<Review, String> {
    let review = Review {
        schema_version: REVIEW_SCHEMA_VERSION,
        package: request.package.to_owned(),
        from: request.from.to_owned(),
        to: request.to.to_owned(),
        analysis: request.analysis.to_owned(),
        analysis_sha256: analysis_sha256(repository, request.analysis)?,
        analysed_by: request.analysed_by.to_owned(),
        reviewed_by: request.reviewed_by.to_owned(),
        reviewed_at_utc: now.iso(),
    };
    let problems = check(repository, config, &review);
    if problems.is_empty() {
        Ok(review)
    } else {
        Err(problems.join("; "))
    }
}

/// Writes a review into the reviews directory, never over an existing file.
pub fn write(repository: &Path, config: &Config, review: &Review) -> Result<PathBuf, String> {
    let directory = repository.join(config.reviews());
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let path = directory.join(review.file_name());
    let text = serde_json::to_string_pretty(review).map_err(|e| e.to_string())? + "\n";
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::AlreadyExists => {
                format!("{}: a review of this step exists already", path.display())
            }
            _ => format!("{}: {error}", path.display()),
        })?;
    std::io::Write::write_all(&mut file, text.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review(package: &str, from: &str, to: &str) -> Review {
        Review {
            schema_version: REVIEW_SCHEMA_VERSION,
            package: package.into(),
            from: from.into(),
            to: to.into(),
            analysis: "analysis.md".into(),
            analysis_sha256: String::new(),
            analysed_by: "a".into(),
            reviewed_by: "m".into(),
            reviewed_at_utc: String::new(),
        }
    }

    #[test]
    fn a_chain_runs_forward_only() {
        let reviews = Reviews::new(vec![
            review("hal", "1.0.0", "1.0.1"),
            review("hal", "1.0.1", "1.0.2"),
            review("other", "1.0.2", "1.0.3"),
        ]);
        assert_eq!(reviews.chain("hal", "1.0.0", "1.0.1").unwrap().len(), 1);
        assert_eq!(reviews.chain("hal", "1.0.0", "1.0.2").unwrap().len(), 2);
        assert!(reviews.chain("hal", "1.0.2", "1.0.0").is_none());
        assert!(reviews.chain("hal", "1.0.1", "1.0.0").is_none());
        assert!(
            reviews.chain("hal", "1.0.0", "1.0.3").is_none(),
            "another crate's step"
        );
        assert!(reviews.chain("hal", "1.0.0", "1.0.0").unwrap().is_empty());
    }

    #[test]
    fn a_chain_reads_the_older_revision_form() {
        let reviews = Reviews::new(vec![review("hal", "0.1.0+aaa", "0.1.0+bbb")]);
        assert!(reviews.chain("hal", "0.1.0+rev=aaa", "0.1.0+bbb").is_some());
    }

    #[test]
    fn versions_pair_one_to_one() {
        let reviews = Reviews::new(vec![review("hal", "0.2.0", "0.2.1")]);
        let pair =
            |recorded: &[(&str, &str)], now: &[(&str, &str)]| pair(recorded, now, Some(&reviews));
        assert_eq!(
            pair(&[("hal", "0.2.0")], &[("hal", "0.2.0")]),
            Some(Bridges::new())
        );
        assert_eq!(
            pair(&[("hal", "0.2.0")], &[("hal", "0.2.1")])
                .unwrap()
                .len(),
            1
        );
        assert_eq!(pair(&[("hal", "0.2.0")], &[("hal", "0.2.2")]), None);
        assert_eq!(
            pair(&[("hal", "0.2.0")], &[]),
            None,
            "a crate no longer imported"
        );
        assert_eq!(
            pair(&[], &[("hal", "0.2.0")]),
            None,
            "a crate newly imported"
        );
        // Two copies of one crate: only the reviewed one may move.
        assert!(
            pair(
                &[("hal", "0.2.0"), ("hal", "1.0.0")],
                &[("hal", "0.2.1"), ("hal", "1.0.0")]
            )
            .is_some()
        );
        assert_eq!(
            pair(
                &[("hal", "0.2.0"), ("hal", "1.0.0")],
                &[("hal", "0.2.1"), ("hal", "1.0.1")]
            ),
            None
        );
        assert_eq!(
            super::pair(&[("hal", "0.2.0")], &[("hal", "0.2.1")], None),
            None,
            "without reviews only the same version pairs"
        );
    }

    #[test]
    fn a_file_name_is_one_per_step() {
        assert_eq!(
            review("my-hal", "0.1.0+aaa", "0.1.0+bbb").file_name(),
            "my-hal__0.1.0+aaa__0.1.0+bbb.json"
        );
    }
}
