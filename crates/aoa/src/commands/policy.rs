use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use aoa_policy::{
    ci_workflow, codeowners, infer_owners, precommit_config, proposed_codeowners,
    render_proposal_diff, BlameCount, OwnedPattern, Policy,
};
use serde::Serialize;

use crate::cli::{PolicyArgs, PolicyCommand};
use crate::commands::enforce::install_enforce_hooks;
use crate::commands::generated::write_gitattributes_plane;
use crate::forge::compile_enforcement;
use crate::output::{print_human, print_json};

/// The repo-relative CODEOWNERS location both `compile` and `infer-owners`
/// target.
const CODEOWNERS_REL: &str = ".github/CODEOWNERS";

/// Enforcement-plane policy utilities (R5, R16). `compile` turns the single
/// `aoa-policy.yaml` into all three planes; `guard-staged` is the pre-commit
/// plane's entry point; `infer-owners` proposes a CODEOWNERS map from git-blame
/// line ownership. Every subcommand renders both a human and a JSON register
/// (R17).
pub fn run(args: &PolicyArgs) -> Result<i32> {
    match &args.command {
        PolicyCommand::Compile { repo, forge, json } => compile(repo, forge, *json),
        PolicyCommand::GuardStaged { repo, files, json } => guard_staged(repo, files, *json),
        PolicyCommand::InferOwners { repo, write, json } => infer_owners_cmd(repo, *write, *json),
    }
}

/// The compile result: the plane artifacts written, identical in both registers.
#[derive(Debug, Serialize)]
struct CompileView {
    planes_written: Vec<String>,
}

/// Compile the policy to the runtime, pre-commit, and CI planes. Deterministic:
/// every artifact is a pure function of the policy, so re-running writes
/// byte-identical content (idempotent).
fn compile(repo: &Path, forge: &str, json: bool) -> Result<i32> {
    // Fail loud on an unsupported forge before writing anything (R-silent).
    compile_enforcement(forge).context("cannot compile the CI plane")?;

    let policy = load_policy(repo)?;

    let mut written = Vec::new();

    // Runtime plane: the Claude Code hooks, only when the gate is enabled.
    if policy.reproduction_required {
        written.push(install_enforce_hooks(repo)?);
    }

    // Pre-commit plane.
    written.push(write_artifact(
        &repo.join(".pre-commit-config.yaml"),
        &precommit_config(&policy),
    )?);

    // CI plane: workflow + CODEOWNERS.
    written.push(write_artifact(
        &repo
            .join(".github")
            .join("workflows")
            .join("aoa-policy.yml"),
        &ci_workflow(&policy),
    )?);
    written.push(write_artifact(
        &repo.join(CODEOWNERS_REL),
        &codeowners(&policy),
    )?);

    // R6 generated-artifact marking: a non-destructive `.gitattributes` block
    // (linguist-generated + provenance headers), only when a write changes it.
    if let Some(path) = write_gitattributes_plane(repo, &policy)? {
        written.push(path);
    }

    let view = CompileView {
        planes_written: written.iter().map(|p| p.display().to_string()).collect(),
    };
    if json {
        print_json(&view)?;
    } else {
        let mut message = String::from("compiled aoa-policy.yaml -> 3 enforcement planes\n");
        for path in &view.planes_written {
            message.push_str(&format!("  wrote {path}\n"));
        }
        print_human(&message);
    }
    Ok(0)
}

/// The guard result: the staged files blocked by a protected path, identical
/// in both registers.
#[derive(Debug, Serialize)]
struct GuardStagedView {
    blocked: Vec<String>,
}

/// Pre-commit entry point: exit 1 if any staged file is protected. The human
/// register stays on stderr (what pre-commit surfaces); `--json` reports the
/// same blocked list on stdout.
fn guard_staged(repo: &Path, files: &[PathBuf], json: bool) -> Result<i32> {
    let compiled = load_policy(repo)?
        .compile()
        .context("compiling policy globs")?;

    let view = GuardStagedView {
        blocked: files
            .iter()
            .filter(|f| compiled.is_protected(&f.to_string_lossy()))
            .map(|f| f.display().to_string())
            .collect(),
    };

    if json {
        print_json(&view)?;
    } else {
        for f in &view.blocked {
            eprintln!("aoa: protected path may not be committed: {f}");
        }
    }
    Ok(i32::from(!view.blocked.is_empty()))
}

/// The infer-owners result. Both registers derive from this one value, so the
/// findings are identical: the human render prints `diff` plus a per-entry
/// summary of `entries` (R17).
#[derive(Debug, Serialize)]
struct InferOwnersView {
    codeowners_path: String,
    entries: Vec<OwnedPattern>,
    proposal: String,
    diff: String,
    written: bool,
}

