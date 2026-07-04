//! Integration tests for the mechanical subtree partitioner (aoa-d6t.26):
//! workspace-manifest discovery, longest-prefix path attribution, and
//! per-subtree metric aggregation over synthetic traces.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::{json, Map, Value};
use tempfile::TempDir;

use aoa_metrics::{
    compute_subtree_metrics, discover_partition, IndexQuality, MetricInputRef, SubtreePartition,
    SymbolGraph, TransformMap, WorkspaceSource,
};
use aoa_trace::{Span, SpanSource, SpanType, Trace};

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

// --- discovery: Cargo workspaces ---------------------------------------------

#[test]
fn cargo_glob_members_expand_to_dirs_with_manifest() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n",
    );
    write(dir.path(), "crates/core/Cargo.toml", "[package]\n");
    write(dir.path(), "crates/legacy/Cargo.toml", "[package]\n");
    // No Cargo.toml: a glob-expanded entry must carry the ecosystem manifest.
    fs::create_dir_all(dir.path().join("crates/not-a-crate")).unwrap();

    let partition = discover_partition(dir.path()).unwrap();
    assert_eq!(partition.source(), WorkspaceSource::CargoWorkspace);
    let mut members: Vec<&str> = partition.members().iter().map(String::as_str).collect();
    members.sort_unstable();
    assert_eq!(members, ["crates/core", "crates/legacy"]);
    assert!(partition.is_partitioned());
}

#[test]
fn cargo_explicit_nested_members_attribute_by_longest_prefix() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"app\", \"app/plugin\"]\n",
    );

    let partition = discover_partition(dir.path()).unwrap();
    assert_eq!(partition.attribute("app/src/main.rs"), Some("app"));
    assert_eq!(
        partition.attribute("app/plugin/src/lib.rs"),
        Some("app/plugin"),
        "nested member wins by longest prefix"
    );
    // Prefix must respect path-segment boundaries.
    assert_eq!(partition.attribute("app-other/src/lib.rs"), None);
}

#[test]
fn cargo_exclude_filters_expanded_members() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/legacy\"]\n",
    );
    write(dir.path(), "crates/core/Cargo.toml", "[package]\n");
    write(dir.path(), "crates/legacy/Cargo.toml", "[package]\n");

    let partition = discover_partition(dir.path()).unwrap();
    assert_eq!(
        partition.members(),
        &["crates/core".to_string()],
        "excluded member is dropped"
    );
}

#[test]
fn malformed_cargo_workspace_manifest_errors() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Cargo.toml", "[workspace\nmembers = not toml");

    let err = discover_partition(dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("Cargo.toml"),
        "error names the manifest: {err}"
    );
}

// --- discovery: npm / pnpm / go ----------------------------------------------

#[test]
fn npm_workspaces_array_form() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "package.json",
        r#"{"name":"root","workspaces":["packages/*"]}"#,
    );
    write(dir.path(), "packages/web/package.json", "{}");
    write(dir.path(), "packages/api/package.json", "{}");

    let partition = discover_partition(dir.path()).unwrap();
    assert_eq!(partition.source(), WorkspaceSource::NpmWorkspaces);
    let mut members: Vec<&str> = partition.members().iter().map(String::as_str).collect();
    members.sort_unstable();
    assert_eq!(members, ["packages/api", "packages/web"]);
}

#[test]
fn npm_workspaces_object_form() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "package.json",
        r#"{"workspaces":{"packages":["apps/one"]}}"#,
    );

    let partition = discover_partition(dir.path()).unwrap();
    assert_eq!(partition.source(), WorkspaceSource::NpmWorkspaces);
    assert_eq!(partition.members(), &["apps/one".to_string()]);
}

#[test]
fn pnpm_packages_with_exclusion_pattern() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "pnpm-workspace.yaml",
        "packages:\n  - 'packages/*'\n  - '!packages/internal'\n",
    );
    write(dir.path(), "packages/web/package.json", "{}");
    write(dir.path(), "packages/internal/package.json", "{}");

    let partition = discover_partition(dir.path()).unwrap();
    assert_eq!(partition.source(), WorkspaceSource::PnpmWorkspace);
    assert_eq!(partition.members(), &["packages/web".to_string()]);
}

#[test]
fn pnpm_double_star_matches_nested_package_dirs() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "pnpm-workspace.yaml",
        "packages:\n  - 'apps/**'\n",
    );
    write(dir.path(), "apps/web/package.json", "{}");
    write(dir.path(), "apps/web/admin/package.json", "{}");
    // No package.json: not a member even though it matches the glob.
    fs::create_dir_all(dir.path().join("apps/assets")).unwrap();

    let partition = discover_partition(dir.path()).unwrap();
    let mut members: Vec<&str> = partition.members().iter().map(String::as_str).collect();
    members.sort_unstable();
    assert_eq!(members, ["apps/web", "apps/web/admin"]);
    // Nested members attribute by longest prefix.
    assert_eq!(
        partition.attribute("apps/web/admin/src/index.ts"),
        Some("apps/web/admin")
    );
    assert_eq!(
        partition.attribute("apps/web/src/index.ts"),
        Some("apps/web")
    );
}

