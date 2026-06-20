//! Byte-bounded file reads for attacker-controlled inputs reached by the CLI.
//!
//! `aoa eval-run` and `aoa r0b` walk an untrusted `--codeprobe-run` / `--tasks`
//! directory and read per-trial JSON from it. These helpers bound the bytes held
//! in memory from any one such file so a crafted run dir cannot exhaust memory.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};

/// Largest per-trial JSON file (`scoring.json`) read into memory. These files are
/// small by nature; the cap only trips pathological or hostile input.
pub(crate) const MAX_TRIAL_JSON_BYTES: u64 = 16 * 1024 * 1024;

/// Largest number of task-trial subdirectories accepted under one run dir. Bounds
/// the work a crafted run dir of millions of empty subdirs can induce.
pub(crate) const MAX_TASK_DIRS: usize = 100_000;

/// Read `path` into a `String`, rejecting anything past `max` bytes.
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
}
