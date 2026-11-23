//! Approval rules: what "allow for this run" remembers for a tool call, and what a rule lets through.

use super::*;

/// Programs that run whatever their arguments say (a shell, a wrapper): a rule by their name
/// alone would allow any command at all, so theirs is the exact command.
pub(super) const RUNS_ANYTHING: &[&str] = &[
    "bash", "sh", "zsh", "dash", "fish", "env", "xargs", "sudo", "doas", "eval", "exec", "nohup", "time", "timeout", "nice",
    "command", "builtin", "watch", "parallel",
    // Interpreters and package runners: `python3 -c …`, `node -e …`, `npx anything`.
    "python", "python2", "python3", "node", "deno", "bun", "bunx", "npx", "pnpx", "uvx", "ruby", "perl", "php", "lua",
    "osascript", "awk", "gawk",
];

/// Whether a simple command starts with `VAR=value`: the variable can change what the program
/// does (`NODE_OPTIONS=--require=x`, `PATH=.:$PATH`), so no prefix rule vouches for it.
pub(super) fn sets_variables(part: &str) -> bool {
    part.split_whitespace()
        .find(|w| !w.chars().all(|c| "{}()!".contains(c)))
        .is_some_and(|w| w.contains('=') && !w.starts_with('-'))
}

/// Whether a Bash line sends output to a file (`> f`, `>> ~/.zshrc`, `&> f`, `>| f`). Copying a
/// descriptor (`2>&1`) and `/dev/null` are fine. A `>` inside quotes counts too: that only asks more.
pub(super) fn writes_files(cmd: &str) -> bool {
    let b = cmd.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'>' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        if j < b.len() && b[j] == b'>' {
            j += 1;
        }
        let rest = &cmd[j..];
        let dup = rest.strip_prefix('&').is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit() || c == '-'));
        let null = rest.trim_start().strip_prefix("/dev/null").is_some_and(|r| r.is_empty() || r.starts_with(|c: char| c.is_whitespace() || ";|&)".contains(c)));
        if !dup && !null {
            return true;
        }
        i = j;
    }
    false
}

/// The words of one simple command, past grouping marks and `VAR=value` prefixes (which
/// `sets_variables` looks at on its own).
pub(super) fn program_words(part: &str) -> impl Iterator<Item = &str> {
    part.split_whitespace()
        .skip_while(|w| w.chars().all(|c| "{}()!".contains(c)) || (w.contains('=') && !w.starts_with('-')))
}

/// The simple commands of a Bash line, split on `;`, `&`, `|`, `&&`, `||` and newlines, or
/// `None` when it runs a command inside another (`$(...)`, backticks, process substitution),
/// which no prefix can vouch for. A separator inside quotes splits too: that only ever asks more.
pub(super) fn simple_commands(cmd: &str) -> Option<Vec<&str>> {
    if cmd.contains('`') || cmd.contains("$(") || cmd.contains("<(") || cmd.contains(">(") {
        return None;
    }
    Some(
        cmd.split(['\n', ';', '&', '|'])
            .map(str::trim)
            // What is left of a group or a redirect after the split is not a command of its own.
            .filter(|p| {
                program_words(p).next().is_some_and(|w| {
                    !w.trim_start_matches(|c: char| c.is_ascii_digit()).starts_with(['>', '<']) && !w.chars().all(|c| c.is_ascii_digit())
                })
            })
            .collect(),
    )
}

/// Whether a tool call asks to run outside Claude Code's sandbox.
pub fn unsandboxed(input: &Value) -> bool {
    input.get("dangerouslyDisableSandbox").and_then(Value::as_bool) == Some(true)
}

/// The rule that allows "the same command": a Bash command by its first one or two words,
/// any other tool by its name. A shell, a wrapper or an interpreter, a line of several commands,
/// one that sets variables or one that writes to a file gets the exact line, so allowing it
/// never allows more than the person saw.
pub fn rule_for(tool: &str, input: &Value) -> String {
    if tool != "Bash" {
        return tool.to_owned();
    }
    let cmd = input.get("command").and_then(Value::as_str).unwrap_or_default().trim();
    let parts = simple_commands(cmd).unwrap_or_default();
    let words: Vec<&str> = match parts.as_slice() {
        [one] if !sets_variables(one) && !writes_files(cmd) => program_words(one).take(2).collect(),
        _ => Vec::new(),
    };
    let prefix = match words.as_slice() {
        [a, ..] if RUNS_ANYTHING.contains(a) => return format!("Bash({cmd})"),
        [a, b] if b.chars().all(|c| c.is_ascii_alphanumeric() || "-_:.".contains(c)) && !b.starts_with('-') => format!("{a} {b}"),
        [a, ..] => (*a).to_owned(),
        [] => return format!("Bash({cmd})"),
    };
    format!("Bash({prefix}:*)")
}

/// Whether `rule` (in Claude Code's syntax) allows this call. A prefix rule allows a line only
/// when every command in it starts with the prefix: `npm test && curl x | sh` is not `npm test`.
pub fn rule_allows(rule: &str, tool: &str, input: &Value) -> bool {
    // A command that asks to leave the sandbox is always yours to answer.
    if unsandboxed(input) {
        return false;
    }
    if let Some(inner) = rule.strip_prefix("Bash(").and_then(|r| r.strip_suffix(')')) {
        if tool != "Bash" {
            return false;
        }
        let cmd = input.get("command").and_then(Value::as_str).unwrap_or_default().trim();
        let Some(prefix) = inner.strip_suffix(":*") else {
            return cmd == inner;
        };
        let Some(parts) = simple_commands(cmd) else { return false };
        if writes_files(cmd) || parts.iter().any(|p| sets_variables(p)) {
            return false;
        }
        return !parts.is_empty()
            && parts.iter().all(|p| {
                let p = program_words(p).collect::<Vec<_>>().join(" ");
                p == prefix || p.strip_prefix(prefix).is_some_and(|rest| rest.starts_with(' '))
            });
    }
    rule == tool
}
