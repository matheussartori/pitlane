//! `pit mcp`: a Model Context Protocol server over stdio, so an agent in the terminal (Claude
//! Code, Codex) creates, asks about and moves cards. JSON-RPC 2.0, one message per line.
//! Install: `claude mcp add pitlane -- pit mcp`.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use pit_core::Repo;
use pit_core::cards::{self, Card, CardOut, CardPatch, CardState, NewCard, NewQuestion};
use serde_json::{Value, json};

const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Pitlane keeps this repository's board (cards and decisions) outside the repository, so read and change cards only through these tools. \
Use add_card for each piece of work. Before work starts, ask every question you have in one ask call, \
each with options and the one you recommend; the user answers them in Pitlane's Inbox and each answer \
becomes a recorded project decision that get_card and every run include. Read get_card before working on a card.";

/// `pit mcp --approve <run>`: the server Claude Code asks, through `--permission-prompt-tool`,
/// whenever it wants a tool no rule allows. Each call waits on the daemon for a person's answer.
pub fn serve_approver(run: String) -> Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = msg.get("id").cloned() else { continue };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
                let version = VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(VERSIONS[0]);
                json!({ "protocolVersion": version, "capabilities": { "tools": {} },
                        "serverInfo": { "name": "pitlane_run", "version": env!("CARGO_PKG_VERSION") } })
            }
            "ping" => json!({}),
            "tools/list" => json!({ "tools": [{
                "name": "approve",
                "description": "Asks the person running this Pitlane card whether a tool may run.",
                "inputSchema": { "type": "object", "properties": {
                    "tool_name": { "type": "string" }, "input": { "type": "object" }, "tool_use_id": { "type": "string" } },
                    "required": ["tool_name", "input"] }
            }, {
                "name": "note",
                "description": "Adds a note to the card you are working: context the next session needs (where the code is, what you found, a choice you made and why, what is left). Short markdown; one note per finding.",
                "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }
            }] }),
            "tools/call" if params.get("name").and_then(Value::as_str) == Some("note") => {
                let text = params.pointer("/arguments/text").and_then(Value::as_str).unwrap_or_default();
                match note_on_run(&run, text) {
                    Ok(()) => json!({ "content": [{ "type": "text", "text": "Noted on the card." }] }),
                    Err(e) => json!({ "content": [{ "type": "text", "text": format!("{e:#}") }], "isError": true }),
                }
            }
            "tools/call" => {
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                let answer = ask_daemon(&run, &args).unwrap_or_else(|e| {
                    json!({ "behavior": "deny", "message": format!("Pitlane could not ask anyone: {e:#}") })
                });
                json!({ "content": [{ "type": "text", "text": answer.to_string() }] })
            }
            _ => {
                writeln!(out, "{}", error(id, -32601, &format!("method not found: {method}")))?;
                out.flush()?;
                continue;
            }
        };
        writeln!(out, "{}", json!({ "jsonrpc": "2.0", "id": id, "result": result }))?;
        out.flush()?;
    }
    Ok(())
}

/// A note from a run lands on the run's own card.
fn note_on_run(run: &str, text: &str) -> Result<()> {
    let (home, pitlane) = pit_core::default_homes();
    let meta: pit_core::runs::RunMeta =
        serde_json::from_slice(&std::fs::read(pit_core::runs::runs_dir(&pitlane).join(run).join("meta.json")).context("no such run")?)?;
    let repo = Repo::open(&meta.repo, &home, &pitlane)?;
    cards::note(&repo, meta.card, "claude", text)?;
    Ok(())
}

fn ask_daemon(run: &str, args: &Value) -> Result<Value> {
    let (_, home) = pit_core::default_homes();
    let mut c = pit_core::runs::Client::connect(&home, std::env::current_exe().ok().as_deref())?;
    let v = c.call(json!({
        "op": "ask",
        "run": run,
        "tool": args.get("tool_name").and_then(Value::as_str).unwrap_or_default(),
        "input": args.get("input").cloned().unwrap_or(Value::Null),
        "tool_use_id": args.get("tool_use_id"),
    }))?;
    v.get("answer").cloned().context("the daemon gave no answer")
}

pub fn serve(dir: PathBuf) -> Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&msg, &dir),
            Err(e) => Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(r) = reply {
            writeln!(out, "{r}")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// One message in, at most one message out (notifications get none).
pub fn handle(msg: &Value, dir: &Path) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let id = id?; // a notification: nothing to answer
    let result = match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
            let version = VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(VERSIONS[0]);
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "pitlane", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            match call(name, &args, dir) {
                Ok(v) => json!({ "content": [{ "type": "text", "text": v.to_string() }], "structuredContent": v, "isError": false }),
                Err(e) => json!({ "content": [{ "type": "text", "text": format!("{e:#}") }], "isError": true }),
            }
        }
        _ => return Some(error(id, -32601, &format!("method not found: {method}"))),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn question_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "text": { "type": "string", "description": "The question, in one sentence." },
            "options": { "type": "array", "items": { "type": "string" }, "description": "Two to four answers to pick from." },
            "recommended": { "type": "integer", "description": "0-based index of the option you recommend." },
            "context": { "type": "string", "description": "One to three sentences so the person can answer without opening the card: what part of the work this is about, what you found, and what each option changes." }
        },
        "required": ["text"]
    })
}

