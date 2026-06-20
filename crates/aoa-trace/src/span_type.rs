use serde::{Deserialize, Serialize};

/// The eight kinds of trace span emitted across the AOA Toolkit.
///
/// The serialized discriminants are part of the trace-file wire format and are
/// stable: downstream crates and external consumers match on these exact strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpanType {
    #[serde(rename = "retrieval.search")]
    RetrievalSearch,
    #[serde(rename = "file.read")]
    FileRead,
    #[serde(rename = "symbol.lookup")]
    SymbolLookup,
    #[serde(rename = "write.attempt")]
    WriteAttempt,
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
    pub const ALL: [SpanType; 8] = [
        SpanType::RetrievalSearch,
        SpanType::FileRead,
        SpanType::SymbolLookup,
        SpanType::WriteAttempt,
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
            SpanType::WriteBlocked => "write.blocked",
            SpanType::TestRun => "test.run",
            SpanType::GatewayInvoke => "gateway.invoke",
            SpanType::Abstain => "abstain",
        }
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
            (SpanType::WriteBlocked, "write.blocked"),
            (SpanType::TestRun, "test.run"),
            (SpanType::GatewayInvoke, "gateway.invoke"),
            (SpanType::Abstain, "abstain"),
        ];

        assert_eq!(SpanType::ALL.len(), 8);

        for (variant, wire) in expected {
            let json = serde_json::to_string(&variant).expect("serialize span type");
            assert_eq!(json, format!("\"{wire}\""));
            assert_eq!(variant.as_str(), wire);

            let parsed: SpanType = serde_json::from_str(&json).expect("deserialize span type");
            assert_eq!(parsed, variant);
        }
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