#[test]
fn go_work_use_directives_block_and_single() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "go.work",
        "go 1.22\n\nuse ./svc/auth\n\nuse (\n\t./svc/billing\n\t. // the root module\n)\n",
    );

    let partition = discover_partition(dir.path()).unwrap();
    assert_eq!(partition.source(), WorkspaceSource::GoWork);
    let mut members: Vec<&str> = partition.members().iter().map(String::as_str).collect();
    members.sort_unstable();
    assert_eq!(members, [".", "svc/auth", "svc/billing"]);
}

// --- discovery: fallbacks and precedence --------------------------------------

#[test]
fn no_manifest_yields_single_implicit_root() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "src/lib.rs", "");

    let partition = discover_partition(dir.path()).unwrap();
    assert_eq!(partition.source(), WorkspaceSource::ImplicitRoot);
    assert_eq!(partition.members(), &[".".to_string()]);
    assert!(!partition.is_partitioned());
    assert_eq!(partition.attribute("src/lib.rs"), Some("."));
}

#[test]
fn cargo_without_workspace_table_falls_through_to_npm() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Cargo.toml", "[package]\nname = \"solo\"\n");
    write(
        dir.path(),
        "package.json",
        r#"{"workspaces":["packages/a"]}"#,
    );

    let partition = discover_partition(dir.path()).unwrap();
    assert_eq!(partition.source(), WorkspaceSource::NpmWorkspaces);
}

#[test]
fn cargo_workspace_takes_precedence_over_npm() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"rustpart\"]\n",
    );
    write(
        dir.path(),
        "package.json",
        r#"{"workspaces":["packages/a"]}"#,
    );

    let partition = discover_partition(dir.path()).unwrap();
    assert_eq!(partition.source(), WorkspaceSource::CargoWorkspace);
    assert_eq!(partition.members(), &["rustpart".to_string()]);
}

// --- attribution normalization -------------------------------------------------

#[test]
fn attribution_normalizes_dot_slash_and_repo_root_prefix() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/core\"]\n",
    );

    let partition = discover_partition(dir.path()).unwrap();
    assert_eq!(
        partition.attribute("./crates/core/src/lib.rs"),
        Some("crates/core")
    );
    let absolute = dir.path().join("crates/core/src/lib.rs");
    assert_eq!(
        partition.attribute(absolute.to_str().unwrap()),
        Some("crates/core"),
        "absolute path under the repo root is attributed"
    );
    assert_eq!(partition.attribute("docs/README.md"), None);
}

// --- per-subtree aggregation ----------------------------------------------------

fn span(span_type: SpanType, seq: u64, attrs: &[(&str, Value)]) -> Span {
    let mut attributes = Map::new();
    for (k, v) in attrs {
        attributes.insert((*k).to_string(), v.clone());
    }
    Span {
        span_type,
        source: SpanSource::Native,
        seq,
        attributes,
    }
}

fn two_member_partition(dir: &TempDir) -> SubtreePartition {
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/core\", \"crates/legacy\", \"crates/idle\"]\n",
    );
    discover_partition(dir.path()).unwrap()
}

fn graph() -> SymbolGraph {
    SymbolGraph {
        nodes: vec!["n".to_string()],
        edges: vec![],
        writable: BTreeSet::new(),
        quality: IndexQuality::BestEffort,
    }
}

