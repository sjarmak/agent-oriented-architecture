use aoa_gap::MetricName;
use serde::{Deserialize, Serialize};

use crate::tier::{EnforcementPlane, Tier};

/// The kind of finding a [`PunchItem`] reports — the audit's own stable taxonomy
/// of what it measures. A machine-stable discriminant alongside [`Tier`]: it lets
/// a downstream consumer key on the *kind* of finding rather than parsing the
/// human title (which interpolates runtime values and is not a join key).
///
/// Adding a variant forces every construction site to set it (a required field on
/// [`PunchItem`]) — so a new finding can never ship without a kind. The
/// `recommend` layer's join exhaustively matches these, so a new variant there is
/// a compile error until its metric/fix association is declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// The context-document token closure exceeds the budget ceiling.
    ContextBudget,
    /// Writable files reachable within the mutation-surface depth.
    MutationSurface,
    /// A required enforcement plane (runtime hook, pre-commit, CI) is absent.
    MissingPlane,
    /// Package roots lacking a navigability anchor (README).
    NavigabilityAnchor,
    /// Source files exceeding the self-calibrating module-size threshold.
    ModuleSizeOutlier,
    /// Likely-unused imports by the Rust syntactic proxy.
    UnusedImportProxy,
    /// Package roots with no statically discoverable way to verify a change
    /// (no test dir/files, test config, CI test step, or documented command).
    VerificationReachability,
    /// Package roots with no statically discoverable declared rules/invariants
    /// (no policy file, agent-context/CONTRIBUTING doc, lint/format config,
    /// pre-commit config, or CODEOWNERS) reachable before editing.
    InvariantDiscoverability,
    /// No dependency-pinning lockfile at the repo root (build determinism is
    /// undeclared).
    BuildDeterminism,
    /// No reproducible dev-environment declaration (devcontainer, nix flake,
    /// toolchain/version pin) at the repo root.
    DevEnvironmentDeclaration,
    /// No task-discovery surface (issue-template path or in-repo issue tracker)
    /// at its well-known location.
    TaskDiscoverySurface,
    /// The generated-artifact-protection convention (`.gitattributes`
    /// `linguist-generated`) is absent — generated files are not marked
    /// off-limits to an agent (R6).
    GeneratedArtifactProtection,
    /// Well-known write-boundary declaration surfaces (CODEOWNERS / safe-write
    /// zone policy) are absent — no declared narrow write surface (R5).
    WriteSafetyZone,
}

impl FindingKind {
    /// Every variant, for exhaustive iteration in cross-crate consistency tests.
    /// Kept in sync with the enum by [`tests::all_lists_every_variant`], which
    /// matches each variant so a new one fails compilation until listed here.
    pub const ALL: &'static [FindingKind] = &[
        FindingKind::ContextBudget,
        FindingKind::MutationSurface,
        FindingKind::MissingPlane,
        FindingKind::NavigabilityAnchor,
        FindingKind::ModuleSizeOutlier,
        FindingKind::UnusedImportProxy,
        FindingKind::VerificationReachability,
        FindingKind::InvariantDiscoverability,
        FindingKind::BuildDeterminism,
        FindingKind::DevEnvironmentDeclaration,
        FindingKind::TaskDiscoverySurface,
        FindingKind::GeneratedArtifactProtection,
        FindingKind::WriteSafetyZone,
    ];

    /// The canonical construct-validity metric name this finding kind informs,
    /// if any. `None` for a kind that maps to no gating-candidate metric (a
    /// missing enforcement plane, verification-reachability, or invariant-
    /// discoverability — pure presence facts with no metric).
    ///
    /// This is the single source of truth for the `FindingKind → metric-name`
    /// association: `aoa_recommend`'s join reads its metric column from here, and
    /// `aoa`'s corpus reducer reads names from here under its own orientation
    /// include/exclude policy, so a rename lands in one place. The typed
    /// [`MetricName`] return makes every named metric a registered
    /// `aoa_gap::GATING_CANDIDATES` entry by construction — the compiler now
    /// enforces what a runtime drift guard used to assert across the crate
    /// boundary (`aoa-gap` sits below `aoa-audit` and cannot key on
    /// `FindingKind`, so the association lives here).
    ///
    /// The match is exhaustive: a new variant forces its metric association to
    /// be declared (compile error) rather than silently defaulting.
    #[must_use]
    pub fn metric_name(self) -> Option<MetricName> {
        match self {
            FindingKind::ContextBudget => Some(MetricName::BudgetAdherence),
            FindingKind::MutationSurface => Some(MetricName::MutationSurface),
            FindingKind::NavigabilityAnchor => Some(MetricName::NavigabilityAnchorAbsence),
            FindingKind::ModuleSizeOutlier => Some(MetricName::ModuleSizeOutliers),
            FindingKind::UnusedImportProxy => Some(MetricName::UnusedImportProxy),
            FindingKind::BuildDeterminism => Some(MetricName::BuildDeterminismAbsence),
            FindingKind::DevEnvironmentDeclaration => {
                Some(MetricName::DevEnvironmentDeclarationAbsence)
            }
            FindingKind::TaskDiscoverySurface => Some(MetricName::TaskDiscoverySurfaceAbsence),
            FindingKind::GeneratedArtifactProtection => {
                Some(MetricName::GeneratedArtifactProtectionAbsence)
            }
            FindingKind::WriteSafetyZone => Some(MetricName::WriteSafetyZoneAbsence),
            // Pure presence facts with no construct-validity metric.
            FindingKind::MissingPlane
            | FindingKind::VerificationReachability
            | FindingKind::InvariantDiscoverability => None,
        }
    }
}

