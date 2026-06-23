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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    ];
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
                | FindingKind::UnusedImportProxy => {}
            }
        }
        assert_eq!(FindingKind::ALL.len(), 6);
    }

    #[test]
    fn punch_item_round_trips_with_its_kind() {
        let item = PunchItem {
            title: "package roots without a navigability anchor (README)".into(),
            kind: FindingKind::NavigabilityAnchor,
            tier: Tier::Tier3,
            measured_cost: MeasuredCost::new(2, "package roots"),
            plane: None,
        };
        let json = serde_json::to_string(&item).expect("serialize");
        // The kind serializes to its snake_case wire name.
        assert!(
            json.contains("\"kind\":\"navigability_anchor\""),
            "kind missing from wire form: {json}"
        );
        let parsed: PunchItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, item);
    }
}
