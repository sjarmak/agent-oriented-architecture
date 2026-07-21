//! `aoa init` (R11): scaffold — or update — the minimal agent-ready repo shape.
//!
//! Rust-native replacement for the copier-template sketch: the template set is
//! embedded at compile time, rendered with a single `{{project_name}}`
//! substitution, and tracked by a manifest (`.aoa/init.json`) recording the
//! template version plus a sha256 of each rendered file.
//!
//! Ejectable by construction: every generated file is plain text, none of them
//! references `.aoa/`, and the manifest is the only file written there —
//! deleting `.aoa/` costs nothing but `--update` support.
//!
//! `--update` never silently overwrites: a file whose on-disk hash no longer
//! matches the manifest (user-edited, or never written by init) gets the new
//! render beside it as `<path>.new` for review instead.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cli::InitArgs;
use crate::commands::fsutil::{read_to_string_capped, MAX_JSON_BYTES};
use crate::output::{print_human, print_json};

/// Version of the embedded template set, recorded in the manifest.
const TEMPLATE_VERSION: u32 = 1;

/// Manifest location, relative to the repo root. The ONLY file init writes
/// under `.aoa/` (ejectability contract).
const MANIFEST_REL: &str = ".aoa/init.json";

/// One embedded template: target path (relative to the repo root) and raw body.
struct Template {
    path: &'static str,
    body: &'static str,
}

/// The full scaffold, flat and in write order.
const TEMPLATES: &[Template] = &[
    Template {
        path: "AGENTS.md",
        body: include_str!("../../templates/AGENTS.md"),
    },
    Template {
        path: "aoa-policy.yaml",
        body: include_str!("../../templates/aoa-policy.yaml"),
    },
    Template {
        path: "features/README.md",
        body: include_str!("../../templates/features/README.md"),
    },
    Template {
        // Source lives flat under templates/ (the repo gitignores `.claude/`
        // everywhere, which would silently drop a nested template source).
        path: ".claude/skills/feature-capsule/SKILL.md",
        body: include_str!("../../templates/skill-feature-capsule.md"),
    },
];

/// The persisted manifest: template version + sha256 of every rendered file
/// init owns, so `--update` can tell a pristine file from a user-edited one.
///
/// Deliberately NOT `deny_unknown_fields`, and for a different reason than the
/// slice types elsewhere in this crate: this is tool-owned state AOA writes and
/// reads back, so the constraint is forward compatibility — an older binary must
/// still read a `.aoa/init.json` written by a newer one during `--update`, which
/// a strict parse would turn into a hard failure on the operator's next upgrade.
/// Every field is required, so a typo in a hand-edited manifest still fails loud.
#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    template_version: u32,
    files: Vec<ManifestFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestFile {
    /// Relative to the repo root, forward-slash form of the template path.
    path: String,
    /// Hex sha256 of the rendered content init last wrote (or adopted).
    sha256: String,
}

/// The wire/human view of one run.
#[derive(Debug, Serialize)]
struct InitView {
    mode: &'static str,
    template_version: u32,
    /// Files written (created or cleanly updated) this run.
    written: Vec<String>,
    /// Files left untouched: pre-existing on init, already up-to-date on update.
    skipped: Vec<String>,
    /// `<path>.new` review artifacts written because the target has local edits.
    review: Vec<String>,
}

/// Scaffold or update the repo at `--repo`.
pub fn run(args: &InitArgs) -> Result<i32> {
    if !args.repo.is_dir() {
        bail!(
            "{} is not a directory (aoa init scaffolds into an existing repo root)",
            args.repo.display()
        );
    }
    let project = project_name(&args.repo)?;
    let view = if args.update {
        update(&args.repo, &project)?
    } else {
        init(&args.repo, &project)?
    };

    if args.json {
        print_json(&view)?;
    } else {
        print_human(&render_human(&view));
    }
    Ok(0)
}

