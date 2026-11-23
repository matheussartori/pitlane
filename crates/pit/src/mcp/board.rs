//! `pit mcp`: the board's tools (cards, questions, decisions) for any Claude Code session in a repository.

use super::*;

pub fn serve(dir: PathBuf) -> Result<()> {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock();
    let mut out = std::io::stdout().lock();
    while let Some(line) = read_message(&mut lines)? {
        let reply = match serde_json::from_str::<Value>(&line) {
            // A batch (an array) is not something these servers take: said, not dropped silently.
            Ok(msg) if msg.is_array() => Some(error(Value::Null, -32600, "batches are not supported; send one message per line")),
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
    msg.get("method")?; // a response from the client: nothing to answer
    let result = match method {
        "initialize" => initialize(&params, "pitlane", Some(INSTRUCTIONS)),
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

pub(super) fn question_schema() -> Value {
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

pub(super) fn tools() -> Value {
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
            "description": "Change a card's title, spec or tags, link it to a lane (a branch name; empty string unlinks), or move it: to the backlog (ideas, out of any batch) or to the queue (Run all takes it). Running, review and done are Pitlane's to set.",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "integer" },
                "title": { "type": "string" },
                "body": { "type": "string" },
                "state": { "type": "string", "enum": ["backlog", "queued"] },
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

pub(super) fn open_repo(dir: &Path) -> Result<Repo> {
    #[cfg(test)]
    if let Some(home) = TEST_HOME.with(|h| h.borrow().clone()) {
        return Repo::open(dir, &home, &home.join(".pitlane"));
    }
    Repo::open_default(dir)
}

pub(super) fn call(name: &str, args: &Value, dir: &Path) -> Result<Value> {
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
    // An agent working a card (the daemon marks it with PITLANE_RUN) adds cards and notes, but
    // the board's decisions stay the person's and the daemon's: it does not answer questions,
    // move cards, relink lanes or delete cards.
    if std::env::var_os(RUN_ENV).is_some() {
        let moves = name == "update_card" && (args.get("state").is_some() || args.get("lane").is_some());
        if moves || matches!(name, "answer" | "delete_card") {
            bail!("{name} is not available to an agent working a card: answers come from the person, and Pitlane moves the card when the run ends");
        }
    }
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
            let id = id()?;
            // A move follows the Board's rules; a running card is stopped from Pitlane, not here.
            let moved = match str_arg("state") {
                None => CardPatch::default(),
                Some(s) => {
                    let to = match s.as_str() {
                        "inbox" | "ready" => MoveTo::Backlog,
                        other => MoveTo::parse(other)?,
                    };
                    let card = cards::get(&repo, id)?;
                    if matches!(card.state, CardState::Running | CardState::Verifying) {
                        bail!("card #{id} is running; stop it or move it in Pitlane");
                    }
                    cards::move_rule(&card, to, false, None)?.patch
                }
            };
            let patch = CardPatch {
                title: str_arg("title"),
                body: str_arg("body"),
                lane: str_arg("lane").map(|l| Some(l).filter(|l| !l.is_empty())),
                tags: tags(),
                ..moved
            };
            card_json(&cards::update(&repo, id, patch)?)
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
            let card = pit_core::runs::recheck_after_answer(&repo, done.card);
            json!({ "decision": done.decision, "card": card_json(&card) })
        }
        "add_note" => {
            let text = str_arg("text").context("text is required")?;
            card_json(&cards::note(&repo, id()?, "claude", &text)?)
        }
        _ => bail!("unknown tool {name}"),
    })
}

pub(super) fn card_json(c: &Card) -> Value {
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

        let moved = rpc(&dir, 5, "tools/call", json!({ "name": "update_card", "arguments": { "id": 1, "state": "queued", "lane": "feat/invite-limits" } }));
        assert_eq!(moved["result"]["structuredContent"]["state"], "queued");
        let set = rpc(&dir, 51, "tools/call", json!({ "name": "update_card", "arguments": { "id": 1, "state": "running" } }));
        assert_eq!(set["result"]["isError"], true, "running is Pitlane's to set");

        let bad = rpc(&dir, 6, "tools/call", json!({ "name": "update_card", "arguments": { "id": 1, "state": "flying" } }));
        assert_eq!(bad["result"]["isError"], true);

        let listed = rpc(&dir, 7, "tools/call", json!({ "name": "list_cards", "arguments": {} }));
        assert_eq!(listed["result"]["structuredContent"]["cards"][0]["lane"], "feat/invite-limits");

        let unknown = rpc(&dir, 8, "resources/list", json!({}));
        assert_eq!(unknown["error"]["code"], -32601);
    }
}
