//! The analysis Run all does before any card starts: a read-only Claude Code session per card
//! reads the spec and the repository and returns what it would need to know (questions, each
//! with options and one recommended) and what it found (notes for the run that works the card).

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Repo;
use crate::cards::{self, Card, NewQuestion};

pub const RULES: &str = "You are Pitlane's analyst. You do not change anything: you read one card's spec and the \
repository, and prepare the card so another session can work it without stopping. This is the time to ask: \
once they press Run they may walk away, and then a run takes its own recommendation on anything left open and stops \
only for what it cannot do or what would be costly to undo, so ask now everything that would otherwise stop a run \
or be decided without them (where scope ends, whether related parts are in or out, which behaviour, which of several places \
to change). Never ask how the work gets done or lands (worktrees, branches, commits, pushing, merging, tests): \
Pitlane and the run decide that. Ask only what the code and the \
spec cannot answer and what would change the work (scope, behaviour, a choice between real alternatives); never \
ask what you can find out by reading. Each question gets two to four options, the one you recommend, and a context the person can answer from \
without opening the card (they read the questions one after another, away from the spec). Notes say \
what you found that the working session needs: where the code is (paths and symbols), how it fits together, \
risks, and how to verify. Keep each note short and specific. If the card is clear and small, return no questions. \
You also say how this card sits among the other cards of the same batch, because they run in parallel on branches \
of their own: `after` lists the cards whose work this one needs in place first (it builds on them, or would \
conflict with them badly enough that it must start from their result); `areas` names the shared parts of the code \
it changes that other cards may also change (an engine, a shared schema, a core module, a build config), as short \
lowercase names; cards with an area in common never run at the same time. Leave both empty when the card is \
independent. Never list a card in `after` without a concrete reason in a note. When the code this card changes lives \
(also) in a sibling repository next to this one, name its folder in `repos`: Pitlane then gives the runs a worktree \
there on the same branch. Never ask where the changes should be made.";

/// What the analyst returns.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "questions": { "type": "array", "items": { "type": "object", "properties": {
                "text": { "type": "string" },
                "options": { "type": "array", "items": { "type": "string" } },
                "recommended": { "type": "integer", "description": "0-based index into options." },
                "context": { "type": "string", "description": "One to three sentences so the person can answer without opening the card: what part of the work this is about, what you found, and what each option changes." }
            }, "required": ["text", "options", "context"] } },
            "notes": { "type": "array", "items": { "type": "string" }, "description": "Findings in markdown, one per item." },
            "after": { "type": "array", "items": { "type": "integer" }, "description": "Ids of cards in this batch that must land first." },
            "areas": { "type": "array", "items": { "type": "string" }, "description": "Shared parts of the code this card changes, short lowercase names." },
            "repos": { "type": "array", "items": { "type": "string" }, "description": "Folder names of sibling git repositories (next to this one, e.g. amity-games) that hold code this card changes. Empty when it is all in this repository." }
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
    /// Sibling repositories (folder names next to this one) the card's code also lives in.
    #[serde(default)]
    pub repos: Vec<String>,
}

