use std::path::PathBuf;

/// Errors produced while reading a codeprobe transcript from disk.
///
/// Parsing itself is lenient at the line level (malformed lines are skipped and
/// surfaced as warnings on [`crate::ShimResult`], mirroring codeprobe's own
/// stream-json reader); the only hard failure is being unable to read the file.
#[derive(Debug, thiserror::Error)]
pub enum ShimError {
    /// The transcript file could not be read from disk.
    #[error("failed to read transcript {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
