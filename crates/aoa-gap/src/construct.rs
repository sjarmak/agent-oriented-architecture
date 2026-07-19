use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// An external outcome a metric can be correlated against to earn gating status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalOutcome {
    /// Post-merge revert rate. Lower is better.
    RevertRate,
    /// Production incident count. Lower is better.
    IncidentCount,
    /// Human review-acceptance rate. Higher is better.
    ReviewAcceptance,
}

impl ExternalOutcome {
    /// Whether a HIGHER value of this outcome corresponds to BETTER real-world
    /// code. Reverts and incidents are harms (lower is better); review
    /// acceptance is a good (higher is better). Combined with a metric's own
    /// orientation, this fixes the sign a *confirming* correlation must have.
    fn higher_is_better(self) -> bool {
        matches!(self, ExternalOutcome::ReviewAcceptance)
    }
}

/// Which direction of a metric reads as "better" code. Required to interpret a
/// correlation's sign: the same external outcome confirms construct validity
/// with opposite signs depending on whether more of the metric is good
/// (`edit_locality`) or bad (`mutation_surface`, `reward_hacking_gap`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricOrientation {
    HigherIsBetter,
    LowerIsBetter,
}

impl MetricOrientation {
    fn higher_is_better(self) -> bool {
        matches!(self, MetricOrientation::HigherIsBetter)
    }
}

/// A single tie between a metric and one external outcome, carrying the signed
/// coefficient (sign + magnitude), the sample size behind it, and the exact
/// two-sided permutation p-value. A bare "positive" flag is deliberately absent:
/// whether a correlation *confirms* validity depends on the metric's
/// orientation and the gating thresholds, evaluated by [`OutcomeCorrelation::is_confirming`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OutcomeCorrelation {
    pub outcome: ExternalOutcome,
    /// Signed rank-correlation coefficient in `[-1.0, 1.0]`.
    pub coefficient: f64,
    /// Number of paired observations backing the coefficient.
    pub n: usize,
    /// Exact two-sided permutation p-value.
    pub p_value: f64,
}

impl OutcomeCorrelation {
    /// The coefficient sign a confirming correlation must have, given the
    /// metric's orientation: positive when metric-good and outcome-good point
    /// the same way, negative when they oppose.
    fn confirming_is_positive(&self, orientation: MetricOrientation) -> bool {
        orientation.higher_is_better() == self.outcome.higher_is_better()
    }

    /// Whether this correlation is strong enough, in the right direction, and
    /// unlikely enough to be noise to count as evidence for gating, under the
    /// supplied thresholds.
    pub fn is_confirming(
        &self,
        orientation: MetricOrientation,
        thresholds: &GatingThresholds,
    ) -> bool {
        let sign_ok = if self.confirming_is_positive(orientation) {
            self.coefficient > 0.0
        } else {
            self.coefficient < 0.0
        };
        sign_ok
            && self.coefficient.abs() >= thresholds.min_magnitude
            && self.n >= thresholds.min_n
            && self.p_value <= thresholds.max_p_value
    }
}

/// Explicit, inspectable thresholds a correlation must clear to gate. Carried as
/// data — not hidden constants — so the gating preconditions are auditable.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GatingThresholds {
    /// Minimum `|coefficient|`: a correlation weaker than this is too small to
    /// gate even if statistically significant.
    pub min_magnitude: f64,
    /// Minimum sample size: fewer observations cannot earn gating regardless of
    /// coefficient.
    pub min_n: usize,
    /// Maximum two-sided p-value (alpha): the correlation must be this unlikely
    /// under the no-association null.
    pub max_p_value: f64,
}

impl Default for GatingThresholds {
    /// Moderate effect (`|rho| >= 0.3`), at least 5 observations, significance
    /// at alpha = 0.05. Documented defaults, overridable per call.
    fn default() -> Self {
        Self {
            min_magnitude: 0.3,
            min_n: 5,
            max_p_value: 0.05,
        }
    }
}

