use aoa_codeprobe_shim::{parse_transcript, GenericLogBackend, TraceBackend};
use aoa_trace::{SpanType, TRACE_SCHEMA};
use hegel::{generators as gs, TestCase};
use serde_json::{json, Value};

fn schema_validator() -> jsonschema::Validator {
    let schema: Value = serde_json::from_str(TRACE_SCHEMA).expect("schema is valid JSON");
    jsonschema::draft202012::options()
        .build(&schema)
        .expect("schema compiles")
}

fn assert_schema_valid(trace: &aoa_trace::Trace) {
    let value = serde_json::to_value(trace).expect("trace serializes");
    let validator = schema_validator();
    let errors: Vec<_> = validator
        .iter_errors(&value)
        .map(|err| err.to_string())
        .collect();
    assert!(errors.is_empty(), "schema errors: {errors:?}");
}

#[test]
fn both_producers_emit_schema_valid_targeted_traces() {
    let reconstructed = GenericLogBackend
        .parse("search needle\nread src/lib.rs\nwrite src/lib.rs\ntest cargo test\ngateway graph\n")
        .expect("reconstructed trace parses");
    assert_schema_valid(&reconstructed.trace);

    let native = parse_transcript(
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"r","name":"Read","input":{"file_path":"src/lib.rs"}},{"type":"tool_use","id":"s","name":"Grep","input":{"pattern":"needle"}},{"type":"tool_use","id":"w","name":"Write","input":{"file_path":"src/lib.rs"}},{"type":"tool_use","id":"t","name":"Bash","input":{"command":"cargo test"}},{"type":"tool_use","id":"g","name":"mcp__graph__lookup","input":{}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"w","is_error":false}]}}"#,
    )
    .expect("native trace parses");
    assert_schema_valid(&native.trace);
}

#[test]
fn reconstructed_targetless_mapped_line_names_the_line() {
    for verb in [
        "search",
        "read",
        "write",
        "committed",
        "blocked",
        "test",
        "gateway",
    ] {
        let err = GenericLogBackend.parse(verb).unwrap_err();
        let rendered = err.to_string();
        assert!(rendered.contains("line 1"), "{verb}: {rendered}");
        assert!(rendered.contains(verb), "{verb}: {rendered}");
    }
}

#[test]
fn native_targetless_nonwrite_tools_fail_loudly() {
    for (name, input) in [("Read", json!({})), ("Grep", json!({}))] {
        let transcript = json!({
            "type": "assistant",
            "message": {
                "content": [{"type": "tool_use", "id": "tool", "name": name, "input": input}]
            }
        })
        .to_string();
        let err = parse_transcript(&transcript).unwrap_err();
        let rendered = err.to_string();
        assert!(rendered.contains(name), "{name}: {rendered}");
        assert!(rendered.contains("target"), "{name}: {rendered}");
    }
}

#[hegel::test]
fn every_unusable_native_write_target_is_preserved_as_unresolved(tc: TestCase) {
    let name = tc.draw(gs::sampled_from(vec![
        "Edit",
        "Write",
        "MultiEdit",
        "NotebookEdit",
    ]));
    let unusable = match tc.draw(gs::sampled_from(vec![0_u8, 1, 2, 3, 4])) {
        0 => Value::Null,
        1 => json!(false),
        2 => json!(7),
        3 => json!(""),
        _ => json!("  "),
    };
    let transcript = json!({
        "type": "assistant",
        "message": {
            "content": [{
                "type": "tool_use",
                "id": "tool",
                "name": name,
                "input": {
                    "file_path": unusable,
                    "notebook_path": null,
                    "path": null
                }
            }]
        }
    })
    .to_string();
    let result = parse_transcript(&transcript).expect("unusable native target converts");
    let span = &result.trace.spans[0];
    assert_eq!(span.span_type, SpanType::WriteAttempt);
    assert_eq!(span.attributes.get("tool"), Some(&json!(name)));
    assert_eq!(span.attributes.get("target_unresolved"), Some(&json!(true)));
    assert_schema_valid(&result.trace);
}

#[test]
fn targetless_write_error_result_preserves_blocked_lifecycle() {
    assert_targetless_write_result(true, SpanType::WriteBlocked);
}

#[test]
fn targetless_write_success_result_preserves_committed_lifecycle() {
    assert_targetless_write_result(false, SpanType::WriteCommitted);
}

fn assert_targetless_write_result(is_error: bool, expected: SpanType) {
    let transcript = format!(
        "{}\n{}",
        json!({
            "type": "assistant",
            "message": {
                "content": [{"type": "tool_use", "id": "write", "name": "Write", "input": {}}]
            }
        }),
        json!({
            "type": "user",
            "message": {
                "content": [{"type": "tool_result", "tool_use_id": "write", "is_error": is_error}]
            }
        })
    );
    let result = parse_transcript(&transcript).expect("targetless write result converts");
    assert_eq!(result.trace.spans.len(), 1);
    let span = &result.trace.spans[0];
    assert_eq!(span.span_type, expected);
    assert_eq!(span.attributes.get("target_unresolved"), Some(&json!(true)));
    assert_schema_valid(&result.trace);
}

#[test]
fn published_schema_still_rejects_missing_consumed_keys() {
    let validator = schema_validator();
    for span in [
        json!({"type": "retrieval.search", "source": "native", "seq": 0, "attributes": {}}),
        json!({"type": "file.read", "source": "native", "seq": 0, "attributes": {}}),
        json!({"type": "symbol.lookup", "source": "native", "seq": 0, "attributes": {}}),
        json!({"type": "test.run", "source": "native", "seq": 0, "attributes": {}}),
        json!({"type": "gateway.invoke", "source": "native", "seq": 0, "attributes": {}}),
    ] {
        assert!(!validator.is_valid(&json!({"spans": [span]})));
    }
}
