//! The analysis Run all does before any card starts: a read-only Claude Code session per card
//! reads the spec and the repository and returns what it would need to know (questions, each
//! with options and one recommended) and what it found (notes for the run that works the card).

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Repo;
use crate::cards::{self, Card, NewQuestion};

pub const RULES: &str = "You are Pitlane's analyst. You do not change anything: you read one card's spec and the \
repository, and prepare the card so another session can work it without stopping. This is the only time the person is asked: \
once they press Run they walk away and the runs must not stop for anything, so ask now everything that would \
otherwise stop a run (where scope ends, whether related parts are in or out, which behaviour, which of several places \
to change). Never ask how the work gets done or lands (worktrees, branches, commits, pushing, merging, tests): \
Pitlane and the run decide that. Ask only what the code and the \
spec cannot answer and what would change the work (scope, behaviour, a choice between real alternatives); never \
ask what you can find out by reading. Each question gets two to four options and the one you recommend. Notes say \
what you found that the working session needs: where the code is (paths and symbols), how it fits together, \
risks, and how to verify. Keep each note short and specific. If the card is clear and small, return no questions. \
You also say how this card sits among the other cards of the same batch, because they run in parallel on branches \
of their own: `after` lists the cards whose work this one needs in place first (it builds on them, or would \
conflict with them badly enough that it must start from their result); `areas` names the shared parts of the code \
it changes that other cards may also change (an engine, a shared schema, a core module, a build config), as short \
lowercase names; cards with an area in common never run at the same time. Leave both empty when the card is \
independent. Never list a card in `after` without a concrete reason in a note.";

/// What the analyst returns.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "questions": { "type": "array", "items": { "type": "object", "properties": {
                "text": { "type": "string" },
                "options": { "type": "array", "items": { "type": "string" } },
                "recommended": { "type": "integer", "description": "0-based index into options." }
            }, "required": ["text", "options"] } },
            "notes": { "type": "array", "items": { "type": "string" }, "description": "Findings in markdown, one per item." },
            "after": { "type": "array", "items": { "type": "integer" }, "description": "Ids of cards in this batch that must land first." },
            "areas": { "type": "array", "items": { "type": "string" }, "description": "Shared parts of the code this card changes, short lowercase names." }
        },
        "required": ["questions", "notes"]
    })
}

#[derive(Debug, Default, Deserialize)]
pub struct Analysis {
    #[serde(default)]
    pub questions: Vec<NewQuestion>,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub after: Vec<u32>,
    #[serde(default)]
    pub areas: Vec<String>,
}

/// The card as the analyst sees it: spec, decisions so far, notes and the project's decisions.
/// `batch` is every card of this Run all (this one included), so the analyst can order them.
pub fn prompt(repo: &Repo, card: &Card, batch: &[Card]) -> String {
    let mut p = crate::runs::briefing(repo, card);
    if let Some(i) = p.rfind("\nWhen you finish, end with the handoff.") {
        p.truncate(i);
    }
    let others: Vec<&Card> = batch.iter().filter(|c| c.id != card.id).collect();
    if !others.is_empty() {
        p.push_str("\n\n## The other cards of this batch\n");
        for c in others {
            let first: String = c.body.split("\n\n").next().unwrap_or("").chars().take(300).collect();
            p.push_str(&format!("- #{} {}: {}\n", c.id, c.title, first.replace('\n', " ").trim()));
        }
    }
    p.push_str("\n\nAnalyse this card for the session that will work it. Return questions, notes, after and areas.\n");
    p
}

/// Reads `claude -p --output-format json`: the structured output and the cost.
pub fn read(stdout: &str) -> Result<(Analysis, Option<f64>)> {
    let v: Value = stdout
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str::<Value>(l).ok().filter(|v| v.get("type").and_then(Value::as_str) == Some("result")))
        .or_else(|| serde_json::from_str(stdout).ok())
        .context("the analysis gave no result")?;
    if v.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
        anyhow::bail!("the analysis stopped with an error: {}", v.get("result").map(|r| r.to_string()).unwrap_or_default());
    }
    let a: Analysis = serde_json::from_value(v.get("structured_output").cloned().context("the analysis returned nothing")?)
        .context("the analysis does not match the schema")?;
    Ok((a, v.get("total_cost_usd").and_then(Value::as_f64)))
}

/// Writes the analysis on the card: its notes, its questions, and the mark that it was analysed.
pub fn record(repo: &Repo, id: u32, a: Analysis) -> Result<Card> {
    for n in a.notes.iter().filter(|n| !n.trim().is_empty()) {
        cards::note(repo, id, "claude", n)?;
    }
    let qs: Vec<NewQuestion> = a.questions.into_iter().filter(|q| !q.text.trim().is_empty()).collect();
    if !qs.is_empty() {
        cards::ask(repo, id, qs)?;
    }
    cards::mark_analyzed(repo, id, a.after, a.areas)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_analysis() {
        let out = r#"{"type":"result","is_error":false,"total_cost_usd":0.02,
            "structured_output":{"questions":[{"text":"Which limit?","options":["256 KB","1 MB"],"recommended":0}],"notes":["Uploads go through src/upload.ts."]}}"#;
        let (a, cost) = read(&out.replace('\n', " ")).unwrap();
        assert_eq!(a.questions.len(), 1);
        assert_eq!(a.notes[0], "Uploads go through src/upload.ts.");
        assert_eq!(cost, Some(0.02));
    }
}
