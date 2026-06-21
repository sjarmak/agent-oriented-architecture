//! [`DeadImportFix`] — a compiler-verified, oracle-blind, strictly-subtractive
//! code-layer migration that removes rustc-certified unused imports.
//!
//! Unlike the navigability anchor (a pure function of the directory tree), this
//! fix asks the *compiler* what is unused. The mechanism, in [`DeadImportFix::plan`]:
//!
//! 1. Honest-degrade to an empty plan if there is no root `Cargo.toml` — this is
//!    a cargo-only treatment, and a non-cargo tree is simply ineligible, not an
//!    error.
//! 2. Copy the repo to an isolated [`TempDir`], excluding build output and VCS
//!    metadata, so the real checkout is never written and the compiler runs on a
//!    throwaway tree.
//! 3. Run `cargo check --all-features --all-targets --offline --message-format=json`
//!    in the copy. The toolchain pin (a copied `rust-toolchain.toml`) is honored
//!    automatically because cargo resolves it from the working directory.
//! 4. Parse the JSON stream (never regex on stderr) and keep **only**
//!    `unused_imports`-class `MachineApplicable` suggestions via the `rustfix`
//!    crate — deliberately *not* `cargo fix`, which also applies edition and
//!    `dyn` rewrites that are out of scope.
//! 5. Apply those suggestions to each touched file's source and emit an
//!    [`Overwrite`](crate::ChangeAction::Overwrite) [`PlannedChange`] whose path
//!    is mapped back from the temp tree to the real repo exactly, so the apply
//!    step's `strip_prefix(repo)` round-trips.
//!
//! The [`TempDir`] is dropped when planning returns, so there is no residue and
//! the real repo is byte-unchanged. The fix only ever *deletes* import items the
//! compiler certified unused, so it is strictly subtractive and reads only the
//! target's own source — it cannot transcribe a held-out answer (oracle-blind).
//!
//! Construct-validity boundary: "subtractive" is not the same as "correct". A
//! `use` referenced only under a `#[cfg(feature = "x")]` path looks unused when
//! that feature is off; `--all-features` activates every feature at once to
//! mitigate this, but a repo with *mutually exclusive* features or
//! platform-`cfg` imports cannot be made safe by a single invocation. Those are
//! declared (unchecked) eligibility exclusions in [`DEAD_IMPORT_ELIGIBILITY`].

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use rustfix::{Filter, Suggestion};
use serde_json::Value;

use crate::error::MigrateError;
use crate::fix::{ChangeAction, CodeFix, FixProvenance, PlannedChange};

/// Stable id, selectable on the CLI via `--fix dead-imports`.
const DEAD_IMPORT_ID: &str = "dead-imports";

/// The single rustc lint class this fix acts on. Kept deliberately tight: broader
/// `cargo fix` suggestions (edition idioms, `dyn` rewrites, `redundant_imports`)
/// are *not* in scope, and widening this set would weaken the subtractivity
/// guarantee.
const UNUSED_IMPORTS_LINT: &str = "unused_imports";

/// Directory basenames excluded from the isolated copy, at any nesting depth:
/// build output (`target`, `build`), VCS metadata (`.git`), this tool's own
/// bookkeeping (`.aoa`), and vendored JS (`node_modules`). Excluding nested
/// `target/` matters for workspaces with per-member build dirs.
const COPY_EXCLUDE_DIRS: &[&str] = &["target", ".git", ".aoa", "node_modules", "build"];

/// The R0 eligibility precondition for the dead-import fix. Several of these are
/// **declared, not enforced** — stating them honestly (rather than pretending to
/// validate) is the point: a campaign operator pre-registers that the target repo
/// class satisfies them.
pub(crate) const DEAD_IMPORT_ELIGIBILITY: &str = "Compiler-verified unused-import removal is a construct-valid, reproducible code-layer treatment only when: \
(1) the target repo pins its toolchain (rust-toolchain.toml) — without a pin the result is reproducible only under the toolchain recorded in the manifest provenance; \
(2) the repo has NO mutually-exclusive features and NO platform-cfg-gated imports — `--all-features --all-targets` activates every feature and target at once, but a `use` reachable only under a cfg path no single invocation compiles would be seen as unused and (wrongly, though still subtractively) removed [UNCHECKED precondition]; \
(3) the dependency closure resolves offline (populated Cargo.lock + cached/vendored crates) — `--offline` is used so the build neither hits the network nor varies with registry state. \
A build that fails these (offline-unresolvable, non-cleanly-compiling) is a LOUD error, never a silent empty plan.";

