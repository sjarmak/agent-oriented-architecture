//! Pure tool-name -> [`SpanType`] classification.
//!
//! Separated from the event walk in [`crate::parse`] so the mapping rules can
//! be unit-tested in isolation and reviewed against the spec without dragging
//! in transcript-parsing state.

use aoa_trace::SpanType;
use serde_json::Value;

/// Classification of a single `tool_use` block.
///
/// `Span` carries the mapped span type, the attribute key under which the call's
/// target (path / query) is recorded, and the `input` field names to read that
/// target from (first present wins; different tools name the field differently).
/// `Unknown` means the tool name matched no documented rule — the caller records
/// it as a warning rather than silently emitting a span (see [`crate::parse`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Mapping {
    Span {
        span_type: SpanType,
        target_key: &'static str,
        target_fields: &'static [&'static str],
    },
    Unknown,
}

/// Mechanical heuristic: does a Bash command run a test suite?
///
/// This is a documented, deterministic substring check (not a semantic
/// judgment): the spec prescribes mapping `pytest` / `test.sh` Bash commands to
/// `test.run`. Common runners across ecosystems are matched. Anything else
/// stays a generic write/exec and is treated as an unmapped Bash command by the
/// caller.
///
/// Exposed as the canonical "is this a reproduction step?" detector so the live
/// enforcement hook (R7) classifies a Bash command exactly as the offline shim
/// does — one source of truth, no drift between the two paths.
pub fn bash_runs_tests(command: &str) -> bool {
    const TEST_MARKERS: [&str; 7] = [
        "pytest",
        "test.sh",
        "cargo test",
        "go test",
        "npm test",
        "npm run test",
        "jest",
    ];
    let lower = command.to_ascii_lowercase();
    TEST_MARKERS.iter().any(|m| lower.contains(m))
}

/// Map a tool-use block (name + input) to a [`Mapping`].
///
/// Rules (documented in the crate docs and the bead):
/// - `Grep` / `Glob` / any name containing `search` -> `retrieval.search` (target = query/pattern)
/// - `Read` -> `file.read` (target = path)
/// - `Edit` / `Write` / `MultiEdit` -> `write.attempt` (target = path)
/// - `Bash` whose command runs tests -> `test.run` (target = command)
/// - `mcp__*` -> `gateway.invoke` (target = the MCP tool name)
/// - anything else -> `Unknown` (recorded as a warning, never silently dropped)
///
/// `symbol.lookup` is intentionally never produced here: it is populated by
/// joining tool results to the SCIP graph (aoa-671), which may not exist yet.
pub(crate) fn classify(name: &str, input: &Value) -> Mapping {
    // MCP tools take precedence: a name may be e.g. mcp__server__grep_search
    // and must map to the gateway, not retrieval.
    if name.starts_with("mcp__") {
        // The MCP tool name is itself the meaningful target; the parser reads it
        // from the block name rather than an input field (hence no fields here).
        return Mapping::Span {
            span_type: SpanType::GatewayInvoke,
            target_key: "tool",
            target_fields: &[],
        };
    }

    match name {
        "Read" => Mapping::Span {
            span_type: SpanType::FileRead,
            target_key: "path",
            target_fields: &["file_path", "path"],
        },
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => Mapping::Span {
            span_type: SpanType::WriteAttempt,
            target_key: "path",
            target_fields: &["file_path", "notebook_path", "path"],
        },
        "Grep" | "Glob" => Mapping::Span {
            span_type: SpanType::RetrievalSearch,
            target_key: "query",
            target_fields: &["pattern", "query"],
        },
        "Bash" => {
            let command = input.get("command").and_then(Value::as_str).unwrap_or("");
            if bash_runs_tests(command) {
                Mapping::Span {
                    span_type: SpanType::TestRun,
                    target_key: "command",
                    target_fields: &["command"],
                }
            } else {
                // Non-test Bash is out of the 8-span vocabulary; surface it as
                // unknown so it is logged rather than miscategorised.
                Mapping::Unknown
            }
        }
        _ if name.to_ascii_lowercase().contains("search") => Mapping::Span {
            span_type: SpanType::RetrievalSearch,
            target_key: "query",
            target_fields: &["query", "pattern"],
        },
        _ => Mapping::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_maps_to_file_read_with_path() {
        let m = classify("Read", &json!({"file_path": "src/lib.rs"}));
        match m {
            Mapping::Span {
                span_type,
                target_key,
                target_fields,
            } => {
                assert_eq!(span_type, SpanType::FileRead);
                assert_eq!(target_key, "path");
                assert_eq!(target_fields, &["file_path", "path"]);
            }
            Mapping::Unknown => panic!("Read should map"),
        }
    }

    #[test]
    fn grep_and_glob_map_to_retrieval_search() {
        for name in ["Grep", "Glob"] {
            match classify(name, &json!({"pattern": "fn main"})) {
                Mapping::Span { span_type, .. } => {
                    assert_eq!(span_type, SpanType::RetrievalSearch, "{name}")
                }
                Mapping::Unknown => panic!("{name} should map"),
            }
        }
    }

    #[test]
    fn write_edit_multiedit_map_to_write_attempt() {
        for name in ["Edit", "Write", "MultiEdit"] {
            match classify(name, &json!({"file_path": "x"})) {
                Mapping::Span { span_type, .. } => {
                    assert_eq!(span_type, SpanType::WriteAttempt, "{name}")
                }
                Mapping::Unknown => panic!("{name} should map"),
            }
        }
    }

    #[test]
    fn bash_test_command_maps_to_test_run() {
        match classify("Bash", &json!({"command": "pytest -q"})) {
            Mapping::Span { span_type, .. } => assert_eq!(span_type, SpanType::TestRun),
            Mapping::Unknown => panic!("pytest Bash should map to test.run"),
        }
    }

    #[test]
    fn bash_non_test_command_is_unknown() {
        assert_eq!(
            classify("Bash", &json!({"command": "ls -la"})),
            Mapping::Unknown
        );
    }

    #[test]
    fn mcp_tool_maps_to_gateway_invoke() {
        match classify("mcp__server__do_thing", &json!({})) {
            Mapping::Span { span_type, .. } => assert_eq!(span_type, SpanType::GatewayInvoke),
            Mapping::Unknown => panic!("mcp__ should map to gateway.invoke"),
        }
    }

    #[test]
    fn mcp_search_tool_still_maps_to_gateway_not_retrieval() {
        match classify("mcp__cg__codegraph_search", &json!({"query": "x"})) {
            Mapping::Span { span_type, .. } => assert_eq!(span_type, SpanType::GatewayInvoke),
            Mapping::Unknown => panic!("mcp search should be gateway"),
        }
    }

    #[test]
    fn search_named_tool_maps_to_retrieval() {
        match classify("CodebaseSearch", &json!({"query": "x"})) {
            Mapping::Span { span_type, .. } => assert_eq!(span_type, SpanType::RetrievalSearch),
            Mapping::Unknown => panic!("*search* should map to retrieval"),
        }
    }

    #[test]
    fn unknown_tool_is_unknown() {
        assert_eq!(classify("WeirdCustomTool", &json!({})), Mapping::Unknown);
    }

    #[test]
    fn bash_test_detection_is_case_insensitive_and_covers_runners() {
        assert!(bash_runs_tests("CARGO TEST --all"));
        assert!(bash_runs_tests("./scripts/test.sh"));
        assert!(bash_runs_tests("go test ./..."));
        assert!(!bash_runs_tests("echo hello"));
    }
}
