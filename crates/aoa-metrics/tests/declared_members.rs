use std::fs;
use std::path::Path;

use tempfile::TempDir;

use aoa_metrics::{declared_members, discover_partition, SubtreeError, WorkspaceSource};

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn declared(settings_name: &str, settings: &str) -> Vec<String> {
    let dir = TempDir::new().unwrap();
    write(dir.path(), settings_name, settings);
    let declared = declared_members(dir.path());
    assert!(declared.failures.is_empty(), "{:?}", declared.failures);
    declared.dirs
}

#[test]
fn a_repo_with_no_workspace_manifest_declares_no_members() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "main.rs", "fn main() {}\n");
    let declared = declared_members(dir.path());
    assert!(declared.dirs.is_empty());
    assert!(declared.failures.is_empty());
}

#[test]
fn groovy_bare_include_maps_project_paths_to_dirs() {
    let members = declared(
        "settings.gradle",
        "rootProject.name = 'shop'\ninclude ':services:api', ':tools'\ninclude 'ops'\n",
    );
    assert_eq!(members, ["ops", "services/api", "tools"]);
}

#[test]
fn groovy_bare_include_continues_across_trailing_commas() {
    let members = declared(
        "settings.gradle",
        "include ':services:api',\n        ':services:billing',\n        ':tools'\nrootProject.name = 'shop'\n",
    );
    assert_eq!(members, ["services/api", "services/billing", "tools"]);
}

#[test]
fn kotlin_call_include_reads_every_argument() {
    let members = declared(
        "settings.gradle.kts",
        "rootProject.name = \"shop\"\ninclude(\":services:api\", \":ops\")\ninclude(\n    \":tools:lint\",\n)\n",
    );
    assert_eq!(members, ["ops", "services/api", "tools/lint"]);
}

#[test]
fn gradle_comments_and_other_directives_declare_nothing() {
    let members = declared(
        "settings.gradle.kts",
        "// include(\":commented\")\n/* include(\":blocked\")\n   include(\":also-blocked\") */\nincludeBuild(\"build-logic\")\nincludeFlat(\"sibling\")\npluginManagement { repositories { mavenCentral() } }\ninclude(\":real\")\n",
    );
    assert_eq!(members, ["real"]);
}

#[test]
fn gradle_interpolated_project_names_are_not_guessed() {
    let members = declared("settings.gradle", "include \":services:$name\", ':tools'\n");
    assert_eq!(members, ["tools"]);
}

#[test]
fn gradle_member_escaping_the_repo_root_is_an_error() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "settings.gradle", "include '../outside'\n");
    let declared = declared_members(dir.path());
    assert!(declared.dirs.is_empty(), "{:?}", declared.dirs);
    let [err] = declared.failures.as_slice() else {
        panic!("expected one failure, got {:?}", declared.failures);
    };
    assert!(matches!(err, SubtreeError::Manifest { .. }), "{err}");
    assert!(err.to_string().contains("settings.gradle"), "{err}");
}

#[test]
fn members_from_every_declaring_manifest_are_merged() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "pnpm-workspace.yaml",
        "packages:\n  - 'services/*'\n",
    );
    write(dir.path(), "services/web/package.json", "{}");
    write(
        dir.path(),
        "package.json",
        "{\"workspaces\": [\"tools/cli\"]}",
    );
    write(
        dir.path(),
        "settings.gradle.kts",
        "include(\":ops:deploy\")\n",
    );

    let declared = declared_members(dir.path());
    assert!(declared.failures.is_empty(), "{:?}", declared.failures);
    assert_eq!(declared.dirs, ["ops/deploy", "services/web", "tools/cli"]);
}

#[test]
fn gradle_settings_do_not_change_the_subtree_partition() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "settings.gradle", "include ':services:api'\n");
    let partition = discover_partition(dir.path()).unwrap();
    assert_eq!(partition.source(), WorkspaceSource::ImplicitRoot);
}

#[test]
fn a_malformed_manifest_costs_only_its_own_ecosystem_its_members() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "package.json", "{ \"name\": \"x\", }");
    write(dir.path(), "settings.gradle", "include ':tools'\n");
    write(dir.path(), "go.work", "use ./services/api\n");

    let declared = declared_members(dir.path());

    assert_eq!(declared.dirs, ["services/api", "tools"]);
    let [failure] = declared.failures.as_slice() else {
        panic!("expected one failure, got {:?}", declared.failures);
    };
    assert!(failure.to_string().contains("package.json"), "{failure}");
}

#[test]
fn gradle_block_comment_holding_a_url_does_not_swallow_the_includes() {
    let members = declared(
        "settings.gradle",
        "/* See https://example.test/docs */\ninclude ':services:api'\n// include ':commented'\ninclude ':tools' /* trailing */\n",
    );
    assert_eq!(members, ["services/api", "tools"]);
}

#[test]
fn gradle_escaped_quote_before_comment_punctuation_does_not_open_a_comment() {
    let members = declared(
        "settings.gradle.kts",
        "println(\"escaped \\\"/*\")\ninclude(\":real\")\n",
    );
    assert_eq!(members, ["real"]);
}
