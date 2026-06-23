//! The `recommend` pillar — the connective tissue that makes AOA's four pillars
//! (audit → evaluate → **recommend** → migrate) one instrument.
//!
//! [`recommend`] joins three independently-computed surfaces into one
//! operator-facing view, one record per audit finding:
//!
//! - **(a)** the ranked audit punch-list ([`aoa_audit::AuditReport`]) — measured
//!   facts, deliberately stripped of any "deficiency" framing (anti-Goodhart);
//! - **(b)** the R9c construct-validity determination
//!   ([`aoa_gap::ConstructValidityReport`]) — whether each metric may *gate* a
//!   decision (`Gating`) or is `Advisory` only;
//! - **(c)** the migration registry ([`aoa_migrate::all_fixes`]) — which fixes
//!   exist and under what eligibility precondition.
//!
//! ## The anti-Goodhart discipline, made executable
//!
//! A finding is tagged [`Actionability::ActionableNow`] **only** when a migration
//! exists *and* the finding's metric has earned `Gating` status. This is the
//! executable form of the rule "a recommendation may NOT assert a fix is worth
//! applying on a metric still Advisory": the `Gating` conjunct gates it. With no
//! external-outcome corpus available, every metric is currently `Advisory`
//! (see [`aoa_gap::current_determination`]), so every finding is
//! [`Actionability::AdvisoryOnly`] today — the honest result. The same code
//! promotes a finding to actionable-now automatically once a confirming
//! correlation lifts its metric to `Gating`.
//!
//! ## ZFC
//!
//! The join is mechanism, not judgment: a static, exhaustive, inspectable table
//! ([`join`]), set-membership against the live fix registry, dictionary lookup of
//! the metric mode, and a boolean actionability predicate. No semantic scoring,
//! no hardcoded thresholds — the gating decision is delegated wholly to
//! `aoa-gap`. `recommend` reads only a metric's *mode*, never its orientation.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use aoa_audit::{AuditReport, FindingKind, MeasuredCost, PunchItem, Tier};
use aoa_gap::{ConstructValidityReport, MetricMode};
use aoa_migrate::CodeFix;

/// Whether AOA recommends acting on a finding now, or merely surfaces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actionability {
    /// A migration exists AND the finding's metric has earned `Gating` status:
    /// AOA recommends applying the fix now.
    ActionableNow,
    /// AOA surfaces the finding but does not assert a fix is worth applying.
    AdvisoryOnly,
}

/// Why a finding is advisory-only — the dominant blocker keeping it from
/// actionable-now. `None` of these applies when the finding is actionable-now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdvisoryReason {
    /// No mechanical migration exists for this finding. It can never become
    /// actionable-now until one is built, regardless of construct validity
    /// (e.g. `module_size_outliers`, whose `LowerIsBetter` orientation is a
    /// registered hypothesis with no backing migration; or a missing plane).
    NoFixAvailable,
    /// A migration exists, but the finding's metric has not earned `Gating`
    /// status under R9c (it is `Advisory`, or — only for a partial determination
    /// supplied by a caller — unclassified). Applying the fix is the operator's
    /// call; AOA will not assert it is worthwhile until the metric gates
    /// (anti-Goodhart).
    MetricAdvisory,
}

/// One finding's recommendation: the measured fact (from the audit), its
/// construct-validity standing (from the gap determination), the available
/// migration (from the migrate registry), and the resulting tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingRecommendation {
    pub kind: FindingKind,
    pub title: String,
    pub tier: Tier,
    pub measured_cost: MeasuredCost,
    /// The construct-validity metric this finding informs, if any. `None` for a
    /// finding with no gating-candidate metric (e.g. a missing enforcement
    /// plane) — distinct from a metric that exists but is `Advisory`.
    pub metric: Option<String>,
    /// The metric's current `Advisory`/`Gating` mode, when the finding has one.
    pub metric_mode: Option<MetricMode>,
    /// The migration that addresses this finding, if one is registered (and
    /// present in the supplied registry).
    pub fix_id: Option<String>,
    /// That fix's R0 eligibility precondition, surfaced so the operator sees the
    /// constraint before applying.
    pub fix_eligibility: Option<String>,
    pub actionability: Actionability,
    /// Why the finding is advisory-only; `None` when it is actionable-now.
    pub advisory_reason: Option<AdvisoryReason>,
}

/// The joined recommendation set, in the audit's rank order, with the tag tallies
/// precomputed for the operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecommendationReport {
    pub items: Vec<FindingRecommendation>,
    pub actionable_now: usize,
    pub advisory_only: usize,
}

