//! TypeScript/JS dead-import adapter: a vendored, pinned ESLint +
//! `eslint-plugin-unused-imports` run hermetically against the isolated copy.
//!
//! Core ESLint `no-unused-vars` is variable-scoped (covers locals and params) AND
//! non-fixable, so it cannot be the lint class here. The only import-scoped,
//! auto-fixable rule is the community plugin's `unused-imports/no-unused-imports`,
//! which deletes only unused import specifiers/declarations and never reorders or
//! adds imports. Because that rule is not in ESLint core, we vendor a pinned
//! `node_modules` (ESLint + the plugin + the TS parser) and a single-rule flat
//! config inside the crate's `assets/eslint/` dir and run them hermetically:
//!
//! - `--config <our-config> --no-config-lookup` makes ESLint ignore the repo's own
//!   `eslint.config.*` — the repo cannot widen scope.
//! - `--no-inline-config` ignores in-source `/* eslint ... */` directives.
//! - `--no-ignore` stops the repo's ignore files from shrinking the set unexpectedly.
//!
//! Residual construct-validity disclosure (recorded in [`TS_DEAD_IMPORT_ELIGIBILITY`]):
//! unlike ruff's vendor-defined `F401`, WE author the analyzer config, so the
//! "exactly one lint class = unused-import" binding is our assertion (the plugin
//! choice + our config), not an upstream-blessed lint-class id. Provenance pins
//! the node/eslint/plugin versions and a config fingerprint so the assertion is
//! auditable.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use super::{collect_files, io_err, subtract_via_inplace_fix, ImportAdapter, SubtractedFile};
use crate::error::MigrateError;
use crate::fix::FixProvenance;

const TS_DEAD_IMPORT_ID: &str = "dead-imports-typescript";

/// Markers that make a tree a TypeScript/JS migration target.
const PROJECT_MARKERS: &[&str] = &["package.json", "tsconfig.json"];

/// Source extensions the adapter lints. Matches the `files` glob in the vendored
/// flat config.
const TS_EXTENSIONS: &[&str] = &["js", "jsx", "mjs", "cjs", "ts", "tsx"];

pub(crate) const TS_DEAD_IMPORT_ELIGIBILITY: &str = "ESLint unused-import removal is a construct-valid, reproducible code-layer treatment only when: \
(1) the analyzer is the vendored, pinned ESLint + eslint-plugin-unused-imports run with `--no-config-lookup --no-inline-config --no-ignore`, so neither the repo's eslint config nor in-source directives can widen scope — reproducibility is anchored by the node/eslint/plugin versions and config fingerprint recorded in provenance; \
(2) DISCLOSED HOLE: unlike a vendor-defined lint code (ruff F401), the 'exactly one lint class = unused-import' binding is OUR assertion (the plugin's `no-unused-imports` rule plus our single-rule flat config), not an upstream-blessed id — the certification rests on us controlling eslint+plugin+config and the repo controlling nothing; \
(3) a file ESLint cannot parse is a LOUD RepoDoesNotCheck error, never a silent empty plan. \
`node` must be on PATH; its absence is a LOUD ToolchainUnavailable.";

/// Removes ESLint-certified unused imports via a vendored, hermetic
/// `eslint --fix` restricted to the single `unused-imports/no-unused-imports` rule.
pub(crate) struct TsImportAdapter;

impl ImportAdapter for TsImportAdapter {
    fn id(&self) -> &'static str {
        TS_DEAD_IMPORT_ID
    }

    fn describe(&self) -> &'static str {
        "remove ESLint-certified unused imports via a vendored, hermetic eslint --fix (strictly subtractive)"
    }

    fn eligibility_note(&self) -> &'static str {
        TS_DEAD_IMPORT_ELIGIBILITY
    }

    fn is_eligible(&self, repo: &Path) -> bool {
        PROJECT_MARKERS.iter().any(|m| repo.join(m).is_file())
    }

    fn subtract_imports(&self, work: &Path) -> Result<Vec<SubtractedFile>, MigrateError> {
        let files = collect_files(work, &is_ts_file)?;
        // An eligible project with no source files yields a legitimate empty plan;
        // running ESLint with no file arguments would be a usage error.
        if files.is_empty() {
            return Ok(Vec::new());
        }

        // 1) Classify: a parse error is a LOUD RepoDoesNotCheck before we touch
        // anything; a missing `node` is a LOUD ToolchainUnavailable.
        classify(work, &files)?;

        // 2) Run the vendored eslint --fix in place, then diff the touched files.
        subtract_via_inplace_fix(work, &is_ts_file, run_eslint_fix)
    }

    fn provenance(&self, _repo: &Path) -> Result<Option<FixProvenance>, MigrateError> {
        let node = node_version()?;
        let eslint = vendored_pkg_version("eslint")?;
        let plugin = vendored_pkg_version("eslint-plugin-unused-imports")?;
        let config_fp = config_fingerprint()?;
        Ok(Some(FixProvenance {
            fix_id: TS_DEAD_IMPORT_ID.to_string(),
            toolchain: format!(
                "{node}; eslint {eslint}; eslint-plugin-unused-imports {plugin}; config-fp {config_fp}"
            ),
            // The analyzer is vendored and pinned by this tool, not by the repo,
            // and `--no-config-lookup` ignores any repo config: the repo pins
            // nothing we honor.
            pin_present: false,
        }))
    }
}

