//! Answer-task oracle chain: the repo files an oracle-following agent would
//! consult, derived mechanically from the mined comprehension artifacts.
//!
//! Comprehension tasks (`comprehension-v1` ground truth + `consensus.v1`
//! divergence report) reference concrete repo files in three places:
//!
//! - a `file_list` answer in `ground_truth.json` (the files the agent must
//!   identify — e.g. every transitive importer of a module);
//! - the divergence report's `defining_file` and `consensus_files`;
//! - the divergence report's `symbol`, which names modules or file-qualified
//!   symbols (`pkg.mod_a->pkg.mod_b` for transitive-dependency questions,
//!   `tests/test_x.py::Class.method` for return-type questions, dotted
//!   `pkg.mod.member` otherwise).
//!
//! [`OracleChainFacts::resolve`] joins those references onto a repo file
//! universe (the SCIP index's document paths) with deterministic, purely
//! mechanical path matching. References that resolve to no universe file are
//! dropped — the chain is only usable where the symbol graph can measure it,
//! and the caller treats an empty resolution as excluded-with-reason, never as
//! a fabricated input.

use std::collections::BTreeSet;

/// The raw oracle-chain references read from a task's artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OracleChainFacts {
    /// `ground_truth.json` `answer` entries when `answer_type == "file_list"`.
    pub answer_files: BTreeSet<String>,
    /// `divergence_report.json` `consensus_files`.
    pub consensus_files: BTreeSet<String>,
    /// `divergence_report.json` `defining_file`, when non-empty.
    pub defining_file: Option<String>,
    /// `divergence_report.json` `symbol`, verbatim.
    pub symbol: Option<String>,
}

impl OracleChainFacts {
    /// Resolve every reference onto the repo file universe.
    ///
    /// Returns the oracle chain `O` as repo-relative paths drawn from
    /// `universe`. Empty when nothing resolves.
    pub fn resolve(&self, universe: &BTreeSet<String>) -> BTreeSet<String> {
        let mut chain = BTreeSet::new();

        let file_refs = self
            .answer_files
            .iter()
            .chain(self.consensus_files.iter())
            .chain(self.defining_file.iter());
        for reference in file_refs {
            if let Some(path) = resolve_repo_file(reference, universe) {
                chain.insert(path);
            }
        }

        if let Some(symbol) = &self.symbol {
            if let Some((a, b)) = symbol.split_once("->") {
                for module in [a.trim(), b.trim()] {
                    if let Some(path) = resolve_module(module, universe) {
                        chain.insert(path);
                    }
                }
            } else if let Some((path, _)) = symbol.split_once("::") {
                if let Some(path) = resolve_repo_file(path, universe) {
                    chain.insert(path);
                }
            } else if let Some(path) = resolve_module(symbol, universe) {
                chain.insert(path);
            }
        }

        chain
    }
}

/// Resolve a miner-spelled repo path onto the universe: exact match first,
/// else the shortest universe path ending with `/<path>` (the miner and the
/// indexer may root the same file differently, e.g. `src/`-layout repos).
fn resolve_repo_file(path: &str, universe: &BTreeSet<String>) -> Option<String> {
    if universe.contains(path) {
        return Some(path.to_string());
    }
    let suffix = format!("/{path}");
    universe
        .iter()
        .filter(|u| u.ends_with(&suffix))
        .min_by_key(|u| u.len())
        .cloned()
}

/// Resolve a dotted module (or module-member) name onto the universe.
///
/// For `a.b.c`, tries `a/b/c.py` then `a/b/c/__init__.py`; when neither
/// resolves, strips trailing components one at a time (`a.b.c` may be a member
/// of module `a.b`) until a candidate resolves. Longest prefix wins.
fn resolve_module(dotted: &str, universe: &BTreeSet<String>) -> Option<String> {
    let parts: Vec<&str> = dotted.split('.').filter(|p| !p.is_empty()).collect();
    for take in (1..=parts.len()).rev() {
        let prefix = parts[..take].join("/");
        for candidate in [format!("{prefix}.py"), format!("{prefix}/__init__.py")] {
            if let Some(path) = resolve_repo_file(&candidate, universe) {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn universe(files: &[&str]) -> BTreeSet<String> {
        files.iter().map(|s| s.to_string()).collect()
    }

    /// The marshmallow-shaped universe: src-layout package plus root tests.
    fn marshmallow_like() -> BTreeSet<String> {
        universe(&[
            "src/marshmallow/__init__.py",
            "src/marshmallow/class_registry.py",
            "examples/flask_example.py",
            "tests/base.py",
            "tests/test_context.py",
        ])
    }

    #[test]
    fn transitive_dependency_symbol_resolves_both_module_endpoints() {
        let facts = OracleChainFacts {
            symbol: Some("examples.flask_example->marshmallow.class_registry".to_string()),
            ..Default::default()
        };
        assert_eq!(
            facts.resolve(&marshmallow_like()),
            universe(&[
                "examples/flask_example.py",
                "src/marshmallow/class_registry.py"
            ])
        );
    }

    #[test]
    fn file_qualified_symbol_resolves_its_path_prefix() {
        let facts = OracleChainFacts {
            symbol: Some("tests/test_context.py::TestContext.test_nested_list_fields".to_string()),
            ..Default::default()
        };
        assert_eq!(
            facts.resolve(&marshmallow_like()),
            universe(&["tests/test_context.py"])
        );
    }

    #[test]
    fn dotted_member_symbol_strips_to_its_defining_module() {
        // `tests.base.validate` is a member of module `tests.base`.
        let facts = OracleChainFacts {
            symbol: Some("tests.base.validate".to_string()),
            ..Default::default()
        };
        assert_eq!(
            facts.resolve(&marshmallow_like()),
            universe(&["tests/base.py"])
        );
    }

    #[test]
    fn package_symbol_resolves_to_its_init_module() {
        let facts = OracleChainFacts {
            symbol: Some("marshmallow".to_string()),
            ..Default::default()
        };
        assert_eq!(
            facts.resolve(&marshmallow_like()),
            universe(&["src/marshmallow/__init__.py"])
        );
    }

    #[test]
    fn answer_defining_and_consensus_files_join_the_chain() {
        let facts = OracleChainFacts {
            answer_files: universe(&["tests/test_context.py"]),
            consensus_files: universe(&["examples/flask_example.py"]),
            defining_file: Some("tests/base.py".to_string()),
            symbol: None,
        };
        assert_eq!(
            facts.resolve(&marshmallow_like()),
            universe(&[
                "tests/test_context.py",
                "examples/flask_example.py",
                "tests/base.py"
            ])
        );
    }

    #[test]
    fn unresolvable_references_are_dropped_never_fabricated() {
        let facts = OracleChainFacts {
            answer_files: universe(&["docs/index.rst"]),
            symbol: Some("no.such.module".to_string()),
            ..Default::default()
        };
        assert!(facts.resolve(&marshmallow_like()).is_empty());
    }

    #[test]
    fn suffix_resolution_prefers_the_shortest_universe_path() {
        // Both a vendored copy and the real module end with the same suffix;
        // the least-nested one wins deterministically.
        let u = universe(&["src/pkg/mod.py", "vendor/deep/src/pkg/mod.py"]);
        let facts = OracleChainFacts {
            symbol: Some("pkg.mod".to_string()),
            ..Default::default()
        };
        assert_eq!(facts.resolve(&u), universe(&["src/pkg/mod.py"]));
    }
}
