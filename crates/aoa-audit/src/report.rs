use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::punch::PunchItem;
use crate::tier::Tier;

/// Exit code returned when `fail_on_tier1` is set and a Tier-1 gap exists.
const TIER1_FAILURE_CODE: i32 = 2;

/// The full audit result: a ranked punch-list. Serializes to structured JSON
/// and renders to a human-readable ranked list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditReport {
    pub items: Vec<PunchItem>,
    /// Present when a workspace manifest exists but could not be used for
    /// subtree discovery: the punch-list is complete, but path-carrying
    /// findings stay repo-wide (no `subtree` labels). Never set for a repo
    /// with no workspace manifest — that is the implicit-root partition, not
    /// a failure. Omitted from the wire form when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtree_discovery_warning: Option<String>,
}

impl AuditReport {
    pub fn new(items: Vec<PunchItem>) -> Self {
        Self {
            items,
            subtree_discovery_warning: None,
        }
    }

    /// Whether any punch-list item is a Tier-1 gap.
    pub fn has_tier1_gap(&self) -> bool {
        self.items.iter().any(|item| item.tier == Tier::Tier1)
    }

    /// Render the ranked punch-list as human-readable text. Each line carries
    /// the item's tier, title, and its measured cost.
    pub fn render_human(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "AOA audit punch-list ({} item(s))", self.items.len());
        if let Some(warning) = &self.subtree_discovery_warning {
            let _ = writeln!(out, "warning: {warning}");
        }
        for (index, item) in self.items.iter().enumerate() {
            let _ = writeln!(
                out,
                "{:>2}. [{}] {} — cost: {} {}",
                index + 1,
                item.tier.label(),
                item.title,
                item.measured_cost.value,
                item.measured_cost.unit,
            );
        }
        out
    }
}

/// The audit exit code: `0` by default — even with gaps present — and non-zero
/// only when `fail_on_tier1` is set AND a Tier-1 gap exists.
pub fn exit_code(report: &AuditReport, fail_on_tier1: bool) -> i32 {
    if fail_on_tier1 && report.has_tier1_gap() {
        TIER1_FAILURE_CODE
    } else {
        0
    }
}