/// A real, measured cost attached to a punch-list item — never a guess.
///
/// The `value` is a measured number (token overflow, writable-file count, …)
/// and `unit` names what was measured so the rendering is self-describing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasuredCost {
    pub value: u64,
    pub unit: String,
}

impl MeasuredCost {
    pub fn new(value: u64, unit: impl Into<String>) -> Self {
        Self {
            value,
            unit: unit.into(),
        }
    }
}

/// A single ranked finding in the audit punch-list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PunchItem {
    pub title: String,
    /// The stable machine taxonomy of this finding (the join key for `recommend`).
    pub kind: FindingKind,
    pub tier: Tier,
    pub measured_cost: MeasuredCost,
    /// The enforcement plane this item concerns, when the finding is a missing
    /// plane. `None` for findings that are not plane-shaped (oversized context,
    /// mutation surface).
    pub plane: Option<EnforcementPlane>,
    /// The workspace subtree this finding is scoped to: a member dir relative
    /// to the repo root, set when every offending path attributes to a single
    /// member of a partitioned workspace (`aoa_metrics::SubtreePartition`).
    /// `None` for findings that carry no path, for single-subtree repos, and
    /// for findings whose paths span members or fall outside all of them —
    /// attribution is unanimous or absent, never a majority guess. Omitted
    /// from the wire form when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtree: Option<String>,
}

/// Rank punch-list items deterministically: Tier-1 before Tier-2 before
/// Tier-3, then larger measured cost first, then title as a stable tiebreak.
///
/// This is transparent arithmetic ordering (ZFC): no hidden judgment, every
/// comparison key is data already on the item.
pub fn rank(items: &mut [PunchItem]) {
    items.sort_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            .then(b.measured_cost.value.cmp(&a.measured_cost.value))
            .then(a.title.cmp(&b.title))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lists_every_variant() {
        // Match every variant so adding one without listing it in ALL fails to
        // compile, keeping the iteration source exhaustive for the drift tests.
        for kind in FindingKind::ALL {
            match kind {
                FindingKind::ContextBudget
                | FindingKind::MutationSurface
                | FindingKind::MissingPlane
                | FindingKind::NavigabilityAnchor
                | FindingKind::ModuleSizeOutlier
                | FindingKind::UnusedImportProxy
                | FindingKind::VerificationReachability
                | FindingKind::InvariantDiscoverability
                | FindingKind::BuildDeterminism
                | FindingKind::DevEnvironmentDeclaration
                | FindingKind::TaskDiscoverySurface
                | FindingKind::GeneratedArtifactProtection
                | FindingKind::WriteSafetyZone => {}
            }
        }
        assert_eq!(FindingKind::ALL.len(), 13);
    }

    #[test]
    fn punch_item_round_trips_with_its_kind() {
        let item = PunchItem {
            title: "package roots without a navigability anchor (README)".into(),
            kind: FindingKind::NavigabilityAnchor,
            tier: Tier::Tier3,
            measured_cost: MeasuredCost::new(2, "package roots"),
            plane: None,
            subtree: None,
        };
        let json = serde_json::to_string(&item).expect("serialize");
        // The kind serializes to its snake_case wire name.
        assert!(
            json.contains("\"kind\":\"navigability_anchor\""),
            "kind missing from wire form: {json}"
        );
        // An unattributed finding carries no subtree key at all on the wire.
        assert!(
            !json.contains("subtree"),
            "None subtree must be omitted from wire form: {json}"
        );
        let parsed: PunchItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, item);
    }

    #[test]
    fn punch_item_serializes_its_subtree_when_attributed() {
        let item = PunchItem {
            title: "source files exceeding 4.0x the repo median size".into(),
            kind: FindingKind::ModuleSizeOutlier,
            tier: Tier::Tier3,
            measured_cost: MeasuredCost::new(1, "outlier files"),
            plane: None,
            subtree: Some("crates/foo".into()),
        };
        let json = serde_json::to_string(&item).expect("serialize");
        assert!(
            json.contains("\"subtree\":\"crates/foo\""),
            "attributed subtree missing from wire form: {json}"
        );
        let parsed: PunchItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, item);
    }
}
