//! Runs: an agent working a card in the card's worktree. The daemon (`pit daemon`, crate `pitd`)
//! owns the processes; this module holds what every side shares: the run record, the events as
//! the UI shows them, the briefing, and a small blocking client for the daemon's socket.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Repo;
use crate::cards::{self, Card};

pub fn socket_path(pitlane_home: &Path) -> PathBuf {
    pitlane_home.join("pitd.sock")
}

pub fn runs_dir(pitlane_home: &Path) -> PathBuf {
    pitlane_home.join("runs")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunState {
    Starting,
    Running,
    /// The agent finished; proofs and the judge are checking its work.
    Verifying,
    /// Finished with a handoff.
    Done,
    /// The agent or the process failed, or a limit was hit.
    Failed,
    /// Stopped on request.
    Stopped,
    /// The daemon went away while the run was open.
    Interrupted,
}

impl RunState {
    pub fn is_live(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Verifying)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMeta {
    pub id: String,
    /// Main worktree of the repository.
    pub repo: PathBuf,
    pub card: u32,
    pub card_title: String,
    pub branch: String,
    pub worktree: PathBuf,
    pub agent: String,
    #[serde(default)]
    pub model: Option<String>,
    pub session_id: String,
    #[serde(default)]
    pub pid: Option<u32>,
    pub state: RunState,
    pub started_at: u64,
    #[serde(default)]
    pub ended_at: Option<u64>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub turns: Option<u32>,
    #[serde(default)]
    pub tokens: Tokens,
    /// The agent's final report, validated by the handoff schema.
    #[serde(default)]
    pub handoff: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
    /// Tools the agent asked for and was not allowed.
    #[serde(default)]
    pub denied: Vec<String>,
    #[serde(default)]
    pub resumed_from: Option<String>,
    /// Tools the agent is waiting on a person to allow, while the run lives.
    #[serde(default)]
    pub waiting: Vec<Approval>,
    /// The account the route chose (`default` is your normal login).
    #[serde(default = "default_account")]
    pub account: String,
    /// Why the route chose this account and model.
    #[serde(default)]
    pub reason: Option<String>,
    /// The run this one continues after it hit a usage limit.
    #[serde(default)]
    pub handoff_from: Option<String>,
    /// Accounts that hit their limit on this card's attempt; the route will not take them again.
    #[serde(default)]
    pub exhausted: Vec<String>,
    /// Set when the run hit a usage limit and the next option needs your yes (an API key): the
    /// option, e.g. `opus on anthropic-api`.
    #[serde(default)]
    pub next: Option<String>,
    /// When the limit it hit resets (unix seconds), if the agent said.
    #[serde(default)]
    pub resets_at: Option<u64>,
    /// Review rounds: 0 for the first run of a card, +1 each time a person asks for changes.
    #[serde(default)]
    pub round: u32,
    /// The judge's verdict, once the checks ran.
    #[serde(default)]
    pub verdict: Option<crate::verify::Verdict>,
    /// Every proof passed (None: no proofs ran).
    #[serde(default)]
    pub proofs_ok: Option<bool>,
    /// The lane's commit when the agent started: the diff of this round starts here.
    #[serde(default)]
    pub base_sha: Option<String>,
    /// The map of where to look that the first message carried, if any.
    #[serde(default)]
    pub brief: Option<crate::brief::BriefStats>,
    /// What this session is: a card's run (the default), or a side session the app shows beside
    /// it (Run all's analysis, the judge).
    #[serde(default)]
    pub kind: SessionKind,
    /// The run a side session belongs to (the judge's run).
    #[serde(default)]
    pub parent: Option<String>,
}

/// Every Claude Code session Pitlane starts, so people can watch and talk to all of them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionKind {
    #[default]
    Run,
    Analysis,
    Judge,
    /// Run all's closing step: what the person asked for when everything is done.
    Finish,
    /// The Board's assistant: turns what the person says into cards.
    Assist,
}

fn default_account() -> String {
    crate::accounts::DEFAULT_ACCOUNT.into()
}

/// A tool the agent wants to use that no rule allows: it waits for a person.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Approval {
    pub id: String,
    pub tool: String,
    /// The command, the file or the URL, in one line.
    pub detail: String,
    pub input: Value,
    /// The rule "the same command" would add: `Bash(npm test:*)`, or the tool's name.
    pub rule: String,
    pub asked_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Decision {
    /// This call only.
    Once,
    /// The same command until the run ends.
    Run,
    /// The same command in this repository from now on (`[run] allow` in flow.toml).
    Always,
    Deny,
}

impl Decision {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "once" => Self::Once,
            "run" => Self::Run,
            "always" => Self::Always,
            "deny" => Self::Deny,
            _ => bail!("unknown decision {s:?}; one of once, run, always, deny"),
        })
    }
}

