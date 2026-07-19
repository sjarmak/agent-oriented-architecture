use serde::{Deserialize, Serialize};

/// The eleven kinds of trace span emitted across the AOA Toolkit.
///
/// The serialized discriminants are part of the trace-file wire format and are
/// stable: downstream crates and external consumers match on these exact strings.
///
/// # The write lifecycle
///
/// A mutation produces an *intent* span and, separately, an *outcome* span. The
/// two are distinct records because intent is known before execution and outcome
/// only after, and conflating them silently contaminates edit ground truth: a
/// write that was attempted but never landed is not an edit. Only
/// [`SpanType::WriteCommitted`] attests a mutation that actually happened — see
/// [`SpanType::is_confirmed_mutation`].
///
/// Each outcome corresponds to a distinct Claude Code hook event, so the
/// producer never has to classify a payload to decide which one occurred:
///
/// | Span | Hook event | Landed? |
/// |------|-----------|---------|
/// | [`WriteAttempt`](SpanType::WriteAttempt) | `PreToolUse` (allowed) | intent only |
/// | [`WriteCommitted`](SpanType::WriteCommitted) | `PostToolUse` | yes |
/// | [`WriteFailed`](SpanType::WriteFailed) | `PostToolUseFailure` | no |
/// | [`WriteDenied`](SpanType::WriteDenied) | `PermissionDenied` | no |
/// | [`WriteBlocked`](SpanType::WriteBlocked) | AOA's own policy gate | no |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpanType {
    #[serde(rename = "retrieval.search")]
    RetrievalSearch,
    #[serde(rename = "file.read")]
    FileRead,
    #[serde(rename = "symbol.lookup")]
    SymbolLookup,
    /// A mutation was permitted to proceed. Intent, recorded before execution —
    /// never proof that anything landed.
    #[serde(rename = "write.attempt")]
    WriteAttempt,
    /// A mutation completed successfully. The only span that attests a landed
    /// edit, and therefore the sole basis for edit ground truth.
    #[serde(rename = "write.committed")]
    WriteCommitted,
    /// A mutation ran and errored.
    #[serde(rename = "write.failed")]
    WriteFailed,
    /// A mutation was refused before execution by the host or the user.
    #[serde(rename = "write.denied")]
    WriteDenied,
    /// A mutation was refused before execution by an AOA policy rule.
    #[serde(rename = "write.blocked")]
    WriteBlocked,
    #[serde(rename = "test.run")]
    TestRun,
    #[serde(rename = "gateway.invoke")]
    GatewayInvoke,
    #[serde(rename = "abstain")]
    Abstain,
}

impl SpanType {
    /// Every span type, in declaration order. Useful for exhaustive reporting.
    ///
    /// The `write.*` variants are kept adjacent: [`Ord`](crate::SpanType) is
    /// derived from position here, and that ordering is the iteration order of
    /// the span-count report.
    pub const ALL: [SpanType; 11] = [
        SpanType::RetrievalSearch,
        SpanType::FileRead,
        SpanType::SymbolLookup,
        SpanType::WriteAttempt,
        SpanType::WriteCommitted,
        SpanType::WriteFailed,
        SpanType::WriteDenied,
        SpanType::WriteBlocked,
        SpanType::TestRun,
        SpanType::GatewayInvoke,
        SpanType::Abstain,
    ];

