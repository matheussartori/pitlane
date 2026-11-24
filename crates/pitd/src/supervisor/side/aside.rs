//! Asking a card's run something without stopping or steering it.

use super::*;
use crate::supervisor::launch::sandbox_settings;

/// How long an aside may take.
const ASIDE_LIMIT: Duration = Duration::from_secs(20 * 60);
/// Turns an aside may take.
const ASIDE_MAX_TURNS: &str = "40";
/// Earlier asides of the same run the next one is told about, so a follow-up makes sense.
const ASIDE_MEMORY: usize = 3;

impl Supervisor {
    /// A question for a card's run that the run never reads: a copy of its Claude Code
    /// conversation (`--fork-session`) answers it beside it, in the same worktree, as its own
    /// session in view, and the question and the answer show in the run's log. The run goes on
    /// with what it was doing, as if nobody had asked. Returns the aside's id.
    pub async fn aside(self: &Arc<Self>, run: &str, text: &str) -> Result<String> {
        let text = text.trim();
        if text.is_empty() {
            bail!("say what to ask");
        }
        let m = self.get(run)?;
        if m.kind != SessionKind::Run {
            bail!("only a card's run is asked on the side");
        }
        // Its Claude Code said it started: the conversation exists to be copied.
        if !self.read_timeline(&m.id, 0).iter().any(|(_, e)| matches!(e, RunEvent::Started { .. })) {
            bail!("the run has not started its conversation yet; ask again in a moment");
        }
        // Claude Code keeps the session where the run's account signed in.
        let config_dir = accounts::profile_dir(&m.account, &self.cfg.home);
        if !m.worktree.is_dir() {
            bail!("the run's worktree is gone, and its conversation lives there");
        }
        let r = self.repo(&m.repo)?;
        let mut args = read_only_args(&mode_for(&r.root));
        // Like the run: Claude in Chrome on, to look at what it made. What no rule allows is
        // asked to you through Pitlane.
        args.extend(["--chrome".into(), "--allowedTools".into(), READ_ONLY_TOOLS.into(), "--max-turns".into(), ASIDE_MAX_TURNS.into()]);
        for l in &m.related {
            args.extend(["--add-dir".into(), l.worktree.to_string_lossy().into_owned()]);
        }
        if r.flow.run.sandbox {
            let settings = sandbox_settings(&self.cfg.home, &m.worktree, m.related.iter().map(|l| l.worktree.as_path()), &r.flow.run.write, r.flow.run.ask_unsandboxed);
            args.extend(["--settings".into(), Value::Object(settings).to_string()]);
        }
        args.extend(["--append-system-prompt".into(), ASIDE_RULES.into()]);
        if let Some(model) = &m.model {
            args.extend(["--model".into(), model.clone()]);
        }
        let side = Side {
            kind: SessionKind::Aside,
            repo: m.repo.clone(),
            cwd: m.worktree.clone(),
            card: m.card,
            card_title: m.card_title.clone(),
            parent: Some(m.id.clone()),
            account: m.account.clone(),
            config_dir,
            model: m.model.clone(),
            args,
            prompt: aside_prompt(text, &self.asides(&m.id)),
            limit: ASIDE_LIMIT,
            board: false,
            images: Vec::new(),
            resume: false,
            fork: Some(m.session_id.clone()),
        };
        let id = self.open_side(&side)?;
        self.emit(&id, RunEvent::Said { text: text.to_owned() });
        self.record(&m.id, RunEvent::Aside { session: id.clone(), by: "you".into(), text: text.to_owned() });
        let me = Arc::clone(self);
        let (aside, run) = (id.clone(), m.id);
        tokio::spawn(async move {
            let ev = match me.drive_side(aside.clone(), side).await {
                Ok(line) => match reply_of(&line) {
                    Some(text) => RunEvent::Aside { session: aside, by: "agent".into(), text },
                    None => RunEvent::Aside { session: aside, by: "failed".into(), text: "It ended without an answer.".into() },
                },
                Err(e) => RunEvent::Aside { session: aside, by: "failed".into(), text: format!("{e:#}") },
            };
            me.record(&run, ev);
        });
        Ok(id)
    }

    /// The run's earlier asides, oldest first: each question with its answer, when it got one.
    fn asides(&self, run: &str) -> Vec<(String, Option<String>)> {
        let mut out: Vec<(String, String, Option<String>)> = Vec::new();
        for (_, ev) in self.read_timeline(run, 0) {
            if let RunEvent::Aside { session, by, text } = ev {
                match by.as_str() {
                    "you" => out.push((session, text, None)),
                    "agent" => {
                        if let Some(q) = out.iter_mut().find(|q| q.0 == session) {
                            q.2 = Some(text);
                        }
                    }
                    _ => {}
                }
            }
        }
        let skip = out.len().saturating_sub(ASIDE_MEMORY);
        out.into_iter().skip(skip).map(|(_, q, a)| (q, a)).collect()
    }
}

/// What the aside is asked: the person's words, after what was asked on the side before.
fn aside_prompt(text: &str, earlier: &[(String, Option<String>)]) -> String {
    let mut p = String::new();
    if !earlier.is_empty() {
        p.push_str("Asked on the side before (you are a fresh copy and did not see these):\n");
        for (q, a) in earlier {
            p.push_str(&format!("- They asked: {q}\n  Answer: {}\n", a.as_deref().unwrap_or("(none)")));
        }
        p.push('\n');
    }
    p.push_str(text);
    p
}

/// The text of a session's `result` line.
fn reply_of(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    let text = v.get("result")?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

const ASIDE_RULES: &str = "You are a copy of this conversation, made so the person can ask something on the side \
while the original agent goes on working in this same worktree right now. The original never sees this exchange. \
Answer what they ask from what you did and know, and look at the files, the history or a page in the browser when it \
helps. Never change the worktree: no edits, no commits, no git command that moves a branch, the index or the stash, \
no installs, no generated files in it; the original is working there. Do not carry on with the card's work and do not \
write a handoff. If what they ask is work only the original should do (a change, a test that writes, a new step), \
say so plainly in one line, so they can tell the run itself (⌘Enter in its log). Reply in short, plain text.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_aside_hears_what_was_asked_before() {
        let p = aside_prompt("e o cofre?", &[("ficou bom?".into(), Some("Sim.".into())), ("e a luz?".into(), None)]);
        assert!(p.contains("They asked: ficou bom?\n  Answer: Sim."));
        assert!(p.contains("Answer: (none)"));
        assert!(p.ends_with("e o cofre?"));
        assert_eq!(aside_prompt("só isso", &[]), "só isso");
    }

    #[test]
    fn the_answer_is_the_result_text() {
        assert_eq!(reply_of(r#"{"type":"result","result":" Ficou. "}"#).as_deref(), Some("Ficou."));
        assert_eq!(reply_of(r#"{"type":"result","result":""}"#), None);
        assert_eq!(reply_of("nope"), None);
    }
}
