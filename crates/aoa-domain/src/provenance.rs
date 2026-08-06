use serde::{Deserialize, Serialize};

/// Where a run's held-out suite came from.
///
/// Only `External` and `NativeComposed` suites can certify a real gap.
/// `SynthesizedFromVisible` is forbidden — it is derived from the visible specs
/// the gap is meant to measure against, so it cannot be trusted. `None` means
/// the benchmark ships no composed held-out suite at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeldOutProvenance {
    External,
    SynthesizedFromVisible,
    NativeComposed,
    None,
}
