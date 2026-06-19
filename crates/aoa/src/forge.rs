use thiserror::Error;

/// The forges for which this CLI can compile an enforcement plane. This is the
/// fail-loud guarantee (R-silent), not full Wave-1 policy compilation: the set
/// is deliberately small and an unknown forge is rejected, never silently
/// no-op'd.
const SUPPORTED_FORGES: [&str; 2] = ["github-actions", "gitlab-ci"];

/// Failure to compile an enforcement plane for a forge.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ForgeError {
    /// The named forge has no enforcement-plane adapter. We refuse rather than
    /// emit nothing, so a misconfigured forge fails loudly.
    #[error("unsupported forge '{forge}': no enforcement-plane adapter (supported: {supported})")]
    Unsupported { forge: String, supported: String },
}

/// Compile the enforcement plane for `forge`.
///
/// Returns the adapter identifier on success and a loud [`ForgeError`] for any
/// forge without an adapter — it never returns `Ok` for an unknown forge.
pub fn compile_enforcement(forge: &str) -> Result<String, ForgeError> {
    if SUPPORTED_FORGES.contains(&forge) {
        Ok(format!("compiled enforcement plane for {forge}"))
    } else {
        Err(ForgeError::Unsupported {
            forge: forge.to_string(),
            supported: SUPPORTED_FORGES.join(", "),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_forge_compiles() {
        assert!(compile_enforcement("github-actions").is_ok());
    }

    #[test]
    fn unknown_forge_fails_loudly() {
        let err = compile_enforcement("svn-hooks").unwrap_err();
        assert!(err.to_string().contains("unsupported forge"));
    }
}
