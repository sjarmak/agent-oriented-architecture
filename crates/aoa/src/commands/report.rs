//! `aoa report`: the end-to-end operator readiness view.
//!
//! Composes the four pillars into ONE view (human + JSON dual-register, R17):
//! the ranked audit punch-list, the R9c Advisory/Gating determination, the
//! migration registry, and the recommend join — plus, when a
//! `falsification.json` exists under the repo root, the R0 verdict that gates
//! whether the migrate pillar is live at all. It answers: "how agent-ready is
//! this repo, what would I change, and am I allowed to act on the metric that
//! says so".
//!
//! Absent inputs are reported as absent, never fabricated: no
//! `falsification.json` means the R0 gate has not produced a verdict, so the
//! view says exactly that (and the pillar is not live). A present but
//! unparsable `falsification.json` is a hard error — silently treating it as
//! absent would fabricate "the gate never ran".
//!
//! Exit code is always 0: like `recommend`, a readiness view must not pressure
//! an operator to act on advisory findings.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use aoa_audit::AuditReport;
use aoa_construct::ConstructValidityReport;
use aoa_falsify::Verdict;
use aoa_recommend::RecommendationReport;

use crate::cli::ReportArgs;
use crate::commands::fsutil::load_json_capped;
use crate::output::{escape_terminal, print_human, print_json};

/// The filename `aoa falsify` writes by default, consulted under the repo root.
const FALSIFICATION_FILENAME: &str = "falsification.json";

/// One registered migration, as surfaced to the operator: what exists and under
/// what R0 eligibility precondition.
#[derive(Debug, Clone, Serialize)]
struct MigrationView {
    id: String,
    description: String,
    eligibility: String,
}

/// The R0 falsification input as the report found it. `Absent` is a first-class
/// state — the gate simply has not produced a verdict for this repo.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum FalsificationView {
    Absent,
    Present {
        verdict: Verdict,
        /// Set when the recorded verdict came from an unmet precondition (e.g.
        /// `too_few_repos`) rather than the gate's own logic.
        #[serde(skip_serializing_if = "Option::is_none")]
        precondition_unmet: Option<String>,
    },
}

/// The slice of `falsification.json` this command reads (see the
/// `FalsificationOutput` schema in the `falsify` command).
///
/// Being a declared slice, it must tolerate unknown fields — `deny_unknown_fields`
/// would reject every real file. Note the direction of the default, which is the
/// opposite of `falsify::BuildMeta`'s: `precondition_unmet` defaults to `None`,
/// so a hand-edited file that misspells the key reads as a clean `proceed` and
/// `pillar_live` reports the migrate pillar live on a verdict that actually
/// carried an unmet precondition. That is fail-open, and it is why this type
/// must stay a read of tool output rather than an operator input boundary.
#[derive(Debug, Deserialize)]
struct FalsificationSlice {
    verdict: Verdict,
    #[serde(default)]
    precondition_unmet: Option<String>,
}

/// The composed readiness view — the JSON register.
#[derive(Debug, Serialize)]
struct ReadinessView {
    audit: AuditReport,
    construct_validity: ConstructValidityReport,
    migrations: Vec<MigrationView>,
    recommendations: RecommendationReport,
    falsification: FalsificationView,
    /// True only for a present, real (no unmet precondition) `proceed` verdict.
    migrate_pillar_live: bool,
}

/// Compose the readiness view and render it in the requested register.
pub fn run(args: &ReportArgs) -> Result<i32> {
    let cfg = crate::commands::audit::repo_audit_config(&args.repo)?;
    let audit = aoa_audit::audit(&args.repo, &cfg)
        .with_context(|| format!("failed to audit {}", args.repo.display()))?;
    // Condition on the signal the audit just measured, as `aoa recommend`
    // does. `report` composes both into one document, so an unconditioned
    // determination would contradict the audit half it ships alongside.
    let determination = aoa_construct::determination_with_signal(&audit.behavioral_signal);
    let fixes = aoa_migrate::all_fixes();
    let recommendations = aoa_recommend::recommend(&audit, &determination, &fixes);
    let migrations: Vec<MigrationView> = fixes
        .iter()
        .map(|f| MigrationView {
            id: f.id().to_string(),
            description: f.describe().to_string(),
            eligibility: f.eligibility_note().to_string(),
        })
        .collect();
    let falsification = load_falsification(&args.repo.join(FALSIFICATION_FILENAME))?;

    let view = ReadinessView {
        migrate_pillar_live: pillar_live(&falsification),
        audit,
        construct_validity: determination,
        migrations,
        recommendations,
        falsification,
    };

    if args.json {
        print_json(&view)?;
    } else {
        print_human(&render_human(&view, &args.repo));
    }
    Ok(0)
}

/// Read the R0 verdict slice from `path`. A missing file is the `Absent` state;
/// a present but unreadable/unparsable file is a hard error.
fn load_falsification(path: &Path) -> Result<FalsificationView> {
    if !path.exists() {
        return Ok(FalsificationView::Absent);
    }
    let slice: FalsificationSlice = load_json_capped(path, "falsification slice")?;
    Ok(FalsificationView::Present {
        verdict: slice.verdict,
        precondition_unmet: slice.precondition_unmet,
    })
}

