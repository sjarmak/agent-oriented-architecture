use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use aoa_construct::{BehavioralSignal, InsufficientDataNote};

use crate::punch::PunchItem;
use crate::tier::Tier;

/// Exit code returned when `fail_on_tier1` is set and a Tier-1 gap exists.
const TIER1_FAILURE_CODE: i32 = 2;

/// The full audit result: a ranked punch-list plus the repo's held-out
/// behavioral signal. Serializes to structured JSON and renders to a
/// human-readable ranked list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditReport {
    pub items: Vec<PunchItem>,
    /// Present when a workspace manifest exists but could not be used for
    /// subtree discovery: the punch-list is complete, but path-carrying
    /// findings stay repo-wide (no `subtree` labels). Never set for a repo
    /// with no workspace manifest — that is the implicit-root partition, not
    /// a failure. Omitted from the wire form when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtree_discovery_warning: Option<String>,
    /// The repo's held-out behavioral signal (observe-captured sessions under
    /// `.aoa/traces/` counted against [`aoa_construct::MIN_HELD_OUT_OBSERVATIONS`]).
    /// Reports from producers that predate the field deserialize to zero
    /// observations.
    #[serde(default)]
    pub behavioral_signal: BehavioralSignal,
    /// Present when the signal is below the window: the behavioral metrics
    /// (the four locality metrics) are InsufficientData, with the reason.
    /// Their punch items are withheld rather than fabricated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insufficient_data: Option<InsufficientDataNote>,
}

#[derive(Deserialize)]
struct AuditReportWire {
    items: Vec<PunchItem>,
    #[serde(default)]
    subtree_discovery_warning: Option<String>,
    #[serde(default)]
    behavioral_signal: BehavioralSignal,
    // Accepted for wire compatibility, but this is a projection of
    // `behavioral_signal`, never independent input.
    #[serde(default, rename = "insufficient_data")]
    _insufficient_data: Option<InsufficientDataNote>,
}

impl<'de> Deserialize<'de> for AuditReport {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = AuditReportWire::deserialize(deserializer)?;
        Ok(Self {
            items: wire.items,
            subtree_discovery_warning: wire.subtree_discovery_warning,
            insufficient_data: wire.behavioral_signal.insufficient_data(),
            behavioral_signal: wire.behavioral_signal,
        })
    }
}

impl AuditReport {
    /// A report over `items` with no recorded behavioral signal (zero
    /// observations). For synthetic reports in tests and tools; `audit()`
    /// builds through [`AuditReport::with_signal`] with the measured count.
    pub fn new(items: Vec<PunchItem>) -> Self {
        Self::with_signal(items, BehavioralSignal::from_observations(0))
    }

    /// A report over `items` carrying the repo's measured behavioral signal.
    /// The insufficient-data note is derived from the signal here, so the two
    /// fields can never disagree.
    pub fn with_signal(items: Vec<PunchItem>, behavioral_signal: BehavioralSignal) -> Self {
        Self {
            items,
            subtree_discovery_warning: None,
            insufficient_data: behavioral_signal.insufficient_data(),
            behavioral_signal,
        }
    }

    /// Whether any punch-list item is a Tier-1 gap.
    pub fn has_tier1_gap(&self) -> bool {
        self.items.iter().any(|item| item.tier == Tier::Tier1)
    }

    /// Render the ranked punch-list as human-readable text. Each line carries
    /// the item's tier, title, and its measured cost. A repo below the
    /// behavioral-signal window gets the InsufficientData block with its
    /// reason, so the withheld behavioral metrics are explicit in this
    /// register too.
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
        if let Some(note) = &self.insufficient_data {
            let _ = writeln!(out, "{}", note.render_line(&self.behavioral_signal));
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
