use tiktoken_rs::CoreBPE;

use crate::error::BudgetError;

/// The pinned reference encoding name. Every budget report includes a count
/// under this encoding regardless of the target model, so reports are
/// comparable across targets over time.
pub const REFERENCE_ENCODING: &str = "o200k_base";

/// Build the pinned reference encoder (o200k_base).
pub fn reference_encoder() -> Result<CoreBPE, BudgetError> {
    o200k()
}

/// Resolve a target-model tokenizer NAME to a concrete encoder.
///
/// Accepts encoding names (`o200k_base`, `cl100k_base`) and common model-family
/// aliases. An unrecognized name returns [`BudgetError::UnknownTargetTokenizer`]
/// — it is never silently defaulted, so a misconfigured target fails the gate.
pub fn target_encoder(name: &str) -> Result<CoreBPE, BudgetError> {
    match canonical_target(name) {
        Some("o200k_base") => o200k(),
        Some("cl100k_base") => cl100k(),
        _ => Err(BudgetError::UnknownTargetTokenizer {
            name: name.to_string(),
            supported: "o200k_base, cl100k_base".to_string(),
        }),
    }
}

/// Map a target-model name or alias to its canonical encoding name.
fn canonical_target(name: &str) -> Option<&'static str> {
    let n = name.trim().to_ascii_lowercase();
    match n.as_str() {
        "o200k_base" | "gpt-4o" | "gpt-4o-mini" | "gpt-4.1" | "o1" | "o3" => Some("o200k_base"),
        "cl100k_base" | "gpt-4" | "gpt-4-turbo" | "gpt-3.5-turbo" => Some("cl100k_base"),
        _ => None,
    }
}

fn o200k() -> Result<CoreBPE, BudgetError> {
    tiktoken_rs::o200k_base().map_err(|e| BudgetError::UnknownTargetTokenizer {
        name: "o200k_base".to_string(),
        supported: format!("encoder load failed: {e}"),
    })
}

fn cl100k() -> Result<CoreBPE, BudgetError> {
    tiktoken_rs::cl100k_base().map_err(|e| BudgetError::UnknownTargetTokenizer {
        name: "cl100k_base".to_string(),
        supported: format!("encoder load failed: {e}"),
    })
}

/// Count tokens in `text` under `encoder`, including special tokens.
pub fn count_tokens(encoder: &CoreBPE, text: &str) -> usize {
    encoder.encode_with_special_tokens(text).len()
}