/// Fresh scaffold. Refuses to run twice (the manifest is the marker) and never
/// overwrites a pre-existing file — collisions are skipped and reported.
fn init(repo: &Path, project: &str) -> Result<InitView> {
    let manifest_path = safe_target(repo, MANIFEST_REL)?;
    if exists_nofollow(&manifest_path) {
        bail!(
            "already initialized ({} exists); run `aoa init --update` to migrate",
            manifest_path.display()
        );
    }

    let mut view = InitView {
        mode: "init",
        template_version: TEMPLATE_VERSION,
        written: Vec::new(),
        skipped: Vec::new(),
        review: Vec::new(),
    };
    let mut files = Vec::new();

    for template in TEMPLATES {
        let rendered = render(template.body, project);
        let target = safe_target(repo, template.path)?;
        if exists_nofollow(&target) {
            // Never overwrite a pre-existing file. Adopt it into the manifest
            // only when its content already equals our render, so a later
            // `--update` treats it as pristine (clean rewrite on a template
            // bump) instead of parking a `.new` forever. Genuinely different
            // user content stays unrecorded, hence user-owned.
            if content_equals(&target, &rendered) {
                files.push(manifest_entry(template.path, &rendered));
            }
            view.skipped.push(template.path.to_string());
            continue;
        }
        write_file(&target, &rendered)?;
        files.push(manifest_entry(template.path, &rendered));
        view.written.push(template.path.to_string());
    }

    write_manifest(&manifest_path, files)?;
    Ok(view)
}

/// Migrate a previously scaffolded repo to the current template set.
///
/// Per file: missing → restore; content already equals the render → adopt as
/// up-to-date; on-disk hash matches the manifest (pristine) → rewrite in
/// place; anything else is user-owned → write `<path>.new` for review and
/// leave the file alone.
fn update(repo: &Path, project: &str) -> Result<InitView> {
    let manifest_path = safe_target(repo, MANIFEST_REL)?;
    if !exists_nofollow(&manifest_path) {
        bail!(
            "no manifest at {} — this repo was not scaffolded here; run `aoa init` first",
            manifest_path.display()
        );
    }
    let raw = read_to_string_capped(&manifest_path, MAX_JSON_BYTES)?;
    let manifest: Manifest = serde_json::from_str(&raw)
        .with_context(|| format!("invalid manifest at {}", manifest_path.display()))?;
    let recorded: BTreeMap<String, String> = manifest
        .files
        .into_iter()
        .map(|f| (f.path, f.sha256))
        .collect();

    let mut view = InitView {
        mode: "update",
        template_version: TEMPLATE_VERSION,
        written: Vec::new(),
        skipped: Vec::new(),
        review: Vec::new(),
    };
    let mut files = Vec::new();

    for template in TEMPLATES {
        let (outcome, entry) = reconcile(repo, template, project, &recorded)?;
        match outcome {
            Outcome::Written => view.written.push(template.path.to_string()),
            Outcome::Skipped => view.skipped.push(template.path.to_string()),
            Outcome::Review => view.review.push(format!("{}.new", template.path)),
        }
        files.extend(entry);
    }

    write_manifest(&manifest_path, files)?;
    Ok(view)
}

/// What `--update` did with a single template.
enum Outcome {
    /// File was missing or pristine-vs-manifest and got the fresh render.
    Written,
    /// On-disk content already equals the render; nothing to do.
    Skipped,
    /// File carries local edits; the render was parked as `<path>.new`.
    Review,
}

/// Reconcile one template against the repo: the four-way state machine
/// (missing → write; disk == render → skip; disk matches the recorded hash →
/// rewrite; else user-owned → park `.new`). Returns the outcome plus the
/// manifest entry to persist, if any (a review outcome retains the old hash so
/// the local edit stays detected on the next update).
fn reconcile(
    repo: &Path,
    template: &Template,
    project: &str,
    recorded: &BTreeMap<String, String>,
) -> Result<(Outcome, Option<ManifestFile>)> {
    let path = template.path.to_string();
    let rendered = render(template.body, project);
    let target = safe_target(repo, template.path)?;
    let fresh = || manifest_entry(&path, &rendered);

    if !exists_nofollow(&target) {
        write_file(&target, &rendered)?;
        return Ok((Outcome::Written, Some(fresh())));
    }
    let disk =
        read_nonsymlink(&target).with_context(|| format!("failed to read {}", target.display()))?;
    if disk == rendered {
        return Ok((Outcome::Skipped, Some(fresh())));
    }
    if recorded.get(&path) == Some(&sha256_hex(&disk)) {
        write_file(&target, &rendered)?;
        return Ok((Outcome::Written, Some(fresh())));
    }
    // User-owned content (edited, or never written by init): park the new
    // render beside it and keep the old hash, if any, so the edit stays
    // detected on the next update.
    let review_target = safe_target(repo, &format!("{}.new", template.path))?;
    write_file(&review_target, &rendered)?;
    let retained = recorded.get(&path).map(|old| ManifestFile {
        path: path.clone(),
        sha256: old.clone(),
    });
    Ok((Outcome::Review, retained))
}

/// Render a template body: the single mechanical substitution.
fn render(body: &str, project: &str) -> String {
    body.replace("{{project_name}}", project)
}