fn is_ts_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| TS_EXTENSIONS.contains(&e))
}

/// Absolute path to the vendored ESLint assets dir, baked at compile time.
fn vendor_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/eslint")
}

fn eslint_bin() -> PathBuf {
    vendor_dir().join("node_modules/eslint/bin/eslint.js")
}

/// The vendored `node_modules` is gitignored and regenerated with `npm ci`. A
/// missing install is a LOUD `ToolchainUnavailable`: `node` would otherwise exit 1
/// with a "Cannot find module" message that `classify` reads as a clean,
/// empty-findings run — i.e. a silent empty plan, the one outcome this adapter
/// promises never to produce.
fn ensure_vendored_eslint() -> Result<(), MigrateError> {
    let bin = eslint_bin();
    if bin.is_file() {
        return Ok(());
    }
    Err(MigrateError::ToolchainUnavailable {
        detail: format!(
            "vendored ESLint not installed at {}; run `npm ci` in {}",
            bin.display(),
            vendor_dir().display()
        ),
    })
}

fn eslint_config() -> PathBuf {
    vendor_dir().join("eslint.config.mjs")
}

/// The hermetic eslint argv shared by the classify and fix passes (minus
/// `--fix`/`--format`, which the callers append).
fn base_eslint_args() -> Vec<String> {
    vec![
        eslint_bin().to_string_lossy().into_owned(),
        "--config".to_string(),
        eslint_config().to_string_lossy().into_owned(),
        "--no-config-lookup".to_string(),
        "--no-inline-config".to_string(),
        "--no-ignore".to_string(),
    ]
}