    /// The stable wire discriminant for this span type.
    pub fn as_str(&self) -> &'static str {
        match self {
            SpanType::RetrievalSearch => "retrieval.search",
            SpanType::FileRead => "file.read",
            SpanType::SymbolLookup => "symbol.lookup",
            SpanType::WriteAttempt => "write.attempt",
            SpanType::WriteCommitted => "write.committed",
            SpanType::WriteFailed => "write.failed",
            SpanType::WriteDenied => "write.denied",
            SpanType::WriteBlocked => "write.blocked",
            SpanType::TestRun => "test.run",
            SpanType::GatewayInvoke => "gateway.invoke",
            SpanType::Abstain => "abstain",
        }
    }

    /// Whether this span attests a mutation that actually landed.
    ///
    /// The single definition of edit ground truth. Every consumer that derives
    /// edits — held-out corpus edits, `F_edit` — must route through this rather
    /// than matching variants itself, so the counting rule cannot drift apart
    /// between call sites. It did drift before: the corpus counted
    /// `write.attempt` as a landed edit while the evaluator counted
    /// `write.attempt` *and* `write.blocked`, so a denied write inflated
    /// `F_edit`.
    pub fn is_confirmed_mutation(&self) -> bool {
        matches!(self, SpanType::WriteCommitted)
    }

    /// Whether this span belongs to the write lifecycle, landed or not.
    ///
    /// Distinct from [`is_confirmed_mutation`](SpanType::is_confirmed_mutation):
    /// this is the *attention* question, not the *ground truth* question. A file
    /// the agent tried and failed to write is still a file it navigated to, so
    /// footprint-style metrics count every one of these; edit ground truth
    /// counts only the committed one.
    pub fn is_write_lifecycle(&self) -> bool {
        matches!(
            self,
            SpanType::WriteAttempt
                | SpanType::WriteCommitted
                | SpanType::WriteFailed
                | SpanType::WriteDenied
                | SpanType::WriteBlocked
        )
    }

    /// Whether this span marks the point where the agent undertook a write.
    ///
    /// The *ordering* question, distinct from both siblings above: metrics that
    /// split a trace into before-writing and after-writing need the seq where
    /// writing began. Three spans answer yes — intent recorded at `PreToolUse`,
    /// a mutation that landed, and one that ran and errored.
    ///
    /// Refusals deliberately answer no. `write.blocked` and `write.denied` are
    /// *pre-execution* refusals: the agent was stopped before it could write, so
    /// a read that follows one is still genuinely a pre-write read. Counting
    /// them would invert the meaning of the metric on the enforce path, where
    /// the policy gate emits `write.blocked` without a preceding attempt — an
    /// agent whose first call is a blocked edit would have its boundary pinned
    /// at seq 0 and could never register a pre-write read again. Since the R7
    /// reproduction gate exists to bounce the agent back into reading, that
    /// would make `--enforce` depress the very measurement it improves.
    ///
    /// Narrower than [`is_write_lifecycle`](SpanType::is_write_lifecycle), which
    /// asks the *attention* question and so counts refusals: a file the agent
    /// was refused is still a file it navigated to.
    pub fn opens_write_boundary(&self) -> bool {
        matches!(
            self,
            SpanType::WriteAttempt | SpanType::WriteCommitted | SpanType::WriteFailed
        )
    }
}

