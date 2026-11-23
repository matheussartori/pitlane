//! How Claude Code is launched: its arguments, settings and MCP servers.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use pit_core::Repo;
use pit_core::accounts::Choice;
use pit_core::runs::{self, RunEvent};
use serde_json::Value;
use tokio::process::Command;

use super::card::Prepared;
use super::side::Side;
use super::{Supervisor, home_dir};

/// How long Claude Code waits for an MCP tool to answer, in milliseconds: a day, so an approval
/// waits for its person.
const MCP_TOOL_TIMEOUT_MS: &str = "86400000";

/// Turns a one-shot call (a card's description, a report's digest) may take.
const ONE_SHOT_MAX_TURNS: &str = "2";
/// How long a one-shot call may take.
const ONE_SHOT_LIMIT: Duration = Duration::from_secs(120);

impl Supervisor {
    /// A one-shot Claude Code call with no tools (a card's description, a report's digest): Sonnet,
    /// JSON out, in `cwd` on the account's directory. `what` names it in errors.
    pub(super) async fn one_shot(
        &self,
        cwd: &Path,
        choice: Option<&Choice>,
        system: &str,
        schema: &str,
        prompt: String,
        what: &str,
    ) -> Result<Output> {
        let mut cmd = Command::new(&self.cfg.claude_bin);
        cmd.args(["-p", "--output-format", "json", "--model", pit_core::describe::MODEL])
            .args(["--setting-sources", "project,local", "--tools", "", "--max-turns", ONE_SHOT_MAX_TURNS])
            .args(["--append-system-prompt", system])
            .args(["--json-schema", schema])
            .arg(prompt)
            .current_dir(cwd)
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CODE_CHILD_SESSION")
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(dir) = choice.and_then(|c| c.config_dir.as_ref()) {
            cmd.env("CLAUDE_CONFIG_DIR", dir);
        }
        tokio::time::timeout(ONE_SHOT_LIMIT, cmd.output())
            .await
            .with_context(|| format!("{what} took over two minutes"))?
            .with_context(|| format!("could not start {}", self.cfg.claude_bin))
    }

    /// A card's run: Claude Code in its worktree, resumed or new, with the repository's tools,
    /// the route's account and model, and Pitlane's approvals. Notes the effort and the route on
    /// the run.
    pub(super) fn run_command(
        &self,
        id: &str,
        repo: &Path,
        resuming: bool,
        prep: &Prepared,
        choice: &Choice,
    ) -> Result<Command> {
        let session = self.get(id)?.session_id;
        let mut cmd = stream_json(&self.cfg.claude_bin);
        // Claude in Chrome on from the start: a run that needs the browser finds it there,
        // and one that never touches it pays nothing.
        cmd.arg("--chrome");
        if resuming {
            cmd.args(["--resume", &session]);
        } else {
            cmd.args(["--session-id", &session]);
        }
        // Run all asks nothing: its runs never stop for a permission (the person chose so).
        let autopilot = self.get(id).is_ok_and(|m| m.autopilot);
        let mode = if autopilot {
            "bypassPermissions".to_owned()
        } else {
            mode_for(repo)
        };
        cmd.args([
            "--setting-sources",
            "project,local",
            "--permission-mode",
            &mode,
        ])
        .arg("--allowedTools")
        .arg(
            prep.tools
                .iter()
                .cloned()
                .chain(prep.browser.then(|| BROWSER_TOOLS.to_owned()))
                .collect::<Vec<_>>()
                .join(","),
        )
        // Nobody is watching a run: it decides, notes the choice, or ends blocked.
        .args(["--disallowedTools", "AskUserQuestion"])
        .args(prep.related.iter().flat_map(|l| {
            [
                "--add-dir".to_owned(),
                l.worktree.to_string_lossy().into_owned(),
            ]
        }))
        .args(["--max-turns", &prep.max_turns.to_string()])
        .arg("--append-system-prompt")
        .arg(format!(
            "{}{}{}",
            runs::RULES,
            if prep.browser { runs::BROWSER_RULES } else { "" },
            pit_core::language::rule(&pit_core::language::agent_language(&self.cfg.home))
        ))
        .arg("--json-schema")
        .arg(runs::handoff_schema().to_string());
        // The model: the route's, the repository's, or else the one you use in Claude Code.
        let picked = choice.model.clone().or(prep.model.clone());
        let (your_model, your_effort) =
            your_claude(choice.config_dir.as_deref(), picked.as_deref());
        if let Some(m) = picked.as_ref().or(your_model.as_ref()) {
            cmd.args(["--model", m]);
        }
        if let Some(e) = &your_effort {
            cmd.args(["--effort", e]);
            let e = e.clone();
            self.update(id, move |m| m.effort = Some(e));
        }
        // The account: its own Claude Code directory, where you signed in. Pitlane only points at it.
        if let Some(dir) = &choice.config_dir {
            fs::create_dir_all(dir)?;
            cmd.env("CLAUDE_CONFIG_DIR", dir);
        }
        // An API key account: Claude Code runs your helper command; the key never passes here.
        if let Some(helper) = &choice.api_key_helper {
            cmd.arg("--settings")
                .arg(serde_json::json!({ "apiKeyHelper": helper }).to_string());
        }
        self.emit(
            id,
            RunEvent::Note {
                text: format!("Route: {}", self.get(id)?.reason.unwrap_or_default()),
            },
        );
        if let Some(pit) = &self.cfg.pit_bin {
            // Anything no rule allows is asked to a person, through Pitlane, and waits for them.
            let mut mcp = approval_servers(pit, id);
            if prep.browser {
                mcp["mcpServers"]["browser"] = browser_server();
            }
            ask_through_pitlane(&mut cmd, &mcp);
        }
        cmd.current_dir(&prep.worktree)
            // Tells the board tools (`pit mcp`) they serve an agent working a card.
            .env("PITLANE_RUN", id)
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CODE_CHILD_SESSION")
            // Performance may pause a session for a while: a request that went quiet then is
            // retried by Claude Code, not dropped.
            .env("CLAUDE_STREAM_IDLE_TIMEOUT_MS", crate::perf::STREAM_IDLE_MS)
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(false);
        Ok(cmd)
    }