/// The rule that allows "the same command": a Bash command by its first one or two words,
/// any other tool by its name.
pub fn rule_for(tool: &str, input: &Value) -> String {
    if tool != "Bash" {
        return tool.to_owned();
    }
    let cmd = input.get("command").and_then(Value::as_str).unwrap_or_default().trim();
    // Skip grouping marks and VAR=value prefixes: the rule names the program that runs.
    let words: Vec<&str> = cmd
        .split_whitespace()
        .skip_while(|w| w.chars().all(|c| "{(!".contains(c)) || (w.contains('=') && !w.starts_with('-')))
        .take(2)
        .collect();
    let prefix = match words.as_slice() {
        [a, b] if b.chars().all(|c| c.is_ascii_alphanumeric() || "-_:.".contains(c)) && !b.starts_with('-') => format!("{a} {b}"),
        [a, ..] => (*a).to_owned(),
        [] => String::new(),
    };
    format!("Bash({prefix}:*)")
}

/// Whether `rule` (in Claude Code's syntax) allows this call.
pub fn rule_allows(rule: &str, tool: &str, input: &Value) -> bool {
    if let Some(inner) = rule.strip_prefix("Bash(").and_then(|r| r.strip_suffix(')')) {
        if tool != "Bash" {
            return false;
        }
        let cmd = input.get("command").and_then(Value::as_str).unwrap_or_default().trim();
        return match inner.strip_suffix(":*") {
            Some(prefix) => cmd == prefix || cmd.strip_prefix(prefix).is_some_and(|rest| rest.starts_with(' ')),
            None => cmd == inner,
        };
    }
    rule == tool
}

