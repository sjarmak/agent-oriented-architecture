use thiserror::Error;

/// A self-contained R0 evidence-builder failure.
///
/// The builder records some nested failures as persisted exclusion reasons, so
/// callers must never need to walk a source chain to recover the useful text.
/// The message is therefore flattened once at the public boundary and carries
/// no `#[source]`.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct FalsifyBuildError {
    pub(crate) message: String,
}

impl FalsifyBuildError {
    pub(crate) fn from_anyhow(error: anyhow::Error) -> Self {
        let rendered = format!("{error:#}");
        let mut message = String::with_capacity(rendered.len());
        // The library cannot assume its caller has the CLI's terminal
        // sanitizer. Escape control characters at this boundary while
        // preserving operator-facing punctuation byte-for-byte.
        for character in rendered.chars() {
            if character.is_control() {
                message.extend(character.escape_debug());
            } else {
                message.push(character);
            }
        }
        Self { message }
    }
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, FalsifyBuildError>;
