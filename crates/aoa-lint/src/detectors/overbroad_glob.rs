use crate::category::SmellCategory;
use crate::detectors::LintedFile;
use crate::finding::Finding;

/// Flag a directive that scopes itself with a bare recursive glob (`**` or
/// `**/*`), which matches the entire tree. Catalog: over-broad scope.
pub fn detect(file: &LintedFile) -> Vec<Finding> {
    let mut findings = Vec::new();
    for line in file.text.lines() {
        for token in line.split_whitespace() {
            if is_overbroad(token) {
                findings.push(Finding {
                    file: file.path.clone(),
                    message: format!("over-broad glob scope: '{token}' matches the entire tree"),
                    category: SmellCategory::OverBroadGlob,
                });
            }
        }
    }
    findings
}

/// A glob token is over-broad when, after stripping any backtick/quote fences,
/// it is exactly `**` or `**/*`.
fn is_overbroad(token: &str) -> bool {
    let stripped = token.trim_matches(|c| c == '`' || c == '"' || c == '\'');
    matches!(stripped, "**" | "**/*")
}
