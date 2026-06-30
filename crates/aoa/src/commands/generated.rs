//! The R6 generated-artifact plane: wiring the `aoa-enforce` generated-artifact
//! primitives into the two CLI surfaces that consume the operator's declared
//! `generated_paths`.
//!
//! - `enforce check` builds [`GeneratedRule`]s to block a hand-edit of a derived
//!   file, redirecting the agent to the source.
//! - `policy compile` emits the `.gitattributes` marking + provenance headers so
//!   the same files are flagged generated to git (collapsed diffs, excluded from
//!   language stats) and to a human reader.
//!
//! Both read the one declared list, so the runtime gate and the on-disk markers
//! never disagree.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use aoa_enforce::{gitattributes_entry, provenance_header, GeneratedRule};
use aoa_policy::{GeneratedPath, Policy};

/// Sentinels delimiting the AOA-managed region of a repo's `.gitattributes`, so
/// compile can refresh its block without clobbering unrelated entries. Mirrors
/// the idempotent settings.json merge the runtime plane uses.
const BLOCK_BEGIN: &str = "# >>> aoa generated-artifacts (managed by `aoa policy compile`) >>>";
const BLOCK_END: &str = "# <<< aoa generated-artifacts <<<";

/// The source a generated path redirects to: the declared `source:` when present,
/// else the glob itself — the block and header never interpolate an empty string.
fn rule_source(path: &GeneratedPath) -> &str {
    path.source().unwrap_or_else(|| path.glob())
}

/// Compile the policy's declared generated paths into matchable rules. Fails loud
/// on a bad glob rather than silently dropping a rule (mirrors protected-path
/// compilation).
pub fn generated_rules(policy: &Policy) -> Result<Vec<GeneratedRule>> {
    policy
        .generated_paths
        .iter()
        .map(|p| {
            GeneratedRule::new(p.glob(), rule_source(p))
                .with_context(|| format!("invalid generated glob '{}'", p.glob()))
        })
        .collect()
}

/// The AOA-managed `.gitattributes` block body — a provenance-header comment plus
/// a `linguist-generated -diff` entry per declared path — or `None` when no
/// generated paths are declared.
fn gitattributes_body(policy: &Policy) -> Option<String> {
    if policy.generated_paths.is_empty() {
        return None;
    }
    let mut body = String::new();
    for p in &policy.generated_paths {
        body.push_str(&format!("# {}\n", provenance_header(rule_source(p))));
        body.push_str(&gitattributes_entry(p.glob()));
        body.push('\n');
    }
    Some(body)
}

/// Drop the AOA-managed block (sentinels inclusive) from existing content,
/// preserving every unrelated line verbatim. Returns content that ends in a
/// newline when non-empty.
fn strip_block(existing: &str) -> String {
    let lines: Vec<&str> = existing.lines().collect();
    let begin = lines.iter().position(|l| *l == BLOCK_BEGIN);
    let end = lines.iter().position(|l| *l == BLOCK_END);
    let kept: Vec<&str> = match (begin, end) {
        (Some(b), Some(e)) if b <= e => lines
            .iter()
            .enumerate()
            .filter(|(i, _)| *i < b || *i > e)
            .map(|(_, l)| *l)
            .collect(),
        _ => lines,
    };
    if kept.is_empty() {
        String::new()
    } else {
        format!("{}\n", kept.join("\n"))
    }
}

/// Merge the managed block into existing `.gitattributes` content idempotently:
/// replace an existing AOA block, append a fresh one, or strip it when `body` is
/// `None`. Unrelated lines survive verbatim, so re-running is byte-stable.
fn merge_gitattributes(existing: &str, body: Option<&str>) -> String {
    let mut out = strip_block(existing);
    let Some(body) = body else {
        return out;
    };
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(BLOCK_BEGIN);
    out.push('\n');
    out.push_str(body);
    out.push_str(BLOCK_END);
    out.push('\n');
    out
}

/// Write/refresh the R6 `.gitattributes` plane. Non-destructive and idempotent:
/// only the AOA-managed block is touched, unrelated entries survive, and a re-run
/// that changes nothing performs no write. Returns the path when the file was
/// created or changed, `None` when it was already current.
pub fn write_gitattributes_plane(repo: &Path, policy: &Policy) -> Result<Option<PathBuf>> {
    let path = repo.join(".gitattributes");
    let existing = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };

    let merged = merge_gitattributes(&existing, gitattributes_body(policy).as_deref());
    if merged == existing {
        return Ok(None);
    }
    std::fs::write(&path, &merged)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(yaml: &str) -> Policy {
        Policy::from_yaml(yaml).expect("valid policy")
    }

    #[test]
    fn rule_source_falls_back_to_the_glob_when_unsourced() {
        let p = policy("generated_paths: [\"**/*.gen.rs\"]");
        assert_eq!(rule_source(&p.generated_paths[0]), "**/*.gen.rs");
    }

    #[test]
    fn generated_rules_block_declared_paths_with_their_source() {
        let p =
            policy("generated_paths:\n  - glob: \"**/*.gen.rs\"\n    source: \"schema.json\"\n");
        let rules = generated_rules(&p).unwrap();
        let decision = aoa_enforce::generated_artifact_gate(&rules, "crates/api/types.gen.rs");
        assert_eq!(
            decision,
            aoa_enforce::Decision::Block(aoa_enforce::BlockReason::GeneratedArtifact {
                path: "crates/api/types.gen.rs".to_string(),
                source: "schema.json".to_string(),
            })
        );
    }

    #[test]
    fn body_emits_entry_and_provenance_per_path() {
        let p =
            policy("generated_paths:\n  - glob: \"**/*.gen.rs\"\n    source: \"schema.json\"\n");
        let body = gitattributes_body(&p).expect("paths declared");
        assert!(body.contains("**/*.gen.rs linguist-generated -diff"));
        assert!(body.contains("@generated"));
        assert!(body.contains("schema.json"));
    }

    #[test]
    fn body_is_none_when_no_paths_declared() {
        assert!(gitattributes_body(&Policy::default()).is_none());
    }

    #[test]
    fn merge_appends_block_preserving_unrelated_lines() {
        let body = "X linguist-generated -diff\n";
        let merged = merge_gitattributes("* text=auto\n", Some(body));
        assert!(merged.starts_with("* text=auto\n"), "user line kept first");
        assert!(merged.contains(BLOCK_BEGIN));
        assert!(merged.contains("X linguist-generated -diff"));
        assert!(merged.trim_end().ends_with(BLOCK_END));
    }

    #[test]
    fn merge_is_idempotent() {
        let body = "X linguist-generated -diff\n";
        let once = merge_gitattributes("* text=auto\n", Some(body));
        let twice = merge_gitattributes(&once, Some(body));
        assert_eq!(once, twice, "second merge must be a no-op");
    }

    #[test]
    fn merge_strips_block_when_body_is_none() {
        let with_block = merge_gitattributes("* text=auto\n", Some("X -diff\n"));
        let stripped = merge_gitattributes(&with_block, None);
        assert_eq!(stripped, "* text=auto\n", "only user content survives");
        assert!(!stripped.contains(BLOCK_BEGIN));
    }
}
