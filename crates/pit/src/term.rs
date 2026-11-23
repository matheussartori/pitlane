//! Terminal output: colours, prompts and making text safe to print.
//!
//! `paint` decides on colour by whether stdout is a terminal, also for text sent to stderr.

use std::io::{BufRead, IsTerminal, Write};

use anyhow::{Result, bail};

/// 256-colour approximations of design/tokens.css: accent (hue 58) means "your move" only.
pub const ACCENT: &str = "38;5;208";
pub const DONE: &str = "38;5;114";
pub const FAIL: &str = "38;5;203";
pub const RUN: &str = "38;5;75";
pub const MUTED: &str = "38;5;244";
pub const BOLD: &str = "1";

pub fn term_width() -> Option<usize> {
    std::io::stdout().is_terminal().then(|| terminal_size::terminal_size().map(|(w, _)| usize::from(w.0)))?
}

pub fn color_on() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

/// Text that came from an agent or a card, safe to print: control characters (escape
/// sequences that move the cursor, rewrite the line or set the clipboard) are dropped, so what
/// you read in `pit approvals` is what you approve. Line breaks and tabs stay.
pub fn plain(text: &str) -> String {
    text.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).collect()
}

/// A commit's short form; never panics on a short or empty id.
pub fn short(sha: &str) -> &str {
    sha.get(..8).unwrap_or(sha)
}

/// The Claude Code executable: `$PITLANE_CLAUDE_BIN`, else `claude` on the PATH.
pub fn claude_bin() -> String {
    std::env::var("PITLANE_CLAUDE_BIN").unwrap_or_else(|_| "claude".into())
}

pub fn paint(style: &str, text: &str) -> String {
    if color_on() { format!("\x1b[{style}m{text}\x1b[0m") } else { text.to_owned() }
}

pub fn confirm(question: &str) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("not a terminal; pass --yes to proceed without asking");
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes" | "s" | "sim"))
}

pub fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-~".contains(c)) {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}
