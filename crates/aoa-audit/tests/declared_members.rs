use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use aoa_audit::{audit, navigability_sites, AuditConfig};

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn repo_with_readme() -> TempDir {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "README.md", "# repo\n");
    dir
}

fn sites(repo: &Path) -> BTreeSet<PathBuf> {
    navigability_sites(repo)
        .unwrap()
        .into_iter()
        .map(|site| site.strip_prefix(repo).unwrap().to_path_buf())
        .collect()
}

fn paths(rels: &[&str]) -> BTreeSet<PathBuf> {
    rels.iter().map(PathBuf::from).collect()
}

#[test]
fn pnpm_workspace_members_outside_container_dirs_are_package_roots() {
    let repo = repo_with_readme();
    write(
        repo.path(),
        "pnpm-workspace.yaml",
        "packages:\n  - 'services/*'\n  - 'tools/cli'\n",
    );
    write(repo.path(), "services/api/package.json", "{}");
    write(repo.path(), "services/web/package.json", "{}");
    write(repo.path(), "services/web/README.md", "# web\n");
    write(repo.path(), "tools/cli/package.json", "{}");

    assert_eq!(sites(repo.path()), paths(&["services/api", "tools/cli"]));
}

#[test]
fn package_json_workspaces_members_are_package_roots() {
    let repo = repo_with_readme();
    write(
        repo.path(),
        "package.json",
        "{\"workspaces\": {\"packages\": [\"ops/*\", \"services/deep/worker\"]}}",
    );
    write(repo.path(), "ops/deploy/package.json", "{}");
    write(repo.path(), "services/deep/worker/package.json", "{}");

    assert_eq!(
        sites(repo.path()),
        paths(&["ops/deploy", "services/deep/worker"])
    );
}

#[test]
fn settings_gradle_includes_are_package_roots() {
    let repo = repo_with_readme();
    write(
        repo.path(),
        "settings.gradle",
        "rootProject.name = 'shop'\ninclude ':services:api', ':tools'\n",
    );
    write(repo.path(), "services/api/build.gradle", "");
    write(repo.path(), "tools/build.gradle", "");

    assert_eq!(sites(repo.path()), paths(&["services/api", "tools"]));
}

#[test]
fn settings_gradle_kts_includes_are_package_roots() {
    let repo = repo_with_readme();
    write(
        repo.path(),
        "settings.gradle.kts",
        "include(\":services:api\")\ninclude(\":ops:deploy\")\n",
    );
    write(repo.path(), "services/api/build.gradle.kts", "");
    write(repo.path(), "ops/deploy/build.gradle.kts", "");
    write(repo.path(), "ops/deploy/README.md", "# deploy\n");

    assert_eq!(sites(repo.path()), paths(&["services/api"]));
}

#[test]
fn a_kotlin_dsl_build_file_marks_a_container_member() {
    let repo = repo_with_readme();
    write(repo.path(), "libs/core/build.gradle.kts", "");

    assert_eq!(sites(repo.path()), paths(&["libs/core"]));
}

#[test]
fn a_member_found_by_both_rules_is_counted_once() {
    let repo = repo_with_readme();
    write(
        repo.path(),
        "pnpm-workspace.yaml",
        "packages:\n  - 'packages/*'\n",
    );
    write(repo.path(), "packages/ui/package.json", "{}");

    let found = navigability_sites(repo.path()).unwrap();
    assert_eq!(found, vec![repo.path().join("packages/ui")]);
}

#[test]
fn undeclared_fixture_packages_stay_excluded() {
    let repo = repo_with_readme();
    write(
        repo.path(),
        "pnpm-workspace.yaml",
        "packages:\n  - 'services/*'\n",
    );
    write(repo.path(), "services/api/package.json", "{}");
    write(
        repo.path(),
        "services/api/tests/fixtures/sample/package.json",
        "{}",
    );
    write(repo.path(), "tests/fixtures/broken-pkg/package.json", "{}");
    write(repo.path(), "examples/demo/package.json", "{}");
    write(repo.path(), "settings.gradle", "include ':services:api'\n");
    write(repo.path(), "tests/fixtures/gradle-app/build.gradle", "");

    assert_eq!(sites(repo.path()), paths(&["services/api"]));
}

#[test]
fn a_recursive_declaration_never_reaches_dependency_installs_or_hidden_dirs() {
    let repo = repo_with_readme();
    write(
        repo.path(),
        "pnpm-workspace.yaml",
        "packages:\n  - 'services/**'\n  - '.cache/pkg'\n",
    );
    write(repo.path(), "services/api/package.json", "{}");
    write(
        repo.path(),
        "services/api/node_modules/left-pad/package.json",
        "{}",
    );
    write(repo.path(), ".cache/pkg/package.json", "{}");

    assert_eq!(sites(repo.path()), paths(&["services/api"]));
}

#[test]
fn a_declared_member_that_does_not_exist_is_not_a_package_root() {
    let repo = repo_with_readme();
    write(repo.path(), "settings.gradle", "include ':services:gone'\n");

    assert!(sites(repo.path()).is_empty());
}

#[cfg(unix)]
#[test]
fn a_declared_member_behind_a_symlink_is_not_a_package_root() {
    let outside = TempDir::new().unwrap();
    write(outside.path(), "api/package.json", "{}");
    let repo = repo_with_readme();
    write(
        repo.path(),
        "package.json",
        "{\"workspaces\": [\"services/api\"]}",
    );
    std::os::unix::fs::symlink(outside.path(), repo.path().join("services")).unwrap();

    assert!(sites(repo.path()).is_empty());
    assert!(!outside.path().join("api/README.md").exists());
}

#[test]
fn a_malformed_member_declaration_keeps_conventional_roots_and_warns() {
    let repo = repo_with_readme();
    write(repo.path(), "settings.gradle", "include '../outside'\n");
    write(repo.path(), "packages/ui/package.json", "{}");
    write(repo.path(), "main.rs", "fn main() {}\n");

    assert_eq!(sites(repo.path()), paths(&["packages/ui"]));

    let report = audit(repo.path(), &AuditConfig::default()).unwrap();
    let warning = report
        .subtree_discovery_warning
        .as_deref()
        .expect("the member discovery failure must be surfaced");
    assert!(warning.contains("settings.gradle"), "{warning}");
}

#[test]
fn a_malformed_manifest_keeps_the_members_other_ecosystems_declare() {
    let repo = repo_with_readme();
    write(repo.path(), "package.json", "{ \"name\": \"x\", }");
    write(repo.path(), "settings.gradle", "include ':services:api'\n");
    write(repo.path(), "services/api/build.gradle", "");

    assert_eq!(sites(repo.path()), paths(&["services/api"]));

    let report = audit(repo.path(), &AuditConfig::default()).unwrap();
    let warning = report
        .subtree_discovery_warning
        .as_deref()
        .expect("the member discovery failure must be surfaced");
    assert!(warning.contains("package.json"), "{warning}");
}

#[test]
fn both_discovery_failures_are_surfaced_together() {
    let repo = repo_with_readme();
    write(repo.path(), "Cargo.toml", "[workspace\n");
    write(repo.path(), "settings.gradle", "include '../outside'\n");

    let report = audit(repo.path(), &AuditConfig::default()).unwrap();
    let warning = report
        .subtree_discovery_warning
        .as_deref()
        .expect("both discovery failures must be surfaced");
    assert!(warning.contains("subtree discovery failed"), "{warning}");
    assert!(warning.contains("settings.gradle"), "{warning}");
}