/// Join the audit punch-list with the construct-validity determination and the
/// migration registry, producing one [`FindingRecommendation`] per finding, in
/// the audit's rank order.
pub fn recommend(
    audit: &AuditReport,
    determination: &ConstructValidityReport,
    fixes: &[Box<dyn CodeFix>],
) -> RecommendationReport {
    let items: Vec<FindingRecommendation> = audit
        .items
        .iter()
        .map(|item| recommend_one(item, determination, fixes))
        .collect();
    let actionable_now = items
        .iter()
        .filter(|r| r.actionability == Actionability::ActionableNow)
        .count();
    let advisory_only = items.len() - actionable_now;
    RecommendationReport {
        items,
        actionable_now,
        advisory_only,
    }
}

/// Recommend on a single finding.
fn recommend_one(
    item: &PunchItem,
    determination: &ConstructValidityReport,
    fixes: &[Box<dyn CodeFix>],
) -> FindingRecommendation {
    let (metric_name, fix_name) = join(item.kind);

    // Resolve the named fix against the *live* registry: a campaign may pass a
    // subset of fixes, and a named-but-absent fix is treated as no fix (so the
    // recommendation reflects what can actually be applied, not the join table).
    let fix = fix_name.and_then(|id| fixes.iter().find(|f| f.id() == id));
    let fix_id = fix.map(|f| f.id().to_string());
    let fix_eligibility = fix.map(|f| f.eligibility_note().to_string());

    let metric_mode = metric_name.and_then(|name| mode_for(determination, name));

    let is_gating = metric_mode == Some(MetricMode::Gating);
    let (actionability, advisory_reason) = classify(fix.is_some(), is_gating);

    FindingRecommendation {
        kind: item.kind,
        title: item.title.clone(),
        tier: item.tier,
        measured_cost: item.measured_cost.clone(),
        metric: metric_name.map(str::to_string),
        metric_mode,
        fix_id,
        fix_eligibility,
        actionability,
        advisory_reason,
    }
}

/// The actionability decision: pure boolean mechanism over (a fix is present, the
/// metric has earned gating). Actionable-now requires BOTH. Otherwise
/// advisory-only, with the dominant blocker as the reason: no fix outranks an
/// un-promoted metric, because without a migration there is nothing to apply even
/// if the metric were gating.
///
/// `is_gating` is computed by the caller as `metric_mode == Some(Gating)`, so the
/// "not gating" case folds Advisory and unclassified (a metric absent from a
/// partial determination) together — the operative fact for both is "has not
/// earned gating". Taking a `bool` rather than `Option<MetricMode>` makes the
/// function total over `(bool, bool)`: there is no input that can produce a
/// reason inconsistent with the record's `metric_mode`.
fn classify(has_fix: bool, is_gating: bool) -> (Actionability, Option<AdvisoryReason>) {
    match (has_fix, is_gating) {
        (true, true) => (Actionability::ActionableNow, None),
        (false, _) => (
            Actionability::AdvisoryOnly,
            Some(AdvisoryReason::NoFixAvailable),
        ),
        (true, false) => (
            Actionability::AdvisoryOnly,
            Some(AdvisoryReason::MetricAdvisory),
        ),
    }
}

/// The current mode of the named metric in the determination, if classified.
fn mode_for(determination: &ConstructValidityReport, metric: &str) -> Option<MetricMode> {
    determination
        .metrics
        .iter()
        .find(|m| m.metric == metric)
        .map(|m| m.mode)
}

/// The join policy: the construct-validity metric and the migration fix (if any)
/// associated with each finding kind. The connective tissue — mechanism, not
/// judgment (ZFC): a static, exhaustive, inspectable table in one place.
///
/// Drift from the upstream registries is caught by the tests
/// (`every_joined_metric_is_a_gating_candidate`, `every_joined_fix_exists`), and
/// the exhaustive match makes a new [`FindingKind`] a compile error here until its
/// association is declared.
///
/// `ContextBudget` names `budget_adherence` though no migration drives it: the
/// overflow finding informs that construct-validity metric, and `recommend` reads
/// only the metric's *mode*, so the `HigherIsBetter`/overflow direction mismatch
/// is irrelevant. `UnusedImportProxy` joins the Rust `dead-imports` fix; the
/// audit's unused-import proxy is Rust-only, so the `dead-imports-python` /
/// `dead-imports-typescript` adapters in [`aoa_migrate::all_fixes`] correspond to
/// no audit finding and intentionally appear in no join row.
fn join(kind: FindingKind) -> (Option<&'static str>, Option<&'static str>) {
    match kind {
        FindingKind::ContextBudget => (Some("budget_adherence"), None),
        FindingKind::MutationSurface => (Some("mutation_surface"), None),
        FindingKind::MissingPlane => (None, None),
        FindingKind::NavigabilityAnchor => (
            Some("navigability_anchor_absence"),
            Some("navigability-anchor"),
        ),
        FindingKind::ModuleSizeOutlier => (Some("module_size_outliers"), None),
        FindingKind::UnusedImportProxy => (Some("unused_import_proxy"), Some("dead-imports")),
    }
}