/// R16: propose a CODEOWNERS map from git-blame line ownership. Mechanism only:
/// `git blame` supplies per-line author attribution; the aoa-policy crate does
/// the majority arithmetic. Default is read-only (prints the reviewable diff);
/// `--write` writes the proposal to `.github/CODEOWNERS`.
fn infer_owners_cmd(repo: &Path, write: bool, json: bool) -> Result<i32> {
    let mut counts = Vec::new();
    for path in tracked_blob_paths(repo)? {
        counts.extend(blame_counts(repo, &path)?);
    }
    let entries = infer_owners(&counts);
    let proposal = proposed_codeowners(&entries);

    let codeowners_path = repo.join(CODEOWNERS_REL);
    let existing = match std::fs::read_to_string(&codeowners_path) {
        Ok(content) => Some(content),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to read {}", codeowners_path.display()))
        }
    };
    let diff = render_proposal_diff(CODEOWNERS_REL, existing.as_deref(), &proposal);

    // Writing an ownerless proposal would clobber CODEOWNERS with an empty
    // spine; refuse the write and say so instead of silently degrading.
    if write && entries.is_empty() {
        bail!(
            "no blame-attributed lines under {} — refusing to write an empty CODEOWNERS proposal",
            repo.display()
        );
    }
    if write {
        write_artifact(&codeowners_path, &proposal)?;
    }

    let view = InferOwnersView {
        codeowners_path: CODEOWNERS_REL.to_string(),
        entries,
        proposal,
        diff,
        written: write,
    };
    if json {
        print_json(&view)?;
    } else {
        print_human(&render_infer_owners_human(&view));
    }
    Ok(0)
}

/// Human register for infer-owners: the reviewable diff, the per-entry line
/// arithmetic backing it, and what (if anything) was written.
fn render_infer_owners_human(view: &InferOwnersView) -> String {
    if view.entries.is_empty() {
        return "AOA infer-owners: no blame-attributed lines; nothing to propose.\n".to_string();
    }
    let mut out =
        String::from("AOA infer-owners: proposed CODEOWNERS from git-blame line ownership.\n\n");
    out.push_str(&view.diff);
    out.push('\n');
    for entry in &view.entries {
        out.push_str(&format!(
            "  {} -> {} (owns {} of {} attributed lines)\n",
            entry.pattern, entry.owner, entry.owned_lines, entry.total_lines
        ));
    }
    if view.written {
        out.push_str(&format!("\nwrote {}\n", view.codeowners_path));
    } else {
        out.push_str(&format!(
            "\nProposal only — run with --write to write {}.\n",
            view.codeowners_path
        ));
    }
    out
}

/// List tracked blob paths via `git ls-files -sz`, skipping gitlink (submodule)
/// entries, which cannot be blamed.
fn tracked_blob_paths(repo: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "-sz"])
        .output()
        .context("running git ls-files (is git installed?)")?;
    if !output.status.success() {
        bail!(
            "git ls-files failed in {}: {}",
            repo.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let stdout = String::from_utf8(output.stdout).context("git ls-files output was not UTF-8")?;
    let mut paths = Vec::new();
    for entry in stdout.split('\0').filter(|e| !e.is_empty()) {
        // Entry shape: "<mode> <oid> <stage>\t<path>".
        let (meta, path) = entry
            .split_once('\t')
            .with_context(|| format!("unparseable git ls-files entry: {entry}"))?;
        let mode = meta.split(' ').next().unwrap_or_default();
        if mode == "160000" {
            continue; // Submodule gitlink: no lines to attribute.
        }
        paths.push(path.to_string());
    }
    Ok(paths)
}

/// Count blamed lines per author for one committed file. Uses
/// `--line-porcelain` so every line carries its `author-mail` attribution.
fn blame_counts(repo: &Path, path: &str) -> Result<Vec<BlameCount>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["blame", "--line-porcelain", "HEAD", "--"])
        .arg(path)
        .output()
        .context("running git blame")?;
    if !output.status.success() {
        bail!(
            "git blame failed for {path}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut per_author: BTreeMap<String, u64> = BTreeMap::new();
    for line in text.lines() {
        if let Some(mail) = line.strip_prefix("author-mail ") {
            let author = mail
                .trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string();
            *per_author.entry(author).or_default() += 1;
        }
    }
    Ok(per_author
        .into_iter()
        .map(|(author, lines)| BlameCount {
            path: path.to_string(),
            author,
            lines,
        })
        .collect())
}

/// Read and parse `<repo>/aoa-policy.yaml`, failing loud if it is absent or
/// malformed — compiling planes from a missing policy is a user error, not a
/// silent empty default.
fn load_policy(repo: &Path) -> Result<Policy> {
    let path = repo.join("aoa-policy.yaml");
    let raw = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "no policy file at {} (create aoa-policy.yaml)",
            path.display()
        )
    })?;
    Policy::from_yaml(&raw).with_context(|| format!("invalid policy at {}", path.display()))
}

/// Write a generated artifact, creating parent directories. Content is
/// deterministic, so an unchanged policy rewrites identical bytes.
fn write_artifact(path: &Path, content: &str) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path.to_path_buf())
}