/// One thing that happened in a run, as people read it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RunEvent {
    Started { model: String, session: String },
    /// `parent` is set inside a subagent: the id of the Task/Agent tool call that started it.
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    Tool {
        id: String,
        name: String,
        detail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    ToolDone {
        id: String,
        ok: bool,
        detail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    /// A message from the person, sent while the agent works.
    Said { text: String },
    Quota { window: String, used: f64, resets_at: u64 },
    /// The account refused more work until `resets_at`.
    Limited { window: String, resets_at: u64 },
    /// One of the repository's checks ran on the lane.
    Proof { command: String, ok: bool, duration_ms: u64 },
    /// The judge, a separate session, gave its verdict.
    Judged { verdict: crate::verify::Verdict, summary: String, findings: usize },
    Finished {
        ok: bool,
        cost_usd: f64,
        duration_ms: u64,
        turns: u32,
        tokens: Tokens,
        handoff: Option<Value>,
        denied: Vec<String>,
        /// It ended because of a usage limit.
        #[serde(default)]
        limited: bool,
    },
    Note { text: String },
    /// The agent waits for a person to allow a tool.
    Approval { approval: Approval },
    Decided { id: String, decision: Decision, message: Option<String> },
    Exited { code: Option<i32>, state: RunState },
}

/// Reads one line of Claude Code's stream-json output into what the UI shows.
/// Unknown or noisy events (thinking, hooks) give nothing.
pub fn read_claude_line(v: &Value) -> Vec<RunEvent> {
    let ty = v.get("type").and_then(Value::as_str).unwrap_or_default();
    let sub = v.get("subtype").and_then(Value::as_str).unwrap_or_default();
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    // Messages of a subagent carry the tool call that started it.
    let parent = v.get("parent_tool_use_id").and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_owned);
    match (ty, sub) {
        ("system", "init") => vec![RunEvent::Started { model: s(v, "model"), session: s(v, "session_id") }],
        ("assistant", _) => v
            .pointer("/message/content")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let t = s(b, "text");
                            (!t.trim().is_empty()).then_some(RunEvent::Text { text: t, parent: parent.clone() })
                        }
                        Some("tool_use") if s(b, "name") != "StructuredOutput" => Some(RunEvent::Tool {
                            id: s(b, "id"),
                            name: s(b, "name"),
                            detail: tool_detail(&s(b, "name"), b.get("input").unwrap_or(&Value::Null)),
                            parent: parent.clone(),
                        }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        ("user", _) => v
            .pointer("/message/content")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
                    .filter(|b| !s(b, "content").starts_with("Structured output provided"))
                    .map(|b| RunEvent::ToolDone {
                        id: s(b, "tool_use_id"),
                        ok: !b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                        detail: first_line(&content_text(b.get("content"))),
                        parent: parent.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        ("rate_limit_event", _) => {
            let info = v.get("rate_limit_info").cloned().unwrap_or(Value::Null);
            let mut out: Vec<RunEvent> = info
                .get("unifiedWindows")
                .and_then(Value::as_object)
                .map(|w| {
                    w.iter()
                        .map(|(name, i)| RunEvent::Quota {
                            window: name.clone(),
                            used: i.get("utilization").and_then(Value::as_f64).unwrap_or(0.0),
                            resets_at: i.get("resetsAt").and_then(Value::as_u64).unwrap_or(0),
                        })
                        .collect()
                })
                .unwrap_or_default();
            if info.get("status").and_then(Value::as_str) == Some("rejected") {
                out.push(RunEvent::Limited {
                    window: s(&info, "rateLimitType"),
                    resets_at: info.get("resetsAt").and_then(Value::as_u64).unwrap_or(0),
                });
            }
            out
        }
        ("result", _) => {
            let u = v.get("usage").cloned().unwrap_or(Value::Null);
            let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
            vec![RunEvent::Finished {
                ok: !v.get("is_error").and_then(Value::as_bool).unwrap_or(false) && sub == "success",
                cost_usd: v.get("total_cost_usd").and_then(Value::as_f64).unwrap_or(0.0),
                duration_ms: v.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
                turns: v.get("num_turns").and_then(Value::as_u64).unwrap_or(0) as u32,
                tokens: Tokens {
                    input: n("input_tokens"),
                    output: n("output_tokens"),
                    cache_read: n("cache_read_input_tokens"),
                    cache_write: n("cache_creation_input_tokens"),
                },
                handoff: v.get("structured_output").cloned().filter(|h| !h.is_null()),
                denied: v
                    .get("permission_denials")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|d| {
                                let name = s(d, "tool_name");
                                let input = d.get("tool_input").unwrap_or(&Value::Null);
                                format!("{name}: {}", tool_detail(&name, input))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                limited: v.get("is_error").and_then(Value::as_bool).unwrap_or(false) && {
                    let text = v.get("result").map(|r| r.to_string().to_lowercase()).unwrap_or_default();
                    ["usage limit", "rate limit", "limit reached", "hit your limit", "limit will reset"].iter().any(|k| text.contains(k))
                },
            }]
        }
        _ => Vec::new(),
    }
}

/// The one line that says what a tool call does: the command, the file, the pattern.
fn tool_detail(name: &str, input: &Value) -> String {
    let s = |k: &str| input.get(k).and_then(Value::as_str).map(str::to_owned);
    let d = match name {
        "Bash" => s("command"),
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => s("file_path"),
        "Glob" | "Grep" => s("pattern"),
        "WebFetch" => s("url"),
        "WebSearch" => s("query"),
        "Task" | "Agent" => s("description"),
        _ => None,
    };
    first_line(&d.unwrap_or_default())
}

fn content_text(c: Option<&Value>) -> String {
    match c {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn first_line(s: &str) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim();
    if line.chars().count() > 400 { format!("{}…", line.chars().take(399).collect::<String>()) } else { line.to_owned() }
}

/// The JSON Schema every run ends with. It feeds the review, and its questions go to the Inbox.
pub fn handoff_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "status": { "type": "string", "enum": ["done", "partial", "blocked"],
                "description": "done: the card is finished and committed. partial: progress committed, more is needed. blocked: cannot continue without an answer." },
            "summary": { "type": "string", "description": "What changed and why, in two or three sentences." },
            "files_changed": { "type": "array", "items": { "type": "string" } },
            "commits": { "type": "array", "items": { "type": "string" }, "description": "Short shas with their subject." },
            "tests": { "type": "object", "properties": {
                "command": { "type": "string" }, "passed": { "type": "boolean" }, "notes": { "type": "string" } } },
            "next_steps": { "type": "array", "items": { "type": "string" } },
            "questions": { "type": "array", "description": "Only when blocked: every question at once, each with options and the one you recommend.",
                "items": { "type": "object", "properties": {
                    "text": { "type": "string" },
                    "options": { "type": "array", "items": { "type": "string" } },
                    "recommended": { "type": "integer" } }, "required": ["text"] } }
        },
        "required": ["status", "summary"]
    })
}