/// The card as the analyst sees it: spec, decisions so far, notes and the project's decisions.
/// `batch` is every card of this Run all (this one included), so the analyst can order them.
pub fn prompt(repo: &Repo, card: &Card, batch: &[Card]) -> String {
    let mut p = card_context(repo, card);
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

/// The run's briefing without its closing line about the handoff: spec, decisions, notes.
fn card_context(repo: &Repo, card: &Card) -> String {
    crate::runs::card_context(repo, card)
}

pub const RECHECK_RULES: &str = "You are Pitlane's analyst. You do not change anything. The session working this \
card stopped to ask the person some questions, and every one is answered now. Read the card, the answers and, where \
it helps, the repository, and say whether the work can go on. Ask again only what the answers and the code still \
leave open and would stop the work (scope, behaviour, a choice between real alternatives); never ask something \
already answered, how the work gets done or lands, or what you can find out by reading. Each question gets two to four \
options, the one you recommend, and a context the person can answer from without opening the card. Return no \
questions when the work can go on, which is the usual case. Notes only for what is new and the session will need \
(an answer that changes where to look, say); usually none.";

pub const TRIAGE_RULES: &str = "You are Pitlane's analyst. You do not change anything. The session working this \
card stopped blocked without asking the person anything; its last handoff and notes are on the card. Read them, and \
the repository where it helps, and turn what stopped it into questions the person can answer: a decision it could \
not make, something only the person can do or give (access, a credential, a file, a service to start), or a part of \
the spec that cannot be done as written. For something the person has to do outside, ask with options such as \
\"Done, go on\" and \"Leave that part out\". Each question gets two to four options, the one you recommend, and a \
context the person can answer from without opening the card. Return no questions only when nothing the person could \
say or do would help (the session gave up on something it can do, say); then put in a note what the next session \
should do differently.";

/// The card read again after its questions were answered: its spec, notes, questions with their
/// answers, and why it stopped.
pub fn recheck_prompt(repo: &Repo, card: &Card) -> String {
    let mut p = card_context(repo, card);
    push_why(&mut p, card);
    p.push_str("\n\nThe questions are answered. Can the work go on? Return questions only for what is still open.\n");
    p
}

/// The card of a session that stopped blocked without asking: what would get it going again.
pub fn triage_prompt(repo: &Repo, card: &Card) -> String {
    let mut p = card_context(repo, card);
    push_why(&mut p, card);
    p.push_str("\n\nThe session stopped here without asking anything. What does the person need to answer or do so the work can go on?\n");
    p
}

fn push_why(p: &mut String, card: &Card) {
    if let Some(b) = card.blocked.as_ref().filter(|b| !b.detail.is_empty()) {
        p.push_str(&format!("\n\n## Why the work stopped\n{}\n", b.detail));
    }
}

/// Writes a recheck on the card: its notes, and any question still open. True when nothing is.
pub fn record_recheck(repo: &Repo, id: u32, a: Analysis) -> Result<bool> {
    for n in a.notes.iter().filter(|n| !n.trim().is_empty()) {
        cards::note(repo, id, "claude", n)?;
    }
    let qs: Vec<NewQuestion> = a.questions.into_iter().filter(|q| !q.text.trim().is_empty()).map(NewQuestion::sound).collect();
    let clear = qs.is_empty();
    if !clear {
        cards::ask(repo, id, qs)?;
    }
    Ok(clear)
}

pub const TALK_RULES: &str = "You are Pitlane's analyst, talking one of this card's questions over with the person \
before the work starts. You do not change anything; you may read the repository to answer well. Answer what they said \
directly and briefly, like a colleague at their desk: if they propose another way, weigh it honestly against what you \
recommended, with what you know of the code (what it changes, what it costs, what it risks), and say plainly whether \
you would now go their way, yours, or a third one. If they ask something, answer it. If you need something from them \
to decide, ask it in one sentence. Then give the options as they stand now (two to four, their idea among them when \
it is a real alternative, worded as a decision someone else could carry out), the one you recommend now, and the \
context updated to what was said. Keep the options as they were when nothing changed them. Never ask or talk about \
how the work gets done or lands (worktrees, branches, commits, tests). Write in the person's language.";

/// What the analyst answers in a conversation about one question.
pub fn talk_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "reply": { "type": "string", "description": "Your answer to what the person just said, in markdown, two to six sentences." },
            "options": { "type": "array", "items": { "type": "string" }, "description": "The options as they stand now, two to four." },
            "recommended": { "type": "integer", "description": "0-based index into options of the one you recommend now." },
            "context": { "type": "string", "description": "One to three sentences: the question's context, updated to what was said." }
        },
        "required": ["reply", "options", "recommended", "context"]
    })
}

#[derive(Debug, Default, Deserialize)]
pub struct TalkReply {
    pub reply: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub recommended: Option<usize>,
    #[serde(default)]
    pub context: Option<String>,
}

/// The card, the question with its options and what was decided, the conversation so far.
pub fn talk_prompt(repo: &Repo, card: &Card, question: u32) -> Result<String> {
    let q = card
        .questions
        .iter()
        .find(|q| q.id == question)
        .with_context(|| format!("card #{} has no question {question}", card.id))?;
    let mut p = card_context(repo, card);
    p.push_str(&format!("\n\n## The question under discussion\n\n{}\n", q.text));
    if let Some(c) = &q.context {
        p.push_str(&format!("\nContext: {c}\n"));
    }
    if !q.options.is_empty() {
        p.push_str("\nOptions now:\n");
        for (i, o) in q.options.iter().enumerate() {
            let mark = if q.recommended == Some(i) { " (recommended)" } else { "" };
            p.push_str(&format!("{}. {o}{mark}\n", i + 1));
        }
    }
    match &q.answer {
        Some(a) => p.push_str(&format!(
            "\nThe person already decided: {a}{}. They came back to it; it stays until they choose again.\n",
            q.decision.as_deref().map(|d| format!(" ({d})")).unwrap_or_default()
        )),
        None => p.push_str("\nNot decided yet.\n"),
    }
    if !q.talk.is_empty() {
        p.push_str("\n## The conversation so far\n\n");
        for t in &q.talk {
            let who = if t.by == "claude" { "You" } else { "The person" };
            p.push_str(&format!("{who}: {}\n\n", t.text.trim()));
        }
    }
    p.push_str("\nAnswer the person's last message. Return reply, options, recommended and context.\n");
    Ok(p)
}

/// Reads the analyst's answer in a conversation.
pub fn read_talk(stdout: &str) -> Result<(TalkReply, Option<f64>)> {
    structured(stdout, "answer")
}

/// Reads `claude -p --output-format json`: the structured output and the cost.
pub fn read(stdout: &str) -> Result<(Analysis, Option<f64>)> {
    structured(stdout, "analysis")
}

fn structured<T: serde::de::DeserializeOwned>(stdout: &str, what: &str) -> Result<(T, Option<f64>)> {
    let a = crate::runs::answer::structured(stdout, what)?;
    Ok((a.output, a.cost_usd))
}

/// Writes the analysis on the card: its notes, its questions, and the mark that it was analysed.
pub fn record(repo: &Repo, id: u32, a: Analysis) -> Result<Card> {
    for n in a.notes.iter().filter(|n| !n.trim().is_empty()) {
        cards::note(repo, id, "claude", n)?;
    }
    let qs: Vec<NewQuestion> = a.questions.into_iter().filter(|q| !q.text.trim().is_empty()).map(NewQuestion::sound).collect();
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
