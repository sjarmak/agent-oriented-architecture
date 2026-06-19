use anyhow::Result;
use serde::Serialize;

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