    /// A side session: Claude Code with the session's arguments, the model and effort you use
    /// where it sets none, and Pitlane's approvals. Notes the effort on the session.
    pub(super) fn side_command(&self, id: &str, side: &Side) -> Result<Command> {
        let mut cmd = stream_json(&self.cfg.claude_bin);
        cmd.args(["--session-id", &self.get(id)?.session_id])
            .args(with_language(&side.args, &self.cfg.home));
        // Whatever the session does not set comes from your Claude Code: model and effort.
        let set = side
            .args
            .iter()
            .position(|a| a == "--model")
            .and_then(|i| side.args.get(i + 1))
            .cloned();
        let (your_model, your_effort) = your_claude(side.config_dir.as_deref(), set.as_deref());
        if !side.args.iter().any(|a| a == "--model")
            && let Some(m) = your_model
        {
            cmd.args(["--model", &m]);
        }
        let effort = match side.args.iter().position(|a| a == "--effort") {
            Some(i) => side.args.get(i + 1).cloned(),
            None => {
                if let Some(e) = &your_effort {
                    cmd.args(["--effort", e]);
                }
                your_effort
            }
        };
        if let Some(e) = effort {
            self.update(id, move |m| m.effort = Some(e));
        }
        // Like a run: what no rule allows (a read outside the repository, say) is asked to a
        // person in Pitlane, instead of being denied with nobody to ask.
        if let Some(pit) = &self.cfg.pit_bin {
            let mut mcp = approval_servers(pit, id);
            if side.board {
                mcp["mcpServers"]["pitlane"] =
                    serde_json::json!({ "command": pit, "args": ["mcp"] });
            }
            ask_through_pitlane(&mut cmd, &mcp);
        } else {
            cmd.args(["--permission-prompts", "none"]);
        }
        if let Some(dir) = &side.config_dir {
            cmd.env("CLAUDE_CONFIG_DIR", dir);
        }
        cmd.current_dir(&side.cwd)
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CODE_CHILD_SESSION")
            // Performance may pause a session for a while: a request that went quiet then is
            // retried by Claude Code, not dropped.
            .env("CLAUDE_STREAM_IDLE_TIMEOUT_MS", crate::perf::STREAM_IDLE_MS)
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        Ok(cmd)
    }
}

