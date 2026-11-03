//! A project's roots, read from the document that owns them.
//!
//! The roots stay prose in the project's own document; this
//! reads only their IDs and titles, so the document remains the one owner.

use std::path::Path;

use crate::config::Config;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Root {
    pub id: String,
    pub title: String,
}

/// Reads every `N. **Gn - Title.**` line of the project's roots.
pub fn load(repository: &Path, config: &Config) -> Result<Vec<Root>, String> {
    let path = repository.join(&config.roots);
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", config.roots))?;
    let roots = parse(&text, &config.root_prefix);
    if roots.is_empty() {
        return Err(format!("{}: no root found", config.roots));
    }
    Ok(roots)
}

pub fn parse(text: &str, prefix: &str) -> Vec<Root> {
    text.lines()
        .filter_map(|line| parse_line(line, prefix))
        .collect()
}

fn parse_line(line: &str, prefix: &str) -> Option<Root> {
    let (number, rest) = line.trim_start().split_once(&format!(". **{prefix}"))?;
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let (digits, rest) = rest.split_once(" - ")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let (title, _) = rest.split_once(".**")?;
    Some(Root {
        id: format!("{prefix}{digits}"),
        title: title.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_numbered_roots_and_ignores_other_lines() {
        let text = "# Governing Requirements\n\
                    \n\
                    1. **R1 - First goal.** The first goal.\n\
                    2. **R2 - Second goal.** The second goal.\n\
                    **Authority.** Not a root.\n\
                    - **W9 - Not numbered.** Ignored.\n";
        assert_eq!(
            parse(text, "R"),
            vec![
                Root {
                    id: "R1".into(),
                    title: "First goal".into()
                },
                Root {
                    id: "R2".into(),
                    title: "Second goal".into()
                },
            ]
        );
    }
}
