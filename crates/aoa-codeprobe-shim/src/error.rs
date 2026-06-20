use std::path::PathBuf;

/// Errors produced while reading or parsing a codeprobe transcript.
///
/// Parsing is lenient at the line level (malformed lines are skipped and
/// surfaced as warnings on [`crate::ShimResult`], mirroring codeprobe's own
/// stream-json reader). The hard failures are being unable to read the file and
/// resource-bound breaches on attacker-controlled input — an oversized
/// transcript or a span count past the cap. Bound breaches fail loud rather than
/// silently truncating the trace, because the trace feeds R0 process metrics.
#[derive(Debug, thiserror::Error)]
pub enum ShimError {
    /// The transcript file could not be read from disk.
    #[error("failed to read transcript {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The transcript file exceeded the byte cap before parsing began.
    #[error("transcript {path} exceeds {max} byte cap (DoS guard)")]
    TranscriptTooLarge { path: PathBuf, max: u64 },

    /// The transcript would exceed the span cap. Failing here is deliberate: a
    /// silently truncated trace would feed wrong locality metrics.
    #[error("transcript exceeds the {max}-span cap (DoS guard)")]
    TooManySpans { max: usize },
}
