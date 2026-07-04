//! Ownership inference (R16): git-blame arithmetic aggregated into a
//! CODEOWNERS proposal.
//!
//! Pure mechanism (ZFC): the CLI collects per-author blamed-line counts from
//! `git blame`; this module only groups them by top-level pattern, assigns
//! each pattern its dominant author by line count, and renders the proposal
//! plus its reviewable diff. No git IO and no semantic judgment happen here —
//! ownership is majority arithmetic with an explicit lexicographic tiebreak.

use std::collections::BTreeMap;

use serde::Serialize;

/// Header stamped on the proposal so a reviewer knows its provenance and that
/// it is a proposal, not an operator declaration.
const PROPOSAL_HEADER: &str = "# PROPOSED by `aoa policy infer-owners` from git-blame line ownership — review before committing.";

/// Blamed-line count for one (file, author) pair, as collected by the caller
/// from `git blame`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameCount {
    /// Repo-relative `/`-separated path.
    pub path: String,
    /// The blame `author-mail` identity (emails are valid CODEOWNERS owners).
    pub author: String,
    /// Lines of `path` attributed to `author`.
    pub lines: u64,
}

/// One proposed CODEOWNERS entry: the dominant author of a top-level pattern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnedPattern {
    /// `/dir/` for a top-level directory; `/*` for files at the repo root.
    pub pattern: String,
    /// The author owning the most attributed lines under the pattern; a tie
    /// breaks toward the lexicographically smallest author.
    pub owner: String,
    /// Lines attributed to `owner` under this pattern.
    pub owned_lines: u64,
    /// All attributed lines under this pattern.
    pub total_lines: u64,
}

/// Aggregate blame counts into per-pattern ownership: group by top-level
/// directory (root files under `/*`), sum lines per author, and assign each
/// pattern its majority author. Deterministic: entries are sorted by pattern
/// and ties break lexicographically. Zero-line counts carry no ownership
/// evidence and are ignored.
pub fn infer_owners(counts: &[BlameCount]) -> Vec<OwnedPattern> {
    let mut by_pattern: BTreeMap<String, BTreeMap<&str, u64>> = BTreeMap::new();
    for count in counts.iter().filter(|c| c.lines > 0) {
        let authors = by_pattern.entry(pattern_for(&count.path)).or_default();
        *authors.entry(count.author.as_str()).or_default() += count.lines;
    }
    by_pattern
        .into_iter()
        .filter_map(|(pattern, authors)| {
            let total_lines: u64 = authors.values().sum();
            // Strictly-greater keeps the first (lexicographically smallest)
            // author on a tie, since BTreeMap iterates in key order.
            let (owner, owned_lines) =
                authors
                    .into_iter()
                    .fold(
                        None,
                        |best: Option<(&str, u64)>, (author, lines)| match best {
                            Some((_, best_lines)) if lines <= best_lines => best,
                            _ => Some((author, lines)),
                        },
                    )?;
            Some(OwnedPattern {
                pattern,
                owner: owner.to_string(),
                owned_lines,
                total_lines,
            })
        })
        .collect()
}

/// The CODEOWNERS pattern a repo-relative path falls under: its top-level
/// directory, or `/*` (root files only) for a path with no directory.
fn pattern_for(path: &str) -> String {
    match path.split_once('/') {
        Some((top, _)) => format!("/{top}/"),
        None => "/*".to_string(),
    }
}

/// Render the inferred entries as CODEOWNERS file content: the provenance
/// header, then one `pattern owner` line per entry.
pub fn proposed_codeowners(entries: &[OwnedPattern]) -> String {
    let mut out = format!("{PROPOSAL_HEADER}\n");
    for entry in entries {
        out.push_str(&format!("{} {}\n", entry.pattern, entry.owner));
    }
    out
}

