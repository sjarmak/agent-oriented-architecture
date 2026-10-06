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

    #[error("refusing to archive context file {path}: {archive} already exists")]
    ArchiveExists { path: PathBuf, archive: PathBuf },

    #[error("failed to replace {path}, and {}", left_behind(.left))]
    TempFileLeftBehind {
        path: PathBuf,
        left: Vec<LeftBehindTemp>,
        #[source]
        source: std::io::Error,
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

#[derive(Debug)]
pub struct LeftBehindTemp {
    pub temp: PathBuf,
    pub removal: std::io::Error,
}

fn left_behind(left: &[LeftBehindTemp]) -> String {
    let named: Vec<String> = left
        .iter()
        .map(|file| format!("{} ({})", file.temp.display(), file.removal))
        .collect();
    match named.as_slice() {
        [one] => format!("its temporary file {one} could not be removed and is left behind"),
        many => format!(
            "its temporary files {} could not be removed and are left behind",
            many.join(" and ")
        ),
    }
}