/// A construct-validity report tying a metric to its external-outcome
/// correlations. The metric is `Gating` only if at least one correlation is
/// confirming under the gating thresholds; otherwise it stays `Advisory`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorrelationReport {
    pub metric: String,
    pub orientation: MetricOrientation,
    pub correlations: Vec<OutcomeCorrelation>,
}

impl CorrelationReport {
    /// Whether at least one external-outcome correlation confirms validity
    /// under the supplied thresholds (right direction, sufficient magnitude,
    /// sufficient sample, significant).
    pub fn has_confirming_correlation(&self, thresholds: &GatingThresholds) -> bool {
        self.correlations
            .iter()
            .any(|c| c.is_confirming(self.orientation, thresholds))
    }
}

/// Whether a metric may gate a decision, is advisory only, or has no data at
/// all for the repo under evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricMode {
    Advisory,
    Gating,
    /// The repo has no held-out behavioral signal yet (greenfield/cold-start),
    /// so the metric cannot be computed at all. DISTINCT from `Advisory`: an
    /// advisory metric was measured but has not earned external validation; an
    /// insufficient-data metric has nothing to measure. Never produced by
    /// [`classify_metric`] — only the behavioral-signal precondition
    /// ([`crate::determination_with_signal`]) assigns it, with
    /// [`crate::INSUFFICIENT_DATA_REASON`].
    InsufficientData,
}

impl MetricMode {
    /// The human-readable label for this mode, used in the operator-facing
    /// text output. Intentionally PascalCase — distinct from the snake_case
    /// serde representation used on the JSON path.
    pub fn as_str(self) -> &'static str {
        match self {
            MetricMode::Advisory => "Advisory",
            MetricMode::Gating => "Gating",
            MetricMode::InsufficientData => "InsufficientData",
        }
    }
}

/// Classify a metric as gating or advisory under R9c construct validity.
///
/// A metric is `Gating` only when a correlation report is supplied whose
/// evidence clears the gating thresholds — a confirming correlation to at least
/// one external outcome with the right sign, sufficient magnitude, sufficient
/// sample size, and significance. Without that evidence (no report, or one that
/// falls short on any axis) it is `Advisory`, regardless of how the metric looks
/// in isolation.
pub fn classify_metric(
    correlation: Option<&CorrelationReport>,
    thresholds: &GatingThresholds,
) -> MetricMode {
    match correlation {
        Some(report) if report.has_confirming_correlation(thresholds) => MetricMode::Gating,
        _ => MetricMode::Advisory,
    }
}

/// The typed identity of every gating-candidate metric — the single source of
/// truth the stringly-typed wire literal (e.g. `write_safety_zone_absence`)
/// derives from via [`MetricName::as_str`]. Keying [`GATING_CANDIDATES`],
/// [`STRUCTURE_MEASURE_SPECS`], [`crate::BEHAVIORAL_METRICS`], and
/// `FindingKind::metric_name` (aoa-audit) on this enum makes "every named
/// metric is a registered candidate" a type-level fact instead of a
/// runtime-tested one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MetricName {
    RetrievalLocality,
    EditLocality,
    InvariantDiscoverability,
    MutationSurface,
    BudgetAdherence,
    RewardHackingGap,
    NavigabilityAnchorAbsence,
    ModuleSizeOutliers,
    UnusedImportProxy,
    BuildDeterminismAbsence,
    DevEnvironmentDeclarationAbsence,
    TaskDiscoverySurfaceAbsence,
    GeneratedArtifactProtectionAbsence,
    WriteSafetyZoneAbsence,
}

