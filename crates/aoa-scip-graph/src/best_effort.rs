use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use aoa_metrics::{IndexQuality, SymbolGraph};

use crate::error::ScipGraphError;
use crate::index::IndexedRepo;

/// Scan a repo's Python source into a best-effort, low-confidence [`IndexedRepo`].
///
/// This is a heuristic line scanner, not a precise index: it recognizes
/// top-level `def` definitions and `from <module> import <name>` statements, then
/// records a reference edge whenever an imported name is called inside a function
/// body. It is deliberately honest about its limits — nested defs, aliases, and
/// dynamic dispatch are out of scope — which is exactly why the result is tagged
/// [`IndexQuality::BestEffort`] (R15 low confidence).
///
/// The writable set is every defined node (a best-effort index cannot
/// distinguish edit policy), and `G_t`/`I_t` are left empty: gold and invariant
/// curation needs precise resolution this source does not provide.
pub fn index_best_effort(repo_dir: &Path) -> Result<IndexedRepo, ScipGraphError> {
    let mut nodes: BTreeSet<String> = BTreeSet::new();
    let mut edges: BTreeSet<(String, String)> = BTreeSet::new();

    let mut files: Vec<std::path::PathBuf> = Vec::new();
    collect_py_files(repo_dir, &mut files)?;
    files.sort();

    for file in &files {
        let rel = file.strip_prefix(repo_dir).unwrap_or(file);
        let module = module_name(rel);
        let source = std::fs::read_to_string(file).map_err(|source| ScipGraphError::Io {
            path: file.display().to_string(),
            source,
        })?;
        scan_module(&module, &source, &mut nodes, &mut edges);
    }

    let writable: BTreeSet<String> = nodes.clone();
    let graph = SymbolGraph {
        nodes: nodes.into_iter().collect(),
        edges: edges.into_iter().collect(),
        writable,
        quality: IndexQuality::BestEffort,
    };

    Ok(IndexedRepo {
        graph,
        gold_set: BTreeSet::new(),
        invariant_set: BTreeSet::new(),
        degrade_reason: None,
    })
}

/// Recursively collect `.py` files under `dir`, skipping hidden directories.
fn collect_py_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<(), ScipGraphError> {
    let entries = std::fs::read_dir(dir).map_err(|source| ScipGraphError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| ScipGraphError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_py_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "py") {
            out.push(path);
        }
    }
    Ok(())
}

/// Map a relative file path `pkg/auth.py` to its module name `pkg.auth`.
fn module_name(rel: &Path) -> String {
    let without_ext = rel.with_extension("");
    without_ext
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(".")
}

/// Scan one module's source, adding its defined nodes and reference edges.
///
/// Two passes: the first records imports and all top-level `def` symbols into a
/// name→fully-qualified-symbol table, so a call to a function defined later in
/// the file still resolves (forward reference). The second walks the lines again,
/// tracking the enclosing `def` and emitting an edge for each call to a known
/// name.
fn scan_module(
    module: &str,
    source: &str,
    nodes: &mut BTreeSet<String>,
    edges: &mut BTreeSet<(String, String)>,
) {
    let mut resolved: BTreeMap<String, String> = BTreeMap::new();

    for line in source.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if let Some(rest) = trimmed.strip_prefix("from ") {
            record_import(rest, &mut resolved);
        } else if indent == 0 {
            if let Some(name) = parse_def(trimmed) {
                let symbol = format!("{module}.{name}");
                nodes.insert(symbol.clone());
                resolved.insert(name.to_string(), symbol);
            }
        }
    }

    let mut current_def: Option<String> = None;
    for line in source.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();

        if indent == 0 {
            if let Some(name) = parse_def(trimmed) {
                current_def = Some(format!("{module}.{name}"));
                continue;
            }
            // A statement back at column 0 that is not a def ends the function.
            if !trimmed.is_empty() {
                current_def = None;
            }
        }

        if let Some(enclosing) = &current_def {
            for callee in called_names(trimmed) {
                if let Some(target) = resolved.get(callee) {
                    if target != enclosing {
                        edges.insert((enclosing.clone(), target.clone()));
                    }
                }
            }
        }
    }
}

/// Parse `from pkg.tokens import issue_token` into name→symbol bindings.
fn record_import(rest: &str, imports: &mut BTreeMap<String, String>) {
    let Some((module_part, names_part)) = rest.split_once(" import ") else {
        return;
    };
    let module = module_part.trim();
    for name in names_part.split(',') {
        let name = name.trim();
        // Aliases (`x as y`) are out of scope for a best-effort scan.
        if name.is_empty() || name.contains(" as ") {
            continue;
        }
        imports.insert(name.to_string(), format!("{module}.{name}"));
    }
}

/// Extract the function name from a `def name(...)` line, if present.
fn parse_def(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix("def ")?;
    let name = rest.split('(').next()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Collect identifiers that appear immediately before a `(` on a line.
fn called_names(line: &str) -> Vec<&str> {
    let mut calls = Vec::new();
    let bytes = line.as_bytes();
    let mut start: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        let is_ident = b.is_ascii_alphanumeric() || b == b'_';
        if is_ident {
            if start.is_none() {
                start = Some(i);
            }
        } else {
            if b == b'(' {
                if let Some(s) = start {
                    calls.push(&line[s..i]);
                }
            }
            start = None;
        }
    }
    calls
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(module: &str, src: &str) -> (BTreeSet<String>, BTreeSet<(String, String)>) {
        let mut nodes = BTreeSet::new();
        let mut edges = BTreeSet::new();
        scan_module(module, src, &mut nodes, &mut edges);
        (nodes, edges)
    }

    #[test]
    fn module_name_joins_path_components() {
        assert_eq!(module_name(Path::new("pkg/auth.py")), "pkg.auth");
        assert_eq!(module_name(Path::new("top.py")), "top");
    }

    #[test]
    fn defs_become_nodes_and_imported_calls_become_edges() {
        let src = "from pkg.tokens import issue_token\n\
                   def login(user):\n    return issue_token(user)\n";
        let (nodes, edges) = scan("pkg.auth", src);
        assert!(nodes.contains("pkg.auth.login"));
        assert!(edges.contains(&("pkg.auth.login".into(), "pkg.tokens.issue_token".into())));
    }

    #[test]
    fn local_calls_resolve_within_the_module() {
        let src = "def issue_token(user):\n    return verify_secret(user)\n\
                   def verify_secret(user):\n    return user\n";
        let (nodes, edges) = scan("pkg.tokens", src);
        assert!(nodes.contains("pkg.tokens.issue_token"));
        assert!(nodes.contains("pkg.tokens.verify_secret"));
        assert!(edges.contains(&(
            "pkg.tokens.issue_token".into(),
            "pkg.tokens.verify_secret".into()
        )));
    }

    #[test]
    fn aliased_imports_are_skipped() {
        let mut imports = BTreeMap::new();
        record_import("pkg.x import foo as bar", &mut imports);
        assert!(imports.is_empty());
    }

    #[test]
    fn no_self_edge_when_a_function_names_itself() {
        let src = "def f(x):\n    return f(x)\n";
        let (_nodes, edges) = scan("m", src);
        assert!(edges.is_empty());
    }
}
