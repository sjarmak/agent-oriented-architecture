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
use std::path::Path;

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
    let manifest_path = repo.join(MANIFEST_REL);
    if manifest_path.exists() {
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
        let target = repo.join(template.path);
        if target.exists() {
            view.skipped.push(template.path.to_string());
            continue;
        }
        write_file(&target, &rendered)?;
        files.push(ManifestFile {
            path: template.path.to_string(),
            sha256: sha256_hex(&rendered),
        });
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
    let manifest_path = repo.join(MANIFEST_REL);
    if !manifest_path.exists() {
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
        let rendered = render(template.body, project);
        let rendered_hash = sha256_hex(&rendered);
        let target = repo.join(template.path);
        let path = template.path.to_string();

        if !target.exists() {
            write_file(&target, &rendered)?;
            view.written.push(path.clone());
        } else {
            let disk = std::fs::read_to_string(&target)
                .with_context(|| format!("failed to read {}", target.display()))?;
            if disk == rendered {
                view.skipped.push(path.clone());
            } else if recorded.get(&path) == Some(&sha256_hex(&disk)) {
                write_file(&target, &rendered)?;
                view.written.push(path.clone());
            } else {
                // User-owned content (edited, or never written by init): park
                // the new render beside it and keep the old hash, if any, so
                // the local edit stays detected on the next update.
                write_file(&repo.join(format!("{}.new", template.path)), &rendered)?;
                view.review.push(format!("{path}.new"));
                if let Some(old) = recorded.get(&path) {
                    files.push(ManifestFile {
                        path,
                        sha256: old.clone(),
                    });
                }
                continue;
            }
        }
        files.push(ManifestFile {
            path: template.path.to_string(),
            sha256: rendered_hash,
        });
    }

    write_manifest(&manifest_path, files)?;
    Ok(view)
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
    match canonical.file_name() {
        Some(name) => Ok(name.to_string_lossy().into_owned()),
        None => bail!(
            "cannot derive a project name from {} (filesystem root?)",
            repo.display()
        ),
    }
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