#[test]
fn subtree_metrics_split_a_two_subtree_trace() {
    let dir = TempDir::new().unwrap();
    let partition = two_member_partition(&dir);

    // core: one miss then the gold hit (first-relevant = 2).
    // legacy: gold hit immediately (first-relevant = 1) plus an edit.
    let trace = Trace {
        spans: vec![
            span(
                SpanType::FileRead,
                1,
                &[("path", json!("crates/core/src/other.rs"))],
            ),
            span(
                SpanType::FileRead,
                2,
                &[("path", json!("crates/core/src/gold.rs"))],
            ),
            span(
                SpanType::FileRead,
                3,
                &[("path", json!("crates/legacy/src/gold.rs"))],
            ),
            span(
                SpanType::WriteAttempt,
                4,
                &[("path", json!("crates/legacy/src/gold.rs"))],
            ),
            // Unattributable: no path attribute at all.
            span(SpanType::RetrievalSearch, 5, &[]),
            // Outside every member.
            span(SpanType::FileRead, 6, &[("path", json!("docs/README.md"))]),
        ],
    };

    let gold_set: BTreeSet<String> = ["crates/core/src/gold.rs", "crates/legacy/src/gold.rs"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let edited_files: BTreeSet<String> = ["crates/legacy/src/gold.rs".to_string()].into();
    let solutions = vec![
        ["crates/legacy/src/gold.rs".to_string()].into(),
        [
            "crates/legacy/src/gold.rs".to_string(),
            "crates/core/src/gold.rs".to_string(),
        ]
        .into(),
    ];
    let transform = TransformMap::default();
    let invariant_set = BTreeSet::new();
    let graph = graph();

    let input = MetricInputRef {
        trace: &trace,
        gold_set: &gold_set,
        invariant_set: &invariant_set,
        transform: &transform,
        edited_files: &edited_files,
        accepted_solutions: &solutions,
        graph: &graph,
        k: 2,
        held_out_success: true,
    };

    let rows = compute_subtree_metrics(input, &partition);

    // Only active subtrees appear, in lexicographic order; crates/idle is absent.
    let names: Vec<&str> = rows.iter().map(|r| r.subtree.as_str()).collect();
    assert_eq!(names, ["crates/core", "crates/legacy"]);

    let core = &rows[0];
    assert_eq!(core.attributed_span_count, 2);
    assert_eq!(core.edited_file_count, 0);
    assert_eq!(
        core.retrieval_locality
            .tool_calls_to_first_relevant_artifact,
        Some(2),
        "core's first gold hit comes after one miss, on the filtered trace"
    );

    let legacy = &rows[1];
    assert_eq!(legacy.attributed_span_count, 2);
    assert_eq!(legacy.edited_file_count, 1);
    assert_eq!(
        legacy
            .retrieval_locality
            .tool_calls_to_first_relevant_artifact,
        Some(1)
    );
    // Edit-locality on the legacy-filtered sets: F_edit=1, intersection=1, union=1.
    let edit = legacy.edit_locality.as_ref().expect("2 solutions given");
    assert_eq!(edit.f_edit_size, 1);
    assert_eq!(edit.intersection_size, 1);
    assert_eq!(edit.union_size, 1);
}

#[test]
fn subtree_metrics_surface_insufficient_solutions_instead_of_fabricating() {
    let dir = TempDir::new().unwrap();
    let partition = two_member_partition(&dir);

    let trace = Trace {
        spans: vec![span(
            SpanType::WriteAttempt,
            1,
            &[("path", json!("crates/core/src/lib.rs"))],
        )],
    };
    let gold_set = BTreeSet::new();
    let edited_files: BTreeSet<String> = ["crates/core/src/lib.rs".to_string()].into();
    let solutions: Vec<BTreeSet<String>> = vec![["crates/core/src/lib.rs".to_string()].into()];
    let transform = TransformMap::default();
    let invariant_set = BTreeSet::new();
    let graph = graph();

    let input = MetricInputRef {
        trace: &trace,
        gold_set: &gold_set,
        invariant_set: &invariant_set,
        transform: &transform,
        edited_files: &edited_files,
        accepted_solutions: &solutions,
        graph: &graph,
        k: 2,
        held_out_success: true,
    };

    let rows = compute_subtree_metrics(input, &partition);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].edit_locality.is_none());
    assert!(rows[0]
        .edit_locality_unavailable
        .as_deref()
        .unwrap()
        .contains("insufficient"));
}

#[test]
fn subtree_metrics_anchor_gold_through_the_transform_map() {
    let dir = TempDir::new().unwrap();
    let partition = two_member_partition(&dir);

    // Gold is named by its base-repo path; the trace touches the migrated path.
    let mut transform = TransformMap::default();
    transform.base_to_migrated.insert(
        "crates/core/src/base.rs".to_string(),
        "crates/core/src/migrated.rs".to_string(),
    );
    let gold_set: BTreeSet<String> = ["crates/core/src/base.rs".to_string()].into();

    let trace = Trace {
        spans: vec![span(
            SpanType::FileRead,
            1,
            &[("path", json!("crates/core/src/migrated.rs"))],
        )],
    };
    let edited_files = BTreeSet::new();
    let solutions = vec![BTreeSet::new(), BTreeSet::new()];
    let invariant_set = BTreeSet::new();
    let graph = graph();

    let input = MetricInputRef {
        trace: &trace,
        gold_set: &gold_set,
        invariant_set: &invariant_set,
        transform: &transform,
        edited_files: &edited_files,
        accepted_solutions: &solutions,
        graph: &graph,
        k: 2,
        held_out_success: true,
    };

    let rows = compute_subtree_metrics(input, &partition);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].subtree, "crates/core");
    assert_eq!(
        rows[0]
            .retrieval_locality
            .tool_calls_to_first_relevant_artifact,
        Some(1),
        "anchored (migrated) gold path matches the trace and the subtree"
    );
}
