use serde::{Deserialize, Serialize};

/// A config-smell catalog category from the arXiv:2606.15828 taxonomy.
///
/// Each variant names one mechanically-detectable structural smell class. The
/// id strings are stable wire identifiers (machine-readable) and must not change
/// once emitted, since downstream consumers key on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmellCategory {
    /// Two directives that structurally contradict each other (e.g. an "always"
    /// and a "never" rule over the same token). Catalog: contradictory guidance.
    Contradiction,
    /// Redundant structure — the same heading repeated within one file.
    /// Catalog: duplication / redundancy.
    Duplication,
    /// A section whose body exceeds the size threshold. Catalog: verbosity /
    /// bloat.
    Verbosity,
    /// A markdown link to a local file that does not exist on disk (a dead
    /// link). Catalog: stale reference.
    StaleReference,
    /// An over-broad glob scope (a bare `**` / `**/*`). Catalog: over-broad
    /// scope.
    OverBroadGlob,
}

impl SmellCategory {
    /// Stable machine-readable id string for this category.
    pub fn id(&self) -> &'static str {
        match self {
            SmellCategory::Contradiction => "contradiction",
            SmellCategory::Duplication => "duplication",
            SmellCategory::Verbosity => "verbosity",
            SmellCategory::StaleReference => "stale_reference",
            SmellCategory::OverBroadGlob => "overbroad_glob",
        }
    }
}
