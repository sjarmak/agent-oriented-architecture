use thiserror::Error;

/// A self-contained R0 evidence-builder failure.
///
/// The builder records some nested failures as persisted exclusion reasons, so
/// callers must never need to walk a source chain to recover the useful text.
/// The message is therefore flattened once at the public boundary and carries
/// no `#[source]`.
///
/// This type is also where the crate's `anyhow` dependency stops. The workspace
/// convention is thiserror for libraries, and that is what callers see: no
/// public item in this crate names an `anyhow` type. Internally the builder
/// threads ad-hoc string context through several assembly stages, which is what
/// `anyhow` is for and what a typed enum would only reimplement; [`from_anyhow`]
/// is `pub(crate)` and converts at the single boundary.
///
/// [`from_anyhow`]: FalsifyBuildError::from_anyhow
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
