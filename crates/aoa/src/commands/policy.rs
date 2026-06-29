use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use aoa_policy::{ci_workflow, codeowners, precommit_config, Policy};

use crate::cli::{PolicyArgs, PolicyCommand};
use crate::commands::enforce::install_enforce_hooks;
use crate::forge::compile_enforcement;
use crate::output::print_human;

/// Enforcement-plane policy utilities (R5). `compile` turns the single
/// `aoa-policy.yaml` into all three planes; `guard-staged` is the pre-commit
/// plane's entry point.
pub fn run(args: &PolicyArgs) -> Result<i32> {
    match &args.command {
        PolicyCommand::Compile { repo, forge } => compile(repo, forge),
        PolicyCommand::GuardStaged { repo, files } => guard_staged(repo, files),
    }
}

/// Compile the policy to the runtime, pre-commit, and CI planes. Deterministic:
/// every artifact is a pure function of the policy, so re-running writes
/// byte-identical content (idempotent).
fn compile(repo: &Path, forge: &str) -> Result<i32> {
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
        &repo.join(".github").join("CODEOWNERS"),
        &codeowners(&policy),
    )?);

    let mut message = String::from("compiled aoa-policy.yaml -> 3 enforcement planes\n");
    for path in &written {
        message.push_str(&format!("  wrote {}\n", path.display()));
    }
    print_human(&message);
    Ok(0)
}

/// Pre-commit entry point: exit 1 if any staged file is protected.
fn guard_staged(repo: &Path, files: &[PathBuf]) -> Result<i32> {
    let compiled = load_policy(repo)?
        .compile()
        .context("compiling policy globs")?;

    let blocked: Vec<&PathBuf> = files
        .iter()
        .filter(|f| compiled.is_protected(&f.to_string_lossy()))
        .collect();

    if blocked.is_empty() {
        return Ok(0);
    }
    for f in &blocked {
        eprintln!("aoa: protected path may not be committed: {}", f.display());
    }
    Ok(1)
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
