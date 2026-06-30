//! Operator-policy enforcement primitives for the AOA Toolkit (Wave 1N).
//!
//! This crate is the *intervention* layer: pure, deterministic policy checks a
//! runtime hook consults to ALLOW or BLOCK a pending agent action. It is the
//! keystone of the reproduction-before-mutation gate (PRD R7) and the policy
//! core the three enforcement planes (R5) reuse.
//!
//! ## Why this needs no construct-validity gate
//!
//! These checks enforce a policy the *operator* declared, not a recommendation
//! AOA *inferred*. They are mechanism — an ordering check over a span stream —
//! squarely inside the ZFC "policy enforcement (limits, sandboxing)" allowance.
//! The R9c gating discipline governs whether `aoa recommend` asserts a fix is
//! worth applying; it does not govern whether an operator may opt into their own
//! gate. The two are decoupled by design, which is why this layer ships without
//! waiting on an external-outcome corpus.

use std::fmt;

use aoa_trace::{Span, SpanSource, SpanType};
use serde_json::{Map, Value};

/// The outcome of consulting a policy on a pending action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The action may proceed.
    Allow,
    /// The action is rejected; the [`BlockReason`] says which policy fired.
    Block(BlockReason),
}

impl Decision {
    /// `true` only for [`Decision::Allow`].
    pub fn is_allowed(&self) -> bool {
        matches!(self, Decision::Allow)
    }
}

/// Why a pending action was blocked. One variant per policy; extended as the
/// R5/R6 planes land (protected paths, generated artifacts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockReason {
    /// R7: a write was attempted before any reproduction (`test.run`) span.
    ReproductionRequired,
    /// R5: a write targeted a path the policy declares protected. Carries the
    /// offending repo-relative path for the diagnostic.
    ProtectedPath(String),
    /// R6: a write targeted a declared generated artifact. Carries the offending
    /// `path` and the `source` it derives from, so the block can redirect the
    /// agent to edit the source schema instead of the derived file.
    GeneratedArtifact { path: String, source: String },
}

impl BlockReason {
    /// Stable machine-readable policy key, recorded on the emitted
    /// `write.blocked` span so a consumer can match on it without parsing prose.
    pub fn policy_key(&self) -> &'static str {
        match self {
            BlockReason::ReproductionRequired => "reproduction_before_mutation",
            BlockReason::ProtectedPath(_) => "protected_path",
            BlockReason::GeneratedArtifact { .. } => "generated_artifact",
        }
    }

    /// The source an agent should edit instead, when the block carries one (R6).
    /// Recorded as its own span attribute so a consumer can act on the redirect
    /// without parsing the human reason string.
    pub fn source_pointer(&self) -> Option<&str> {
        match self {
            BlockReason::GeneratedArtifact { source, .. } => Some(source),
            BlockReason::ReproductionRequired | BlockReason::ProtectedPath(_) => None,
        }
    }
}

impl fmt::Display for BlockReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockReason::ReproductionRequired => f.write_str(
                "reproduction-before-mutation: a test.run span must precede the first write.attempt",
            ),
            BlockReason::ProtectedPath(path) => {
                write!(f, "protected path: policy forbids writing '{path}'")
            }
            BlockReason::GeneratedArtifact { path, source } => write!(
                f,
                "generated artifact: '{path}' is derived from '{source}' — edit '{source}' and regenerate, do not edit the artifact"
            ),
        }
    }
}

/// The reproduction-before-mutation gate (PRD R7).
///
/// Given the spans observed so far in a live trace, decide whether a *pending*
/// `write.attempt` may proceed. The policy: a write is allowed once reproduction
/// has happened — i.e. at least one `test.run` span precedes it. Until then the
/// write is blocked, nudging the agent to reproduce before it mutates.
///
/// Pure and deterministic: a structural check over span ordering, not judgment.
/// A `test.run` of either provenance counts — a test genuinely ran regardless of
/// whether the span was emitted natively or reconstructed.
pub fn reproduction_gate(prior_spans: &[Span]) -> Decision {
    let reproduced = prior_spans
        .iter()
        .any(|span| span.span_type == SpanType::TestRun);
    if reproduced {
        Decision::Allow
    } else {
        Decision::Block(BlockReason::ReproductionRequired)
    }
}

