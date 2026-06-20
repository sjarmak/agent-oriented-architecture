//! Aggregating fixes into a single [`MigrationPlan`] and previewing it as a diff.

use std::path::Path;

use crate::error::MigrateError;
use crate::fix::{ChangeAction, CodeFix, PlannedChange};

/// The full set of changes a migration would make, gathered from one or more
/// [`CodeFix`]es. Building a plan writes nothing — it is the dry-run artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationPlan {
    /// Every planned change, ordered deterministically by target path.
    pub changes: Vec<PlannedChange>,
    /// The ids of the fixes that contributed changes — provenance recorded in
    /// the manifest so a campaign can pre-register exactly which migrations ran.
    pub fix_ids: Vec<String>,
}

impl MigrationPlan {
    /// Build a plan by running each fix over `repo` and collecting the changes.
    pub fn build(repo: &Path, fixes: &[&dyn CodeFix]) -> Result<Self, MigrateError> {
        let mut changes = Vec::new();
        let mut fix_ids = Vec::new();
        for fix in fixes {
            let fix_changes = fix.plan(repo)?;
            if !fix_changes.is_empty() {
                fix_ids.push(fix.id().to_string());
            }
            changes.extend(fix_changes);
        }
        // Stable ordering so the preview and the manifest are reproducible even
        // if two fixes return their changes in different orders.
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Self { changes, fix_ids })
    }

    /// Whether the plan would change nothing (the repo already conforms).
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Render the plan as a unified-style diff preview. Created files show every
    /// line as an addition; overwritten files show the old lines removed and the
    /// new lines added. This is what `aoa migrate --plan` prints.
    pub fn render_diff(&self) -> String {
        if self.changes.is_empty() {
            return "No changes: every package root already conforms.\n".to_string();
        }
        let mut out = String::new();
        for change in &self.changes {
            let verb = match change.action {
                ChangeAction::Create => "create",
                ChangeAction::Overwrite => "overwrite",
            };
            out.push_str(&format!("--- {verb}: {}\n", change.path.display()));
            if let Some(old) = &change.old_content {
                for line in old.lines() {
                    out.push_str(&format!("-{line}\n"));
                }
            }
            for line in change.new_content.lines() {
                out.push_str(&format!("+{line}\n"));
            }
            out.push('\n');
        }
        out
    }
}
