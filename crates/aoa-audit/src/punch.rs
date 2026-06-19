use serde::{Deserialize, Serialize};

use crate::tier::{EnforcementPlane, Tier};

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
