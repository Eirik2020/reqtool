//! What requirement text may hold. A statement is one testable claim of what
//! must hold, its rationale why it matters. Neither carries history,
//! provenance, plans, evidence or paths into the code: version control, the
//! evidence records and the modules' own declarations hold those.

use crate::project::Project;
use crate::schema::Requirement;

pub const STATEMENT_MAX_WORDS: usize = 60;
pub const RATIONALE_MAX_WORDS: usize = 50;

/// Words and phrases that narrate history or name who decided.
const NARRATIVE: [&str; 12] = [
    "the user",
    "previously",
    "originally",
    "formerly",
    "at first",
    "was changed",
    "used to",
    "no longer",
    "earlier proposal",
    "was proposed",
    "was rejected",
    "was decided",
];

/// Every finding in the project's requirement text, each prefixed with where
/// it is.
pub fn project(project: &Project) -> Vec<String> {
    let mut findings = Vec::new();
    for requirement in project.requirements.values() {
        for finding in self::requirement(requirement) {
            findings.push(format!("{}: {finding}", requirement.id));
        }
    }
    findings
}

/// The findings for one requirement's text.
pub fn requirement(requirement: &Requirement) -> Vec<String> {
    let mut findings = Vec::new();
    let statement = &requirement.statement;
    let words = word_count(statement);
    if words > STATEMENT_MAX_WORDS {
        findings.push(format!(
            "statement is {words} words; at most {STATEMENT_MAX_WORDS}"
        ));
    }
    let claims = shall_count(statement);
    if claims != 1 {
        findings.push(format!(
            "statement has {claims} \"shall\"; one claim each, so split it"
        ));
    }
    let words = word_count(&requirement.rationale);
    if words > RATIONALE_MAX_WORDS {
        findings.push(format!(
            "rationale is {words} words; at most {RATIONALE_MAX_WORDS}"
        ));
    }
    for (field, text) in [
        ("title", &requirement.title),
        ("statement", statement),
        ("rationale", &requirement.rationale),
    ] {
        for finding in forbidden(text) {
            findings.push(format!("{field}: {finding}"));
        }
    }
    findings
}

/// What text in a requirement or a FAQ answer may not hold.
fn forbidden(text: &str) -> Vec<String> {
    let mut findings = Vec::new();
    let lower = text.to_lowercase();
    for phrase in NARRATIVE {
        if contains_phrase(&lower, phrase) {
            findings.push(format!("narrates history or provenance (\"{phrase}\")"));
        }
    }
    if has_date(text) {
        findings.push("holds a date".into());
    }
    if has_numbered(text, "ADR-") || has_numbered(text, "#") {
        findings.push("names an ADR or a PR".into());
    }
    if text.contains("::") || text.contains(".rs") || text.contains("src/") {
        findings.push("names a path into the code; modules declare their requirements".into());
    }
    findings
}

fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

fn shall_count(text: &str) -> usize {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.eq_ignore_ascii_case("shall"))
        .count()
}

fn contains_phrase(lower: &str, phrase: &str) -> bool {
    lower.match_indices(phrase).any(|(start, _)| {
        let before = lower[..start].chars().next_back();
        let after = lower[start + phrase.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// A `YYYY-MM-DD` date.
fn has_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.windows(10).any(|w| {
        w[..4].iter().all(u8::is_ascii_digit)
            && w[4] == b'-'
            && w[5..7].iter().all(u8::is_ascii_digit)
            && w[7] == b'-'
            && w[8..].iter().all(u8::is_ascii_digit)
    })
}

/// `prefix` followed directly by a digit, as in `ADR-0042` or `#58`.
fn has_numbered(text: &str, prefix: &str) -> bool {
    text.match_indices(prefix).any(|(start, _)| {
        text[start + prefix.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(statement: &str, rationale: &str) -> Requirement {
        serde_saphyr::from_str(&format!(
            "id: X-001\ntitle: A title\nstatement: >-\n  {statement}\nrationale: >-\n  {rationale}\n\
             derived_from: [R2]\nverification:\n  entries: []\n"
        ))
        .unwrap()
    }

    #[test]
    fn a_clean_requirement_has_no_findings() {
        let clean = with(
            "The parser shall reject a frame whose CRC does not match.",
            "A corrupt frame read as valid would be acted on.",
        );
        assert!(requirement(&clean).is_empty());
    }

    #[test]
    fn length_and_claim_count_are_limited() {
        let long = "word ".repeat(STATEMENT_MAX_WORDS);
        let findings = requirement(&with(
            &format!("It shall hold and it shall also hold {long}"),
            &"why ".repeat(RATIONALE_MAX_WORDS + 1),
        ));
        assert!(findings.iter().any(|f| f.starts_with("statement is")));
        assert!(findings.iter().any(|f| f.contains("2 \"shall\"")));
        assert!(findings.iter().any(|f| f.starts_with("rationale is")));
    }

    #[test]
    fn history_provenance_and_paths_are_refused() {
        let findings = requirement(&with(
            "The link shall never block, as the user chose on 2026-10-17 in ADR-0055.",
            "It previously lived in snapshots.rs and #58 moved it to crate_a::observer.",
        ));
        let has = |needle: &str| findings.iter().any(|f| f.contains(needle));
        assert!(has("\"the user\""));
        assert!(has("a date"));
        assert!(has("an ADR or a PR"));
        assert!(has("\"previously\""));
        assert!(has("a path into the code"));
    }

    #[test]
    fn a_word_inside_another_is_not_narration() {
        assert!(!contains_phrase("refused to used tool", "used to"));
        assert!(contains_phrase("it used to hold", "used to"));
        assert!(!contains_phrase("theuser", "the user"));
    }
}
