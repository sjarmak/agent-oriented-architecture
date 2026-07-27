use std::io::Write as _;
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
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    write_human(&mut out, text).expect("failed to write human output");
}

/// Print human-facing text to stderr through the same terminal-safe boundary.
pub fn eprint_human(text: &str) {
    let stderr = std::io::stderr();
    let mut out = stderr.lock();
    write_terminal(&mut out, text, false).expect("failed to write human error output");
    writeln!(out).expect("failed to terminate human error output");
}

/// Print an anyhow error chain to stderr without allowing terminal controls
/// carried by any source error to survive the boundary.
pub fn eprint_error(error: &anyhow::Error) {
    eprint_human(&format!("error: {error:#}"));
}

fn write_human(mut out: impl std::io::Write, text: &str) -> std::io::Result<()> {
    write_terminal(&mut out, text, true)?;
    if !text.ends_with('\n') {
        writeln!(out)?;
    }
    Ok(())
}

fn write_terminal(
    mut out: impl std::io::Write,
    text: &str,
    preserve_newlines: bool,
) -> std::io::Result<()> {
    for character in text.chars() {
        if (preserve_newlines && character == '\n') || !character.is_control() {
            write!(out, "{character}")?;
        } else {
            for escaped in character.escape_debug() {
                write!(out, "{escaped}")?;
            }
        }
    }
    Ok(())
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

    #[test]
    fn human_boundary_preserves_layout_but_neutralises_terminal_controls() {
        let mut output = Vec::new();
        write_human(&mut output, "first\nhostile\u{1b}[2J\ttext\n").unwrap();
        let rendered = String::from_utf8(output).unwrap();

        assert_eq!(rendered, "first\nhostile\\u{1b}[2J\\ttext\n");
        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains('\t'));
    }
}