/// Claude Code taking and printing stream-json: how every session's command starts.
fn stream_json(bin: &str) -> Command {
    let mut cmd = Command::new(bin);
    cmd.args([
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    cmd
}

/// Pitlane's approval server for session `id` (`pit mcp --approve <id>`), as `--mcp-config`
/// takes it; more servers can be added beside it.
fn approval_servers(pit: &Path, id: &str) -> Value {
    serde_json::json!({ "mcpServers": { "pitlane_run": { "command": pit, "args": ["mcp", "--approve", id] } } })
}

/// Gives the session the MCP servers in `mcp` and sends its permission prompts to Pitlane's
/// approval server, which waits for a person.
fn ask_through_pitlane(cmd: &mut Command, mcp: &Value) {
    cmd.args(["--mcp-config", &mcp.to_string()])
        .args([
            "--permission-prompts",
            "host",
            "--permission-prompt-tool",
            "mcp__pitlane_run__approve",
        ])
        .env("MCP_TOOL_TIMEOUT", MCP_TOOL_TIMEOUT_MS);
}

/// Claude Code's effort levels (`--effort`, `/effort`).
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// The model and effort you chose in Claude Code (`/model`), from the account's settings.json:
/// Pitlane starts sessions with project settings only, so it passes these on itself. `model` is
/// the one the session will use when Pitlane already picked one; the effort is yours for that
/// model (`modelSettings.<id>.effortLevel`, matched by alias: `opus` finds `claude-opus-5-5`).
/// Only these values are read; nothing about the login.
fn your_claude(config_dir: Option<&Path>, model: Option<&str>) -> (Option<String>, Option<String>) {
    let dir = config_dir
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from))
        .unwrap_or_else(|| home_dir().join(".claude"));
    let Ok(v) = fs::read_to_string(dir.join("settings.json"))
        .map_err(anyhow::Error::from)
        .and_then(|t| Ok(serde_json::from_str::<Value>(&t)?))
    else {
        return (None, None);
    };
    let yours = v
        .get("model")
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty())
        .map(str::to_owned);
    let Some(m) = model.map(str::to_owned).or_else(|| yours.clone()) else {
        return (None, None);
    };
    let effort = v
        .get("modelSettings")
        .and_then(Value::as_object)
        .and_then(|all| {
            let level = |k: &String| all.get(k)?.get("effortLevel")?.as_str().map(str::to_owned);
            // The exact id, else the newest id that contains the alias.
            let exact = all.keys().find(|k| **k == m).cloned();
            let by_alias = || {
                let mut ks: Vec<&String> = all.keys().filter(|k| k.contains(m.as_str())).collect();
                ks.sort();
                ks.last().map(|k| (*k).clone())
            };
            exact.or_else(by_alias).and_then(|k| level(&k))
        });
    (yours, effort)
}

/// A session's arguments with the language you chose for agents added to its system prompt.
fn with_language(args: &[String], home: &Path) -> Vec<String> {
    let rule = pit_core::language::rule(&pit_core::language::agent_language(home));
    let mut out = args.to_vec();
    match out.iter().position(|a| a == "--append-system-prompt") {
        Some(i) if i + 1 < out.len() => out[i + 1].push_str(&rule),
        _ => out.extend([
            "--append-system-prompt".into(),
            rule.trim_start().to_owned(),
        ]),
    }
    out
}

/// The repository's permission mode for Claude Code (`[run] permission_mode`, `auto` by default).
pub(super) fn mode_for(repo: &Path) -> String {
    Repo::open(repo, &home_dir(), &pit_core::default_homes().1)
        .map(|r| r.flow.run.permission_mode)
        .unwrap_or_else(|_| "auto".into())
}

/// A session that reads and never writes (analysis, judge): the repository's mode, without the
/// editing tools, and without asking the person in the middle (its questions go in its answer).
pub(super) fn read_only_args(mode: &str) -> Vec<String> {
    [
        "--setting-sources",
        "project,local",
        "--permission-mode",
        mode,
        "--disallowedTools",
        "Edit,Write,NotebookEdit,AskUserQuestion",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Playwright's MCP server, pinned, headless and with a throwaway profile: the agent's browser
/// when the repository turns it on. Its tools are `mcp__browser__*`.
const BROWSER_MCP: &str = "@playwright/mcp@0.0.84";
const BROWSER_TOOLS: &str = "mcp__browser";

fn browser_server() -> Value {
    serde_json::json!({ "command": "npx", "args": ["-y", BROWSER_MCP, "--headless", "--isolated"] })
}

/// Claude Code's own directory when no account profile points elsewhere.
pub(super) fn claude_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".claude"))
}

/// Claude Code keeps each session as `projects/<folder>/<session>.jsonl` in its directory.
pub(super) fn session_saved(config: &Path, session: &str) -> bool {
    let file = format!("{session}.jsonl");
    fs::read_dir(config.join("projects"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|d| d.path().join(&file).is_file())
}

#[cfg(test)]
mod your_claude_tests {
    use super::*;

    #[test]
    fn reads_your_model_and_its_effort() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("settings.json"),
            r#"{"model":"opus","modelSettings":{"claude-opus-5-5":{"effortLevel":"medium"},"claude-fable-5-2":{"effortLevel":"xhigh"}}}"#,
        )
        .unwrap();
        assert_eq!(
            your_claude(Some(d.path()), None),
            (Some("opus".into()), Some("medium".into()))
        );
        // A model Pitlane picked gets your effort for that model.
        assert_eq!(
            your_claude(Some(d.path()), Some("fable")).1,
            Some("xhigh".into())
        );
        assert_eq!(your_claude(Some(d.path()), Some("sonnet")).1, None);
        assert_eq!(
            your_claude(Some(&d.path().join("none")), None),
            (None, None)
        );
    }
}
