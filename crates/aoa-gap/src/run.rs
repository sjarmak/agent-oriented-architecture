use serde::{Deserialize, Serialize};

use crate::provenance::HeldOutProvenance;

/// A single task's per-suite outcome: did the visible suite pass, did the
/// held-out suite pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskOutcome {
    pub visible_success: bool,
    pub held_out_success: bool,
}

/// A known held-out item injected as an integrity canary. `expected_held_out`
/// is the outcome a clean (non-leaking) run must produce; an observed
/// `held_out_success` that diverges from it is an unexpected flip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanaryItem {
    pub id: String,
    pub held_out_success: bool,
    pub expected_held_out: bool,
}

impl CanaryItem {
    /// Whether the observed canary outcome diverges from its expected outcome.
    pub fn flipped(&self) -> bool {
        self.held_out_success != self.expected_held_out
    }
}

/// The result of one evaluation run over a task set, with held-out provenance
/// and any injected integrity canaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunResult {
    pub tasks: Vec<TaskOutcome>,
    pub held_out_provenance: HeldOutProvenance,
    #[serde(default)]
    pub canaries: Vec<CanaryItem>,
}

/// Mean of a per-task boolean projection; `0.0` for an empty task set.
fn rate(tasks: &[TaskOutcome], pass: impl Fn(&TaskOutcome) -> bool) -> f64 {
    if tasks.is_empty() {
        return 0.0;
    }
    let passed = tasks.iter().filter(|t| pass(t)).count();
    passed as f64 / tasks.len() as f64
}

impl RunResult {
    /// Fraction of tasks whose visible suite passed.
    pub fn visible_rate(&self) -> f64 {
        rate(&self.tasks, |t| t.visible_success)
    }

    /// Fraction of tasks whose held-out suite passed.
    pub fn held_out_rate(&self) -> f64 {
        rate(&self.tasks, |t| t.held_out_success)
    }

    /// Whether any injected canary flipped against its expected outcome.
    pub fn any_canary_flipped(&self) -> bool {
        self.canaries.iter().any(CanaryItem::flipped)
    }
}
