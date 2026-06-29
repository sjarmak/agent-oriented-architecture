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
}

impl BlockReason {
    /// Stable machine-readable policy key, recorded on the emitted
    /// `write.blocked` span so a consumer can match on it without parsing prose.
    pub fn policy_key(&self) -> &'static str {
        match self {
            BlockReason::ReproductionRequired => "reproduction_before_mutation",
            BlockReason::ProtectedPath(_) => "protected_path",
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
    Span {
        span_type: SpanType::WriteBlocked,
        source: SpanSource::Native,
        seq,
        attributes,
    }
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
}