impl MetricName {
    /// Every metric, in registration order. The order is wire-load-bearing: it
    /// drives [`GATING_CANDIDATES`], and through it the metric order of
    /// [`current_determination`] and the committed report fixtures — the six
    /// process metrics first (pre-existing positional order), then the eight
    /// code-structure measures appended last.
    pub const ALL: [MetricName; 14] = [
        MetricName::RetrievalLocality,
        MetricName::EditLocality,
        MetricName::InvariantDiscoverability,
        MetricName::MutationSurface,
        MetricName::BudgetAdherence,
        MetricName::RewardHackingGap,
        MetricName::NavigabilityAnchorAbsence,
        MetricName::ModuleSizeOutliers,
        MetricName::UnusedImportProxy,
        MetricName::BuildDeterminismAbsence,
        MetricName::DevEnvironmentDeclarationAbsence,
        MetricName::TaskDiscoverySurfaceAbsence,
        MetricName::GeneratedArtifactProtectionAbsence,
        MetricName::WriteSafetyZoneAbsence,
    ];

    /// The snake_case wire literal for this metric — the exact string used in
    /// report JSON (`CorrelationReport::metric`), corpus rows
    /// (`Repo::structure_counts` keys), and every human rendering. Wire types
    /// stay `String` and convert here at the boundary.
    pub const fn as_str(self) -> &'static str {
        match self {
            MetricName::RetrievalLocality => "retrieval_locality",
            MetricName::EditLocality => "edit_locality",
            MetricName::InvariantDiscoverability => "invariant_discoverability",
            MetricName::MutationSurface => "mutation_surface",
            MetricName::BudgetAdherence => "budget_adherence",
            MetricName::RewardHackingGap => "reward_hacking_gap",
            MetricName::NavigabilityAnchorAbsence => "navigability_anchor_absence",
            MetricName::ModuleSizeOutliers => "module_size_outliers",
            MetricName::UnusedImportProxy => "unused_import_proxy",
            MetricName::BuildDeterminismAbsence => "build_determinism_absence",
            MetricName::DevEnvironmentDeclarationAbsence => "dev_environment_declaration_absence",
            MetricName::TaskDiscoverySurfaceAbsence => "task_discovery_surface_absence",
            MetricName::GeneratedArtifactProtectionAbsence => {
                "generated_artifact_protection_absence"
            }
            MetricName::WriteSafetyZoneAbsence => "write_safety_zone_absence",
        }
    }

    /// Which direction of this metric reads as "better" code.
    /// `mutation_surface` and `reward_hacking_gap` are harms (smaller is
    /// better), as is every `*_absence` / outlier / unused-import count; the
    /// rest are goods. Exhaustive so a new metric forces the orientation
    /// decision here (compile error) rather than defaulting.
    pub const fn orientation(self) -> MetricOrientation {
        match self {
            MetricName::RetrievalLocality
            | MetricName::EditLocality
            | MetricName::InvariantDiscoverability
            | MetricName::BudgetAdherence => MetricOrientation::HigherIsBetter,
            MetricName::MutationSurface
            | MetricName::RewardHackingGap
            | MetricName::NavigabilityAnchorAbsence
            | MetricName::ModuleSizeOutliers
            | MetricName::UnusedImportProxy
            | MetricName::BuildDeterminismAbsence
            | MetricName::DevEnvironmentDeclarationAbsence
            | MetricName::TaskDiscoverySurfaceAbsence
            | MetricName::GeneratedArtifactProtectionAbsence
            | MetricName::WriteSafetyZoneAbsence => MetricOrientation::LowerIsBetter,
        }
    }
}

impl std::fmt::Display for MetricName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The gating-candidate metrics and their orientations — every metric that R9c
/// could let gate a generative feature once a confirming correlation exists.
///
/// Derived from [`MetricName::ALL`] and [`MetricName::orientation`], so every
/// [`MetricName`] is a registered candidate by construction — cross-crate
/// consumers that key on the enum cannot name an unregistered metric, which is
/// what retired the runtime drift guards that used to assert it.
pub const GATING_CANDIDATES: [(MetricName, MetricOrientation); MetricName::ALL.len()] = {
    let mut table = [(
        MetricName::RetrievalLocality,
        MetricOrientation::HigherIsBetter,
    ); MetricName::ALL.len()];
    let mut i = 0;
    while i < MetricName::ALL.len() {
        table[i] = (MetricName::ALL[i], MetricName::ALL[i].orientation());
        i += 1;
    }
    table
};

