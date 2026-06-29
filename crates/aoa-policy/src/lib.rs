//! The single policy source — `aoa-policy.yaml` — and its compilation to the
//! three enforcement planes (PRD R5).
//!
//! One declared policy (protected paths, generated globs, mutation-gateway
//! allowlist, the reproduction-gate toggle) compiles deterministically to:
//!
//! - a **runtime** plane (the Claude Code hooks; generated in the CLI layer,
//!   which owns the hook wire-format),
//! - a **pre-commit** plane ([`precommit_config`]),
//! - a **CI** plane ([`ci_workflow`] + [`codeowners`]).
//!
//! Three planes because each has a different bypass: the runtime hook guards the
//! agent, the pre-commit hook guards a local `git commit` (bypassable with
//! `--no-verify`), and CI is the unbypassable backstop.
//!
//! This crate is mechanism only (ZFC): schema parsing, glob membership, and
//! string templating. It makes no semantic judgment — the *policy* is the
//! operator's, declared in the file.

use serde::Deserialize;
use thiserror::Error;

mod generate;

pub use generate::{ci_workflow, codeowners, precommit_config};

/// Errors raised parsing or compiling a policy.
#[derive(Debug, Error)]
pub enum PolicyError {
    /// The YAML did not parse against the [`Policy`] schema (unknown key, wrong
    /// type, malformed document). Fails loud rather than defaulting.
    #[error("invalid aoa-policy.yaml: {0}")]
    Parse(#[from] serde_yaml::Error),

    /// A declared glob is not a valid pattern.
    #[error("invalid glob '{glob}' in {field}: {source}")]
    Glob {
        field: &'static str,
        glob: String,
        source: globset::Error,
    },
}

/// The operator's declared policy. Every field is optional and defaults to the
/// least-surprising empty/permissive value, except `reproduction_required`
/// which defaults on — the safe default is to enforce the R7 gate.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Globs an agent may not write. Enforced at all three planes.
    #[serde(default)]
    pub protected_paths: Vec<String>,

    /// Globs that are generated artifacts (edit the source instead). Declared
    /// here; the in-band marking is R6.
    #[serde(default)]
    pub generated_paths: Vec<String>,

    /// Modules permitted to perform state-changing writes (mutation gateways);
    /// becomes the CODEOWNERS spine.
    #[serde(default)]
    pub gateway_allowlist: Vec<String>,

    /// Whether the runtime reproduction-before-mutation gate (R7) is active.
    #[serde(default = "default_true")]
    pub reproduction_required: bool,
}

fn default_true() -> bool {
    true
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            protected_paths: Vec::new(),
            generated_paths: Vec::new(),
            gateway_allowlist: Vec::new(),
            reproduction_required: true,
        }
    }
}

impl Policy {
    /// Parse a policy from YAML text, failing loud on any schema violation.
    pub fn from_yaml(yaml: &str) -> Result<Self, PolicyError> {
        let policy: Policy = serde_yaml::from_str(yaml)?;
        // Compile eagerly so a bad glob is reported at load, not at first write.
        policy.compile()?;
        Ok(policy)
    }

    /// Compile the protected-path globs into a matcher. Surfaces a bad pattern
    /// as a typed error rather than silently dropping it.
    pub fn compile(&self) -> Result<CompiledPolicy, PolicyError> {
        let mut builder = globset::GlobSetBuilder::new();
        for pattern in &self.protected_paths {
            let glob = globset::Glob::new(pattern).map_err(|source| PolicyError::Glob {
                field: "protected_paths",
                glob: pattern.clone(),
                source,
            })?;
            builder.add(glob);
        }
        let protected = builder.build().map_err(|source| PolicyError::Glob {
            field: "protected_paths",
            glob: self.protected_paths.join(", "),
            source,
        })?;
        Ok(CompiledPolicy { protected })
    }
}

/// A policy with its globs compiled, ready to test paths against.
#[derive(Debug, Clone)]
pub struct CompiledPolicy {
    protected: globset::GlobSet,
}

impl CompiledPolicy {
    /// Whether `path` (repo-relative) is write-protected by the policy.
    pub fn is_protected(&self, path: &str) -> bool {
        self.protected.is_match(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_policy() {
        let yaml = r#"
protected_paths:
  - "migrations/**"
  - ".github/**"
generated_paths:
  - "**/*.gen.rs"
gateway_allowlist:
  - "src/db/gateway.rs"
reproduction_required: true
"#;
        let policy = Policy::from_yaml(yaml).unwrap();
        assert_eq!(policy.protected_paths.len(), 2);
        assert!(policy.reproduction_required);
    }

    #[test]
    fn reproduction_required_defaults_on() {
        let policy = Policy::from_yaml("protected_paths: []").unwrap();
        assert!(policy.reproduction_required);
    }

    #[test]
    fn unknown_key_fails_loud() {
        let err = Policy::from_yaml("protcted_paths: []").unwrap_err();
        assert!(matches!(err, PolicyError::Parse(_)));
    }

    #[test]
    fn bad_glob_fails_loud_at_load() {
        let err = Policy::from_yaml("protected_paths: [\"[unclosed\"]").unwrap_err();
        assert!(matches!(err, PolicyError::Glob { .. }));
    }

    #[test]
    fn protected_matching_respects_globs() {
        let policy =
            Policy::from_yaml("protected_paths: [\"migrations/**\", \".github/**\"]").unwrap();
        let compiled = policy.compile().unwrap();
        assert!(compiled.is_protected("migrations/0001_init.sql"));
        assert!(compiled.is_protected(".github/workflows/ci.yml"));
        assert!(!compiled.is_protected("src/lib.rs"));
    }
}