/// Build the `write.blocked` span to append when a gate returns [`Decision::Block`].
///
/// Carries the firing policy's stable key and human reason as attributes so both
/// output registers (agent-JSON and human) read the same record.
pub fn blocked_span(seq: u64, reason: BlockReason) -> Span {
    let mut attributes = Map::new();
    attributes.insert(
        "policy".to_string(),
        Value::String(reason.policy_key().to_string()),
    );
    attributes.insert("reason".to_string(), Value::String(reason.to_string()));
    if let Some(source) = reason.source_pointer() {
        attributes.insert("source".to_string(), Value::String(source.to_string()));
    }
    Span {
        span_type: SpanType::WriteBlocked,
        source: SpanSource::Native,
        seq,
        attributes,
    }
}

/// One declared generated-artifact rule: paths matching `glob` are derived from
/// `source`. Built once (the glob compiles eagerly, failing loud on a bad
/// pattern) and matched against pending write targets by [`generated_artifact_gate`].
#[derive(Debug, Clone)]
pub struct GeneratedRule {
    glob: globset::GlobMatcher,
    source: String,
}

impl GeneratedRule {
    /// Compile a generated-path glob paired with the source it derives from.
    /// Returns the glob error rather than silently dropping an unmatched rule.
    pub fn new(glob: &str, source: impl Into<String>) -> Result<Self, globset::Error> {
        Ok(GeneratedRule {
            glob: globset::Glob::new(glob)?.compile_matcher(),
            source: source.into(),
        })
    }
}

/// The generated-artifact protection gate (PRD R6).
///
/// Given the declared generated rules and a pending write's repo-relative
/// `target`, block the write when the target is a generated artifact, pointing
/// the agent at the `source` it should edit instead. Returns the first matching
/// rule's source. Pure and deterministic: glob membership, not judgment.
pub fn generated_artifact_gate(declared: &[GeneratedRule], target: &str) -> Decision {
    match declared.iter().find(|rule| rule.glob.is_match(target)) {
        Some(rule) => Decision::Block(BlockReason::GeneratedArtifact {
            path: target.to_string(),
            source: rule.source.clone(),
        }),
        None => Decision::Allow,
    }
}

/// The machine-readable provenance header line stamped into a generated
/// artifact. The `@generated` token is the conventional marker review tools
/// (GitHub, Phabricator) detect; naming `source` makes the redirect actionable.
/// Callers wrap it in their file's comment syntax.
#[must_use]
pub fn provenance_header(source: &str) -> String {
    format!("@generated by aoa from {source} — edit {source} and regenerate; do not edit this file directly")
}

