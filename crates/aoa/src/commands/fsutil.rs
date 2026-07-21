//! Byte-bounded file reads for untrusted and operator-supplied JSON reached by
//! the CLI.
//!
//! Two threat classes route through here. `aoa eval-run` walks an untrusted
//! `--codeprobe-run` directory and reads per-trial JSON from it
//! (attacker-controlled); `aoa r0b` and `aoa eval experiment` read their
//! per-trial JSON through `aoa_bench` instead, and reach this module only for
//! their operator-supplied manifests. Other commands (`falsify`, `eval compare`, `eval
//! experiment`, the canary manifest) read operator-supplied JSON paths and
//! external-tool output (codeprobe `aggregate.json`). Both bound the bytes held
//! in memory from any one file so a crafted or pathological input cannot exhaust
//! memory.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

/// Largest single JSON file read into memory by the CLI. These files (per-trial
/// `scoring.json`, run files, manifests, `aggregate.json`) are small by nature;
/// the cap only trips pathological or hostile input.
pub(crate) const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;

/// Largest number of mined task directories accepted under one task tree. Bounds
/// the work an operator-supplied tree of millions of subdirs can induce.
///
/// `aoa-bench` caps *trials* under a codeprobe run dir separately — same value,
/// different tree and different threat, deliberately kept independently tunable.
pub(crate) const MAX_TASK_DIRS: usize = 100_000;

/// Read `path` into a `String`, rejecting anything past `max` bytes.
///
/// Names `path` in all three failure modes, and `main` renders `{err:#}`, so a
/// caller that adds its own path-naming context prints the path twice. Name the
/// file class ("run file") if it adds something; leave the path to this function.
///
/// Bounded via [`Read::take`] rather than a pre-read `metadata().len()` check: a
/// file that grows (or a symlink whose target swaps) between stat and read cannot
/// blow past the cap. One byte past `max` is read so an exactly-`max` file is
/// accepted while a larger one is rejected.
pub(crate) fn read_to_string_capped(path: &Path, max: u64) -> Result<String> {
    let file =
        std::fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut raw = String::new();
    let read = file
        .take(max + 1)
        .read_to_string(&mut raw)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if read as u64 > max {
        anyhow::bail!("{} exceeds {} byte cap (DoS guard)", path.display(), max);
    }
    Ok(raw)
}

/// Read a byte-capped JSON file and deserialize it in one step.
///
/// Folds the `read_to_string_capped` -> `serde_json::from_str` -> `with_context`
/// trio that every JSON-loading command otherwise hand-rolls: the [`MAX_JSON_BYTES`]
/// DoS guard is applied structurally rather than re-remembered per author
/// (arch-review Finding #5). `label` names the file class (e.g. `"run file"`,
/// `"canary manifest"`) in both error contexts. Only the parse leg adds the path,
/// per [`read_to_string_capped`]'s contract — serde's error carries none. The path
/// is `Display`, never interpolated attacker text.
///
/// Sites whose parse error carries a bespoke diagnostic (e.g. "expected a
/// bias_warnings array") intentionally stay hand-rolled; this helper is for the
/// symmetric read/parse pair.
pub(crate) fn load_json_capped<T: DeserializeOwned>(path: &Path, label: &str) -> Result<T> {
    let raw = read_to_string_capped(path, MAX_JSON_BYTES)
        .with_context(|| format!("failed to read {label}"))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {label} {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_over_cap_and_accepts_exactly_cap() {
        let dir = std::env::temp_dir().join(format!("aoa-fsutil-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scoring.json");
        std::fs::write(&path, "0123456789").unwrap(); // 10 bytes

        let err = read_to_string_capped(&path, 4).unwrap_err();
        assert!(err.to_string().contains("byte cap"));
        assert_eq!(read_to_string_capped(&path, 10).unwrap().len(), 10);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_json_capped_parses_and_labels_errors() {
        let dir = std::env::temp_dir().join(format!("aoa-fsutil-json-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let ok_path = dir.join("ok.json");
        std::fs::write(&ok_path, r#"["a","b"]"#).unwrap();
        let parsed: Vec<String> = load_json_capped(&ok_path, "widget list").unwrap();
        assert_eq!(parsed, vec!["a".to_string(), "b".to_string()]);

        // A parse failure names the label and the path so the operator can tell
        // which file class tripped.
        let bad_path = dir.join("bad.json");
        std::fs::write(&bad_path, "not json").unwrap();
        let err = load_json_capped::<Vec<String>>(&bad_path, "widget list").unwrap_err();
        assert!(err.to_string().contains("failed to parse widget list"));

        // A missing file surfaces through the read leg, also labelled.
        let missing = dir.join("nope.json");
        let err = load_json_capped::<Vec<String>>(&missing, "widget list").unwrap_err();
        assert!(err.to_string().contains("failed to read widget list"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
