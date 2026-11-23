//! What a run does to its card: the worktree it prepares, the checkpoints and where the card goes
//! when the run ends.

use std::path::{Path, PathBuf};

use anyhow::Result;
use pit_core::Repo;
use pit_core::actions::{self, NewOptions};
use pit_core::cards::{self, CardPatch, CardState, NewQuestion};
use pit_core::flow::RUN_BASE_TOOLS;
use pit_core::runs::{self, RunEvent, RunMeta, RunState, SessionKind};
use serde_json::Value;

use super::home_dir;

pub(super) struct Prepared {
    pub(super) branch: String,
    pub(super) worktree: PathBuf,
    pub(super) briefing: String,
    pub(super) brief: Option<pit_core::brief::BriefStats>,
    pub(super) tools: Vec<String>,
    pub(super) max_turns: u32,
    pub(super) model: Option<String>,
    /// The repository turned the headless browser on (`[run] browser`).
    pub(super) browser: bool,
    /// The card's images, sent with the briefing.
    pub(super) images: Vec<(&'static str, Vec<u8>)>,
    /// The same branch in the project's related repositories.
    pub(super) related: Vec<pit_core::related::RelatedLane>,
    /// The repository keeps its runs' commands in the sandbox (`[run] sandbox`).
    pub(super) sandbox: bool,
}

/// The card's lane and worktree, made when missing, and what the agent is launched with.
/// Commits whatever is uncommitted in the worktree, so the next agent starts from it.
pub(super) fn checkpoint(worktree: &Path, card: u32) -> Result<Option<String>> {
    let g = pit_core::git::Git::new(worktree).without_hooks();
    if g.lines(&["status", "--porcelain"])?.is_empty() {
        return Ok(None);
    }
    g.run(&["add", "-A"])?;
    g.run(&[
        "commit",
        "-q",
        "-m",
        &format!("pitlane: checkpoint of card #{card} at a usage limit"),
    ])?;
    Ok(Some(g.run(&["rev-parse", "--short", "HEAD"])?))
}

/// The agent's last words in a run: its handoff summary if any, else its last two messages.
pub(super) fn last_words(timeline: &[(usize, RunEvent)]) -> String {
    if let Some(s) = timeline.iter().rev().find_map(|(_, e)| match e {
        RunEvent::Finished {
            handoff: Some(h), ..
        } => h.get("summary").and_then(|s| s.as_str()).map(str::to_owned),
        _ => None,
    }) {
        return s;
    }
    let texts: Vec<&str> = timeline
        .iter()
        .filter_map(|(_, e)| match e {
            RunEvent::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    texts
        .iter()
        .rev()
        .take(2)
        .rev()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The line that closes a briefing: what Pitlane adds to one goes in before it.
const HANDOFF_END: &str = "\nWhen you finish, end with the handoff.\n";

/// Adds `text` to a briefing before its closing line, or at its end when it has none.
pub(super) fn add_to_briefing(briefing: &mut String, text: &str) {
    let at = briefing.rfind(HANDOFF_END).unwrap_or(briefing.len());
    briefing.insert_str(at, text);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prepare(
    repo: &Path,
    card: u32,
    home: &Path,
    continues: Option<(&RunMeta, &str)>,
    brief: bool,
    into: Option<&str>,
    direct: bool,
    note: impl Fn(String),
) -> Result<Prepared> {
    let r = Repo::open(repo, &home_dir(), home)?;
    let c = cards::get(&r, card)?;
    let into = into.filter(|b| !b.trim().is_empty());
    let branch = match (&c.lane, into) {
        (Some(b), _) => b.clone(),
        (None, Some(t)) if direct => {
            note(format!("Working directly in {t}"));
            cards::update(
                &r,
                card,
                CardPatch {
                    lane: Some(Some(t.to_owned())),
                    target: Some(Some(t.to_owned())),
                    ..Default::default()
                },
            )?;
            t.to_owned()
        }
        (None, _) => {
            let branch = format!(
                "{}{}-{}",
                r.flow.run.branch_prefix,
                c.id,
                cards::slug(&c.title)
            );
            let from = into.filter(|t| !r.flow.flow.is_trunk(t));
            match from {
                Some(f) => note(format!("Creating lane {branch} from {f}")),
                None => note(format!(
                    "Creating lane {branch} from {}/{}",
                    r.flow.flow.remote, r.flow.flow.base
                )),
            }
            actions::new_lane_from(
                &r,
                &branch,
                NewOptions {
                    fetch: true,
                    setup: true,
                },
                from,
            )?;
            cards::update(
                &r,
                card,
                CardPatch {
                    lane: Some(Some(branch.clone())),
                    target: Some(into.map(str::to_owned)),
                    ..Default::default()
                },
            )?;
            if let Some(t) = into {
                note(format!("It lands in {t} once you approve it"));
            }
            branch
        }
    };
    let worktree = if r.git.rev_parse(&format!("refs/heads/{branch}"))?.is_some() {
        actions::ensure_worktree(&r, &branch)?
    } else {
        note(format!("Checking out {branch} from {}", r.flow.flow.remote));
        actions::new_lane(
            &r,
            &branch,
            NewOptions {
                fetch: true,
                setup: true,
            },
        )?
        .path
    };
    note(format!("Working in {}", worktree.display()));
    let mut tools: Vec<String> = RUN_BASE_TOOLS.iter().map(|s| s.to_string()).collect();
    tools.push("mcp__pitlane_run__note".into());
    tools.extend(r.flow.run.allow.iter().cloned());
    let the_card = cards::get(&r, card)?;
    // The same branch in each related repository, made and landed like this one.
    let related = pit_core::related::prepare(
        &r,
        &branch,
        the_card.target.as_deref(),
        the_card.direct(),
        &note,
    );
    // Where to look, from the index. A failure here never stops the run; it runs without the map.
    let map = if brief && r.flow.run.brief {
        match pit_core::brief::compile(&r, home, &the_card, pit_core::brief::DEFAULT_BUDGET) {
            Ok(b) if !b.text.is_empty() => {
                note(format!(
                    "Briefed with {} from the index (~{} tokens)",
                    plural(b.stats.files, "file"),
                    b.stats.chars / 4
                ));
                Some(b)
            }
            Ok(_) => {
                note("Nothing in the index matches this card; running without a map".into());
                None
            }
            Err(e) => {
                note(format!(
                    "Could not compile the map ({e:#}); running without it"
                ));
                None
            }
        }
    } else {
        None
    };
    Ok(Prepared {
        images: cards::image_bytes(&r, &the_card),
        related: related.clone(),
        branch,
        worktree: worktree.clone(),
        brief: map.as_ref().map(|b| b.stats.clone()),
        briefing: {
            let mut b = runs::briefing(&r, &the_card);
            let also = pit_core::related::briefing(&related);
            if !also.is_empty() {
                add_to_briefing(&mut b, &also);
            }
            if let Some(m) = &map {
                add_to_briefing(&mut b, &format!("\n{}", m.text));
            }
            if let Some((prev, words)) = continues {
                let g = pit_core::git::Git::new(&worktree);
                // Production as the yard resolves it: the remote branch, or the local one without a remote.
                let base = pit_core::lanes::Target::resolve(&r, &r.flow.flow.production)
                    .ok()
                    .flatten()
                    .map(|t| t.sha);
                let log = match base {
                    Some(b) => g
                        .run(&["log", "--oneline", "-20", &format!("{b}..HEAD")])
                        .unwrap_or_default(),
                    None => g.run(&["log", "--oneline", "-5"]).unwrap_or_default(),
                };
                let status = g.run(&["status", "--short"]).unwrap_or_default();
                b.push_str(&format!(
                    "\n## You are continuing run {}\n\nIt stopped at a usage limit on another account or model. Its work is committed on this branch; carry on from there, do not redo it.\n",
                    prev.id
                ));
                if !words.trim().is_empty() {
                    b.push_str(&format!("\nWhere it was:\n\n{}\n", words.trim()));
                }
                if !log.is_empty() {
                    b.push_str(&format!("\nCommits so far:\n\n{log}\n"));
                }
                if !status.is_empty() {
                    b.push_str(&format!("\nUncommitted:\n\n{status}\n"));
                }
            }
            b
        },
        tools,
        max_turns: r.flow.run.max_turns,
        model: r.flow.run.model.clone(),
        browser: r.flow.run.browser,
        sandbox: r.flow.run.sandbox,
    })
}

/// Blocks the run's card, saying why.
pub(super) fn block_card(m: &RunMeta, reason: cards::BlockReason, detail: &str, home: &Path) -> Result<()> {
    let r = Repo::open(&m.repo, &home_dir(), home)?;
    cards::update(
        &r,
        m.card,
        CardPatch {
            state: Some(CardState::Blocked),
            blocked: Some(cards::Block::new(reason, short(detail), Some(m.id.clone()))),
            ..Default::default()
        },
    )?;
    Ok(())
}

/// The first line of a reason, cut to fit a board tile and a card's header.
pub(super) fn short(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or_default().trim();
    if line.chars().count() <= 200 { line.to_owned() } else { format!("{}…", line.chars().take(199).collect::<String>()) }
}

pub(super) fn set_card_state(m: &RunMeta, state: CardState, home: &Path) -> Result<()> {
    let r = Repo::open(&m.repo, &home_dir(), home)?;
    cards::update(
        &r,
        m.card,
        CardPatch {
            state: Some(state),
            ..Default::default()
        },
    )?;
    Ok(())
}

/// The first message of a run that resumes a stopped one: carry on, plus what the card got
/// while the agent was away (notes, answers), which its session has not seen.
pub(super) fn resume_message(c: &cards::Card, prev: &RunMeta) -> String {
    let since = cards::timestamp(prev.ended_at.unwrap_or(prev.started_at));
    let mut news: Vec<String> = c
        .notes
        .iter()
        .filter(|n| n.at >= since)
        .map(|n| format!("- Note ({}): {}", n.by, n.text.trim()))
        .collect();
    news.extend(c.questions.iter().filter(|q| {
        q.answered_at.as_deref().is_some_and(|a| a >= since.as_str())
    }).map(|q| {
        format!(
            "- Answered: {} → {}",
            q.text.trim(),
            q.decision.as_deref().or(q.answer.as_deref()).unwrap_or_default().trim()
        )
    }));
    let asked = prev.handoff.as_ref().and_then(|h| h.get("status")).and_then(Value::as_str) == Some("blocked");
    let mut text = if asked {
        "You ended blocked on your questions; they are answered now, and you are resumed in the same session: carry on with the answers below. End with the handoff."
    } else {
        "You were stopped and are resumed in the same session: carry on from where you stopped (check the worktree first, a step may have been cut short). End with the handoff."
    }
    .to_owned();
    if !news.is_empty() {
        text.push_str(if asked { "\n\nSince you stopped, the card got:\n" } else { "\n\nWhile you were stopped, the card got:\n" });
        text.push_str(&news.join("\n"));
    }
    text
}

/// Where the card goes when its run ends. `ran`: the agent got to start (it has a session).
pub(super) fn settle_card(m: &RunMeta, home: &Path, ran: bool) -> Result<()> {
    let r = Repo::open(&m.repo, &home_dir(), home)?;
    let status = m
        .handoff
        .as_ref()
        .and_then(|h| h.get("status"))
        .and_then(Value::as_str);
    // The handoff stays on the card as a note: the next session starts from it.
    if let Some(h) = &m.handoff
        && let Some(summary) = h
            .get("summary")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
    {
        let mut text = format!(
            "Run {} ended {}: {}",
            m.id,
            status.unwrap_or("done"),
            summary.trim()
        );
        // What the card still needs (older runs called it next_steps), only when it is not finished.
        let next: Vec<&str> = h
            .get("left")
            .or_else(|| h.get("next_steps"))
            .filter(|_| status == Some("partial"))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if !next.is_empty() {
            text.push_str("\n\nLeft to do:\n");
            for n in next {
                text.push_str(&format!("- {n}\n"));
            }
        }
        let _ = cards::note(&r, m.card, "claude", &text);
    }
    let mut asked = false;
    if let Some(qs) = m
        .handoff
        .as_ref()
        .and_then(|h| h.get("questions"))
        .and_then(Value::as_array)
    {
        let qs: Vec<NewQuestion> = qs
            .iter()
            .filter_map(|q| serde_json::from_value(q.clone()).ok())
            .collect();
        if status == Some("blocked") {
            if !qs.is_empty() {
                asked = true;
                cards::ask(&r, m.card, qs)?;
            }
        } else {
            // A run that finished does not get to ask: what it would have asked is on the card, for the review.
            for q in qs {
                let _ = cards::note(&r, m.card, "claude", &unasked(&q));
            }
        }
    }
    let summary = m.handoff.as_ref().and_then(|h| h.get("summary")).and_then(Value::as_str).unwrap_or_default();
    let (next, block) = match (m.state, status) {
        // It stopped to ask: once you answer, Pitlane rechecks the card and frees it.
        (RunState::Done, Some("blocked")) if asked => (CardState::Blocked, Some(cards::Block::new(cards::BlockReason::Questions, short(summary), Some(m.id.clone())))),
        (RunState::Done, Some("blocked")) => (CardState::Blocked, Some(cards::Block::new(cards::BlockReason::Stuck, short(summary), Some(m.id.clone())))),
        (RunState::Done, _) => (CardState::Review, None),
        (RunState::Stopped, _) => (CardState::Ready, None),
        _ => (CardState::Blocked, Some(cards::Block::new(cards::BlockReason::Failed, short(m.error.as_deref().unwrap_or("The run ended without finishing.")), Some(m.id.clone())))),
    };
    // Stopped once the agent had started: the next Run resumes its session, not a new one.
    // Stopped before it started: the session it was going to resume is still the one to resume.
    // Blocked on its questions: the next Run resumes it too, with your answers.
    let paused = if m.kind != SessionKind::Run {
        None
    } else if m.state == RunState::Stopped {
        if ran { Some(m.id.clone()) } else { m.resumed_from.clone() }
    } else if block.as_ref().is_some_and(|b| b.reason == cards::BlockReason::Questions) && ran {
        Some(m.id.clone())
    } else {
        None
    };
    cards::update(
        &r,
        m.card,
        CardPatch {
            state: Some(next),
            paused: Some(paused),
            blocked: block,
            ..Default::default()
        },
    )?;
    Ok(())
}

/// A question a finished run raised anyway, as a note: what it asked and what it went with.
fn unasked(q: &NewQuestion) -> String {
    let went = q.recommended.and_then(|i| q.options.get(i));
    match went {
        Some(o) => format!("Decided without asking: {} Went with: {o}", q.text.trim()),
        None => format!("Left for the review: {}", q.text.trim()),
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}