/// Fixed rules for every run: the same bytes each time, so the prompt cache holds.
pub const RULES: &str = "You are running inside Pitlane, on one card, in a git worktree made for it. \
Work only inside the current directory. Commit your work on the current branch with clear messages; \
never push, never switch branches, never merge. Read the card's spec and decisions before you start. \
If a decision is missing and you cannot proceed safely, stop and report status \"blocked\" with all your \
questions at once, each with options and the one you recommend. As you learn something the next session on this \
card would need (where the code is, a choice you made and why, what is left), add it with the note tool, short and \
one finding per note. End every run with the structured handoff.";

/// The first message of a run: the card, its decisions and the repository's decisions file.
pub fn briefing(repo: &Repo, card: &Card) -> String {
    let mut b = format!("# Card #{}: {}\n\n", card.id, card.title);
    if card.body.trim().is_empty() {
        b.push_str("(No spec beyond the title. Keep the change small and say what you assumed.)\n");
    } else {
        b.push_str(card.body.trim());
        b.push('\n');
    }
    let answered: Vec<_> = card.questions.iter().filter(|q| q.answer.is_some()).collect();
    if !answered.is_empty() {
        b.push_str("\n## Decisions on this card\n\n");
        for q in answered {
            b.push_str(&format!(
                "- {}: {}{}\n",
                q.text,
                q.answer.as_deref().unwrap_or_default(),
                q.decision.as_deref().map(|d| format!(" ({d})")).unwrap_or_default()
            ));
        }
    }
    if !card.notes.is_empty() {
        b.push_str("\n## Notes on this card\n\n");
        for n in &card.notes {
            b.push_str(&format!("- {} ({}): {}\n", n.by, n.at, n.text.trim().replace('\n', "\n  ")));
        }
    }
    if let Ok(d) = std::fs::read_to_string(cards::decisions_path(repo))
        && !d.trim().is_empty()
    {
        b.push_str("\n## Project decisions\n\n");
        b.push_str(d.trim());
        b.push('\n');
    }
    b.push_str("\nWhen you finish, end with the handoff.\n");
    b
}

/// A user message in Claude Code's stream-json input format.
pub fn claude_user_line(text: &str) -> String {
    json!({ "type": "user", "message": { "role": "user", "content": text } }).to_string()
}

// ---------------------------------------------------------------- client

/// A blocking connection to the daemon. Starts it when nobody answers.
pub struct Client {
    stream: BufReader<UnixStream>,
}

