//! Claude Code's stream-json, both ways: its output read as run events, and the user lines written to it.

use super::*;

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
        /// What an edit puts in (Edit, MultiEdit, Write): shown as a diff, like Claude Code does.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        change: Option<Change>,
    },
    ToolDone {
        id: String,
        ok: bool,
        detail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// How many lines the result had ("Read 120 lines").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<usize>,
    },
    /// An image a tool gave back (a browser screenshot, a PNG it read), kept in the run's
    /// `shots/` folder as `file`. `tool` is the call that made it.
    Shot {
        tool: String,
        file: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    /// A message from the person, sent while the agent works.
    Said { text: String },
    /// The model or effort changed while the session lives (`/model`, `/effort`).
    Tuned {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
    },
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
        // Answers to Pitlane's own control requests (`/model`, `/effort`): the model it resolved, or why not.
        ("control_response", _) => {
            let r = v.get("response").unwrap_or(&Value::Null);
            let id = s(r, "request_id");
            if !id.starts_with("pit-") {
                return Vec::new();
            }
            if s(r, "subtype") == "error" {
                vec![RunEvent::Note { text: format!("Claude Code did not take it: {}", s(r, "error")) }]
            } else if let Some(m) = r.pointer("/response/model").and_then(Value::as_str) {
                vec![RunEvent::Tuned { model: Some(m.to_owned()), effort: None }]
            } else {
                Vec::new()
            }
        }
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
                            change: tool_change(&s(b, "name"), b.get("input").unwrap_or(&Value::Null)),
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
                    .map(|b| {
                        let text = content_text(b.get("content"));
                        RunEvent::ToolDone {
                            id: s(b, "tool_use_id"),
                            ok: !b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                            detail: first_line(&text),
                            parent: parent.clone(),
                            lines: Some(text.lines().count()).filter(|n| *n > 1),
                        }
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
pub(super) fn tool_detail(name: &str, input: &Value) -> String {
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

/// An edit's before and after, cut to what reads well in a live view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub old: String,
    pub new: String,
    /// Lines left out of `old` and `new` together.
    #[serde(default)]
    pub cut: usize,
}

pub(super) const CHANGE_LINES: usize = 40;

pub(super) fn tool_change(name: &str, input: &Value) -> Option<Change> {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    let (old, new) = match name {
        "Edit" => (s(input, "old_string"), s(input, "new_string")),
        "Write" => (String::new(), s(input, "content")),
        "MultiEdit" => {
            let edits = input.get("edits").and_then(Value::as_array)?;
            let pick = |k: &str| edits.iter().map(|e| s(e, k)).collect::<Vec<_>>().join("\n…\n");
            (pick("old_string"), pick("new_string"))
        }
        _ => return None,
    };
    if old.is_empty() && new.is_empty() {
        return None;
    }
    let cut_to = |t: &str, n: usize| -> (String, usize) {
        let lines: Vec<&str> = t.lines().collect();
        if lines.len() <= n { (t.to_owned(), 0) } else { (lines[..n].join("\n"), lines.len() - n) }
    };
    let (old, a) = cut_to(&old, CHANGE_LINES / 2);
    let (new, b) = cut_to(&new, CHANGE_LINES);
    Some(Change { old, new, cut: a + b })
}

/// An image a tool gave back: the call's id, the subagent's parent, the file extension, the bytes.
pub struct ToolImage {
    pub tool: String,
    pub parent: Option<String>,
    pub ext: &'static str,
    pub bytes: Vec<u8>,
}

/// The images inside a stream-json line's tool results (Claude Code drops them from the text).
pub fn tool_images(v: &Value) -> Vec<ToolImage> {
    use base64::Engine as _;
    if v.get("type").and_then(Value::as_str) != Some("user") {
        return Vec::new();
    }
    let parent = v.get("parent_tool_use_id").and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_owned);
    let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) else { return Vec::new() };
    let mut out = Vec::new();
    for b in blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result")) {
        let tool = b.get("tool_use_id").and_then(Value::as_str).unwrap_or_default();
        let Some(items) = b.get("content").and_then(Value::as_array) else { continue };
        for i in items.iter().filter(|i| i.get("type").and_then(Value::as_str) == Some("image")) {
            let src = i.get("source").unwrap_or(&Value::Null);
            let ext = match src.get("media_type").and_then(Value::as_str) {
                Some("image/png") => "png",
                Some("image/jpeg") => "jpg",
                Some("image/gif") => "gif",
                Some("image/webp") => "webp",
                _ => continue,
            };
            let Some(data) = src.get("data").and_then(Value::as_str) else { continue };
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data.as_bytes()) {
                out.push(ToolImage { tool: tool.to_owned(), parent: parent.clone(), ext, bytes });
            }
        }
    }
    out
}

pub(super) fn content_text(c: Option<&Value>) -> String {
    match c {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

pub(super) fn first_line(s: &str) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim();
    if line.chars().count() > 400 { format!("{}…", line.chars().take(399).collect::<String>()) } else { line.to_owned() }
}

/// A user message with images first (the card's screenshots), then the text.
pub fn claude_user_line_with(text: &str, images: &[(&str, Vec<u8>)]) -> String {
    use base64::Engine as _;
    if images.is_empty() {
        return claude_user_line(text);
    }
    let mut content: Vec<Value> = images
        .iter()
        .map(|(ty, bytes)| json!({ "type": "image", "source": { "type": "base64", "media_type": ty,
            "data": base64::engine::general_purpose::STANDARD.encode(bytes) } }))
        .collect();
    content.push(json!({ "type": "text", "text": text }));
    json!({ "type": "user", "message": { "role": "user", "content": content } }).to_string()
}

/// A user message in Claude Code's stream-json input format.
pub fn claude_user_line(text: &str) -> String {
    json!({ "type": "user", "message": { "role": "user", "content": text } }).to_string()
}

// ---------------------------------------------------------------- client
