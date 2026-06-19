use crate::category::SmellCategory;
use crate::detectors::LintedFile;
use crate::finding::Finding;

/// Maximum number of non-blank body lines a single section may hold before it
/// is flagged as oversized.
const MAX_SECTION_BODY_LINES: usize = 40;

/// Flag any section (the body following an ATX heading, up to the next heading)
/// whose non-blank line count exceeds [`MAX_SECTION_BODY_LINES`] (catalog:
/// verbosity / bloat).
pub fn detect(file: &LintedFile) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut current: Option<(String, usize)> = None;

    let flush = |section: Option<(String, usize)>, out: &mut Vec<Finding>| {
        if let Some((title, body_lines)) = section {
            if body_lines > MAX_SECTION_BODY_LINES {
                out.push(Finding {
                    file: file.path.clone(),
                    message: format!(
                        "oversized section '{title}': {body_lines} body lines (max {MAX_SECTION_BODY_LINES})"
                    ),
                    category: SmellCategory::Verbosity,
                });
            }
        }
    };

    for line in file.text.lines() {
        if let Some(title) = heading_title(line) {
            flush(current.take(), &mut findings);
            current = Some((title, 0));
        } else if let Some((_, count)) = current.as_mut() {
            if !line.trim().is_empty() {
                *count += 1;
            }
        }
    }
    flush(current.take(), &mut findings);
    findings
}

fn heading_title(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let title = trimmed.trim_start_matches('#').trim();
    if title.is_empty() {
        return None;
    }
    Some(title.to_string())
}
