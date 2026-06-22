use std::collections::BTreeSet;
use std::path::Path;

use serde::Deserialize;

use aoa_metrics::{IndexQuality, SymbolGraph};

use crate::bounded::{read_capped, MAX_SCIP_BYTES};
use crate::error::ScipGraphError;
use crate::index::IndexedRepo;

/// A vendored SCIP index document, simplified to the fields the symbol graph
/// needs. This mirrors what a SCIP tool emits — per-document symbol definitions
/// and occurrences with semantic roles — without the full protobuf surface, so
/// tests run fully offline against committed data.
#[derive(Debug, Deserialize)]
struct ScipIndex {
    documents: Vec<ScipDocument>,
    #[serde(default)]
    writable: Vec<String>,
    #[serde(default)]
    gold: Vec<String>,
    #[serde(default)]
    invariants: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ScipDocument {
    #[serde(default)]
    occurrences: Vec<ScipOccurrence>,
}

/// A semantic role on a SCIP occurrence.
///
/// SCIP's role vocabulary is closed, so the two roles the symbol graph cares
/// about are modeled explicitly; any other role a tool emits deserializes to
/// [`ScipRole::Unknown`] (preserving forward compatibility without matching
/// against bare string literals).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ScipRole {
    Definition,
    Reference,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct ScipOccurrence {
    symbol: String,
    #[serde(default)]
    roles: Vec<ScipRole>,
    /// The defining symbol this occurrence sits inside, for reference edges.
    #[serde(default)]
    enclosing: Option<String>,
}

/// Read a vendored SCIP JSON index into a high-confidence [`IndexedRepo`].
///
/// Nodes are the symbols with a `definition` occurrence; edges are
/// `(enclosing, symbol)` for each `reference` occurrence that names its
/// enclosing definition. The resulting graph is tagged [`IndexQuality::Scip`].
pub fn index_with_scip(index_path: &Path) -> Result<IndexedRepo, ScipGraphError> {
    let raw = read_capped(index_path, MAX_SCIP_BYTES)?;
    let index: ScipIndex = serde_json::from_str(&raw).map_err(|source| ScipGraphError::Parse {
        path: index_path.to_path_buf(),
        source,
    })?;

    let mut nodes: BTreeSet<String> = BTreeSet::new();
    let mut edges: BTreeSet<(String, String)> = BTreeSet::new();

    for doc in &index.documents {
        for occ in &doc.occurrences {
            if occ.roles.contains(&ScipRole::Definition) {
                nodes.insert(occ.symbol.clone());
            }
            if occ.roles.contains(&ScipRole::Reference) {
                if let Some(from) = &occ.enclosing {
                    edges.insert((from.clone(), occ.symbol.clone()));
                }
            }
        }
    }

    let graph = SymbolGraph {
        nodes: nodes.into_iter().collect(),
        edges: edges.into_iter().collect(),
        writable: index.writable.into_iter().collect(),
        quality: IndexQuality::Scip,
    };

    Ok(IndexedRepo {
        graph,
        gold_set: index.gold.into_iter().collect(),
        invariant_set: index.invariants.into_iter().collect(),
        degrade_reason: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_an_io_error() {
        let err = index_with_scip(Path::new("/nope/index.scip.json")).unwrap_err();
        assert!(matches!(err, ScipGraphError::Io { .. }));
    }

    #[test]
    fn roles_deserialize_definition_reference_and_unknown() {
        let occ: ScipOccurrence = serde_json::from_str(
            r#"{ "symbol": "s", "roles": ["definition", "reference", "import"] }"#,
        )
        .unwrap();
        assert_eq!(
            occ.roles,
            vec![ScipRole::Definition, ScipRole::Reference, ScipRole::Unknown]
        );
    }

    #[test]
    fn unknown_role_drives_neither_a_node_nor_an_edge() {
        let dir = std::env::temp_dir().join(format!("aoa-scip-role-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("idx.json");
        std::fs::write(
            &path,
            r#"{ "documents": [ { "occurrences": [
                { "symbol": "only_imported", "roles": ["import"] }
            ] } ] }"#,
        )
        .unwrap();
        let repo = index_with_scip(&path).unwrap();
        assert!(repo.graph.nodes.is_empty());
        assert!(repo.graph.edges.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn malformed_json_is_a_parse_error() {
        let dir = std::env::temp_dir().join(format!("aoa-scip-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.json");
        std::fs::write(&path, "{ not json").unwrap();
        let err = index_with_scip(&path).unwrap_err();
        assert!(matches!(err, ScipGraphError::Parse { .. }));
        std::fs::remove_dir_all(&dir).ok();
    }
}