/// Run the vendored ESLint in JSON mode (no `--fix`) and classify the outcome.
/// `Ok(())` means the files parse and any findings are our single rule.
fn classify(work: &Path, files: &[PathBuf]) -> Result<(), MigrateError> {
    ensure_vendored_eslint()?;
    let mut args = base_eslint_args();
    args.push("--format".to_string());
    args.push("json".to_string());
    // `--` terminates option parsing: a repo file literally named `--parser`
    // must reach ESLint as a path, never as a flag (argument injection).
    args.push("--".to_string());
    args.extend(files.iter().map(|f| f.to_string_lossy().into_owned()));

    let output = Command::new("node")
        .current_dir(work)
        .args(&args)
        .output()
        .map_err(|source| MigrateError::ToolchainUnavailable {
            detail: format!("could not run `node` for the vendored eslint: {source}"),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    // Exit 0 (clean) or 1 (lint findings) are expected; 2 is a config/usage fault.
    if !matches!(output.status.code(), Some(0) | Some(1)) {
        return Err(MigrateError::BuildFailed {
            stderr: if stderr.is_empty() {
                stdout.into_owned()
            } else {
                stderr
            },
        });
    }

    // The exit-code gate above already rejected anything but 0/1, so an empty
    // stdout here is a clean run with nothing to report, not a failure.
    let results: Vec<Value> = match serde_json::from_str(stdout.trim()) {
        Ok(v) => v,
        Err(_) if stdout.trim().is_empty() => Vec::new(),
        Err(e) => {
            return Err(MigrateError::BuildFailed {
                stderr: format!("eslint produced unparseable output ({e}):\n{stderr}"),
            })
        }
    };

    // A parse failure surfaces as a `fatal` message (severity 2): the tree does
    // not parse, so subtractivity cannot be certified.
    let has_fatal = results.iter().any(|file| {
        file.get("messages")
            .and_then(Value::as_array)
            .is_some_and(|msgs| {
                msgs.iter()
                    .any(|m| m.get("fatal").and_then(Value::as_bool) == Some(true))
            })
    });
    if has_fatal {
        return Err(MigrateError::RepoDoesNotCheck {
            stderr: "eslint reported a fatal parsing error; the tree does not parse cleanly"
                .to_string(),
        });
    }

    Ok(())
}

/// Apply the vendored eslint autofixer in place. `--fix-type problem` clamps the
/// applied fixes; the single-rule config restricts them to unused-import deletions.
fn run_eslint_fix(work: &Path, files: &[PathBuf]) -> Result<(), MigrateError> {
    if files.is_empty() {
        return Ok(());
    }
    ensure_vendored_eslint()?;
    let mut args = base_eslint_args();
    args.push("--fix".to_string());
    args.push("--fix-type".to_string());
    args.push("problem".to_string());
    // See `classify`: `--` stops a `--`-prefixed repo filename being read as a flag.
    args.push("--".to_string());
    args.extend(files.iter().map(|f| f.to_string_lossy().into_owned()));

    let output = Command::new("node")
        .current_dir(work)
        .args(&args)
        .output()
        .map_err(|source| MigrateError::ToolchainUnavailable {
            detail: format!("could not run `node` for the vendored eslint: {source}"),
        })?;

    // After --fix, remaining lint findings (exit 1) are acceptable — the fix
    // removed what it could. A config/usage fault (exit 2) is loud.
    if !matches!(output.status.code(), Some(0) | Some(1)) {
        return Err(MigrateError::BuildFailed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(())
}

/// `node --version`, e.g. `v22.22.2`.
fn node_version() -> Result<String, MigrateError> {
    let output = Command::new("node")
        .arg("--version")
        .output()
        .map_err(|source| MigrateError::ToolchainUnavailable {
            detail: format!("could not run `node`: {source}"),
        })?;
    if !output.status.success() {
        return Err(MigrateError::ToolchainUnavailable {
            detail: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(format!(
        "node {}",
        String::from_utf8_lossy(&output.stdout).trim()
    ))
}

/// Read `version` from a vendored package's `package.json`. A missing vendored
/// asset is a loud `ToolchainUnavailable` — we never silently proceed without the
/// pinned analyzer.
fn vendored_pkg_version(pkg: &str) -> Result<String, MigrateError> {
    let manifest = vendor_dir()
        .join("node_modules")
        .join(pkg)
        .join("package.json");
    let body = std::fs::read_to_string(&manifest).map_err(|source| {
        MigrateError::ToolchainUnavailable {
            detail: format!("vendored {pkg} missing at {}: {source}", manifest.display()),
        }
    })?;
    let json: Value =
        serde_json::from_str(&body).map_err(|e| MigrateError::ToolchainUnavailable {
            detail: format!("vendored {pkg} package.json is invalid: {e}"),
        })?;
    // A vendored package.json without a string `version` is a broken bundle, not a
    // recoverable default: fail loudly rather than record a misleading provenance.
    json.get("version")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| MigrateError::ToolchainUnavailable {
            detail: format!("vendored {pkg} package.json has no string `version` field"),
        })
}

/// A stable fingerprint of the vendored flat config, so provenance records exactly
/// which analyzer-config produced the subtraction (the disclosed construct-validity
/// hole — we author this config). FNV-1a, NOT `std`'s `DefaultHasher`: the latter is
/// SipHash with a per-process random seed, so it would emit a different value on
/// every run and an auditor could never compare two provenance records. Not
/// cryptographic; a deterministic change-detection digest.
fn config_fingerprint() -> Result<String, MigrateError> {
    let path = eslint_config();
    let body = std::fs::read(&path).map_err(|source| io_err(&path, source))?;
    Ok(format!("{:016x}", fnv1a_64(&body)))
}

/// 64-bit FNV-1a: a tiny, dependency-free, deterministic hash. Used only to
/// fingerprint the vendored config for provenance, not for any security purpose.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET_BASIS, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(PRIME)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn non_ts_tree_is_ineligible() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.ts"), "import x from 'y';\n").unwrap();
        // No package.json/tsconfig.json => ineligible.
        assert!(!TsImportAdapter.is_eligible(dir.path()));
    }

    #[test]
    fn project_marker_makes_a_tree_eligible() {
        for marker in PROJECT_MARKERS {
            let dir = TempDir::new().unwrap();
            fs::write(dir.path().join(marker), "{}").unwrap();
            assert!(
                TsImportAdapter.is_eligible(dir.path()),
                "{marker} should make the tree eligible"
            );
        }
    }

    #[test]
    fn fnv1a_is_deterministic_and_content_sensitive() {
        // The provenance contract: identical config bytes => identical fingerprint
        // (across runs/processes), different bytes => different fingerprint.
        assert_eq!(
            fnv1a_64(b"export default [];"),
            fnv1a_64(b"export default [];")
        );
        assert_ne!(fnv1a_64(b"rule: error"), fnv1a_64(b"rule: off"));
        // Known FNV-1a 64-bit vector for the empty input is the offset basis.
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn is_ts_file_matches_expected_extensions() {
        assert!(is_ts_file(Path::new("a.ts")));
        assert!(is_ts_file(Path::new("a.tsx")));
        assert!(is_ts_file(Path::new("a.mjs")));
        assert!(!is_ts_file(Path::new("a.py")));
        assert!(!is_ts_file(Path::new("a.json")));
    }
}
