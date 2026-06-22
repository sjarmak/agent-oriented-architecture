//! `aoa falsify`: the R0/R0' falsification gate over paired-repo evidence.
//!
//! The gate logic lives in `aoa_falsify`; this command is the IO + policy shell
//! around it. It adds three things the pure crate deliberately does not:
//!
//! 1. **Precondition wrapper.** `aoa_falsify::falsify` returns a structural error
//!    (not a verdict) when the input cannot be evaluated — fewer than five repos
//!    being the case a smoke run hits. The crate is right to keep that an input
//!    error: a 1-repo run must NOT produce a verdict byte-identical to a real
//!    5-repo abstention. So this shell catches `TooFewRepos` and writes a
//!    `falsification.json` whose `verdict` is `inconclusive` but which carries a
//!    `precondition_unmet` discriminator a genuine gate verdict never has.
//! 2. **Convention-degradation abstention.** When the build report
//!    (`--build-meta`) flags degraded convention inputs, the R0' convention-
//!    invariance check cannot be exercised, so the verdict abstains rather than
//!    asserting a `proceed`/`pivot` the hardening cannot back.
//! 3. **codeprobe bias surfacing.** `--bias-warnings reports/aggregate.json`
//!    attaches codeprobe's measurement-bias warnings ALONGSIDE the AOA verdict —
//!    never mutating it. A `no_independent_baseline` warning is flagged as
//!    gate-invalidating so the operator cannot miss it.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use aoa_falsify::{FalsifyError, FalsifyInput, FalsifyReport, Verdict};

use crate::cli::FalsifyArgs;
use crate::commands::fsutil::{read_to_string_capped, MAX_JSON_BYTES};
use crate::output::{print_human, print_json};

/// A codeprobe measurement-bias warning, surfaced verbatim alongside the verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BiasWarning {
    kind: String,
    #[serde(default = "default_severity")]
    severity: String,
    message: String,
    #[serde(default)]
    detail: serde_json::Value,
}

fn default_severity() -> String {
    "warning".to_string()
}

/// codeprobe's bias-warning kind that invalidates its own aggregate ranking:
/// every task's ground truth came from the same backend set, so there is no
/// independent baseline. Coupled by string to codeprobe's `bias_detection.py`;
/// if that kind is renamed upstream, this detection silently stops firing.
const NO_INDEPENDENT_BASELINE: &str = "no_independent_baseline";

/// The slice of codeprobe's `reports/aggregate.json` this command reads.
#[derive(Debug, Deserialize)]
struct AggregateFile {
    #[serde(default)]
    bias_warnings: Vec<BiasWarning>,
}

/// The slice of `aoa eval experiment`'s build report this command reads.
#[derive(Debug, Deserialize)]
struct BuildMeta {
    #[serde(default)]
    convention_inputs_degraded: bool,
}

/// The `falsification.json` payload.
///
/// Keeps `verdict`/`repo_delta`/`harness_delta`/`notes` field names compatible
/// with the pure `FalsifyReport`, and adds `precondition_unmet` (set only when
/// the gate could not produce a real verdict) and the bias surface.
#[derive(Debug, Serialize)]
struct FalsificationOutput {
    verdict: Verdict,
    /// Set ONLY when the verdict comes from an unmet precondition rather than the
    /// gate's own logic. A real 5-repo abstention leaves this `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    precondition_unmet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo_delta: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    harness_delta: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    eligible_repos: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    excluded_repos: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    conventions_tried: Vec<String>,
    notes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    bias_warnings: Vec<BiasWarning>,
    /// True when a `no_independent_baseline` bias warning is present: codeprobe's
    /// comparison has no independent baseline, so its ranking is uninterpretable.
    /// Surfaced for the operator; it does NOT change the AOA verdict.
    bias_gate_invalidating: bool,
}

impl FalsificationOutput {
    /// Build from a real gate report (the gate ran to completion).
    fn from_report(report: FalsifyReport) -> Self {
        FalsificationOutput {
            verdict: report.verdict,
            precondition_unmet: None,
            repo_delta: Some(report.repo_delta),
            harness_delta: Some(report.harness_delta),
            eligible_repos: report.eligible_repos,
            excluded_repos: report.excluded_repos,
            conventions_tried: report.conventions_tried,
            notes: report.notes,
            bias_warnings: Vec::new(),
            bias_gate_invalidating: false,
        }
    }

