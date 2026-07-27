use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

use aoa_trace::{IndexQuality, SymbolGraph};

use crate::bounded::{read_capped, MAX_SCIP_BYTES};
use crate::error::ScipGraphError;
use crate::index::IndexedRepo;

/// A vendored SCIP index document, simplified to the fields the symbol graph
/// needs. This mirrors what a SCIP tool emits — per-document symbol definitions
/// and occurrences with semantic roles — without the full protobuf surface, so
/// tests run fully offline against committed data.
///
/// This top-level shape deliberately tolerates fields from real SCIP exports.
/// AOA-only annotations live under the strict `aoa` namespace so operator
/// mistakes cannot be confused with future tool-emitted fields.
#[derive(Debug, Deserialize)]
struct ScipIndex {
    documents: Vec<ScipDocument>,
    #[serde(default)]
    aoa: AoaExtensions,
    #[serde(rename = "writable")]
    _legacy_writable: Option<RejectLegacyExtension>,
    #[serde(rename = "gold")]
    _legacy_gold: Option<RejectLegacyExtension>,
    #[serde(rename = "invariants")]
    _legacy_invariants: Option<RejectLegacyExtension>,
}

#[derive(Debug)]
struct RejectLegacyExtension;

impl<'de> Deserialize<'de> for RejectLegacyExtension {
    fn deserialize<D: Deserializer<'de>>(_deserializer: D) -> Result<Self, D::Error> {
        Err(D::Error::custom(
            "AOA extension fields must be nested under the `aoa` key",
        ))
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AoaExtensions {
    #[serde(default)]
    writable: Vec<String>,
    #[serde(default)]
    gold: Vec<String>,
    #[serde(default)]
    invariants: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ScipDocument {
    /// The document's repo-relative source path (SCIP `relative_path`). Absent
    /// in older vendored indexes; definitions then carry no node path.
    #[serde(default)]
    relative_path: Option<String>,
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
/// enclosing definition. A definition also maps its symbol to the document's
/// `relative_path` in `node_paths`. The resulting graph is tagged
/// [`IndexQuality::Scip`].
pub fn index_with_scip(index_path: &Path) -> Result<IndexedRepo, ScipGraphError> {
    let raw = read_capped(index_path, MAX_SCIP_BYTES)?;
    let index: ScipIndex = serde_json::from_str(&raw).map_err(|source| ScipGraphError::Parse {
        path: index_path.to_path_buf(),
        source,
    })?;

    let mut nodes: BTreeSet<String> = BTreeSet::new();
    let mut edges: BTreeSet<(String, String)> = BTreeSet::new();
    let mut node_paths: BTreeMap<String, String> = BTreeMap::new();

    for doc in &index.documents {
        for occ in &doc.occurrences {
            if occ.roles.contains(&ScipRole::Definition) {
                nodes.insert(occ.symbol.clone());
                if let Some(path) = &doc.relative_path {
                    node_paths.insert(occ.symbol.clone(), path.clone());
                }
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
        writable: index.aoa.writable.into_iter().collect(),
        node_paths,
        quality: IndexQuality::Scip,
    };

    Ok(IndexedRepo {
        graph,
        gold_set: index.aoa.gold.into_iter().collect(),
        invariant_set: index.aoa.invariants.into_iter().collect(),
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

    /// Pins the unknown-field tolerance documented on `ScipIndex`, so the
    /// decision fails CI rather than only asserting itself in prose. Covers
    /// only the tool-emitted half, so aoa-t49j's strict-extension split — the
    /// part that *wants* rejection — lands without touching this test.
    #[test]
    fn scip_index_tolerates_unknown_tool_emitted_fields() {
        let dir = std::env::temp_dir().join(format!("aoa-scip-tolerant-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("idx.json");
        std::fs::write(
            &path,
            r#"{ "metadata": { "tool_info": { "name": "scip-rust" } },
                 "external_symbols": [ { "symbol": "ext" } ],
                 "documents": [ { "relative_path": "a.rs", "language": "rust",
                                  "occurrences": [
                     { "symbol": "f", "roles": ["definition"], "range": [0, 0, 1] }
                 ] } ] }"#,
        )
        .unwrap();
        let repo = index_with_scip(&path).unwrap();
        assert_eq!(repo.graph.nodes, vec!["f".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scip_index_rejects_unknown_aoa_extension_fields() {
        let dir = std::env::temp_dir().join(format!("aoa-scip-strict-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("idx.json");
        std::fs::write(
            &path,
            r#"{ "documents": [], "aoa": { "writable": [], "goold": ["target"] } }"#,
        )
        .unwrap();

        let error = index_with_scip(&path).unwrap_err();
        assert!(
            error.to_string().contains("unknown field `goold`"),
            "{error}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scip_index_rejects_legacy_flat_aoa_extension_fields() {
        let dir = std::env::temp_dir().join(format!("aoa-scip-legacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("idx.json");
        std::fs::write(&path, r#"{ "documents": [], "gold": ["target"] }"#).unwrap();

        let error = index_with_scip(&path).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("AOA extension fields must be nested under the `aoa` key"),
            "{error}"
        );
        std::fs::remove_dir_all(&dir).ok();
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
