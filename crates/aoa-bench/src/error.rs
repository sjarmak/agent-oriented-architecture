use std::path::PathBuf;

use thiserror::Error;

/// Errors raised while loading a codeprobe-mined task directory.
#[derive(Debug, Error)]
pub enum BenchError {
    /// The task directory or a required file inside it could not be read.
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Neither `metadata.json` nor `task.toml` was present, so the directory is
    /// not a recognizable codeprobe task.
    #[error("{0} is not a codeprobe task dir: no metadata.json or task.toml")]
    NotATask(PathBuf),

    /// A JSON file in the task dir was malformed.
    #[error("failed to parse {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// The `task.toml` manifest was malformed.
    #[error("failed to parse {path}: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    /// A file in the task dir exceeded its byte cap before being read. Guards
    /// against an attacker-controlled task dir feeding an oversized JSON file.
    #[error("{path} exceeds {max} byte cap (DoS guard)")]
    TooLarge { path: PathBuf, max: u64 },
}
