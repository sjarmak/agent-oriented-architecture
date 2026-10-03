use std::collections::BTreeSet;
use std::path::Path;

use aoa_scip_graph::index_best_effort;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("fixture path has a parent")).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn functions(count: usize) -> String {
    (0..count)
        .map(|n| format!("def shipped_{n}():\n    pass\n\n"))
        .collect()
}

fn indexed_nodes(repo: &Path) -> Vec<String> {
    index_best_effort(repo).expect("index repo").graph.nodes
}

#[test]
fn dependency_install_directory_contributes_no_symbols() {
    let repo = TempDir::new().unwrap();
    write(repo.path(), "src/a.py", "def own():\n    pass\n");
    write(repo.path(), "node_modules/dep/gyp/b.py", &functions(10));

    let indexed = index_best_effort(repo.path()).expect("index repo");

    assert_eq!(indexed.graph.nodes, vec!["src.a.own".to_string()]);
    assert_eq!(
        indexed.graph.writable,
        BTreeSet::from(["src.a.own".to_string()])
    );
    assert_eq!(
        indexed.graph.node_paths.keys().collect::<Vec<_>>(),
        vec!["src.a.own"]
    );
}

#[test]
fn every_dependency_and_build_directory_is_skipped_at_any_depth() {
    for dir in [
        "node_modules",
        "venv",
        "site-packages",
        "vendor",
        "target",
        "build",
    ] {
        let repo = TempDir::new().unwrap();
        write(repo.path(), "src/a.py", "def own():\n    pass\n");
        write(repo.path(), &format!("{dir}/top.py"), &functions(2));
        write(
            repo.path(),
            &format!("pkg/lib/{dir}/deep.py"),
            &functions(2),
        );

        assert_eq!(
            indexed_nodes(repo.path()),
            vec!["src.a.own".to_string()],
            "{dir}/ must contribute nothing"
        );
    }
}

#[test]
fn gitignored_directory_contributes_no_symbols() {
    let repo = TempDir::new().unwrap();
    write(repo.path(), ".gitignore", "generated/\n*_pb2.py\n");
    write(repo.path(), "src/a.py", "def own():\n    pass\n");
    write(repo.path(), "src/schema_pb2.py", &functions(3));
    write(repo.path(), "generated/client.py", &functions(4));

    assert_eq!(indexed_nodes(repo.path()), vec!["src.a.own".to_string()]);
}

#[test]
fn nested_gitignore_applies_to_its_own_subtree_only() {
    let repo = TempDir::new().unwrap();
    write(repo.path(), "svc/.gitignore", "out/\n");
    write(repo.path(), "svc/out/emitted.py", &functions(2));
    write(repo.path(), "svc/main.py", "def serve():\n    pass\n");
    write(repo.path(), "out/kept.py", "def kept():\n    pass\n");

    assert_eq!(
        indexed_nodes(repo.path()),
        vec!["out.kept.kept".to_string(), "svc.main.serve".to_string()]
    );
}

#[test]
fn source_file_sharing_a_skipped_directory_name_is_still_indexed() {
    let repo = TempDir::new().unwrap();
    write(repo.path(), "build.py", "def compile_all():\n    pass\n");
    write(repo.path(), "tools/vendor.py", "def sync():\n    pass\n");

    assert_eq!(
        indexed_nodes(repo.path()),
        vec![
            "build.compile_all".to_string(),
            "tools.vendor.sync".to_string()
        ]
    );
}

#[test]
fn repo_root_named_like_a_skipped_directory_is_still_indexed() {
    let parent = TempDir::new().unwrap();
    let repo = parent.path().join("build");
    write(&repo, "a.py", "def own():\n    pass\n");

    assert_eq!(indexed_nodes(&repo), vec!["a.own".to_string()]);
}