    /// Build an abstaining output for an unmet precondition (no real verdict).
    fn precondition(kind: &str, note: String) -> Self {
        FalsificationOutput {
            verdict: Verdict::Inconclusive,
            precondition_unmet: Some(kind.to_string()),
            repo_delta: None,
            harness_delta: None,
            eligible_repos: Vec::new(),
            excluded_repos: Vec::new(),
            conventions_tried: Vec::new(),
            notes: vec![note],
            bias_warnings: Vec::new(),
            bias_gate_invalidating: false,
        }
    }
}

/// Run the gate, applying the precondition/abstention policy, then write
/// `falsification.json`. Exit code: `0` for a real gate verdict (proceed, pivot,
/// or a genuine R0' abstention); `1` when the verdict comes from an unmet
/// precondition (the gate could not be exercised).
pub fn run(args: &FalsifyArgs) -> Result<i32> {
    let raw = read_to_string_capped(&args.repos, MAX_JSON_BYTES)
        .with_context(|| format!("failed to read falsify input {}", args.repos.display()))?;
    let input: FalsifyInput = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse falsify input {}", args.repos.display()))?;

    let degraded = match &args.build_meta {
        Some(path) => load_build_meta(path)?.convention_inputs_degraded,
        None => false,
    };

    let (mut output, mut exit) = match aoa_falsify::falsify(&input) {
        Ok(report) => (FalsificationOutput::from_report(report), 0),
        // Too few repos is the only structural error reframed as a precondition:
        // it IS the power-precondition shortfall a smoke run hits, and a 1-repo
        // input genuinely cannot establish a cross-repo majority.
        Err(FalsifyError::TooFewRepos(n)) => (
            FalsificationOutput::precondition(
                "too_few_repos",
                format!("only {n} repo(s) submitted; R0 needs >= 5 to reason about a majority"),
            ),
            1,
        ),
        // Other errors are genuinely malformed evidence (missing run snapshots,
        // k_runs < 3) — propagate loud, do not launder into a verdict.
        Err(e) => return Err(anyhow::Error::new(e).context("falsification gate failed")),
    };

    // Degraded convention inputs => the convention-invariance precondition cannot
    // be exercised, so abstain regardless of what the base tally computed. Keep
    // the gate's deltas/repos for transparency but override the headline verdict.
    if degraded && output.precondition_unmet.is_none() {
        output.verdict = Verdict::Inconclusive;
        output.precondition_unmet = Some("convention_inputs_degraded".to_string());
        output.notes.push(
            "convention inputs degraded (no per-repo symbol graph): R0' convention-invariance \
             not exercisable; verdict abstains to inconclusive"
                .to_string(),
        );
        exit = 1;
    } else if degraded {
        // A different precondition already drove the verdict (e.g. too_few_repos).
        // Still record that convention inputs were degraded so falsification.json
        // is self-contained — the operator should not have to read the build
        // report to learn the second blocker also held.
        output
            .notes
            .push("convention inputs were also degraded (see build report)".to_string());
    }

    if let Some(path) = &args.bias_warnings {
        let warnings = load_bias_warnings(path)?;
        output.bias_gate_invalidating = warnings.iter().any(|w| w.kind == NO_INDEPENDENT_BASELINE);
        if output.bias_gate_invalidating {
            output.notes.push(
                "codeprobe flagged no_independent_baseline: its comparison has no independent \
                 baseline. This is surfaced alongside, and does NOT alter, the AOA verdict."
                    .to_string(),
            );
        }
        output.bias_warnings = warnings;
    }

    let serialized = serde_json::to_string_pretty(&output)?;
    std::fs::write(&args.out, format!("{serialized}\n"))
        .with_context(|| format!("failed to write {}", args.out.display()))?;

    if args.json {
        print_json(&output)?;
    } else {
        print_human(&render_human(&output, &args.out));
    }
    Ok(exit)
}

