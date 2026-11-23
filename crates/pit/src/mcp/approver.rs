//! `pit mcp --approve <run>`: the server a card's run gets, to ask the person for approvals and calls, and to note on its card.

use super::*;

/// `pit mcp --approve <run>`: the server Claude Code asks, through `--permission-prompt-tool`,
/// whenever it wants a tool no rule allows. Each approval waits on the daemon for a person's
/// answer on a thread of its own, so parallel subagents' asks (and pings) never queue behind it.
pub fn serve_approver(run: String) -> Result<()> {
    let out = std::sync::Arc::new(std::sync::Mutex::new(std::io::stdout()));
    let send = |out: &std::sync::Mutex<std::io::Stdout>, v: Value| {
        let mut o = out.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = writeln!(o, "{v}").and_then(|()| o.flush());
    };
    let waits = Waits::default();
    let stdin = std::io::stdin();
    let mut lines = stdin.lock();
    while let Some(line) = read_message(&mut lines)? {
        let msg = match serde_json::from_str::<Value>(&line) {
            Ok(m) => m,
            Err(e) => {
                send(&out, error(Value::Null, -32700, &format!("parse error: {e}")));
                continue;
            }
        };
        if msg.is_array() {
            send(&out, error(Value::Null, -32600, "batches are not supported; send one message per line"));
            continue;
        }
        if msg.get("method").and_then(Value::as_str) == Some("notifications/cancelled") {
            if let Some(req) = msg.pointer("/params/requestId") {
                waits.cancel(req);
            }
            continue;
        }
        let Some(id) = msg.get("id").cloned() else { continue }; // a notification
        if msg.get("method").is_none() {
            continue; // a response from the client: nothing to answer
        }
        let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let tool = params.get("name").and_then(Value::as_str).unwrap_or_default();
        let result = match (method, tool) {
            ("initialize", _) => initialize(&params, "pitlane_run", None),
            ("ping", _) => json!({}),
            ("tools/list", _) => json!({ "tools": [{
                "name": "approve",
                "description": "Asks the person running this Pitlane card whether a tool may run.",
                "inputSchema": { "type": "object", "properties": {
                    "tool_name": { "type": "string" }, "input": { "type": "object" }, "tool_use_id": { "type": "string" } },
                    "required": ["tool_name", "input"] }
            }, {
                "name": "ask",
                "description": ASK_TOOL,
                "inputSchema": { "type": "object", "properties": {
                    "text": { "type": "string", "description": "The question, one call." },
                    "options": { "type": "array", "items": { "type": "string" }, "description": "Two to four options." },
                    "recommended": { "type": "integer", "description": "0-based index of the option you recommend." },
                    "context": { "type": "string", "description": "One to three sentences so the person can answer without opening the card: what part of the work this is, what you found, what each option changes." },
                    "costly": { "type": "boolean", "description": "True only when the choice would be costly to undo: deleting or migrating data, a public API or schema change, spending money, a different product behaviour." } },
                    "required": ["text", "options", "recommended", "context"] }
            }, {
                "name": "note",
                "description": "Adds a note to the card you are working: context the next session needs (where the code is, what you found, a choice you made and why, what is left). Short markdown; one note per finding.",
                "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }
            }] }),
            ("tools/call", "note") => {
                let text = params.pointer("/arguments/text").and_then(Value::as_str).unwrap_or_default();
                match note_on_run(&run, text) {
                    Ok(()) => json!({ "content": [{ "type": "text", "text": "Noted on the card." }] }),
                    Err(e) => json!({ "content": [{ "type": "text", "text": format!("{e:#}") }], "isError": true }),
                }
            }
            ("tools/call", "ask") => {
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                let (out, run, waits) = (out.clone(), run.clone(), waits.clone());
                waits.open(&id);
                // It may wait for a person: on a thread of its own, like an approval.
                std::thread::spawn(move || {
                    let result = match consult_daemon(&run, &args, &waits, &id) {
                        Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
                        Err(e) => json!({ "content": [{ "type": "text", "text": format!("Pitlane could not ask anyone ({e:#}): go with your recommendation and record it with the note tool.") }] }),
                    };
                    // A cancelled request gets no answer.
                    if waits.close(&id) {
                        send(&out, json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                    }
                });
                continue;
            }
            ("tools/call", "approve") => {
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                let (out, run, waits) = (out.clone(), run.clone(), waits.clone());
                waits.open(&id);
                std::thread::spawn(move || {
                    let answer = ask_daemon(&run, &args, &waits, &id).unwrap_or_else(|e| {
                        json!({ "behavior": "deny", "message": format!("Pitlane could not ask anyone: {e:#}") })
                    });
                    let result = json!({ "content": [{ "type": "text", "text": answer.to_string() }] });
                    if waits.close(&id) {
                        send(&out, json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                    }
                });
                continue;
            }
            ("tools/call", _) => json!({ "content": [{ "type": "text", "text": format!("unknown tool: {tool}") }], "isError": true }),
            _ => {
                send(&out, error(id, -32601, &format!("method not found: {method}")));
                continue;
            }
        };
        send(&out, json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }
    Ok(())
}

/// A note from a run lands on the run's own card.
pub(super) fn note_on_run(run: &str, text: &str) -> Result<()> {
    let (home, pitlane) = pit_core::default_homes();
    let meta: pit_core::runs::RunMeta =
        serde_json::from_slice(&std::fs::read(pit_core::runs::run_dir(&pitlane, run)?.join("meta.json")).context("no such run")?)?;
    let repo = Repo::open(&meta.repo, &home, &pitlane)?;
    cards::note(&repo, meta.card, "claude", text)?;
    Ok(())
}

/// What the run's `ask` tool tells the agent.
pub(super) const ASK_TOOL: &str = "Asks the person a call that is theirs to make and that the spec, the decisions and the code \
do not settle: behaviour they will see, where scope ends, a choice between real alternatives. Never how to do the work, \
and never what you can find out by reading. It waits for the answer while they are here; when they are away or do not \
answer in time it tells you to go with your recommendation and records it for them. Ask one call at a time and rarely: \
each question can hold the run up for minutes.";

/// The approver's requests that wait on the daemon, by JSON-RPC id, so Claude Code can cancel
/// one: its connection is hung up and the daemon takes it off your list.
#[derive(Clone, Default)]
pub(super) struct Waits(std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Wait>>>);

#[derive(Default)]
pub(super) struct Wait {
    conn: Option<std::os::unix::net::UnixStream>,
    cancelled: bool,
}

impl Waits {
    fn map(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, Wait>> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Before the thread starts, so a cancel that comes at once is not missed.
    fn open(&self, id: &Value) {
        self.map().insert(id.to_string(), Wait::default());
    }

    /// The connection the request waits on; false when it was cancelled before it got one.
    fn attach(&self, id: &Value, c: &pit_core::runs::Client) -> bool {
        let mut m = self.map();
        let Some(w) = m.get_mut(&id.to_string()) else { return false };
        if w.cancelled {
            return false;
        }
        w.conn = c.hangup_handle().ok();
        true
    }

    fn cancel(&self, id: &Value) {
        if let Some(w) = self.map().get_mut(&id.to_string()) {
            w.cancelled = true;
            if let Some(c) = &w.conn {
                let _ = c.shutdown(std::net::Shutdown::Both);
            }
        }
    }

    /// Done with the request: true when it should be answered (it was not cancelled).
    fn close(&self, id: &Value) -> bool {
        self.map().remove(&id.to_string()).is_some_and(|w| !w.cancelled)
    }
}

pub(super) fn consult_daemon(run: &str, args: &Value, waits: &Waits, id: &Value) -> Result<String> {
    let (_, home) = pit_core::default_homes();
    // Never start a daemon from here: it would inherit the agent's environment and outlive the app.
    let mut c = pit_core::runs::Client::connect(&home, None)?;
    if !waits.attach(id, &c) {
        bail!("cancelled");
    }
    let question: NewQuestion = serde_json::from_value(args.clone()).context("the question does not read")?;
    let costly = args.get("costly").and_then(Value::as_bool).unwrap_or(false);
    c.consult(run, question, costly)
}

pub(super) fn ask_daemon(run: &str, args: &Value, waits: &Waits, id: &Value) -> Result<Value> {
    let (_, home) = pit_core::default_homes();
    // Never start a daemon from here: it would inherit the agent's environment and outlive the app.
    let mut c = pit_core::runs::Client::connect(&home, None)?;
    if !waits.attach(id, &c) {
        bail!("cancelled");
    }
    let tool = args.get("tool_name").and_then(Value::as_str).unwrap_or_default().to_owned();
    let input = args.get("input").cloned().unwrap_or(Value::Null);
    let use_id = args.get("tool_use_id").and_then(Value::as_str).map(str::to_owned);
    c.ask(run, tool, input, use_id)
}