/// The `.gitattributes` entry marking `glob` a generated artifact:
/// `linguist-generated` excludes it from language stats and collapses it in
/// diffs; `-diff` suppresses the textual diff so reviewers are not shown churn.
#[must_use]
pub fn gitattributes_entry(glob: &str) -> String {
    format!("{glob} linguist-generated -diff")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(span_type: SpanType, seq: u64) -> Span {
        Span {
            span_type,
            source: SpanSource::Native,
            seq,
            attributes: Map::new(),
        }
    }

    #[test]
    fn blocks_write_when_no_reproduction_precedes() {
        // Only search + read happened; the agent never ran a test.
        let prior = [
            span(SpanType::RetrievalSearch, 0),
            span(SpanType::FileRead, 1),
        ];
        assert_eq!(
            reproduction_gate(&prior),
            Decision::Block(BlockReason::ReproductionRequired)
        );
    }

    #[test]
    fn blocks_write_on_empty_trace() {
        assert!(!reproduction_gate(&[]).is_allowed());
    }

    #[test]
    fn allows_write_once_a_test_run_precedes() {
        let prior = [
            span(SpanType::RetrievalSearch, 0),
            span(SpanType::TestRun, 1),
        ];
        assert_eq!(reproduction_gate(&prior), Decision::Allow);
        assert!(reproduction_gate(&prior).is_allowed());
    }

    #[test]
    fn allows_subsequent_writes_after_reproduction() {
        // test.run, then a write already landed; a second write is still allowed.
        let prior = [span(SpanType::TestRun, 0), span(SpanType::WriteAttempt, 1)];
        assert_eq!(reproduction_gate(&prior), Decision::Allow);
    }

    #[test]
    fn reconstructed_test_run_also_satisfies_the_gate() {
        let prior = [Span {
            span_type: SpanType::TestRun,
            source: SpanSource::Reconstructed,
            seq: 0,
            attributes: Map::new(),
        }];
        assert_eq!(reproduction_gate(&prior), Decision::Allow);
    }

    #[test]
    fn blocked_span_carries_policy_key_and_reason() {
        let s = blocked_span(7, BlockReason::ReproductionRequired);
        assert_eq!(s.span_type, SpanType::WriteBlocked);
        assert_eq!(s.source, SpanSource::Native);
        assert_eq!(s.seq, 7);
        assert_eq!(
            s.attributes.get("policy").and_then(Value::as_str),
            Some("reproduction_before_mutation")
        );
        assert!(s
            .attributes
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|r| r.contains("test.run")));
    }

    #[test]
    fn reproduction_block_span_carries_no_source_pointer() {
        // Only the generated-artifact reason has a source; others must not add one.
        let s = blocked_span(0, BlockReason::ReproductionRequired);
        assert!(s.attributes.get("source").is_none());
    }

    fn generated(pattern: &str, source: &str) -> GeneratedRule {
        GeneratedRule::new(pattern, source).expect("valid glob")
    }

    #[test]
    fn blocks_write_to_declared_generated_path_with_its_source() {
        let rules = [
            generated("**/*.gen.rs", "schema.json"),
            generated("api/openapi.yaml", "api/spec.toml"),
        ];
        assert_eq!(
            generated_artifact_gate(&rules, "crates/api/types.gen.rs"),
            Decision::Block(BlockReason::GeneratedArtifact {
                path: "crates/api/types.gen.rs".to_string(),
                source: "schema.json".to_string(),
            })
        );
        // A second rule resolves its own source pointer.
        assert_eq!(
            generated_artifact_gate(&rules, "api/openapi.yaml"),
            Decision::Block(BlockReason::GeneratedArtifact {
                path: "api/openapi.yaml".to_string(),
                source: "api/spec.toml".to_string(),
            })
        );
    }

    #[test]
    fn allows_write_to_non_generated_path() {
        let rules = [generated("**/*.gen.rs", "schema.json")];
        assert_eq!(
            generated_artifact_gate(&rules, "crates/api/handler.rs"),
            Decision::Allow
        );
        assert!(generated_artifact_gate(&rules, "src/lib.rs").is_allowed());
        // No declared rules => nothing is generated.
        assert!(generated_artifact_gate(&[], "anything.gen.rs").is_allowed());
    }

    #[test]
    fn generated_block_span_points_at_the_source() {
        let reason = BlockReason::GeneratedArtifact {
            path: "types.gen.rs".to_string(),
            source: "schema.json".to_string(),
        };
        assert_eq!(reason.policy_key(), "generated_artifact");
        assert_eq!(reason.source_pointer(), Some("schema.json"));
        let s = blocked_span(3, reason);
        assert_eq!(
            s.attributes.get("policy").and_then(Value::as_str),
            Some("generated_artifact")
        );
        // The source pointer is its own machine-readable attribute, not buried in prose.
        assert_eq!(
            s.attributes.get("source").and_then(Value::as_str),
            Some("schema.json")
        );
        assert!(s
            .attributes
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|r| r.contains("schema.json")));
    }

    #[test]
    fn bad_generated_glob_fails_loud() {
        assert!(GeneratedRule::new("[unclosed", "schema.json").is_err());
    }

    #[test]
    fn provenance_header_names_the_source_with_a_machine_marker() {
        let header = provenance_header("schema.json");
        assert!(header.contains("@generated"), "machine-detectable marker");
        assert!(header.contains("schema.json"), "names the source to edit");
    }

    #[test]
    fn gitattributes_entry_marks_the_glob_generated_and_diffless() {
        assert_eq!(
            gitattributes_entry("**/*.gen.rs"),
            "**/*.gen.rs linguist-generated -diff"
        );
    }
}
