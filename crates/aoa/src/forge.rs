use thiserror::Error;

/// The forges for which this CLI can compile an enforcement plane. This is the
/// fail-loud guarantee (R-silent), not full Wave-1 policy compilation: the set
/// is deliberately small and an unknown forge is rejected, never silently
/// no-op'd.
const SUPPORTED_FORGE: &str = "github-actions";

/// Failure to compile an enforcement plane for a forge.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ForgeError {
    /// The named forge has no enforcement-plane adapter. We refuse rather than
    /// emit nothing, so a misconfigured forge fails loudly.
    #[error("unsupported forge '{forge}': no enforcement-plane adapter (supported: {supported})")]
    Unsupported { forge: String, supported: String },
}

pub fn validate_forge(forge: &str) -> Result<(), ForgeError> {
    if forge == SUPPORTED_FORGE {
        Ok(())
    } else {
        Err(ForgeError::Unsupported {
            forge: forge.to_string(),
            supported: SUPPORTED_FORGE.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_github_actions_has_an_enforcement_adapter() {
        assert_eq!(validate_forge("github-actions"), Ok(()));
        assert!(matches!(
            validate_forge("gitlab-ci"),
            Err(ForgeError::Unsupported { forge, .. }) if forge == "gitlab-ci"
        ));
    }
}