impl Client {
    /// `pit_bin` is the `pit` executable to start `pit daemon` with, if it is not running.
    pub fn connect(pitlane_home: &Path, pit_bin: Option<&Path>) -> Result<Self> {
        let sock = socket_path(pitlane_home);
        if let Ok(s) = UnixStream::connect(&sock) {
            return Ok(Self { stream: BufReader::new(s) });
        }
        let Some(bin) = pit_bin else { bail!("the Pitlane daemon is not running (start it with: pit daemon)") };
        spawn_daemon(bin, pitlane_home)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(s) = UnixStream::connect(&sock) {
                return Ok(Self { stream: BufReader::new(s) });
            }
            if Instant::now() > deadline {
                bail!("started the daemon but it does not answer on {}; see {}", sock.display(), pitlane_home.join("pitd.log").display());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn send(&mut self, req: &Value) -> Result<()> {
        let s = self.stream.get_mut();
        writeln!(s, "{req}")?;
        s.flush()?;
        Ok(())
    }

    fn read(&mut self) -> Result<Option<Value>> {
        let mut line = String::new();
        if self.stream.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&line).context("the daemon sent something that is not JSON")?))
    }

    /// One request, one answer. A daemon-side error becomes an `Err`.
    pub fn call(&mut self, req: Value) -> Result<Value> {
        self.send(&req)?;
        let v = self.read()?.context("the daemon closed the connection")?;
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            bail!("{e}");
        }
        Ok(v)
    }

    /// Holds the daemon to this process: blocks while both live. When this connection closes,
    /// the daemon stops every session it started and ends. Returns when the daemon goes away.
    pub fn hold(mut self) -> Result<()> {
        self.call(json!({ "op": "hold" }))?;
        while self.read().ok().flatten().is_some() {}
        Ok(())
    }

    /// Replays a run's events from `from` and then follows it live, until it ends or `each`
    /// returns false. Returns the run as it stands at the end.
    pub fn follow(&mut self, run: &str, from: usize, mut each: impl FnMut(usize, RunEvent) -> bool) -> Result<Option<RunMeta>> {
        self.send(&json!({ "op": "follow", "run": run, "from": from }))?;
        while let Some(v) = self.read()? {
            if let Some(e) = v.get("error").and_then(Value::as_str) {
                bail!("{e}");
            }
            if let Some(end) = v.get("end") {
                return Ok(serde_json::from_value(end.clone()).ok());
            }
            if let (Some(i), Some(ev)) = (v.get("i").and_then(Value::as_u64), v.get("event"))
                && let Ok(ev) = serde_json::from_value::<RunEvent>(ev.clone())
                && !each(i as usize, ev)
            {
                return Ok(None);
            }
        }
        Ok(None)
    }
}

