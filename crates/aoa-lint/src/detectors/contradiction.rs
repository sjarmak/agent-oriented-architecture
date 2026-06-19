use std::collections::BTreeSet;

use crate::category::SmellCategory;
use crate::detectors::LintedFile;
use crate::finding::Finding;

/// Flag a structural contradiction: the same subject directed by both an
/// "always" rule and a "never" rule within one file. The subject is the rule's
/// remaining text after the polarity keyword, compared verbatim (lowercased).
/// Catalog: contradictory guidance.
pub fn detect(file: &LintedFile) -> Vec<Finding> {
    let mut always: BTreeSet<String> = BTreeSet::new();
    let mut never: BTreeSet<String> = BTreeSet::new();

    for line in file.text.lines() {
        if let Some(subject) = rule_subject(line, "always") {
            always.insert(subject);
        }
        if let Some(subject) = rule_subject(line, "never") {
            never.insert(subject);
        }
    }

    always
        .intersection(&never)
        .map(|subject| Finding {
            file: file.path.clone(),
            message: format!(
                "contradictory directives: both 'always' and 'never' rules for '{subject}'"
            ),
            category: SmellCategory::Contradiction,
        })
        .collect()
}

/// The subject of a rule line that begins (after list/heading markers) with
/// `keyword`, normalized to a trailing-punctuation-free lowercase string.
fn rule_subject(line: &str, keyword: &str) -> Option<String> {
    let body = line.trim_start_matches(|c: char| {
        c.is_whitespace() || c == '-' || c == '*' || c == '#' || c == '>'
    });
    let lower = body.to_lowercase();
    let rest = lower.strip_prefix(keyword)?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let subject = rest.trim().trim_end_matches(['.', '!', ',']).trim();
    if subject.is_empty() {
        None
    } else {
        Some(subject.to_string())
    }
}