impl RecommendationReport {
    /// Render the joined recommendation set for the human register: a header with
    /// the tag tallies, then one block per finding showing the measured fact, its
    /// metric and mode (or that it has none), the available fix (or none), and the
    /// verdict with its reason. A footer ties back to `aoa gap` when nothing gates.
    #[must_use]
    pub fn render_human(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "AOA recommendations ({} finding(s); {} actionable-now, {} advisory-only)",
            self.items.len(),
            self.actionable_now,
            self.advisory_only,
        );
        for (index, rec) in self.items.iter().enumerate() {
            let _ = writeln!(
                out,
                "{:>2}. [{}] {} — {} {}",
                index + 1,
                rec.tier.label(),
                rec.title,
                rec.measured_cost.value,
                rec.measured_cost.unit,
            );
            // The metric line distinguishes "no construct-validity metric" from
            // "metric present but Advisory" — both can be advisory-only, for
            // different reasons.
            match (&rec.metric, rec.metric_mode) {
                (Some(metric), Some(mode)) => {
                    let _ = writeln!(out, "    metric {metric}: {}", mode.as_str());
                }
                _ => {
                    let _ = writeln!(out, "    metric: none (not a gating candidate)");
                }
            }
            match &rec.fix_id {
                Some(id) => {
                    let _ = writeln!(out, "    fix available: {id}");
                }
                None => {
                    let _ = writeln!(out, "    fix: none");
                }
            }
            let _ = writeln!(out, "    {}", verdict_line(rec));
        }
        if self.actionable_now == 0 && !self.items.is_empty() {
            let _ = writeln!(
                out,
                "No metric has earned Gating status (see `aoa gap`); fix-backed findings stay \
                 advisory-only until external-outcome correlation promotes their metric."
            );
        }
        out
    }
}

