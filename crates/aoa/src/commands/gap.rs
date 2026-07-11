use std::fmt::Write as _;
use std::io::IsTerminal;

use anyhow::{Context, Result};

use aoa_gap::{CheckboxBaseline, CriterionStatus};

use crate::cli::{CheckboxBaselineArgs, GapArgs, GapCommand};
use crate::output::{print_human, print_json};

/// Dispatch the gap surfaces: the bare command renders the R9c
/// construct-validity determination; `checkbox-baseline` scores a repo against
/// the provisional Factory criteria table.
pub fn run(args: &GapArgs) -> Result<i32> {
    match &args.command {
        Some(GapCommand::CheckboxBaseline(a)) => checkbox_baseline(a, args.json),
        Some(GapCommand::MineCorpus(a)) => super::corpus::mine_corpus(a, args.json),
        None => determination(args.json),
    }
}

/// Surface the R9c construct-validity determination in the requested register.
///
/// This is the live, operator-facing consumer of
/// [`aoa_gap::current_determination`]: it reports which gating-candidate metrics
/// have earned `Gating` status (a confirming external-outcome correlation) and
/// which remain `Advisory`. With no external-outcome corpus available, every
/// candidate is advisory — the surface shows that explicitly rather than letting
/// any metric silently gate a decision.
fn determination(json: bool) -> Result<i32> {
    let report = aoa_gap::current_determination();

    if json {
        print_json(&report)?;
    } else {
        print_human(&report.render_human());
    }

    Ok(0)
}

/// Score one repo tree as a Factory checkbox baseline (aoa-d6t.28): the R0
/// study's baseline feature column, emitted as the full [`CheckboxBaseline`]
/// JSON for batch scoring or as a human summary. `--show-excluded` lists the
/// criteria that cannot be decided from the repo tree, with their reasons —
/// they are carried in the JSON register regardless, never dropped.
fn checkbox_baseline(args: &CheckboxBaselineArgs, json: bool) -> Result<i32> {
    let baseline = aoa_gap::score_repo(args.repo_id.as_str(), &args.root)
        .with_context(|| format!("failed to score {}", args.root.display()))?;

    if json {
        print_json(&baseline)?;
    } else {
        let style = Style {
            enabled: std::io::stdout().is_terminal(),
        };
        print_human(&render_baseline(&baseline, args.show_excluded, &style));
    }

    Ok(0)
}

/// ANSI styling for the human register, enabled only when stdout is a
/// terminal so piped and captured output stays byte-clean.
struct Style {
    enabled: bool,
}

impl Style {
    fn bold(&self, text: &str) -> String {
        self.paint("1", text)
    }

    fn yellow(&self, text: &str) -> String {
        self.paint("33", text)
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

/// Render the checkbox baseline for the human register: the achieved level
/// under the 80%-progression gate, per-level and per-pillar tallies (excluded
/// criteria never enter a denominator), and — behind `show_excluded` — every
/// excluded criterion with its reason.
fn render_baseline(baseline: &CheckboxBaseline, show_excluded: bool, style: &Style) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Factory checkbox baseline: {}", baseline.repo_id);
    let level_name = aoa_gap::FACTORY_LEVEL_NAMES[usize::from(baseline.level) - 1];
    let _ = writeln!(
        out,
        "{}",
        style.bold(&format!(
            "level {} ({level_name}) under the 80%-progression gate",
            baseline.level
        ))
    );
    let _ = writeln!(out, "criteria version: {}", baseline.criteria_version);

    let _ = writeln!(out, "levels (passed/evaluated):");
    for l in &baseline.levels {
        let _ = writeln!(
            out,
            "  level {} {:<12} {}",
            l.level,
            l.name,
            tally_text(l.passed, l.evaluated, l.excluded)
        );
    }

    let _ = writeln!(out, "pillars (passed/evaluated):");
    for p in &baseline.pillars {
        let _ = writeln!(
            out,
            "  {:<28} {}",
            p.pillar.as_str(),
            tally_text(p.passed, p.evaluated, p.excluded)
        );
    }

    let excluded: Vec<_> = baseline
        .criteria
        .iter()
        .filter_map(|c| match &c.status {
            CriterionStatus::Excluded { reason } => Some((c, reason)),
            _ => None,
        })
        .collect();
    if show_excluded {
        let _ = writeln!(
            out,
            "excluded criteria (not mechanically checkable; never scored):"
        );
        for (c, reason) in &excluded {
            let _ = writeln!(
                out,
                "  {} ({}, level {}): {reason}",
                style.yellow(&c.id),
                c.pillar.as_str(),
                c.level
            );
        }
    } else {
        let _ = writeln!(
            out,
            "{} criteria excluded from scoring (--show-excluded lists the reasons)",
            excluded.len()
        );
    }
    out
}

/// One tally line: `passed/evaluated passed`, plus the excluded count when
/// present; a subset with no mechanical criteria says so instead of `0/0`.
fn tally_text(passed: usize, evaluated: usize, excluded: usize) -> String {
    let base = if evaluated == 0 {
        "no mechanical criteria".to_string()
    } else {
        format!("{passed}/{evaluated} passed")
    };
    if excluded == 0 {
        base
    } else {
        format!("{base}, {excluded} excluded")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline() -> CheckboxBaseline {
        let dir = tempfile::tempdir().unwrap();
        aoa_gap::score_repo("styled", dir.path()).unwrap()
    }

    #[test]
    fn terminal_style_emits_ansi_and_pipe_style_stays_clean() {
        let colored = render_baseline(&baseline(), true, &Style { enabled: true });
        assert!(colored.contains("\x1b[1m"), "level line is bold on a tty");
        assert!(
            colored.contains("\x1b[33m"),
            "excluded ids are yellow on a tty"
        );

        let plain = render_baseline(&baseline(), true, &Style { enabled: false });
        assert!(
            !plain.contains('\u{1b}'),
            "piped output carries no ANSI escapes"
        );
    }

    #[test]
    fn tally_text_never_prints_a_zero_denominator() {
        assert_eq!(tally_text(4, 4, 0), "4/4 passed");
        assert_eq!(tally_text(2, 7, 2), "2/7 passed, 2 excluded");
        assert_eq!(tally_text(0, 0, 2), "no mechanical criteria, 2 excluded");
    }
}