/// Render the proposal as a reviewable create/overwrite diff against the
/// existing file content, in the same minus/plus style as the migrate pillar's
/// dry-run preview. Identical content renders as an explicit no-op.
pub fn render_proposal_diff(path: &str, existing: Option<&str>, proposed: &str) -> String {
    if existing == Some(proposed) {
        return format!("No change: {path} already matches the proposal.\n");
    }
    let verb = if existing.is_some() {
        "overwrite"
    } else {
        "create"
    };
    let mut out = format!("--- {verb}: {path}\n");
    if let Some(old) = existing {
        for line in old.lines() {
            out.push_str(&format!("-{line}\n"));
        }
    }
    for line in proposed.lines() {
        out.push_str(&format!("+{line}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(path: &str, author: &str, lines: u64) -> BlameCount {
        BlameCount {
            path: path.to_string(),
            author: author.to_string(),
            lines,
        }
    }

    #[test]
    fn groups_by_top_level_directory_and_root() {
        let entries = infer_owners(&[
            count("crates/aoa/src/main.rs", "alice@example.com", 100),
            count("crates/aoa/src/cli.rs", "alice@example.com", 20),
            count("docs/guide.md", "bob@example.com", 30),
            count("README.md", "bob@example.com", 5),
        ]);
        let patterns: Vec<&str> = entries.iter().map(|e| e.pattern.as_str()).collect();
        assert_eq!(patterns, vec!["/*", "/crates/", "/docs/"]);
        assert_eq!(entries[0].owner, "bob@example.com");
        assert_eq!(entries[1].owner, "alice@example.com");
        assert_eq!(entries[1].owned_lines, 120);
        assert_eq!(entries[1].total_lines, 120);
    }

    #[test]
    fn majority_author_wins_the_pattern() {
        let entries = infer_owners(&[
            count("src/a.rs", "alice@example.com", 10),
            count("src/b.rs", "bob@example.com", 60),
        ]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].owner, "bob@example.com");
        assert_eq!(entries[0].owned_lines, 60);
        assert_eq!(entries[0].total_lines, 70);
    }

    #[test]
    fn tie_breaks_to_lexicographically_smallest_author() {
        let entries = infer_owners(&[
            count("src/a.rs", "zed@example.com", 50),
            count("src/b.rs", "alice@example.com", 50),
        ]);
        assert_eq!(entries[0].owner, "alice@example.com");
    }

    #[test]
    fn zero_line_counts_are_ignored() {
        let entries = infer_owners(&[count("src/a.rs", "alice@example.com", 0)]);
        assert!(entries.is_empty());
    }

    #[test]
    fn proposal_carries_header_and_one_line_per_pattern() {
        let entries = infer_owners(&[
            count("src/a.rs", "alice@example.com", 10),
            count("README.md", "bob@example.com", 5),
        ]);
        let proposal = proposed_codeowners(&entries);
        assert!(proposal.starts_with("# PROPOSED by `aoa policy infer-owners`"));
        assert!(proposal.contains("/src/ alice@example.com\n"));
        assert!(proposal.contains("/* bob@example.com\n"));
    }

    #[test]
    fn diff_against_no_file_is_a_create_of_every_line() {
        let diff = render_proposal_diff(".github/CODEOWNERS", None, "# header\n/src/ a@x\n");
        assert!(diff.starts_with("--- create: .github/CODEOWNERS\n"));
        assert!(diff.contains("+# header\n"));
        assert!(diff.contains("+/src/ a@x\n"));
        assert!(!diff.contains("\n-"));
    }

    #[test]
    fn diff_against_existing_file_shows_removed_and_added_lines() {
        let diff = render_proposal_diff(
            ".github/CODEOWNERS",
            Some("old line\n"),
            "# header\n/src/ a@x\n",
        );
        assert!(diff.starts_with("--- overwrite: .github/CODEOWNERS\n"));
        assert!(diff.contains("-old line\n"));
        assert!(diff.contains("+/src/ a@x\n"));
    }

    #[test]
    fn identical_existing_content_is_a_no_op_diff() {
        let proposal = "# header\n/src/ a@x\n";
        let diff = render_proposal_diff(".github/CODEOWNERS", Some(proposal), proposal);
        assert!(diff.contains("No change"));
        assert!(!diff.contains("+# header"));
    }

    #[test]
    fn inference_is_deterministic() {
        let counts = [
            count("src/a.rs", "alice@example.com", 10),
            count("docs/x.md", "bob@example.com", 3),
        ];
        assert_eq!(infer_owners(&counts), infer_owners(&counts));
    }
}