/// The one-line verdict for a finding. Keyed on `advisory_reason`, which is the
/// state discriminant: `None` is exactly the actionable-now case (`classify`
/// returns no reason only with `ActionableNow`), so this matches the same three
/// states `classify` produces — no unreachable arm.
fn verdict_line(rec: &FindingRecommendation) -> String {
    match rec.advisory_reason {
        None => format!(
            "→ actionable-now: metric is Gating and a fix exists; run `aoa migrate --fix {}`",
            rec.fix_id.as_deref().unwrap_or("<fix>")
        ),
        Some(AdvisoryReason::NoFixAvailable) => {
            "→ advisory-only: no mechanical migration exists for this finding".to_string()
        }
        Some(AdvisoryReason::MetricAdvisory) => "→ advisory-only: a fix exists, but its metric is \
            Advisory — AOA does not assert it is worth applying yet (operator decides)"
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use aoa_gap::{
        build_report, CorrelationReport, ExternalOutcome, GatingThresholds, MetricOrientation,
        OutcomeCorrelation, GATING_CANDIDATES,
    };
    use aoa_migrate::all_fixes;

    fn item(kind: FindingKind, title: &str, tier: Tier, value: u64, unit: &str) -> PunchItem {
        PunchItem {
            title: title.to_string(),
            kind,
            tier,
            measured_cost: MeasuredCost::new(value, unit),
            plane: None,
        }
    }

    /// A determination in which exactly `target` is `Gating` (via a confirming
    /// correlation) and every other gating candidate stays `Advisory`. Models a
    /// future in which an external-outcome corpus has promoted one metric.
    fn determination_gating(target: &str) -> ConstructValidityReport {
        let reports: Vec<CorrelationReport> = GATING_CANDIDATES
            .iter()
            .map(|(metric, orientation)| CorrelationReport {
                metric: (*metric).to_string(),
                orientation: *orientation,
                correlations: if *metric == target {
                    vec![confirming_correlation(*orientation)]
                } else {
                    Vec::new()
                },
            })
            .collect();
        build_report(
            "synthetic test corpus",
            &reports,
            &GatingThresholds::default(),
        )
    }

    /// A correlation that clears the default gating thresholds, with the sign that
    /// *confirms* validity for the metric's orientation against `RevertRate`
    /// (a lower-is-better outcome).
    fn confirming_correlation(orientation: MetricOrientation) -> OutcomeCorrelation {
        // RevertRate is lower-is-better; a same-direction (LowerIsBetter) metric
        // confirms with a positive coefficient, an opposite one with negative.
        let positive = orientation == MetricOrientation::LowerIsBetter;
        OutcomeCorrelation {
            outcome: ExternalOutcome::RevertRate,
            coefficient: if positive { 0.6 } else { -0.6 },
            n: 10,
            p_value: 0.01,
        }
    }

    fn nav_item() -> PunchItem {
        item(
            FindingKind::NavigabilityAnchor,
            "package roots without a navigability anchor (README)",
            Tier::Tier3,
            2,
            "package roots",
        )
    }

    #[test]
    fn all_advisory_determination_yields_no_actionable_now() {
        let audit = AuditReport::new(vec![nav_item()]);
        let report = recommend(&audit, &aoa_gap::current_determination(), &all_fixes());

        assert_eq!(report.actionable_now, 0);
        assert_eq!(report.advisory_only, 1);
        let rec = &report.items[0];
        assert_eq!(rec.actionability, Actionability::AdvisoryOnly);
        // A fix exists, so the blocker is the un-promoted metric, not absence.
        assert_eq!(rec.advisory_reason, Some(AdvisoryReason::MetricAdvisory));
        assert_eq!(rec.fix_id.as_deref(), Some("navigability-anchor"));
        assert_eq!(rec.metric.as_deref(), Some("navigability_anchor_absence"));
        assert_eq!(rec.metric_mode, Some(MetricMode::Advisory));
        assert!(rec.fix_eligibility.is_some(), "fix eligibility surfaced");
    }

    #[test]
    fn synthetic_gating_promotes_a_fixable_finding_to_actionable_now() {
        // The headline path: when an external-outcome corpus lifts the metric to
        // Gating, the SAME join flips the finding to actionable-now. (Exercised
        // with a synthetic determination because the real one is all-Advisory.)
        let audit = AuditReport::new(vec![nav_item()]);
        let determination = determination_gating("navigability_anchor_absence");

        let report = recommend(&audit, &determination, &all_fixes());

        assert_eq!(report.actionable_now, 1);
        let rec = &report.items[0];
        assert_eq!(rec.actionability, Actionability::ActionableNow);
        assert_eq!(rec.advisory_reason, None);
        assert_eq!(rec.metric_mode, Some(MetricMode::Gating));
        assert_eq!(rec.fix_id.as_deref(), Some("navigability-anchor"));
    }

    #[test]
    fn actionable_now_renders_the_migrate_command() {
        // The operator call-to-action line: a regression in the fix id or the
        // `aoa migrate --fix` invocation would silently break the one line that
        // tells the operator what to run. (Untested by the all-advisory path.)
        let audit = AuditReport::new(vec![nav_item()]);
        let determination = determination_gating("navigability_anchor_absence");
        let rendered = recommend(&audit, &determination, &all_fixes()).render_human();
        assert!(rendered.contains("actionable-now"));
        assert!(
            rendered.contains("aoa migrate --fix navigability-anchor"),
            "the actionable verdict must name the exact migrate command"
        );
        // With something actionable, the all-advisory footer must not appear.
        assert!(!rendered.contains("No metric has earned Gating status"));
    }

    #[test]
    fn partial_determination_omitting_a_metric_stays_coherent() {
        // recommend() is public and infallible over an arbitrary determination. A
        // determination that omits a joined metric must not produce a record whose
        // reason contradicts its metric_mode: the fix-present + not-gating case is
        // advisory-only/metric-advisory with metric_mode None — consistent.
        let audit = AuditReport::new(vec![nav_item()]);
        let empty = build_report("partial corpus", &[], &GatingThresholds::default());

        let rec = &recommend(&audit, &empty, &all_fixes()).items[0];
        assert_eq!(rec.metric.as_deref(), Some("navigability_anchor_absence"));
        assert_eq!(
            rec.metric_mode, None,
            "metric absent from the determination"
        );
        assert_eq!(rec.actionability, Actionability::AdvisoryOnly);
        // Not gating (because unclassified) + a fix exists -> metric-advisory,
        // never actionable-now and never asserting the fix is worth applying.
        assert_eq!(rec.advisory_reason, Some(AdvisoryReason::MetricAdvisory));
    }

    #[test]
    fn gating_metric_without_a_fix_stays_advisory_no_fix() {
        // module_size_outliers has a metric but no backing migration (by design).
        // Even when its metric is Gating, no fix means advisory-only — the
        // no-fix blocker dominates the actionability decision.
        let audit = AuditReport::new(vec![item(
            FindingKind::ModuleSizeOutlier,
            "source files exceeding 4.0x the repo median size",
            Tier::Tier3,
            1,
            "outlier files",
        )]);
        let determination = determination_gating("module_size_outliers");

        let rec = &recommend(&audit, &determination, &all_fixes()).items[0];
        assert_eq!(rec.metric_mode, Some(MetricMode::Gating));
        assert_eq!(rec.actionability, Actionability::AdvisoryOnly);
        assert_eq!(rec.advisory_reason, Some(AdvisoryReason::NoFixAvailable));
        assert!(rec.fix_id.is_none());
    }

    #[test]
    fn missing_plane_has_no_metric_and_no_fix() {
        // A plane gap is not a gating-candidate metric: metric is None (distinct
        // from a metric that exists but is Advisory), and there is no migration.
        let mut plane = item(
            FindingKind::MissingPlane,
            "missing enforcement plane: runtime hook",
            Tier::Tier1,
            1,
            "missing plane",
        );
        plane.plane = Some(aoa_audit::EnforcementPlane::RuntimeHook);
        let audit = AuditReport::new(vec![plane]);

        let rec = &recommend(&audit, &aoa_gap::current_determination(), &all_fixes()).items[0];
        assert_eq!(rec.metric, None);
        assert_eq!(rec.metric_mode, None);
        assert_eq!(rec.fix_id, None);
        assert_eq!(rec.actionability, Actionability::AdvisoryOnly);
        assert_eq!(rec.advisory_reason, Some(AdvisoryReason::NoFixAvailable));
    }

    #[test]
    fn fix_absent_from_registry_is_treated_as_no_fix() {
        // The navigability finding's fix exists in the full registry, but a
        // campaign that excludes it (empty registry) must see no fix — the
        // recommendation reflects what can actually be applied.
        let audit = AuditReport::new(vec![nav_item()]);
        let determination = determination_gating("navigability_anchor_absence");

        let rec = &recommend(&audit, &determination, &[]).items[0];
        assert!(rec.fix_id.is_none(), "excluded fix is not reported");
        // Even with the metric Gating, no available fix -> advisory, no-fix.
        assert_eq!(rec.actionability, Actionability::AdvisoryOnly);
        assert_eq!(rec.advisory_reason, Some(AdvisoryReason::NoFixAvailable));
    }

    #[test]
    fn report_round_trips_through_json() {
        let audit = AuditReport::new(vec![nav_item()]);
        let report = recommend(&audit, &aoa_gap::current_determination(), &all_fixes());
        let json = serde_json::to_string(&report).expect("serialize");
        let parsed: RecommendationReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, report);
    }

    #[test]
    fn render_human_names_findings_and_the_gap_footer() {
        let audit = AuditReport::new(vec![nav_item()]);
        let rendered =
            recommend(&audit, &aoa_gap::current_determination(), &all_fixes()).render_human();
        assert!(rendered.contains("AOA recommendations"));
        assert!(rendered.contains("navigability anchor"));
        assert!(rendered.contains("advisory-only"));
        // The footer ties the empty actionable set back to the gap determination.
        assert!(rendered.contains("aoa gap"));
    }

    // --- drift guards against the upstream registries -------------------------

    #[test]
    fn every_joined_metric_is_a_gating_candidate() {
        let candidates: Vec<&str> = GATING_CANDIDATES.iter().map(|(m, _)| *m).collect();
        for kind in FindingKind::ALL {
            if let (Some(metric), _) = join(*kind) {
                assert!(
                    candidates.contains(&metric),
                    "{kind:?} joins metric '{metric}' absent from GATING_CANDIDATES"
                );
            }
        }
    }

    #[test]
    fn every_joined_fix_exists() {
        let registry = all_fixes();
        let ids: Vec<&str> = registry.iter().map(|f| f.id()).collect();
        for kind in FindingKind::ALL {
            if let (_, Some(fix_id)) = join(*kind) {
                assert!(
                    ids.contains(&fix_id),
                    "{kind:?} joins fix '{fix_id}' absent from all_fixes()"
                );
            }
        }
    }

    #[test]
    fn fix_bearing_kind_always_has_a_metric() {
        // The invariant that keeps the MetricAdvisory reason accurate: a fix
        // never appears without a metric, so the classify `(true, _)` arm always
        // means "metric present and Advisory", never "fix without a metric".
        for kind in FindingKind::ALL {
            let (metric, fix) = join(*kind);
            if fix.is_some() {
                assert!(
                    metric.is_some(),
                    "{kind:?} has a fix but no metric — MetricAdvisory would misreport"
                );
            }
        }
    }
}
