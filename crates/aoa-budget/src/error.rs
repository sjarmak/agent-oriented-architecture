use std::path::PathBuf;

use thiserror::Error;

/// Errors raised while resolving, counting, or fixing a context budget.
#[derive(Debug, Error)]
pub enum BudgetError {
    /// A context file could not be read from disk.
    #[error("failed to read context file {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("context file {path} exceeds {max_bytes} bytes")]
    Oversized { path: PathBuf, max_bytes: u64 },

    #[error("context file {path} resolves outside {boundary}")]
    OutsideBoundary { path: PathBuf, boundary: PathBuf },

    #[error("context file {path} is the same file as {archive}, the name its archive would take")]
    ArchiveIsRoot { path: PathBuf, archive: PathBuf },

    #[error("context file {path} is reached through {archive}, the name its archive would take")]
    RootThroughArchive { path: PathBuf, archive: PathBuf },

    #[error(
        "failed to replace {path}, and its temporary file {temp} could not be removed \
         ({removal}) and is left behind"
    )]
    TempFileLeftBehind {
        path: PathBuf,
        temp: PathBuf,
        #[source]
        source: std::io::Error,
        removal: std::io::Error,
    },

    #[error("context closure of {path} came back without the file it was resolved from")]
    RootNotRead { path: PathBuf },

    /// The requested target-model tokenizer name is not supported.
    ///
    /// This is raised loudly (never silently defaulted) so a misconfigured
    /// target model fails the gate instead of being scored against a guessed
    /// encoding.
    #[error("unknown target tokenizer '{name}' (supported: {supported})")]
    UnknownTargetTokenizer { name: String, supported: String },

    /// A `fix` operation ran but the resulting closure is still over budget.
    #[error("fix did not bring closure under ceiling: {target_tokens} target tokens >= ceiling {ceiling}")]
    FixFailed {
        target_tokens: usize,
        ceiling: usize,
    },
}