/// The pre-registered spec for the code-structure measures: each metric paired
/// with the mechanical fact `aoa-audit` measures for it. This is the spec AOA
/// *verifies* — "better-organized / migrated" is fixed by these definitions,
/// never by AOA's own pass-state (anti-Goodhart; runbook guardrail 3). Keyed on
/// [`MetricName`], so each spec names a registered [`GATING_CANDIDATES`] entry
/// by type, and the orientation lives on [`MetricName::orientation`] alone —
/// this table adds only the definition, so the two cannot drift on direction.
///
/// Promotion path: every measure here is born [`MetricMode::Advisory`] and
/// promotes to [`MetricMode::Gating`] ONLY when [`classify_metric`] sees a
/// confirming correlation to an [`ExternalOutcome`] (revert rate, incident
/// count, or review acceptance) clearing [`GatingThresholds`] — the same gate
/// as every other candidate. The external-outcome corpus that would supply such
/// a correlation does not yet exist (see [`NO_EXTERNAL_OUTCOME_SOURCE`]), so all
/// of them are currently Advisory.
///
/// Most measures carry a caveat the navigability and unused-import ones do not:
/// `module_size_outliers`, `build_determinism_absence`,
/// `dev_environment_declaration_absence`, `task_discovery_surface_absence`,
/// `generated_artifact_protection_absence`, and `write_safety_zone_absence`
/// have no backing `aoa-migrate` migration, so their `LowerIsBetter`
/// orientation is a *registered, falsifiable directional hypothesis*, not an
/// earned best-practice. The navigability and unused-import measures each have
/// a mechanical migration that drives them down (`aoa-migrate`); the others do
/// not (splitting a large module is not mechanically safe; pinning
/// dependencies, declaring a dev environment or task-discovery surface, a
/// generated-artifact marker, or a write boundary are human policy choices the
/// audit only *observes*). Their direction earns nothing until
/// external-outcome correlation confirms it — which is exactly what
/// pre-registering a hypothesis under R9c means.
pub const STRUCTURE_MEASURE_SPECS: &[(MetricName, &str)] = &[
    (
        MetricName::NavigabilityAnchorAbsence,
        "count of package roots (repo root + workspace member crates) lacking a \
README navigability anchor, per aoa-audit navigability_sites",
    ),
    (
        MetricName::ModuleSizeOutliers,
        "count of source files exceeding size_outlier_k × the repo's own median \
source-file line count (self-calibrating), per aoa-audit module_size_outlier_item; \
the LowerIsBetter orientation is a registered falsifiable hypothesis (no backing \
migration), promotable only by external-outcome correlation",
    ),
    (
        MetricName::UnusedImportProxy,
        "count of likely-unused imports by a syntactic proxy (a use-bound name \
absent from the file body), per aoa-audit unused_import_proxy_item",
    ),
    (
        MetricName::BuildDeterminismAbsence,
        "1 when no well-known dependency-pinning lockfile exists at the repo root \
(0 otherwise), per aoa-audit build_determinism_item; a pure fixed-path existence \
fact (Factory build-system pillar); the LowerIsBetter orientation is a registered \
falsifiable hypothesis (no backing migration), promotable only by external-outcome \
correlation",
    ),
    (
        MetricName::DevEnvironmentDeclarationAbsence,
        "1 when no reproducible dev-environment declaration (devcontainer / nix \
flake / toolchain or runtime version pin) exists at its well-known path (0 \
otherwise), per aoa-audit dev_environment_item; a pure fixed-path existence fact \
(Factory dev-environment pillar); the LowerIsBetter orientation is a registered \
falsifiable hypothesis (no backing migration), promotable only by external-outcome \
correlation",
    ),
    (
        MetricName::TaskDiscoverySurfaceAbsence,
        "1 when no task-discovery surface (issue-template path or in-repo issue \
tracker) exists at its well-known location (0 otherwise), per aoa-audit \
task_discovery_item; a pure fixed-path existence fact (Factory task-discovery \
pillar); the LowerIsBetter orientation is a registered falsifiable hypothesis (no \
backing migration), promotable only by external-outcome correlation",
    ),
    (
        MetricName::GeneratedArtifactProtectionAbsence,
        "count (0 or 1) of the well-known generated-artifact-protection marker \
(a root .gitattributes declaring linguist-generated) that is absent, per aoa-audit \
generated_artifact_protection_item; a pure convention-existence fact (R6 'mark \
generated files off-limits'), never a classification of which files are generated; \
the LowerIsBetter orientation is a registered falsifiable hypothesis (no backing \
migration), promotable only by external-outcome correlation",
    ),
    (
        MetricName::WriteSafetyZoneAbsence,
        "count of well-known write-boundary declaration surfaces (CODEOWNERS \
ownership map; .aoa safe-write-zone policy) absent from the repo, per aoa-audit \
write_safety_zone_item; a pure file-existence fact (R5 'narrow mutation gateway / \
ownership metadata'); the LowerIsBetter orientation is a registered falsifiable \
hypothesis (no backing migration), promotable only by external-outcome correlation",
    ),
];

