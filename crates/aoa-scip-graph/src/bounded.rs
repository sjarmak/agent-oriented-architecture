//! Byte-bounded file reads for attacker-controlled index sources.

use std::io::Read;
use std::path::Path;

use crate::error::ScipGraphError;

/// Largest SCIP JSON index accepted. The vendored simplified form is small; this
/// ceiling bounds the bytes held from a crafted index without rejecting a real
/// one.
pub(crate) const MAX_SCIP_BYTES: u64 = 128 * 1024 * 1024;

/// Largest single source file read during a best-effort scan. A hand-written
/// module is virtually never this large; generated files (e.g. `*_pb2.py`) stay
/// well under it, so the cap only trips pathological or hostile input.
pub(crate) const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;

/// Read `path` into a `String`, rejecting anything past `max` bytes.
///
/// Bounded via [`Read::take`] rather than a pre-read `metadata().len()` check: a
/// file that grows (or a symlink whose target swaps) between stat and read cannot
/// blow past the cap. One byte past `max` is read so an exactly-`max` file is
/// accepted while a larger one is rejected.
pub(crate) fn read_capped(path: &Path, max: u64) -> Result<String, ScipGraphError> {
    let file = std::fs::File::open(path).map_err(|source| ScipGraphError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let mut raw = String::new();
    let read = file
        .take(max + 1)
        .read_to_string(&mut raw)
        .map_err(|source| ScipGraphError::Io {
            path: path.display().to_string(),
            source,
        })?;
    if read as u64 > max {
        return Err(ScipGraphError::TooLarge {
            path: path.display().to_string(),
            max,
        });
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_files_over_the_cap_and_accepts_exactly_the_cap() {
        let dir = std::env::temp_dir().join(format!("aoa-scip-bounded-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("idx.json");
        std::fs::write(&path, "0123456789").unwrap(); // 10 bytes

        let err = read_capped(&path, 4).unwrap_err();
        assert!(matches!(err, ScipGraphError::TooLarge { max: 4, .. }));
        assert_eq!(read_capped(&path, 10).unwrap().len(), 10);

        std::fs::remove_dir_all(&dir).ok();
    }
}
