/// The inline marker that suppresses an oversized-context failure for the file
/// it appears in.
pub const SUPPRESS_MARKER: &str = "# aoa-allow: oversized-context";

/// Find an inline `# aoa-allow: oversized-context <reason>` marker in a file's
/// text and return the captured reason.
///
/// Returns `Some(reason)` when the marker is present (the reason is the rest of
/// the line, trimmed; empty if none was written), and `None` when absent.
pub fn find_suppression(text: &str) -> Option<String> {
    for line in text.lines() {
        if let Some(idx) = line.find(SUPPRESS_MARKER) {
            let reason = line[idx + SUPPRESS_MARKER.len()..].trim();
            return Some(reason.to_string());
        }
    }
    None
}