/// Provenance of a span: emitted directly by an instrumented component
/// (`native`) or inferred after the fact from logs (`reconstructed`).
///
/// Downstream crates exclude `reconstructed` spans when they need ground truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpanSource {
    Native,
    Reconstructed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_types_serialize_to_exact_discriminants() {
        let expected = [
            (SpanType::RetrievalSearch, "retrieval.search"),
            (SpanType::FileRead, "file.read"),
            (SpanType::SymbolLookup, "symbol.lookup"),
            (SpanType::WriteAttempt, "write.attempt"),
            (SpanType::WriteCommitted, "write.committed"),
            (SpanType::WriteFailed, "write.failed"),
            (SpanType::WriteDenied, "write.denied"),
            (SpanType::WriteBlocked, "write.blocked"),
            (SpanType::TestRun, "test.run"),
            (SpanType::GatewayInvoke, "gateway.invoke"),
            (SpanType::Abstain, "abstain"),
        ];

        assert_eq!(SpanType::ALL.len(), 11);

        for (variant, wire) in expected {
            let json = serde_json::to_string(&variant).expect("serialize span type");
            assert_eq!(json, format!("\"{wire}\""));
            assert_eq!(variant.as_str(), wire);

            let parsed: SpanType = serde_json::from_str(&json).expect("deserialize span type");
            assert_eq!(parsed, variant);
        }
    }

    /// The published schema enumerates the same discriminants the enum does.
    /// Nothing bound these two before, so the schema was free to drift silently
    /// as variants were added.
    #[test]
    fn schema_enum_matches_span_type_all() {
        let raw = include_str!("../schema/trace.schema.json");
        let schema: serde_json::Value = serde_json::from_str(raw).expect("schema is valid JSON");
        let enumerated = schema["$defs"]["span"]["properties"]["type"]["enum"]
            .as_array()
            .expect("schema declares a closed enum of span types");

        let from_schema: Vec<&str> = enumerated
            .iter()
            .map(|v| v.as_str().expect("discriminants are strings"))
            .collect();
        let from_code: Vec<&str> = SpanType::ALL.iter().map(SpanType::as_str).collect();

        assert_eq!(from_schema, from_code);
    }

    /// Only a confirmed post-execution success counts as a landed edit. The
    /// intent, refusal, and failure spans stay observable but must never reach
    /// edit ground truth — that conflation is the bug this rule exists to stop.
    #[test]
    fn only_committed_writes_count_as_edits() {
        let counting: Vec<SpanType> = SpanType::ALL
            .into_iter()
            .filter(SpanType::is_confirmed_mutation)
            .collect();
        assert_eq!(counting, vec![SpanType::WriteCommitted]);
    }

    /// Footprint-style metrics ask "did the agent touch this file", which every
    /// write outcome answers yes to. Pinned so a later variant is not quietly
    /// left out of the lifecycle the way the new outcomes were left out of the
    /// footprint filter.
    #[test]
    fn write_lifecycle_covers_every_write_outcome() {
        let lifecycle: Vec<SpanType> = SpanType::ALL
            .into_iter()
            .filter(SpanType::is_write_lifecycle)
            .collect();
        assert_eq!(
            lifecycle,
            vec![
                SpanType::WriteAttempt,
                SpanType::WriteCommitted,
                SpanType::WriteFailed,
                SpanType::WriteDenied,
                SpanType::WriteBlocked,
            ]
        );
        assert!(SpanType::WriteCommitted.is_write_lifecycle());
        assert!(!SpanType::FileRead.is_write_lifecycle());
        assert!(!SpanType::TestRun.is_write_lifecycle());
    }

    /// The before/after-writing split counts only spans where the agent actually
    /// undertook a write. Refusals are excluded on purpose: pinning the boundary
    /// at a `write.blocked` the policy gate emitted without a preceding attempt
    /// would make every later read look post-write. Pinned as a rule so the two
    /// refusal variants cannot be folded back in by someone reaching for
    /// `is_write_lifecycle` because the names look interchangeable.
    #[test]
    fn only_undertaken_writes_open_the_boundary() {
        let opening: Vec<SpanType> = SpanType::ALL
            .into_iter()
            .filter(SpanType::opens_write_boundary)
            .collect();
        assert_eq!(
            opening,
            vec![
                SpanType::WriteAttempt,
                SpanType::WriteCommitted,
                SpanType::WriteFailed,
            ]
        );
        assert!(!SpanType::WriteBlocked.opens_write_boundary());
        assert!(!SpanType::WriteDenied.opens_write_boundary());
        assert!(!SpanType::FileRead.opens_write_boundary());
    }

    #[test]
    fn span_source_round_trips() {
        for (source, wire) in [
            (SpanSource::Native, "native"),
            (SpanSource::Reconstructed, "reconstructed"),
        ] {
            let json = serde_json::to_string(&source).expect("serialize source");
            assert_eq!(json, format!("\"{wire}\""));
            let parsed: SpanSource = serde_json::from_str(&json).expect("deserialize source");
            assert_eq!(parsed, source);
        }
    }
}
