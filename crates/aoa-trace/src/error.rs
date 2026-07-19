use std::path::PathBuf;

/// Errors produced while loading or validating a trace file.
#[derive(Debug, thiserror::Error)]
pub enum TraceError {
    /// The trace file could not be read from disk.
    #[error("failed to read trace file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The trace file exceeded the byte cap before it could be read into memory.
    #[error("trace file {path} exceeds {max} byte cap (DoS guard)")]
    TooLarge { path: PathBuf, max: u64 },

    /// The file was not structurally valid JSON matching the trace schema.
    #[error("trace file {path} is not schema-valid: {source}")]
    Schema {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// The trace envelope declared a wire-format version this build cannot
    /// parse. Fails fast so a producer-side format change surfaces loudly
    /// instead of silently mis-parsing into the current span shape.
    #[error(
        "trace declares unsupported wire-format version {found} \
         (this build reads version {supported})"
    )]
    UnsupportedVersion { found: u32, supported: u32 },

    /// Spans were not in monotonically non-decreasing `seq` order.
    #[error("trace spans are out of order at index {index}: seq {seq} < previous seq {previous}")]
    OutOfOrder {
        index: usize,
        seq: u64,
        previous: u64,
    },

    /// A post-parse validation failure for a trace loaded from disk, tagged with
    /// the offending file.
    ///
    /// [`UnsupportedVersion`] and [`OutOfOrder`] describe the *content* of a
    /// trace, so they are produced by the path-free in-memory checks
    /// ([`crate::TraceEnvelope::into_trace`], [`crate::validate_trace_value`])
    /// that callers holding a `Trace` reuse. Attaching the path is therefore a
    /// boundary concern, not a per-variant one: carrying an `Option<PathBuf>` on
    /// every variant would make "in-memory validation is path-free" a runtime
    /// property instead of a type-level one.
    ///
    /// The wrapper is depth-1 by construction — [`crate::validate_trace`] is the
    /// only site that builds it, and it wraps only path-free inner failures.
    ///
    /// This is `aoa-trace`'s own disk boundary. An outer crate that already names
    /// the file in its own error (e.g. `aoa_observe_shim`'s corpus ingest, which
    /// calls the path-free entry points directly) must not add the path a second
    /// time.
    ///
    /// Display deliberately does not interpolate `{source}`: the inner message is
    /// rendered by source-chain walking (`anyhow`'s `{err:#}`, `std::error`
    /// iteration), so interpolating it here would print the reason twice.
    ///
    /// [`UnsupportedVersion`]: TraceError::UnsupportedVersion
    /// [`OutOfOrder`]: TraceError::OutOfOrder
    #[error("trace file {path} is invalid")]
    InvalidFile {
        path: PathBuf,
        // Boxed because the variant makes `TraceError` recursive — this is
        // required for the type to be sized, not a size optimization.
        #[source]
        source: Box<TraceError>,
    },
}