/// Whether the migrate pillar is live: exactly a present, real `proceed`
/// verdict. A `proceed` carrying a precondition discriminator is contradictory
/// (the falsify command never writes one), so it is conservatively not live.
fn pillar_live(falsification: &FalsificationView) -> bool {
    matches!(
        falsification,
        FalsificationView::Present {
            verdict: Verdict::Proceed,
            precondition_unmet: None,
        }
    )
}

/// The snake_case verdict label, matching the JSON register's serde rendering.
fn verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Proceed => "proceed",
        Verdict::Pivot => "pivot",
        Verdict::Inconclusive => "inconclusive",
    }
}

/// Render the composed view for the human register: a headline with the migrate
/// pillar's status, then one section per input, reusing each report's own
/// human rendering.
fn render_human(view: &ReadinessView, repo: &Path) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "AOA readiness report: {}", repo.display());
    let _ = writeln!(out, "migrate pillar: {}", render_pillar(view));
    let _ = writeln!(out, "\n== Audit punch-list ==");
    out.push_str(&view.audit.render_human());
    let _ = writeln!(out, "\n== Construct validity (may a metric gate?) ==");
    out.push_str(&view.construct_validity.render_human());
    let _ = writeln!(out, "\n== Available migrations ==");
    for m in &view.migrations {
        let _ = writeln!(out, "  {}: {}", m.id, m.description);
    }
    let _ = writeln!(out, "\n== Recommendations ==");
    out.push_str(&view.recommendations.render_human());
    let _ = writeln!(out, "\n== R0 falsification gate ==");
    out.push_str(&render_falsification(&view.falsification));
    out
}

/// The one-line migrate-pillar status for the headline.
fn render_pillar(view: &ReadinessView) -> String {
    match &view.falsification {
        FalsificationView::Absent => {
            "not live (no falsification.json: the R0 gate has not produced a verdict)".to_string()
        }
        FalsificationView::Present { verdict, .. } if view.migrate_pillar_live => {
            format!("LIVE (R0 verdict: {})", verdict_label(*verdict))
        }
        FalsificationView::Present { verdict, .. } => {
            format!("not live (R0 verdict: {})", verdict_label(*verdict))
        }
    }
}

/// The falsification section body.
fn render_falsification(falsification: &FalsificationView) -> String {
    let mut out = String::new();
    match falsification {
        FalsificationView::Absent => {
            let _ = writeln!(
                out,
                "falsification.json: absent — run `aoa falsify` to exercise the R0 wrong-layer \
                 gate; until it returns `proceed`, the migrate pillar has not earned live status."
            );
        }
        FalsificationView::Present {
            verdict,
            precondition_unmet,
        } => {
            let _ = writeln!(out, "verdict: {}", verdict_label(*verdict));
            if let Some(kind) = precondition_unmet {
                let _ = writeln!(
                    out,
                    "  precondition unmet: {} (not a gate verdict)",
                    escape_terminal(kind)
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tolerance documented on `FalsificationSlice` is a decision, so it is
    /// pinned here rather than only asserted in prose: a real `falsification.json`
    /// carries the rest of `FalsificationOutput` beside the two fields this slice
    /// reads, and `deny_unknown_fields` would reject every one of them.
    #[test]
    fn falsification_slice_tolerates_the_rest_of_the_report() {
        let slice: FalsificationSlice = serde_json::from_str(
            r#"{ "verdict": "proceed", "repos": [], "k_runs": 3,
                 "min_effect_size": 0.05, "generated_at": "2026-07-21" }"#,
        )
        .expect("the slice must read a full falsification.json, not just its own two fields");
        assert!(matches!(slice.verdict, Verdict::Proceed));
        assert!(slice.precondition_unmet.is_none());
    }

    fn present(verdict: Verdict, precondition_unmet: Option<&str>) -> FalsificationView {
        FalsificationView::Present {
            verdict,
            precondition_unmet: precondition_unmet.map(str::to_string),
        }
    }

    #[test]
    fn pillar_live_only_on_a_real_proceed() {
        assert!(pillar_live(&present(Verdict::Proceed, None)));
        assert!(!pillar_live(&present(Verdict::Pivot, None)));
        assert!(!pillar_live(&present(Verdict::Inconclusive, None)));
        assert!(!pillar_live(&FalsificationView::Absent));
        // A contradictory proceed-with-precondition is conservatively not live.
        assert!(!pillar_live(&present(
            Verdict::Proceed,
            Some("too_few_repos")
        )));
    }

    #[test]
    fn load_falsification_reports_a_missing_file_as_absent() {
        let dir = std::env::temp_dir().join(format!("aoa-report-absent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let view = load_falsification(&dir.join(FALSIFICATION_FILENAME)).unwrap();
        assert_eq!(view, FalsificationView::Absent);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_falsification_fails_loud_on_malformed_json() {
        let dir = std::env::temp_dir().join(format!("aoa-report-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FALSIFICATION_FILENAME);
        std::fs::write(&path, "not json").unwrap();
        let err = load_falsification(&path).unwrap_err();
        assert!(format!("{err:#}").contains("failed to parse"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn falsification_section_escapes_the_precondition_kind() {
        // precondition_unmet is free text from an on-disk JSON file; a crafted
        // value must not inject raw control bytes into the terminal.
        let rendered =
            render_falsification(&present(Verdict::Inconclusive, Some("bad\u{1b}[2Jkind")));
        assert!(!rendered.contains('\u{1b}'), "raw ESC leaked: {rendered:?}");
        assert!(rendered.contains("\\u{1b}"));
    }
}