fn tools() -> Value {
    let states: Vec<&str> = CardState::ALL.iter().map(|s| s.as_str()).collect();
    json!([
        {
            "name": "add_card",
            "description": "Create a card for one piece of work. The body is the spec in markdown (goal, acceptance criteria, out of scope, how to verify). Questions may come along, all at once.",
            "inputSchema": { "type": "object", "properties": {
                "title": { "type": "string" },
                "body": { "type": "string" },
                "tags": { "type": "array", "items": { "type": "string" } },
                "questions": { "type": "array", "items": question_schema() }
            }, "required": ["title"] }
        },
        {
            "name": "list_cards",
            "description": "List the cards: id, title, state, lane and how many questions are still open.",
            "inputSchema": { "type": "object", "properties": {
                "state": { "type": "string", "enum": states },
                "open_questions_only": { "type": "boolean" }
            } }
        },
        {
            "name": "get_card",
            "description": "One card in full: spec, questions with their answers and decisions, lane.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] }
        },
        {
            "name": "update_card",
            "description": "Change a card's title, spec, state or tags, or link it to a lane (a branch name; empty string unlinks).",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "integer" },
                "title": { "type": "string" },
                "body": { "type": "string" },
                "state": { "type": "string", "enum": states },
                "lane": { "type": "string" },
                "tags": { "type": "array", "items": { "type": "string" } }
            }, "required": ["id"] }
        },
        {
            "name": "delete_card",
            "description": "Delete a card (duplicate, obsolete, or the person asked). It can be restored from the app.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] }
        },
        {
            "name": "ask",
            "description": "Ask every question you have about a card in one call, each with options and the one you recommend. The user answers them in Pitlane's Inbox; each answer becomes a decision.",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "integer" },
                "questions": { "type": "array", "items": question_schema(), "minItems": 1 }
            }, "required": ["id", "questions"] }
        },
        {
            "name": "answer",
            "description": "Record the user's answer to a card's question (an option number from 1, an option's text, or free text). Only when the user gave that answer.",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "integer" },
                "question_id": { "type": "integer" },
                "answer": { "type": "string" }
            }, "required": ["id", "question_id", "answer"] }
        },
        {
            "name": "add_note",
            "description": "Add a note to a card: context the next one to work it needs (where the code is, what you found, what was tried, what is left). Notes show on the card and go into every run's briefing.",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "integer" },
                "text": { "type": "string", "description": "Markdown." }
            }, "required": ["id", "text"] }
        },
        {
            "name": "brief",
            "description": "Where to look in this repository for a card or a task: the most relevant files with their symbols (line and signature) and the tests nearby, from Pitlane's tree-sitter index. Give a card id, or a text describing the task.",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "integer", "description": "A card number." },
                "text": { "type": "string", "description": "The task in a sentence or two, when there is no card." }
            } }
        }
    ])
}

