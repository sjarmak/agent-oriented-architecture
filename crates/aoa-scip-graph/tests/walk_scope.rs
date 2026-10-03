use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use aoa_scip_graph::{build_symbol_graph, index_best_effort, IndexSource, ScipGraphError};
use aoa_trace::IndexQuality;
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

#[test]
fn python_repo_with_a_checked_in_dependency_install_is_not_degraded() {
    let repo = TempDir::new().unwrap();
    write(repo.path(), "src/a.py", "def own():\n    pass\n");
    write(repo.path(), "src/b.py", "def other():\n    pass\n");
    for n in 0..40 {
        write(
            repo.path(),
            &format!("node_modules/dep/lib/m{n}.js"),
            "module.exports = 1;\n",
        );
    }
    write(repo.path(), "node_modules/dep/gyp/tool.py", &functions(3));

    let indexed = build_symbol_graph(IndexSource::BestEffort {
        repo_dir: repo.path(),
    });

    assert_eq!(indexed.graph.quality, IndexQuality::BestEffort);
    assert_eq!(indexed.degrade_reason, None);
    assert_eq!(
        indexed.graph.nodes,
        vec!["src.a.own".to_string(), "src.b.other".to_string()]
    );
    let coverage = indexed.coverage.expect("coverage");
    assert_eq!(
        coverage.indexed,
        BTreeMap::from([("Python".to_string(), 2)])
    );
    assert_eq!(coverage.unindexed, BTreeMap::new());
}

#[test]
fn ignored_and_dependency_sources_are_not_counted_as_unindexed() {
    let repo = TempDir::new().unwrap();
    write(repo.path(), ".gitignore", "generated/\n");
    write(repo.path(), "src/a.py", "def own():\n    pass\n");
    write(repo.path(), "web/app.ts", "export const app = 1;\n");
    for n in 0..30 {
        write(
            repo.path(),
            &format!("generated/client{n}.ts"),
            "export const c = 1;\n",
        );
    }
    for dir in ["venv", "site-packages", "vendor", "target", "build"] {
        for n in 0..10 {
            write(repo.path(), &format!("{dir}/pkg/f{n}.go"), "package pkg\n");
        }
    }

    let indexed = build_symbol_graph(IndexSource::BestEffort {
        repo_dir: repo.path(),
    });

    assert_eq!(indexed.graph.quality, IndexQuality::BestEffort);
    let coverage = indexed.coverage.expect("coverage");
    assert_eq!(
        coverage.indexed,
        BTreeMap::from([("Python".to_string(), 1)])
    );
    assert_eq!(
        coverage.unindexed,
        BTreeMap::from([("TypeScript".to_string(), 1)])
    );
}

#[test]
fn ignore_file_above_the_indexed_directory_does_not_hide_its_sources() {
    let outer = TempDir::new().unwrap();
    write(outer.path(), ".gitignore", "*.ts\n*.py\n");
    let repo = outer.path().join("snapshot");
    write(&repo, "src/a.py", "def own():\n    pass\n");
    write(&repo, "web/app.ts", "export const a = 1;\n");

    let indexed = index_best_effort(&repo).expect("index repo");

    assert_eq!(indexed.graph.nodes, vec!["src.a.own".to_string()]);
    let coverage = indexed.coverage.expect("best-effort coverage");
    assert_eq!(coverage.source_files(), 2);
}

#[test]
fn ignore_line_the_matcher_cannot_apply_fails_the_index_and_names_the_file() {
    let repo = TempDir::new().unwrap();
    write(repo.path(), "svc/.gitignore", "{foo\ngenerated/\n");
    write(repo.path(), "svc/a.py", "def own():\n    pass\n");
    write(repo.path(), "svc/generated/client.py", &functions(4));

    let err = index_best_effort(repo.path()).expect_err("unapplied ignore rule");
    assert!(err.to_string().contains(".gitignore"), "{err}");

    let degraded = build_symbol_graph(IndexSource::BestEffort {
        repo_dir: repo.path(),
    });
    assert_eq!(degraded.graph.quality, IndexQuality::Degraded);
    let reason = degraded.degrade_reason.expect("degrade reason");
    assert!(reason.contains(".gitignore"), "{reason}");
}

#[test]
fn ignore_file_that_cannot_be_read_fails_the_index_and_names_the_file() {
    let repo = TempDir::new().unwrap();
    write(repo.path(), "svc/a.py", "def own():\n    pass\n");
    write(repo.path(), "svc/generated/client.py", &functions(4));
    std::fs::create_dir(repo.path().join("svc/.gitignore")).unwrap();

    let err = index_best_effort(repo.path()).expect_err("unreadable ignore file");
    assert!(err.to_string().contains("svc/.gitignore"), "{err}");

    let degraded = build_symbol_graph(IndexSource::BestEffort {
        repo_dir: repo.path(),
    });
    assert_eq!(degraded.graph.quality, IndexQuality::Degraded);
    assert!(degraded.degrade_reason.is_some());
}

#[cfg(unix)]
#[test]
fn unreadable_ignore_file_under_a_symlinked_root_fails_the_index_and_names_the_file() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "repo/a.py", "def own():\n    pass\n");
    std::fs::create_dir(dir.path().join("repo/.gitignore")).unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(dir.path().join("repo"), &link).unwrap();

    let err = index_best_effort(&link).expect_err("unreadable ignore file");

    assert!(err.to_string().contains("link/.gitignore"), "{err}");
}

#[test]
fn ignore_file_that_is_not_text_fails_the_index_and_names_the_file() {
    let repo = TempDir::new().unwrap();
    write(repo.path(), "svc/a.py", "def own():\n    pass\n");
    std::fs::write(
        repo.path().join("svc/.gitignore"),
        b"generated/\n\xff\xfe\n",
    )
    .unwrap();

    let err = index_best_effort(repo.path()).expect_err("ignore file is not text");

    assert!(err.to_string().contains("svc/.gitignore"), "{err}");
}

#[test]
fn source_extension_is_recognized_whatever_its_letter_case() {
    let repo = TempDir::new().unwrap();
    write(repo.path(), "svc/own.PY", "def own():\n    pass\n");
    write(repo.path(), "web/view.TS", "export const view = 1;\n");

    let indexed = index_best_effort(repo.path()).unwrap();

    assert_eq!(indexed.graph.nodes, ["svc.own.own"]);
    let coverage = indexed.coverage.expect("best-effort coverage");
    assert_eq!(
        coverage.indexed,
        BTreeMap::from([("Python".to_string(), 1)])
    );
    assert_eq!(
        coverage.unindexed,
        BTreeMap::from([("TypeScript".to_string(), 1)])
    );
}

#[cfg(unix)]
#[test]
fn directory_that_cannot_be_listed_fails_the_index_and_names_the_directory() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TempDir::new().unwrap();
    write(repo.path(), "svc/a.py", "def own():\n    pass\n");
    let sealed = repo.path().join("svc/private");
    std::fs::create_dir(&sealed).unwrap();
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o100)).unwrap();
    let seal_took = std::fs::read_dir(&sealed).is_err();

    let result = index_best_effort(repo.path());

    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o755)).unwrap();
    if seal_took {
        match result.expect_err("unlistable directory") {
            ScipGraphError::Io { path, .. } => assert_eq!(path, sealed),
            other => panic!("expected an io error naming the directory, got {other}"),
        }
    }
}