fn load_build_meta(path: &Path) -> Result<BuildMeta> {
    let raw = read_to_string_capped(path, MAX_JSON_BYTES)
        .with_context(|| format!("failed to read build report {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse build report {}", path.display()))
}

fn load_bias_warnings(path: &Path) -> Result<Vec<BiasWarning>> {
    let raw = read_to_string_capped(path, MAX_JSON_BYTES)
        .with_context(|| format!("failed to read bias-warnings file {}", path.display()))?;
    let aggregate: AggregateFile = serde_json::from_str(&raw).with_context(|| {
        format!(
            "failed to parse codeprobe aggregate.json {} (expected a bias_warnings array)",
            path.display()
        )
    })?;
    Ok(aggregate.bias_warnings)
}

/// Escape each repo id and join with `, ` for safe terminal display.
fn escape_join(ids: &[String]) -> String {
    ids.iter()
        .map(|id| id.escape_debug().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_human(output: &FalsificationOutput, out_path: &Path) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(s, "falsification verdict: {:?}", output.verdict);
    if let Some(kind) = &output.precondition_unmet {
        let _ = writeln!(s, "  precondition unmet: {kind} (not a gate verdict)");
    }
    if let (Some(repo), Some(harness)) = (output.repo_delta, output.harness_delta) {
        let _ = writeln!(s, "  repo-delta: {repo:.4}");
        let _ = writeln!(s, "  harness-delta: {harness:.4}");
    }
    if !output.eligible_repos.is_empty() || !output.excluded_repos.is_empty() {
        // repo_id is operator-authored free text from the FalsifyInput; escape it
        // so a crafted id cannot inject terminal control sequences (matches the
        // hardening in r0b/eval_run).
        let _ = writeln!(
            s,
            "  eligible: {} | excluded: {}",
            escape_join(&output.eligible_repos),
            escape_join(&output.excluded_repos),
        );
    }
    if !output.bias_warnings.is_empty() {
        let _ = writeln!(s, "  codeprobe bias warnings:");
        for w in &output.bias_warnings {
            // severity/kind/message come VERBATIM from codeprobe's untrusted
            // aggregate.json — escape all three before they reach the terminal.
            let _ = writeln!(
                s,
                "    [{}/{}] {}",
                w.severity.escape_debug(),
                w.kind.escape_debug(),
                w.message.escape_debug(),
            );
        }
        if output.bias_gate_invalidating {
            let _ = writeln!(
                s,
                "    ! no_independent_baseline: codeprobe ranking uninterpretable"
            );
        }
    }
    if !output.notes.is_empty() {
        let _ = writeln!(s, "  notes: {}", output.notes.join("; "));
    }
    let _ = writeln!(s, "  written: {}", out_path.display());
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Write an oversized JSON file (one byte past the cap) and return its path.
    fn oversized_file(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, vec![b'x'; (MAX_JSON_BYTES + 1) as usize]).unwrap();
        path
    }

    #[test]
    fn all_falsify_load_sites_reject_oversized_input() {
        let dir = std::env::temp_dir().join(format!("aoa-falsify-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // load_build_meta (--build-meta)
        let err = load_build_meta(&oversized_file(&dir, "build.json")).unwrap_err();
        assert!(
            format!("{err:#}").contains("byte cap"),
            "build_meta: {err:#}"
        );

        // load_bias_warnings (codeprobe aggregate.json)
        let err = load_bias_warnings(&oversized_file(&dir, "aggregate.json")).unwrap_err();
        assert!(
            format!("{err:#}").contains("byte cap"),
            "bias_warnings: {err:#}"
        );

        // run (--repos falsify input) — the inline capped read trips before parse.
        let args = FalsifyArgs {
            repos: oversized_file(&dir, "repos.json"),
            build_meta: None,
            bias_warnings: None,
            out: dir.join("falsification.json"),
            json: false,
        };
        let err = run(&args).unwrap_err();
        assert!(format!("{err:#}").contains("byte cap"), "repos: {err:#}");

        std::fs::remove_dir_all(&dir).ok();
    }
}
