use std::str::EscapeDebug;

use anyhow::Result;
use serde::Serialize;

/// Escape an untrusted string before it reaches a terminal (human register).
///
/// Trace- and transcript-derived fields — task ids, repo ids, error text, file
/// paths — are attacker- or external-tool-controlled and can carry ANSI escape
/// sequences or control bytes that hijack the reader's terminal. Routing every
/// such field through this helper before `write!`/`println!` neutralises the
/// injection. It returns [`EscapeDebug`] (a zero-allocation `Display` +
/// `Iterator<Item = char>` adapter), so it drops into a format string, a
/// `.to_string()`, or a `.map(...)` in place of a bare `.escape_debug()`.
///
/// The point is structural: a named call documents the security intent and
/// gives a single place to change the escaping policy, where a bare
/// `.escape_debug()` reads as debug formatting and is easy to forget on a new
/// untrusted field (arch-review Finding #5).
pub fn escape_terminal(s: &str) -> EscapeDebug<'_> {
    s.escape_debug()
}

/// Print a serializable value as pretty JSON to stdout (the agent register).
pub fn print_json<T: Serialize>(value: &T) -> Result<()> {
    let rendered = serde_json::to_string_pretty(value)?;
    println!("{rendered}");
    Ok(())
}

/// Print human-facing text to stdout (the human register). Kept distinct from
/// [`print_json`] so every audit/eval command exposes both registers (R17).
pub fn print_human(text: &str) {
    print!("{text}");
    if !text.ends_with('\n') {
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_terminal_neutralises_ansi_and_control_bytes() {
        // A crafted task id carrying a raw ESC (0x1b) + CSI clear-screen must not
        // survive to the terminal verbatim.
        let hostile = "task\u{1b}[2Jid\nnext\ttab";
        let escaped = escape_terminal(hostile).to_string();
        assert!(!escaped.contains('\u{1b}'), "raw ESC leaked: {escaped:?}");
        assert!(!escaped.contains('\n'), "raw newline leaked: {escaped:?}");
        assert!(!escaped.contains('\t'), "raw tab leaked: {escaped:?}");
        assert!(escaped.contains("\\u{1b}") || escaped.contains("\\x1b"));
        assert!(escaped.contains("\\n") && escaped.contains("\\t"));
    }

    #[test]
    fn escape_terminal_leaves_ordinary_text_readable() {
        assert_eq!(escape_terminal("repo/pkg-1.2").to_string(), "repo/pkg-1.2");
    }
}
