//! What a card's agent is told before it starts: the rules, the handoff it ends with, and the briefing.

use super::*;

/// Added to a run's rules when the repository turns the browser on (`[run] browser`).
pub const BROWSER_RULES: &str = "\n\nYou have a headless browser (the browser tools). When your change shows on a page \
(a web app, a component, a style), open it there, take a screenshot of the result (browser_take_screenshot), and \
fix what looks wrong before you finish; the person sees every screenshot in the run. If the app needs a dev server, \
start it in the background and stop it when you are done. Screenshot nothing else: no secrets, no pages outside \
this project.";

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
            "left": { "type": "array", "items": { "type": "string" },
                "description": "Only with status partial: what this card's spec still needs, for the next session on it. Never process steps (merging, pushing, removing worktrees or branches, running the batch's gate, deploying): Pitlane and the person already decided those." },
            "suggestions": { "type": "array", "description": "Work you noticed that is outside this card's spec and worth doing: a bug nearby, a follow-up feature, a missing test or asset. Not process steps (merge, push, gate, cleanup, commit something) and not what the card asked for. The person may turn each into a card that another session works from alone.",
                "items": { "type": "object", "properties": {
                    "title": { "type": "string", "description": "A card title, short." },
                    "body": { "type": "string", "description": "The card's spec, complete on its own: what to do and why, where in the code (files, functions), what you found, and how to verify it." } },
                    "required": ["title", "body"] } },
            "questions": { "type": "array", "description": "Only with status blocked, and only for a big call the spec and decisions do not settle: every such question at once, each with options and the one you recommend. Anything smaller you decide and record with the note tool.",
                "items": { "type": "object", "properties": {
                    "text": { "type": "string" },
                    "options": { "type": "array", "items": { "type": "string" } },
                    "recommended": { "type": "integer" },
                    "context": { "type": "string", "description": "One to three sentences so the person can answer without opening the card: what part of the work this is about, what you found, and what each option changes." } }, "required": ["text", "context"] } },
            "check": { "type": "object", "description": "Only with status partial, and only when the next stage builds on something you made whose quality only the person can judge by looking (a model, art, a look, how it feels): one question asking them to look at it before the card goes on, with options (go on as it is, or what to redo) and the one you recommend. The card waits for the answer, and your session hears it. Never for what tests, screenshots you checked yourself or the judge settle.",
                "properties": {
                    "text": { "type": "string", "description": "The question, short: what to look at and whether it is good enough to build on." },
                    "options": { "type": "array", "items": { "type": "string" } },
                    "recommended": { "type": "integer" },
                    "context": { "type": "string", "description": "Where to look (the paths of the renders, screenshots or files) and what you changed, in one to three sentences." } },
                "required": ["text", "context"] }
        },
        "required": ["status", "summary"]
    })
}

/// Fixed rules for every run: the same bytes each time, so the prompt cache holds.
pub const RULES: &str = "You are running inside Pitlane, on one card, in a git worktree made for it. \
Work only inside the current directory and the related repositories' worktrees the briefing lists, if any. Commit your work on the current branch with clear messages; \
never push, never switch branches, never create branches or worktrees (in any repository; Pitlane makes \
them), and merge only when Pitlane asks you to bring a branch in. The project context in the briefing, written by the \
person, overrides these git rules where it says otherwise (what to push, deploy or merge, and where). Read the card's spec and decisions before you start. \
The person answered the analysis's questions and pressed Run; they may be watching or away, and the run must never \
stall. Decide how to do the work yourself (worktrees, branches, commits, tests, file layout, naming, what to measure, \
small calls on scope) from the spec, the decisions and the code: take the option you would have recommended and \
record the choice and why with the note tool, so the review can see it. When you reach a call that is the person's \
(behaviour they will see, where scope ends, a choice between real alternatives) and the spec and decisions do not \
settle it, use the ask tool instead of picking silently, with options and the one you recommend: it brings their \
answer when they are here, and tells you to go with your recommendation when they are away. Ask rarely and one call \
at a time, since the tool waits for the answer. Set costly on a call that would be costly to \
undo (a different product behaviour, deleting or migrating data, a public API or schema change, spending money). \
Report status \"blocked\" only when the work cannot go on, or the ask tool tells you to; finish everything that does \
not depend on it first, then put every question still open in the handoff, each with options and the one you \
recommend, unless the ask tool says it is already on the card. \
As you learn something the next session on this \
card would need (where the code is, a choice you made and why, what is left), add it with the note tool, short and \
one finding per note. End every run with the structured handoff.";

/// The first message of a run: the card, its decisions and the repository's decisions file.
pub fn briefing(repo: &Repo, card: &Card) -> String {
    let mut b = card_context(repo, card);
    b.push_str("\nWhen you finish, end with the handoff.\n");
    b
}

/// What a session reads about a card (its spec, its decisions, its notes, the project's
/// decisions), without the run's closing line: for the sessions that only read it (the analyst).
pub fn card_context(repo: &Repo, card: &Card) -> String {
    let mut b = format!("# Card #{}: {}\n\n", card.id, card.title);
    if card.body.trim().is_empty() {
        b.push_str("(No spec beyond the title. Keep the change small and say what you assumed.)\n");
    } else {
        b.push_str(card.body.trim());
        b.push('\n');
    }
    if !card.images.is_empty() {
        b.push_str(&format!(
            "\n## Images\n\nThe {} attached to this card come with this message, in this order: {}. They are part of the spec.\n",
            if card.images.len() == 1 { "image".to_owned() } else { format!("{} images", card.images.len()) },
            card.images.join(", ")
        ));
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
    let ctx = cards::project_context(repo);
    if !ctx.is_empty() {
        b.push_str("\n## Project context\n\nWritten by the person for every card of this project. It holds for this card too, and where it says otherwise it overrides the general rules on pushing, merging and deploying.\n\n");
        b.push_str(&ctx);
        b.push('\n');
    }
    if let Ok(d) = std::fs::read_to_string(cards::decisions_path(repo))
        && !d.trim().is_empty()
    {
        b.push_str("\n## Project decisions\n\n");
        b.push_str(d.trim());
        b.push('\n');
    }
    b
}
