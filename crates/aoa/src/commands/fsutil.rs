//! Byte-bounded file reads for untrusted and operator-supplied JSON reached by
//! the CLI.
//!
//! Two threat classes route through here. `aoa eval-run` walks an untrusted
//! `--codeprobe-run` directory and reads per-trial JSON from it
//! (attacker-controlled); `aoa r0b` reads its per-trial JSON through
//! `aoa_bench`, while `aoa eval experiment` delegates all run evidence to
//! `aoa_falsify_build`. Both reach this module only for operator-supplied
//! manifests. Other commands (`falsify`, `eval compare`, the canary
//! manifest) read operator-supplied JSON paths and
//! external-tool output (codeprobe `aggregate.json`). Both bound the bytes held
//! in memory from any one file so a crafted or pathological input cannot exhaust
//! memory.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

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

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_as(path, bytes, FileAccess::Default)
}

pub(crate) fn write_atomic_owner_only(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_as(path, bytes, FileAccess::OwnerOnly)
}

#[derive(Clone, Copy)]
enum FileAccess {
    Default,
    OwnerOnly,
}

impl FileAccess {
    fn open_options(self) -> std::fs::OpenOptions {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if matches!(self, Self::OwnerOnly) {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
    }
}

fn write_atomic_as(path: &Path, bytes: &[u8], access: FileAccess) -> Result<()> {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("output path must have a UTF-8 file name")?;
    let temporary = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = access
            .open_options()
            .open(&temporary)
            .with_context(|| format!("failed to create {}", temporary.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed to write {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync {}", temporary.display()))?;
        std::fs::rename(&temporary, path)
            .with_context(|| format!("failed to install {}", path.display()))
    })();
    let Err(err) = result else {
        return Ok(());
    };
    match std::fs::remove_file(&temporary) {
        Err(unlink) if unlink.kind() != std::io::ErrorKind::NotFound => Err(err.context(format!(
            "the temporary file {} was left behind after the failed install: {unlink}",
            temporary.display()
        ))),
        _ => Err(err),
    }
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

    #[test]
    fn a_failed_install_reports_the_target_and_leaves_no_temporary_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("report.json");
        std::fs::create_dir(&target).unwrap();

        let err = write_atomic(&target, b"{}").unwrap_err();

        assert!(
            format!("{err:#}").contains(&format!("failed to install {}", target.display())),
            "{err:#}"
        );
        let left_behind: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left_behind, ["report.json"]);
    }
}