/// One metric's classification within a [`ConstructValidityReport`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricClassification {
    pub metric: String,
    pub orientation: MetricOrientation,
    pub correlations: Vec<OutcomeCorrelation>,
    pub mode: MetricMode,
    /// Why the metric is in its mode, when the mode needs one — today only
    /// [`MetricMode::InsufficientData`] carries a reason. `None` for the
    /// evidence-driven modes, and absent from their wire form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The construct-validity artifact: the data source consulted, the thresholds
/// applied, and the resulting per-metric classification. Reproducible by
/// re-running the pipeline over the same source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstructValidityReport {
    pub data_source: String,
    pub thresholds: GatingThresholds,
    pub metrics: Vec<MetricClassification>,
}

impl ConstructValidityReport {
    /// Render the determination for the human register: a header, every gating
    /// candidate with its earned mode (Gating vs Advisory) and orientation, and
    /// the data source consulted. Surfaces the R9c discipline so an operator can
    /// see which metrics may gate a decision and which are advisory-only —
    /// rather than the discipline being documented but never shown.
    #[must_use]
    pub fn render_human(&self) -> String {
        let count = |mode| self.metrics.iter().filter(|m| m.mode == mode).count();
        let gating = count(MetricMode::Gating);
        let insufficient = count(MetricMode::InsufficientData);
        let advisory = self.metrics.len() - gating - insufficient;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "R9c construct validity: {} candidate(s), {gating} gating, {advisory} advisory, \
             {insufficient} insufficient-data",
            self.metrics.len(),
        );
        for m in &self.metrics {
            let dir = match m.orientation {
                MetricOrientation::HigherIsBetter => "higher-is-better",
                MetricOrientation::LowerIsBetter => "lower-is-better",
            };
            let _ = write!(out, "  [{}] {} ({dir})", m.mode.as_str(), m.metric);
            // The reason rides on the same line so an operator never sees a
            // bare InsufficientData tag without the why.
            match &m.reason {
                Some(reason) => {
                    let _ = writeln!(out, " — {reason}");
                }
                None => {
                    let _ = writeln!(out);
                }
            }
        }
        let _ = writeln!(out, "data source: {}", self.data_source);
        out
    }
}

