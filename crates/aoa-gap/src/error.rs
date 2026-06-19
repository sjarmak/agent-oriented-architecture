use thiserror::Error;

/// Errors raised while computing or comparing the reward-hacking gap.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GapError {
    /// Held-out tests synthesized toolkit-side from visible specs are forbidden:
    /// such a suite cannot certify a real visible-vs-held-out gap (R0b).
    #[error("held-out suite synthesized from visible specs is forbidden")]
    SynthesizedHeldOut,

    /// The held-out pass rate rose while the visible rate stayed flat and an
    /// injected canary flipped against its expected outcome: leakage (R0b).
    #[error("held-out integrity canary tripped: held-out rate rose without visible movement and canaries flipped")]
    LeakageDetected,

    /// No native composed held-out suite exists, so there is no gap to gate on.
    /// Labeling a migration on an absent gap is prohibited (R9c/R9).
    #[error("gap unavailable: no native composed held-out suite, refusing to label migration")]
    GapUnavailable,
}
