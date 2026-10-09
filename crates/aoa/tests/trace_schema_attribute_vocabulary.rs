mod common;

use common::{read, workspace_root};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use syn::visit::Visit;
use syn::{Expr, ExprIndex, ExprMethodCall, Lit, Member};

fn rust_sources(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_path_buf()];
    }
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(path).expect("source directory is readable") {
        let path = entry.expect("source entry is readable").path();
        if path.is_dir() {
            sources.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            sources.push(path);
        }
    }
    sources
}

fn by_name_attribute_reads(source: &str) -> BTreeSet<String> {
    #[derive(Default)]
    struct AttributeReadVisitor {
        reads: BTreeSet<String>,
    }

    fn is_attributes(expr: &Expr) -> bool {
        matches!(
            expr,
            Expr::Field(field)
                if matches!(&field.member, Member::Named(member) if member == "attributes")
        )
    }

    fn string_literal(expr: &Expr) -> Option<String> {
        match expr {
            Expr::Lit(literal) => match &literal.lit {
                Lit::Str(value) => Some(value.value()),
                _ => None,
            },
            _ => None,
        }
    }

    impl<'ast> Visit<'ast> for AttributeReadVisitor {
        fn visit_expr_method_call(&mut self, call: &'ast ExprMethodCall) {
            if call.method == "get" && is_attributes(&call.receiver) {
                if let Some(key) = call.args.first().and_then(string_literal) {
                    self.reads.insert(key);
                }
            }
            syn::visit::visit_expr_method_call(self, call);
        }

        fn visit_expr_index(&mut self, index: &'ast ExprIndex) {
            if is_attributes(&index.expr) {
                if let Some(key) = string_literal(&index.index) {
                    self.reads.insert(key);
                }
            }
            syn::visit::visit_expr_index(self, index);
        }
    }

    let file = syn::parse_file(source).expect("Rust source parses");
    let mut visitor = AttributeReadVisitor::default();
    visitor.visit_file(&file);
    visitor.reads
}

#[test]
fn attribute_read_scan_finds_method_and_index_syntax() {
    let source = r#"
        fn read(span: &Span) {
            let _ = span.attributes.get("path");
            let _ = span.attributes["symbol"];
        }
    "#;
    let expected = BTreeSet::from(["path".to_string(), "symbol".to_string()]);

    assert_eq!(by_name_attribute_reads(source), expected);
}

#[test]
fn every_by_name_attribute_read_is_declared_in_the_trace_schema() {
    let root = workspace_root();
    let targets = [
        root.join("crates/aoa-metrics/src"),
        root.join("crates/aoa-enforce/src"),
        root.join("crates/aoa-observe-shim/src"),
        root.join("crates/aoa/src/commands/eval_run.rs"),
    ];
    let reads: BTreeSet<String> = targets
        .iter()
        .flat_map(|target| rust_sources(target))
        .flat_map(|path| {
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
            by_name_attribute_reads(&source)
        })
        .collect();
    assert!(!reads.is_empty(), "attribute reader scan found nothing");

    let schema: serde_json::Value =
        serde_json::from_str(&read("crates/aoa-trace/schema/trace.schema.json"))
            .expect("trace schema is valid JSON");
    let declared: BTreeSet<String> = schema["$defs"]["span"]["properties"]["attributes"]
        ["properties"]
        .as_object()
        .expect("schema declares attribute properties")
        .keys()
        .cloned()
        .collect();
    let missing: Vec<&String> = reads.difference(&declared).collect();

    assert!(
        missing.is_empty(),
        "attribute keys read by name but absent from the published schema: {missing:?}"
    );
}