#[cfg(test)]
thread_local! {
    /// Tests point the board and index at a temporary home instead of the user's `~/.pitlane`.
    static TEST_HOME: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn open_repo(dir: &Path) -> Result<Repo> {
    #[cfg(test)]
    if let Some(home) = TEST_HOME.with(|h| h.borrow().clone()) {
        return Repo::open(dir, &home, &home.join(".pitlane"));
    }
    Repo::open_default(dir)
}

fn call(name: &str, args: &Value, dir: &Path) -> Result<Value> {
    let repo = open_repo(dir).context("pit mcp must run inside a git repository")?;
    let id = || -> Result<u32> {
        let n = args.get("id").and_then(Value::as_u64).context("id is required")?;
        u32::try_from(n).context("id is out of range")
    };
    let str_arg = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_owned);
    let tags = || -> Option<Vec<String>> {
        args.get("tags")?.as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_owned)).collect())
    };
    let questions = || -> Result<Vec<NewQuestion>> {
        match args.get("questions") {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(v) => serde_json::from_value(v.clone()).context("questions: each needs text, optional options and recommended"),
        }
    };
    Ok(match name {
        "brief" => {
            let card = match (args.get("id").and_then(Value::as_u64), str_arg("text")) {
                (Some(_), _) => cards::get(&repo, id()?)?,
                (None, Some(text)) => cards::Card { title: text, ..Default::default() },
                (None, None) => anyhow::bail!("give id or text"),
            };
            let b = pit_core::brief::compile(&repo, &pit_core::default_homes().1, &card, pit_core::brief::DEFAULT_BUDGET)?;
            let text = if b.text.is_empty() { "Nothing in the index matches; search the repository directly.".to_owned() } else { b.text };
            json!({ "brief": text, "files": b.files.iter().map(|f| &f.path).collect::<Vec<_>>(), "tests": b.tests })
        }
        "add_card" => {
            let title = str_arg("title").context("title is required")?;
            card_json(&cards::add(
                &repo,
                NewCard { title, body: str_arg("body").unwrap_or_default(), tags: tags().unwrap_or_default(), questions: questions()? },
            )?)
        }
        "list_cards" => {
            let state = str_arg("state").map(|s| CardState::parse(&s)).transpose()?;
            let open_only = args.get("open_questions_only").and_then(Value::as_bool).unwrap_or(false);
            let (all, problems) = cards::list(&repo)?;
            let list: Vec<Value> = all
                .iter()
                .filter(|c| state.is_none_or(|s| c.state == s))
                .filter(|c| !open_only || c.open_questions() > 0)
                .map(|c| json!({ "id": c.id, "title": c.title, "state": c.state, "lane": c.lane, "open_questions": c.open_questions() }))
                .collect();
            json!({ "cards": list, "problems": problems })
        }
        "get_card" => card_json(&cards::get(&repo, id()?)?),
        "delete_card" => {
            let c = cards::delete(&repo, id()?)?;
            json!({ "deleted": c.id, "title": c.title })
        }
        "update_card" => {
            let patch = CardPatch {
                title: str_arg("title"),
                body: str_arg("body"),
                state: str_arg("state").map(|s| CardState::parse(&s)).transpose()?,
                lane: str_arg("lane").map(|l| Some(l).filter(|l| !l.is_empty())),
                tags: tags(),
                ..Default::default()
            };
            card_json(&cards::update(&repo, id()?, patch)?)
        }
        "ask" => {
            let qs = questions()?;
            if qs.is_empty() {
                bail!("questions is required");
            }
            card_json(&cards::ask(&repo, id()?, qs)?)
        }
        "answer" => {
            let q = args.get("question_id").and_then(Value::as_u64).context("question_id is required")?;
            let a = str_arg("answer").context("answer is required")?;
            let done = cards::answer(&repo, id()?, u32::try_from(q)?, &a)?;
            json!({ "decision": done.decision, "card": card_json(&done.card) })
        }
        "add_note" => {
            let text = str_arg("text").context("text is required")?;
            card_json(&cards::note(&repo, id()?, "claude", &text)?)
        }
        _ => bail!("unknown tool {name}"),
    })
}

fn card_json(c: &Card) -> Value {
    serde_json::to_value(CardOut::from(c.clone())).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rpc(dir: &Path, id: u64, method: &str, params: Value) -> Value {
        handle(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }), dir).unwrap()
    }

    #[test]
    fn a_session() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("work");
        assert!(std::process::Command::new("git").args(["init", "-q"]).arg(&dir).status().unwrap().success());
        TEST_HOME.with(|h| *h.borrow_mut() = Some(tmp.path().to_path_buf()));

        let init = rpc(&dir, 1, "initialize", json!({ "protocolVersion": "2025-03-26", "capabilities": {} }));
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "pitlane");
        assert!(handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }), &dir).is_none());

        let list = rpc(&dir, 2, "tools/list", json!({}));
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["add_card", "list_cards", "get_card", "update_card", "delete_card", "ask", "answer", "add_note", "brief"]);

        let added = rpc(&dir, 3, "tools/call", json!({ "name": "add_card", "arguments": {
            "title": "Rate limit invites",
            "body": "Goal: stop invite spam.",
            "questions": [{ "text": "Per IP or per account?", "options": ["Per IP", "Per account"], "recommended": 1 }]
        }}));
        assert_eq!(added["result"]["isError"], false);
        let card = &added["result"]["structuredContent"];
        assert_eq!(card["id"], 1);
        assert_eq!(card["open_questions"], 1);

        let answered = rpc(&dir, 4, "tools/call", json!({ "name": "answer", "arguments": { "id": 1, "question_id": 1, "answer": "2" } }));
        assert_eq!(answered["result"]["structuredContent"]["decision"], "D-01");

        let moved = rpc(&dir, 5, "tools/call", json!({ "name": "update_card", "arguments": { "id": 1, "state": "running", "lane": "feat/invite-limits" } }));
        assert_eq!(moved["result"]["structuredContent"]["state"], "running");

        let bad = rpc(&dir, 6, "tools/call", json!({ "name": "update_card", "arguments": { "id": 1, "state": "flying" } }));
        assert_eq!(bad["result"]["isError"], true);

        let listed = rpc(&dir, 7, "tools/call", json!({ "name": "list_cards", "arguments": {} }));
        assert_eq!(listed["result"]["structuredContent"]["cards"][0]["lane"], "feat/invite-limits");

        let unknown = rpc(&dir, 8, "resources/list", json!({}));
        assert_eq!(unknown["error"]["code"], -32601);
    }
}