/// Removes rustc-certified unused imports via an isolated `cargo check` + the
/// `rustfix` crate. The first real producer of `Overwrite` changes for the
/// migrate engine, and the second [`CodeFix`] implementer — the one that earns
/// the trait abstraction.
pub struct DeadImportFix;

impl CodeFix for DeadImportFix {
    fn id(&self) -> &str {
        DEAD_IMPORT_ID
    }

    fn describe(&self) -> &str {
        "remove rustc-certified unused imports via an isolated cargo check + rustfix (strictly subtractive)"
    }

    fn eligibility_note(&self) -> &str {
        DEAD_IMPORT_ELIGIBILITY
    }

    fn plan(&self, repo: &Path) -> Result<Vec<PlannedChange>, MigrateError> {
        // (1) Cargo-only treatment: a tree without a root manifest is ineligible,
        // not broken. Honest-degrade to an empty plan.
        if !repo.join("Cargo.toml").is_file() {
            return Ok(Vec::new());
        }

        // (2) Isolate: copy to a throwaway tree so the compiler never writes the
        // real checkout. Dropped at end of scope => no residue.
        let work = tempfile::Builder::new()
            .prefix("aoa-dead-imports-")
            .tempdir()
            .map_err(|source| MigrateError::Io {
                path: repo.to_path_buf(),
                source,
            })?;
        copy_tree(repo, work.path())?;

        // (3) Ask the compiler what is unused (parsed diagnostics, classified).
        let diagnostics = run_cargo_check(work.path())?;

        // (4) + (5) Filter to the unused-imports class, apply, and map paths back
        // to the real repo.
        compute_changes(&diagnostics, work.path(), repo)
    }

    fn provenance(&self, repo: &Path) -> Result<Option<FixProvenance>, MigrateError> {
        Ok(Some(FixProvenance {
            fix_id: DEAD_IMPORT_ID.to_string(),
            toolchain: toolchain_version(repo)?,
            pin_present: repo.join("rust-toolchain.toml").exists()
                || repo.join("rust-toolchain").exists(),
        }))
    }
}

/// Recursively copy `src` into `dst`, skipping [`COPY_EXCLUDE_DIRS`] at any depth
/// and never following symlinks (a symlink is skipped rather than traversed, so
/// the copy stays within the source tree).
fn copy_tree(src: &Path, dst: &Path) -> Result<(), MigrateError> {
    std::fs::create_dir_all(dst).map_err(|source| io_err(dst, source))?;
    for entry in std::fs::read_dir(src).map_err(|source| io_err(src, source))? {
        let entry = entry.map_err(|source| io_err(src, source))?;
        let name = entry.file_name();
        let from = entry.path();
        let to = dst.join(&name);
        // `file_type` does not follow symlinks.
        let ft = entry.file_type().map_err(|source| io_err(&from, source))?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if COPY_EXCLUDE_DIRS.contains(&name.to_string_lossy().as_ref()) {
                continue;
            }
            copy_tree(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to).map_err(|source| MigrateError::Copy {
                from: from.clone(),
                to: to.clone(),
                source,
            })?;
        }
    }
    Ok(())
}

