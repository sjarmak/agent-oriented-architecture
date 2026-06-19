use serde::{Deserialize, Serialize};

/// Severity tier of a punch-list item, drawn from the report's evidence
/// framework: Tier-1 is evidence-backed and adopted now, Tier-2 is a
/// pilot-and-measure hypothesis, Tier-3 is asserted-but-unsupported.
///
/// Declaration order is severity order (Tier-1 highest), so the derived `Ord`
/// ranks Tier-1 items ahead of Tier-2 ahead of Tier-3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Tier {
    #[serde(rename = "tier-1")]
    Tier1,
    #[serde(rename = "tier-2")]
    Tier2,
    #[serde(rename = "tier-3")]
    Tier3,
}

impl Tier {
    /// The stable wire label for this tier.
    pub fn label(self) -> &'static str {
        match self {
            Tier::Tier1 => "tier-1",
            Tier::Tier2 => "tier-2",
            Tier::Tier3 => "tier-3",
        }
    }
}

/// The three enforcement planes the audit checks for structurally.
///
/// Each plane maps to a fixed tier (see [`crate::audit`] for the mapping and
/// its evidence justification).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnforcementPlane {
    /// A runtime write/mutation hook configuration (reproduction-before-mutation).
    RuntimeHook,
    /// A local pre-commit hook mirroring the CI gate.
    PreCommit,
    /// A CI workflow running the checks as merge findings.
    Ci,
}

impl EnforcementPlane {
    /// The tier this plane's absence is reported at.
    ///
    /// Runtime hook and CI are Tier-1 (the evidence-backed harness/telemetry
    /// lever and the CI-findings requirement); pre-commit is the Tier-2 local
    /// mirror that pilots the same gate.
    pub fn tier(self) -> Tier {
        match self {
            EnforcementPlane::RuntimeHook | EnforcementPlane::Ci => Tier::Tier1,
            EnforcementPlane::PreCommit => Tier::Tier2,
        }
    }

    /// A human label for the plane.
    pub fn label(self) -> &'static str {
        match self {
            EnforcementPlane::RuntimeHook => "runtime hook",
            EnforcementPlane::PreCommit => "pre-commit hook",
            EnforcementPlane::Ci => "CI workflow",
        }
    }
}
