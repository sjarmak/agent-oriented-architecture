use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::category::SmellCategory;

/// A single context-file smell finding.
///
/// Carries the originating file path, a human-readable message, and a
/// machine-readable [`SmellCategory`] mapped to the 2606.15828 catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub file: PathBuf,
    pub message: String,
    pub category: SmellCategory,
}