/// Derive the project name from the repo root's directory name.
fn project_name(repo: &Path) -> Result<String> {
    let canonical = repo
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", repo.display()))?;
    let name = match canonical.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => bail!(
            "cannot derive a project name from {} (filesystem root?)",
            repo.display()
        ),
    };
    // The name is substituted verbatim into YAML/Markdown templates, so a
    // directory name carrying control characters (e.g. an embedded newline)
    // could inject structure into the generated files. Reject at this boundary
    // rather than silently corrupting the output.
    if name.chars().any(char::is_control) {
        bail!("project directory name {name:?} contains control characters");
    }
    Ok(name)
}

/// Resolve `rel` under `repo`, refusing to traverse or land on a symlink.
///
/// `init`/`--update` write scaffold files into a possibly-untrusted checkout;
/// a planted symlink (a file link, or a symlinked directory component) would
/// otherwise let a write escape the repo. Walk every component of `rel` from
/// the repo root down and reject the first that already exists as a symlink,
/// mirroring the no-follow discipline in `aoa-audit`. Only plain `Normal`
/// components are allowed, so `..`/absolute segments can never widen the path.
fn safe_target(repo: &Path, rel: &str) -> Result<PathBuf> {
    let mut cur = repo.to_path_buf();
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(part) => cur.push(part),
            other => bail!("unsafe template path {rel:?}: illegal component {other:?}"),
        }
        if let Ok(meta) = std::fs::symlink_metadata(&cur) {
            if meta.file_type().is_symlink() {
                bail!(
                    "refusing to write through a symlink at {} (aoa init does not follow links)",
                    cur.display()
                );
            }
        }
    }
    Ok(cur)
}

/// No-follow existence check: reports `true` for a real file/dir AND for a
/// dangling symlink, so a planted broken link is never mistaken for "absent"
/// and silently written through. (`safe_target` rejects the symlink case
/// first; this guards the plain "already there, skip it" decision.)
fn exists_nofollow(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// A manifest entry recording that init owns `path` with the given rendered body.
fn manifest_entry(path: &str, rendered: &str) -> ManifestFile {
    ManifestFile {
        path: path.to_string(),
        sha256: sha256_hex(rendered),
    }
}

/// True when the file at `path` reads back byte-for-byte as `rendered`. A read
/// failure (binary content, unreadable file, or a symlink) means "not our
/// render", so the pre-existing file is left unadopted — the conservative
/// default that matches init's never-touch-a-pre-existing-file contract.
fn content_equals(path: &Path, rendered: &str) -> bool {
    read_nonsymlink(path).is_ok_and(|disk| disk == rendered)
}

/// Read `path` as a regular file, refusing to follow a symlink. `safe_target`
/// already rejects a statically-planted symlink before we get here; re-checking
/// immediately before the read narrows the TOCTOU window so a file swapped for a
/// symlink mid-run is not followed, keeping the read consistent with the
/// no-follow invariant `safe_target` documents.
fn read_nonsymlink(path: &Path) -> std::io::Result<String> {
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to follow a symlink",
        ));
    }
    std::fs::read_to_string(path)
}

fn sha256_hex(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

fn write_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

fn write_manifest(path: &Path, files: Vec<ManifestFile>) -> Result<()> {
    let manifest = Manifest {
        template_version: TEMPLATE_VERSION,
        files,
    };
    let json = serde_json::to_string_pretty(&manifest).context("serializing manifest")?;
    write_file(path, &format!("{json}\n"))
}

fn render_human(view: &InitView) -> String {
    let mut out = format!("aoa {} (template v{})\n", view.mode, view.template_version);
    for path in &view.written {
        let _ = writeln!(out, "  wrote {path}");
    }
    let skip_reason = if view.mode == "init" {
        "exists"
    } else {
        "up-to-date"
    };
    for path in &view.skipped {
        let _ = writeln!(out, "  skipped {path} ({skip_reason})");
    }
    for path in &view.review {
        let _ = writeln!(
            out,
            "  review {path} — target has local edits; merge manually \
             (e.g. git diff --no-index {} {path})",
            path.trim_end_matches(".new"),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn read_nonsymlink_refuses_a_symlink_and_does_not_follow_it() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let real = dir.path().join("real.txt");
        std::fs::write(&real, "secret\n").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        // Reading the symlink is refused (not followed to `real`).
        let err = read_nonsymlink(&link).expect_err("symlink must be refused");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        // The regular file it points at still reads normally.
        assert_eq!(read_nonsymlink(&real).unwrap(), "secret\n");
        // content_equals therefore never adopts a symlinked pre-existing file.
        assert!(!content_equals(&link, "secret\n"));
        assert!(content_equals(&real, "secret\n"));
    }
}