fn spawn_daemon(bin: &Path, pitlane_home: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    std::fs::create_dir_all(pitlane_home)?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(pitlane_home.join("pitd.log"))?;
    std::process::Command::new(bin)
        .arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0) // its own group: a Ctrl-C in the terminal that started it does not reach it
        .spawn()
        .with_context(|| format!("could not start {} daemon", bin.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_for_the_same_command() {
        let bash = |c: &str| json!({ "command": c });
        assert_eq!(rule_for("Bash", &bash("npm test -- drift")), "Bash(npm test:*)");
        assert_eq!(rule_for("Bash", &bash("python3 -c \"print(6*7)\"")), "Bash(python3:*)");
        assert_eq!(rule_for("Bash", &bash("cargo build --release")), "Bash(cargo build:*)");
        assert_eq!(rule_for("Bash", &bash("{ python3 --version; } > V.txt")), "Bash(python3:*)");
        assert_eq!(rule_for("Bash", &bash("CI=1 npm test")), "Bash(npm test:*)");
        assert_eq!(rule_for("WebFetch", &json!({ "url": "https://x" })), "WebFetch");
        assert!(rule_allows("Bash(npm test:*)", "Bash", &bash("npm test -- other")));
        assert!(rule_allows("Bash(npm test:*)", "Bash", &bash("npm test")));
        assert!(!rule_allows("Bash(npm test:*)", "Bash", &bash("npm testing")));
        assert!(!rule_allows("Bash(npm test:*)", "Bash", &bash("rm -rf /")));
        assert!(rule_allows("Bash(pwd)", "Bash", &bash("pwd")));
        assert!(rule_allows("WebFetch", "WebFetch", &json!({})));
        assert!(!rule_allows("WebFetch", "Bash", &bash("curl x")));
    }

    #[test]
    fn reads_the_probe_events() {
        let init = json!({"type":"system","subtype":"init","session_id":"s1","model":"claude-opus-5-5"});
        assert_eq!(read_claude_line(&init), vec![RunEvent::Started { model: "claude-opus-5-5".into(), session: "s1".into() }]);

        let a = json!({"type":"assistant","message":{"content":[
            {"type":"thinking","thinking":""},
            {"type":"text","text":"Reading the validator."},
            {"type":"tool_use","id":"t1","name":"Bash","input":{"command":"npm test -- validate\nmore"}},
            {"type":"tool_use","id":"t2","name":"StructuredOutput","input":{}}
        ]}});
        let ev = read_claude_line(&a);
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[1], RunEvent::Tool { id: "t1".into(), name: "Bash".into(), detail: "npm test -- validate".into(), parent: None });

        // Inside a subagent, every event names the Task call that opened it.
        let sub = json!({"type":"assistant","parent_tool_use_id":"task1","message":{"content":[{"type":"text","text":"Looking."}]}});
        assert_eq!(read_claude_line(&sub), vec![RunEvent::Text { text: "Looking.".into(), parent: Some("task1".into()) }]);

        let u = json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"FAIL validate.test.ts"}]}});
        assert_eq!(read_claude_line(&u), vec![RunEvent::ToolDone { id: "t1".into(), ok: false, detail: "FAIL validate.test.ts".into(), parent: None }]);

        let r = json!({"type":"result","subtype":"success","is_error":false,"num_turns":2,"duration_ms":4795,"total_cost_usd":0.19,
            "usage":{"input_tokens":2,"output_tokens":367,"cache_read_input_tokens":0,"cache_creation_input_tokens":23181},
            "structured_output":{"status":"done","summary":"ok"},"permission_denials":[{"tool_name":"Bash","tool_input":{"command":"rm -rf /"}}]});
        match &read_claude_line(&r)[0] {
            RunEvent::Finished { ok, turns, tokens, handoff, denied, .. } => {
                assert!(ok);
                assert_eq!(*turns, 2);
                assert_eq!(tokens.cache_write, 23181);
                assert_eq!(handoff.as_ref().unwrap()["status"], "done");
                assert_eq!(denied, &vec!["Bash: rm -rf /".to_owned()]);
            }
            other => panic!("{other:?}"),
        }

        let q = json!({"type":"rate_limit_event","rate_limit_info":{"unifiedWindows":{"five_hour":{"utilization":0.14,"resetsAt":1795210200}}}});
        assert_eq!(read_claude_line(&q), vec![RunEvent::Quota { window: "five_hour".into(), used: 0.14, resets_at: 1795210200 }]);
        let rejected = json!({"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour","resetsAt":99,"unifiedWindows":{"five_hour":{"utilization":1.0,"resetsAt":99}}}});
        assert_eq!(read_claude_line(&rejected)[1], RunEvent::Limited { window: "five_hour".into(), resets_at: 99 });
        let out = json!({"type":"result","subtype":"error_during_execution","is_error":true,"result":"Claude AI usage limit reached|1795210200","usage":{}});
        assert!(matches!(&read_claude_line(&out)[0], RunEvent::Finished { limited: true, .. }));
        assert!(read_claude_line(&json!({"type":"system","subtype":"hook_started"})).is_empty());
    }
}