/// Build a construct-validity artifact: classify each supplied per-metric
/// correlation report under `thresholds`, recording the `data_source` so the
/// result is reproducible and its provenance inspectable.
pub fn build_report(
    data_source: impl Into<String>,
    reports: &[CorrelationReport],
    thresholds: &GatingThresholds,
) -> ConstructValidityReport {
    let metrics = reports
        .iter()
        .map(|r| MetricClassification {
            metric: r.metric.clone(),
            orientation: r.orientation,
            correlations: r.correlations.clone(),
            mode: classify_metric(Some(r), thresholds),
            reason: None,
        })
        .collect();
    ConstructValidityReport {
        data_source: data_source.into(),
        thresholds: *thresholds,
        metrics,
    }
}

/// The documented data source consulted for the current determination, and the
/// reason it yields no external-outcome correlations.
pub const NO_EXTERNAL_OUTCOME_SOURCE: &str = "codeprobe run history (runs/codeprobe-*): \
no post-merge revert, production-incident, or human review-acceptance fields are recorded. \
The only per-trial outcome is the oracle pass/reward, which is conditioned on held-out success \
and is therefore circular for construct validity. ground_truth_commit anchors the oracle but \
correlating a metric with oracle agreement is the same tautology. No external-outcome corpus is \
available as of 2026-06-20, so every gating candidate stays advisory.";

/// The current R9c determination: with no external-outcome corpus available,
/// every gating candidate has no confirming correlation and is `Advisory`. The
/// returned artifact names the data source it consulted and is reproducible by
/// re-running this function — the executable form of "no metric gates a feature
/// until real external correlation exists".
pub fn current_determination() -> ConstructValidityReport {
    let reports: Vec<CorrelationReport> = GATING_CANDIDATES
        .iter()
        .map(|(metric, orientation)| CorrelationReport {
            metric: metric.to_string(),
            orientation: *orientation,
            correlations: Vec::new(),
        })
        .collect();
    build_report(
        NO_EXTERNAL_OUTCOME_SOURCE,
        &reports,
        &GatingThresholds::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lists_every_variant_with_unique_wire_names() {
        // Match every variant so adding one without listing it in ALL fails to
        // compile, keeping GATING_CANDIDATES (derived from ALL) exhaustive.
        for name in MetricName::ALL {
            match name {
                MetricName::RetrievalLocality
                | MetricName::EditLocality
                | MetricName::InvariantDiscoverability
                | MetricName::MutationSurface
                | MetricName::BudgetAdherence
                | MetricName::RewardHackingGap
                | MetricName::NavigabilityAnchorAbsence
                | MetricName::ModuleSizeOutliers
                | MetricName::UnusedImportProxy
                | MetricName::BuildDeterminismAbsence
                | MetricName::DevEnvironmentDeclarationAbsence
                | MetricName::TaskDiscoverySurfaceAbsence
                | MetricName::GeneratedArtifactProtectionAbsence
                | MetricName::WriteSafetyZoneAbsence => {}
            }
        }
        assert_eq!(MetricName::ALL.len(), 14);

        // The exhaustive as_str match guarantees every variant HAS a wire
        // literal, not that two variants don't share one — a copy-paste
        // collision would silently join the wrong metric downstream.
        let unique: std::collections::HashSet<&str> =
            MetricName::ALL.iter().map(|m| m.as_str()).collect();
        assert_eq!(unique.len(), MetricName::ALL.len());
    }

    #[test]
    fn render_human_lists_every_candidate_as_advisory_with_source() {
        let rendered = current_determination().render_human();
        // Names each pre-registered candidate and marks it Advisory.
        for (metric, _) in GATING_CANDIDATES {
            assert!(
                rendered.contains(metric.as_str()),
                "missing candidate {metric}"
            );
        }
        assert!(rendered.contains("Advisory"));
        assert!(
            !rendered.contains("Gating]"),
            "nothing gates absent a corpus"
        );
        assert!(rendered.contains("0 gating"));
        // Surfaces the consulted data source.
        assert!(rendered.contains(NO_EXTERNAL_OUTCOME_SOURCE));
    }
}