/// Run `cargo check` over the isolated copy and return its raw JSON stdout. The
/// caller classifies the diagnostics; this function only distinguishes a failure
/// to *invoke* the toolchain ([`ToolchainUnavailable`](MigrateError::ToolchainUnavailable))
/// from a successful invocation (whose stdout/stderr/exit are returned for
/// classification).
fn run_cargo_check(workdir: &Path) -> Result<Vec<Value>, MigrateError> {
    let output = Command::new("cargo")
        .current_dir(workdir)
        .args([
            "check",
            "--all-features",
            "--all-targets",
            "--offline",
            "--message-format=json",
        ])
        .output()
        .map_err(|source| MigrateError::ToolchainUnavailable {
            detail: format!("could not run `cargo`: {source}"),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    // A missing/uninstalled pinned toolchain surfaces on stderr, not as a spawn
    // failure (the `cargo`/`rustup` proxy spawns fine, then reports the problem).
    if looks_like_toolchain_error(&stderr) {
        return Err(MigrateError::ToolchainUnavailable { detail: stderr });
    }

    // Parse the JSON stream once; both classification and change-computation read
    // the same inner diagnostics.
    let diagnostics = compiler_messages(&stdout);
    classify_build(&diagnostics, &stderr, output.status.success())?;
    Ok(diagnostics)
}

/// Decide whether the build is usable, inspecting the parsed diagnostics — never
/// the exit code alone (a `#[deny(warnings)]` repo exits non-zero while emitting
/// perfectly valid `unused_imports` JSON, and an unrelated error in one target
/// exits non-zero too; the two need opposite verdicts).
fn classify_build(diagnostics: &[Value], stderr: &str, success: bool) -> Result<(), MigrateError> {
    let mut non_unused_error = false;
    let mut saw_unused_suggestion = false;

    for diag in diagnostics {
        let level = diag.get("level").and_then(Value::as_str).unwrap_or("");
        let code = diag
            .get("code")
            .and_then(|c| c.get("code"))
            .and_then(Value::as_str);
        // A code-less error-level diagnostic (e.g. an unresolved macro) is by
        // definition not our lint class, so it counts as a non-unused error and
        // the tree is treated as not cleanly checking — the safe verdict.
        if level == "error" && code != Some(UNUSED_IMPORTS_LINT) {
            non_unused_error = true;
        }
        if code == Some(UNUSED_IMPORTS_LINT) {
            saw_unused_suggestion = true;
        }
    }

    if non_unused_error {
        // The tree does not cleanly compile for reasons outside our lint class;
        // we cannot certify subtractivity against a broken build.
        return Err(MigrateError::RepoDoesNotCheck {
            stderr: stderr.to_string(),
        });
    }

    if !success && !saw_unused_suggestion {
        // Non-zero exit with nothing actionable found: either an infrastructure
        // failure (offline deps, bad manifest) or a denied lint we do not handle.
        // Either way, refuse loudly rather than silently planning nothing.
        return Err(MigrateError::BuildFailed {
            stderr: stderr.to_string(),
        });
    }

    // success, or non-zero solely from a denied `unused_imports` lint — proceed.
    // A clean build with zero suggestions yields a legitimate empty plan.
    Ok(())
}

/// Extract each `compiler-message`'s inner rustc diagnostic from cargo's
/// line-delimited JSON stream. Non-object lines and other `reason`s are skipped.
fn compiler_messages(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|v| v.get("reason").and_then(Value::as_str) == Some("compiler-message"))
        .filter_map(|mut v| v.get_mut("message").map(std::mem::take))
        .collect()
}

/// Build the `Overwrite` changes from the parsed compiler diagnostics: keep only
/// `unused_imports` `MachineApplicable` suggestions, apply them per file, and map
/// each touched temp path back to the real repo.
fn compute_changes(
    diagnostics: &[Value],
    work: &Path,
    repo: &Path,
) -> Result<Vec<PlannedChange>, MigrateError> {
    // Feed rustfix the inner diagnostics, newline-joined (it streams `Diagnostic`s).
    let mut joined = String::new();
    for diag in diagnostics {
        joined.push_str(
            &serde_json::to_string(diag).map_err(|e| MigrateError::FixCompute {
                message: e.to_string(),
            })?,
        );
        joined.push('\n');
    }

    let only: HashSet<String> = std::iter::once(UNUSED_IMPORTS_LINT.to_string()).collect();
    let suggestions =
        rustfix::get_suggestions_from_json(&joined, &only, Filter::MachineApplicableOnly).map_err(
            |e| MigrateError::FixCompute {
                message: e.to_string(),
            },
        )?;

    // Group suggestions by the file their snippets touch, then apply each group to
    // that one file's source.
    //
    // INVARIANT: every snippet and replacement within a single `unused_imports`
    // suggestion lives in the same file — the warning span and the deletion span
    // coincide. `apply_suggestions` operates on one file's source, so this only
    // holds because the lint class is single-file; a future cross-file lint would
    // need per-replacement file grouping, not grouping by the first snippet.
    let mut by_file: BTreeMap<String, Vec<Suggestion>> = BTreeMap::new();
    for sug in suggestions {
        let Some(file) = sug.snippets.first().map(|s| s.file_name.clone()) else {
            continue;
        };
        debug_assert!(
            sug.snippets.iter().all(|s| s.file_name == file),
            "unused_imports suggestion spans multiple files: {file}"
        );
        by_file.entry(file).or_default().push(sug);
    }

    // Canonicalize both ends once so the temp -> real round-trip is exact even
    // when /tmp is a symlink or paths arrive non-canonical.
    let work_canon = std::fs::canonicalize(work).map_err(|source| io_err(work, source))?;
    let repo_canon = std::fs::canonicalize(repo).map_err(|source| io_err(repo, source))?;

    let mut changes = Vec::new();
    for (file_name, sugs) in by_file {
        let Some(rel) = relative_within(&file_name, work, &work_canon) else {
            // A span outside the copied tree (e.g. a `path = "../sibling"` dep)
            // has no real-repo equivalent; drop it rather than misroute a write.
            continue;
        };
        let temp_file = work.join(&rel);
        let real_file = repo.join(&rel);

        // Defense in depth before apply.rs ever sees the path: the real target
        // must exist and resolve inside the real repo.
        match std::fs::canonicalize(&real_file) {
            Ok(c) if c.starts_with(&repo_canon) => {}
            _ => continue,
        }

        let original =
            std::fs::read_to_string(&temp_file).map_err(|source| io_err(&temp_file, source))?;
        let fixed =
            rustfix::apply_suggestions(&original, &sugs).map_err(|e| MigrateError::FixCompute {
                message: e.to_string(),
            })?;
        if fixed == original {
            continue;
        }
        // `old_content` reflects the *real* repo (what the diff preview and the
        // apply-time archive describe), read independently of the temp copy.
        let old_content =
            std::fs::read_to_string(&real_file).map_err(|source| io_err(&real_file, source))?;
        changes.push(PlannedChange {
            path: real_file,
            action: ChangeAction::Overwrite,
            new_content: fixed,
            old_content: Some(old_content),
        });
    }

    // Deterministic order regardless of map iteration.
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

/// Resolve a rustc span `file_name` (absolute into the temp tree, or relative to
/// the cargo working dir) to a path relative to the copied tree root. Returns
/// `None` for anything that resolves outside the tree.
fn relative_within(file_name: &str, work: &Path, work_canon: &Path) -> Option<PathBuf> {
    let raw = Path::new(file_name);
    let abs = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        work.join(raw)
    };
    let abs_canon = std::fs::canonicalize(&abs).ok()?;
    abs_canon
        .strip_prefix(work_canon)
        .ok()
        .map(Path::to_path_buf)
}

/// `rustc --version --verbose`, resolved under the repo's toolchain pin (cargo's
/// rustup proxy reads `rust-toolchain.toml` from the working directory).
fn toolchain_version(repo: &Path) -> Result<String, MigrateError> {
    let output = Command::new("rustc")
        .current_dir(repo)
        .args(["--version", "--verbose"])
        .output()
        .map_err(|source| MigrateError::ToolchainUnavailable {
            detail: format!("could not run `rustc`: {source}"),
        })?;
    if !output.status.success() {
        return Err(MigrateError::ToolchainUnavailable {
            detail: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Heuristic for "the toolchain itself is missing" vs "the code failed to
/// compile". rustup prints these specific markers when a pinned toolchain or
/// component is absent.
fn looks_like_toolchain_error(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("is not installed")
        || s.contains("toolchain") && s.contains("not installed")
        || s.contains("no such command")
        || s.contains("can't find crate for `core`")
}

fn io_err(path: &Path, source: std::io::Error) -> MigrateError {
    MigrateError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// A cargo `compiler-message` line wrapping an `unused_imports` diagnostic
    /// with a `MachineApplicable` deletion of `use std::collections::HashMap;`.
    fn unused_import_json(file_name: &str) -> String {
        serde_json::json!({
            "reason": "compiler-message",
            "message": {
                "message": "unused import: `std::collections::HashMap`",
                "code": { "code": "unused_imports", "explanation": null },
                "level": "warning",
                "spans": [{
                    "file_name": file_name,
                    "byte_start": 0, "byte_end": 34,
                    "line_start": 1, "line_end": 1,
                    "column_start": 1, "column_end": 35,
                    "is_primary": true,
                    "text": [],
                    "label": null, "suggested_replacement": null,
                    "suggestion_applicability": null, "expansion": null
                }],
                "children": [{
                    "message": "remove the unused import",
                    "code": null, "level": "help",
                    "spans": [{
                        // `use std::collections::HashMap;\n` is exactly 31 bytes;
                        // the MachineApplicable deletion replaces it with nothing.
                        "file_name": file_name,
                        "byte_start": 0, "byte_end": 31,
                        "line_start": 1, "line_end": 2,
                        "column_start": 1, "column_end": 1,
                        "is_primary": true,
                        "text": [],
                        "label": null,
                        "suggested_replacement": "",
                        "suggestion_applicability": "MachineApplicable",
                        "expansion": null
                    }],
                    "children": [], "rendered": null
                }],
                "rendered": null
            }
        })
        .to_string()
    }

    /// A `MachineApplicable` suggestion that is NOT `unused_imports` (an edition
    /// `dyn` rewrite over `Foo` in `fn f(_: Box<Foo>) {}`). The replacement lives
    /// in a child span — exactly as rustc emits it — so that *without* the
    /// `unused_imports` filter rustfix would produce and apply it. The filter must
    /// reject it: this is what proves the fix is not a blind `cargo fix`.
    fn edition_rewrite_json(file_name: &str) -> String {
        serde_json::json!({
            "reason": "compiler-message",
            "message": {
                "message": "trait objects without an explicit `dyn` are deprecated",
                "code": { "code": "bare_trait_objects", "explanation": null },
                "level": "warning",
                "spans": [{
                    "file_name": file_name,
                    "byte_start": 12, "byte_end": 15,
                    "line_start": 1, "line_end": 1,
                    "column_start": 13, "column_end": 16,
                    "is_primary": true,
                    "text": [],
                    "label": null, "suggested_replacement": null,
                    "suggestion_applicability": null, "expansion": null
                }],
                "children": [{
                    "message": "if this is a dyn-compatible trait, use `dyn`",
                    "code": null, "level": "help",
                    "spans": [{
                        // `Foo` sits at bytes 12..15 in `fn f(_: Box<Foo>) {}`.
                        "file_name": file_name,
                        "byte_start": 12, "byte_end": 15,
                        "line_start": 1, "line_end": 1,
                        "column_start": 13, "column_end": 16,
                        "is_primary": true,
                        "text": [],
                        "label": null,
                        "suggested_replacement": "dyn Foo",
                        "suggestion_applicability": "MachineApplicable",
                        "expansion": null
                    }],
                    "children": [], "rendered": null
                }],
                "rendered": null
            }
        })
        .to_string()
    }

    /// Parse a canned cargo JSON stream into the inner diagnostics, the form
    /// [`compute_changes`] and [`classify_build`] consume.
    fn diags(json: &str) -> Vec<Value> {
        compiler_messages(json)
    }

    #[test]
    fn honest_degrades_to_empty_when_no_root_cargo_toml() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
        assert!(DeadImportFix.plan(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn compute_changes_drops_a_certified_unused_import() {
        let repo = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        let src = "use std::collections::HashMap;\nfn main() {}\n";
        // The path round-trip reads from both trees, so the file must exist in both.
        for d in [repo.path(), work.path()] {
            fs::write(d.join("lib.rs"), src).unwrap();
        }

        let json = unused_import_json("lib.rs");
        let changes = compute_changes(&diags(&json), work.path(), repo.path()).unwrap();

        assert_eq!(changes.len(), 1);
        let c = &changes[0];
        assert_eq!(c.action, ChangeAction::Overwrite);
        assert_eq!(c.path, repo.path().join("lib.rs"));
        assert_eq!(
            c.old_content.as_deref(),
            Some(src),
            "old_content is the real repo's bytes"
        );
        assert!(
            !c.new_content.contains("HashMap"),
            "the unused import is removed"
        );
        assert!(c.new_content.contains("fn main"), "the rest is untouched");
    }

    #[test]
    fn compute_changes_filters_out_non_unused_import_machine_applicable_suggestions() {
        // C2: a blind `cargo fix` would apply this edition rewrite. We must not.
        let repo = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        let src = "fn f(_: Box<Foo>) {}\n";
        for d in [repo.path(), work.path()] {
            fs::write(d.join("lib.rs"), src).unwrap();
        }

        let json = edition_rewrite_json("lib.rs");
        let parsed = diags(&json);

        // Sanity: the fixture is a real, applicable suggestion — without the
        // `unused_imports` filter rustfix WOULD apply it. (This is what makes the
        // assertion below non-vacuous.)
        let unfiltered = rustfix::get_suggestions_from_json(
            &serde_json::to_string(&parsed[0]).unwrap(),
            &HashSet::<String>::new(),
            Filter::MachineApplicableOnly,
        )
        .unwrap();
        assert_eq!(
            unfiltered.len(),
            1,
            "fixture is a genuine applicable rewrite"
        );

        // With the production filter, the non-unused_imports rewrite is dropped.
        let changes = compute_changes(&parsed, work.path(), repo.path()).unwrap();
        assert!(
            changes.is_empty(),
            "non-unused_imports suggestions are not applied"
        );
    }

    #[test]
    fn relative_within_maps_absolute_temp_paths_through_a_symlinked_root() {
        // /tmp is frequently a symlink; the round-trip must canonicalize both ends.
        let work = TempDir::new().unwrap();
        fs::create_dir_all(work.path().join("src")).unwrap();
        fs::write(work.path().join("src/lib.rs"), "x\n").unwrap();
        let work_canon = fs::canonicalize(work.path()).unwrap();

        // Absolute path into the (possibly symlinked) temp tree.
        let abs = work.path().join("src/lib.rs");
        let rel = relative_within(&abs.to_string_lossy(), work.path(), &work_canon).unwrap();
        assert_eq!(rel, PathBuf::from("src/lib.rs"));

        // Relative span path (relative to the cargo working dir).
        let rel2 = relative_within("src/lib.rs", work.path(), &work_canon).unwrap();
        assert_eq!(rel2, PathBuf::from("src/lib.rs"));
    }

    #[test]
    fn relative_within_rejects_paths_outside_the_copied_tree() {
        let work = TempDir::new().unwrap();
        let work_canon = fs::canonicalize(work.path()).unwrap();
        // An absolute path to a real file outside the tree resolves, but is not
        // under the tree root, so it must be dropped.
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("evil.rs"), "x\n").unwrap();
        let abs = outside.path().join("evil.rs");
        assert!(relative_within(&abs.to_string_lossy(), work.path(), &work_canon).is_none());
    }

    #[test]
    fn classify_build_rejects_non_unused_errors_as_repo_does_not_check() {
        let json = serde_json::json!({
            "reason": "compiler-message",
            "message": { "message": "mismatched types", "code": { "code": "E0308" },
                         "level": "error", "spans": [], "children": [], "rendered": null }
        })
        .to_string();
        let err = classify_build(&diags(&json), "stderr", false).unwrap_err();
        assert!(matches!(err, MigrateError::RepoDoesNotCheck { .. }));
    }

    #[test]
    fn classify_build_allows_deny_warnings_promoting_unused_imports_to_error() {
        // `#[deny(unused_imports)]` makes cargo exit non-zero while the diagnostic
        // is still our MachineApplicable class — we apply it, not reject it.
        let json = serde_json::json!({
            "reason": "compiler-message",
            "message": { "message": "unused import", "code": { "code": "unused_imports" },
                         "level": "error", "spans": [], "children": [], "rendered": null }
        })
        .to_string();
        assert!(classify_build(&diags(&json), "stderr", false).is_ok());
    }

    #[test]
    fn classify_build_treats_nonzero_exit_with_no_suggestions_as_build_failed() {
        // Infrastructure failure: cargo exits non-zero, emits no usable diagnostics.
        let err = classify_build(&[], "could not resolve deps offline", false).unwrap_err();
        assert!(matches!(err, MigrateError::BuildFailed { .. }));
    }

    #[test]
    fn classify_build_accepts_a_clean_build_with_no_suggestions() {
        // The one legitimate empty plan: success, nothing to remove.
        assert!(classify_build(&[], "", true).is_ok());
    }

    #[test]
    fn provenance_records_toolchain_and_pin_presence() {
        // Exercises `provenance()` itself: the toolchain string is captured from
        // `rustc` (always present under `cargo test`) and pin detection reflects
        // the filesystem.
        let repo = TempDir::new().unwrap();

        let unpinned = DeadImportFix.provenance(repo.path()).unwrap().unwrap();
        assert_eq!(unpinned.fix_id, "dead-imports");
        assert!(
            unpinned.toolchain.contains("rustc"),
            "toolchain identity captured: {}",
            unpinned.toolchain
        );
        assert!(!unpinned.pin_present, "no pin file present");

        fs::write(
            repo.path().join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"stable\"\n",
        )
        .unwrap();
        let pinned = DeadImportFix.provenance(repo.path()).unwrap().unwrap();
        assert!(pinned.pin_present, "pin detected once the file exists");
    }
}
