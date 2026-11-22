//! What Claude makes of a card when it is created or its spec changes: a title written from the
//! person's title and spec together, and a short summary the board shows instead of the spec.
//! Always Sonnet, with no tools: it reads only what it is given.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::cards::Card;

/// The model every description uses.
pub const MODEL: &str = "sonnet";

pub const RULES: &str = "You are Pitlane's card writer. You get a card from a coding board: the title the person \
typed and their spec. You do not read files or run anything. Return a title and a summary. The title is one short \
line (at most 70 characters) naming the change the card asks for, built from the person's title and spec together: \
keep the names and words they used (product names, screens, prefixes such as SIEGE PROTOCOL 1), no trailing period, \
no quotes. The summary is one to three short sentences (at most 220 characters) saying what the card wants done and \
why, plain text, no markdown, no lists. When the spec is empty, keep the title close to the person's and summarize \
the title. Never invent requirements the person did not write.";

pub fn schema(with_title: bool) -> Value {
    let mut required = vec!["summary"];
    if with_title {
        required.push("title");
    }
    json!({
        "type": "object",
        "properties": {
            "title": { "type": "string", "description": "One short line, at most 70 characters." },
            "summary": { "type": "string", "description": "One to three short sentences, at most 220 characters." }
        },
        "required": required
    })
}

pub fn prompt(card: &Card, with_title: bool) -> String {
    let spec = card.body.trim();
    let spec: String = spec.chars().take(12_000).collect();
    let ask = if with_title { "Return title and summary." } else { "Return the summary (keep the title as it is)." };
    format!(
        "## Title the person typed\n\n{}\n\n## Spec\n\n{}\n\n{ask}\n",
        card.title.trim(),
        if spec.is_empty() { "(empty)" } else { &spec }
    )
}

#[derive(Debug, Default, Deserialize)]
pub struct Description {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: String,
}

/// Reads `claude -p --output-format json`.
pub fn read(stdout: &str) -> Result<Description> {
    let v: Value = serde_json::from_str(stdout.trim())
        .ok()
        .or_else(|| stdout.lines().rev().find_map(|l| serde_json::from_str::<Value>(l).ok()))
        .context("the description gave no result")?;
    if v.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
        anyhow::bail!("the description stopped with an error: {}", v.get("result").map(|r| r.to_string()).unwrap_or_default());
    }
    let mut d: Description = serde_json::from_value(v.get("structured_output").cloned().context("the description returned nothing")?)
        .context("the description does not match the schema")?;
    d.title = d.title.map(|t| t.trim().trim_end_matches('.').trim_matches('"').chars().take(120).collect());
    d.summary = d.summary.trim().chars().take(400).collect();
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_description() {
        let out = r#"{"type":"result","is_error":false,"structured_output":{"title":"Câmera volta ao zoom padrão ao morrer.","summary":"Ao morrer com a sniper em zoom, a câmera de morte fica no zoom."}}"#;
        let d = read(out).unwrap();
        assert_eq!(d.title.as_deref(), Some("Câmera volta ao zoom padrão ao morrer"));
        assert!(d.summary.starts_with("Ao morrer"));
    }
}
