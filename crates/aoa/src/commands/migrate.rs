use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use aoa_migrate::{CodeFix, FixEligibility, FixProvenance, MigrationPlan};

use crate::cli::MigrateArgs;
use crate::output::{print_human, print_json};

/// Run the migration engine: preview (default), apply, or rollback. Every mode
/// renders both a human and a JSON register (R17).
pub fn run(args: &MigrateArgs) -> Result<i32> {
    if args.rollback {
        return run_rollback(args);
    }

    let plan = build_plan(args)?;

    if args.apply {
        run_apply(args, plan)
    } else {
        run_preview(args, plan)
    }
}

/// Build the plan from the fix registry, restricted to `--fix` ids when given.
/// An unknown id is a hard error (never silently ignored) so a campaign's pinned
/// treatment set is exactly what runs.
fn build_plan(args: &MigrateArgs) -> Result<MigrationPlan> {
    let registry = aoa_migrate::all_fixes();
    let known: Vec<&str> = registry.iter().map(|f| f.id()).collect();
    for id in &args.fix {
        if !known.contains(&id.as_str()) {
            bail!("unknown fix id '{id}' (known: {})", known.join(", "));
        }
    }
    let selected: Vec<&dyn CodeFix> = registry
        .iter()
        .filter(|f| args.fix.is_empty() || args.fix.iter().any(|id| id == f.id()))
        .map(|f| f.as_ref())
        .collect();

    MigrationPlan::build(&args.repo, &selected)
        .with_context(|| format!("failed to plan migration for {}", args.repo.display()))
}

/// Dry-run: surface the grounding audit finding, preview the diff, write nothing.
fn run_preview(args: &MigrateArgs, plan: MigrationPlan) -> Result<i32> {
    let grounding = navigability_count(&args.repo)?;

    if args.json {
        let changes = change_views(&plan);
        print_json(&MigrateView::Plan {
            grounding_navigability_sites: grounding,
            fix_ids: plan.fix_ids,
            changes,
            eligibility_notes: plan.eligibility_notes,
            provenance: plan.provenance,
        })?;
    } else {
        let mut out = format!(
            "AOA migrate (dry-run): audit reports {grounding} package root(s) without a navigability anchor.\n\n"
        );
        out.push_str(&plan.render_diff());
        if !plan.is_empty() {
            out.push_str("\nRun with --apply to write these changes (archived for rollback).\n");
        }
        out.push_str(&render_eligibility(&plan.eligibility_notes));
        out.push_str(&render_provenance(&plan.provenance));
        print_human(&out);
    }
    Ok(0)
}

/// Write the changes, then independently re-audit to verify the spec is met
/// (verify, not define — the migration's success is spec conformance, and the
/// audit confirms it; it does not define it).
fn run_apply(args: &MigrateArgs, plan: MigrationPlan) -> Result<i32> {
    // Nothing to do: skip apply() entirely so an already-conforming repo never
    // writes (and never clobbers) a manifest.
    if plan.is_empty() {
        if args.json {
            print_json(&MigrateView::Apply {
                fixes_applied: Vec::new(),
                files_written: 0,
                navigability_sites_remaining: navigability_count(&args.repo)?,
                manifest_path: String::new(),
                eligibility_notes: Vec::new(),
                provenance: Vec::new(),
            })?;
        } else {
            print_human("AOA migrate: repo already conforms; nothing to apply.\n");
        }
        return Ok(0);
    }

    let manifest = aoa_migrate::apply(&args.repo, &plan)
        .with_context(|| format!("failed to apply migration to {}", args.repo.display()))?;
    let remaining = navigability_count(&args.repo)?;

    if args.json {
        print_json(&MigrateView::Apply {
            files_written: manifest.entries.len(),
            fixes_applied: manifest.fixes_applied,
            navigability_sites_remaining: remaining,
            manifest_path: aoa_migrate::manifest_path(&args.repo).display().to_string(),
            eligibility_notes: manifest.eligibility_notes,
            provenance: manifest.provenance,
        })?;
    } else {
        let mut out = format!(
            "AOA migrate (applied): wrote {} file(s) via fix(es) [{}].\n\
             Re-audit verifies {remaining} navigability site(s) remaining.\n\
             Rollback record: {}\n",
            manifest.entries.len(),
            manifest.fixes_applied.join(", "),
            aoa_migrate::manifest_path(&args.repo).display(),
        );
        out.push_str(&render_eligibility(&manifest.eligibility_notes));
        out.push_str(&render_provenance(&manifest.provenance));
        print_human(&out);
    }
    Ok(0)
}

/// Undo the recorded migration, restoring the baseline checkout.
fn run_rollback(args: &MigrateArgs) -> Result<i32> {
    let manifest = aoa_migrate::read_manifest(&args.repo).with_context(|| {
        format!(
            "no migration manifest to roll back in {}",
            args.repo.display()
        )
    })?;
    let reverted = manifest.entries.len();
    aoa_migrate::rollback(&args.repo)
        .with_context(|| format!("failed to roll back migration in {}", args.repo.display()))?;

    if args.json {
        print_json(&MigrateView::Rollback {
            files_reverted: reverted,
        })?;
    } else {
        print_human(&format!(
            "AOA migrate (rolled back): reverted {reverted} file(s) to baseline.\n"
        ));
    }
    Ok(0)
}

/// The number of package roots the audit measures as lacking a navigability
/// anchor — the exact set the migration acts on (the audit's reported count is
/// the length of this same walk; see `aoa_audit::navigability_sites`).
fn navigability_count(repo: &Path) -> Result<u64> {
    let sites = aoa_audit::navigability_sites(repo)
        .with_context(|| format!("failed to measure navigability sites in {}", repo.display()))?;
    Ok(sites.len() as u64)
}

/// Render each contributing fix's eligibility precondition as human text.
fn render_eligibility(notes: &[FixEligibility]) -> String {
    notes
        .iter()
        .map(|n| format!("\n[eligibility:{}] {}\n", n.fix_id, n.note))
        .collect()
}

/// Render the reproducibility provenance a compiler-backed fix recorded (the
/// toolchain it ran under and whether the repo pinned it).
fn render_provenance(provenance: &[FixProvenance]) -> String {
    provenance
        .iter()
        .map(|p| {
            let pin = if p.pin_present {
                "pinned"
            } else {
                "UNPINNED (reproducible only on this toolchain)"
            };
            format!(
                "\n[provenance:{}] toolchain {pin}:\n{}\n",
                p.fix_id, p.toolchain
            )
        })
        .collect()
}

fn change_views(plan: &MigrationPlan) -> Vec<ChangeView> {
    plan.changes
        .iter()
        .map(|c| ChangeView {
            path: c.path.display().to_string(),
            action: match c.action {
                aoa_migrate::ChangeAction::Create => "create",
                aoa_migrate::ChangeAction::Overwrite => "overwrite",
            },
        })
        .collect()
}

#[derive(Serialize)]
struct ChangeView {
    path: String,
    action: &'static str,
}

#[derive(Serialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
enum MigrateView {
    Plan {
        grounding_navigability_sites: u64,
        fix_ids: Vec<String>,
        changes: Vec<ChangeView>,
        eligibility_notes: Vec<FixEligibility>,
        provenance: Vec<FixProvenance>,
    },
    Apply {
        fixes_applied: Vec<String>,
        files_written: usize,
        navigability_sites_remaining: u64,
        manifest_path: String,
        eligibility_notes: Vec<FixEligibility>,
        provenance: Vec<FixProvenance>,
    },
    Rollback {
        files_reverted: usize,
    },
}
