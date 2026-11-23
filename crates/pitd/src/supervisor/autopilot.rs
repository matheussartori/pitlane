//! Run all's next step for a finished run, with nobody watching.

use std::path::Path;
use std::sync::Arc;

use pit_core::Repo;
use pit_core::actions;
use pit_core::cards::{self, CardPatch, CardState};
use pit_core::runs::{RunEvent, RunMeta, RunState};
use serde_json::Value;

use super::{StartOpts, Supervisor, home_dir};
use super::verify::fix_request;

/// Times a run that ran out of turns goes on in the same session.
const MAX_EXTENSIONS: u32 = 2;

/// The run stopped at its turn limit without a handoff.
pub(super) fn out_of_turns(m: &RunMeta, max_turns: u32) -> bool {
    m.handoff.is_none() && m.turns.is_some_and(|t| t + 1 >= max_turns)
}

/// A Run all run that failed only for running out of turns, with extensions left: the same
/// session goes on by itself, so the failure is nobody's to look at, and the card stays in progress.
pub(super) fn goes_on(m: &RunMeta, max_turns: u32) -> bool {
    m.autopilot && m.state == RunState::Failed && m.extensions < MAX_EXTENSIONS && out_of_turns(m, max_turns)
}

impl Supervisor {
    /// Run all's next step for a finished run, with nobody watching: more turns for a run that ran
    /// out of them, a fix round for what the proofs or the judge found, else the work lands in its
    /// Into branch. Only a card that cannot go on (proofs still failing, a blocked handoff, an
    /// error) stays behind; the rest of the batch never waits on it.
    pub(super) async fn autopilot(self: &Arc<Self>, id: &str) {
        let Ok(m) = self.get(id) else { return };
        let home = self.cfg.home.clone();
        let Ok(r) = Repo::open(&m.repo, &home_dir(), &home) else {
            return;
        };
        let next = StartOpts {
            autopilot: true,
            round: m.round,
            extensions: m.extensions,
            ..Default::default()
        };
        let say = |text: String| {
            let _ = cards::note(&r, m.card, "pitlane", &text);
        };
        match m.state {
            // Out of turns: the same session goes on, twice at most.
            RunState::Failed if out_of_turns(&m, r.flow.run.max_turns) => {
                if m.extensions >= MAX_EXTENSIONS {
                    say(format!(
                        "Run {} ran out of turns a third time; the card waits for you.",
                        m.id
                    ));
                    return;
                }
                let msg = "You ran out of turns before finishing. Carry on from where you stopped: commit what works, \
                           finish the card, and end with the handoff. Skip anything you already did.";
                match self.start(
                    m.repo.clone(),
                    m.card,
                    Some(m.id.clone()),
                    StartOpts {
                        message: Some(msg.into()),
                        extensions: m.extensions + 1,
                        ..next
                    },
                ) {
                    Ok(n) => self.emit(
                        id,
                        RunEvent::Note {
                            text: format!("Out of turns: continues as run {}.", n.id),
                        },
                    ),
                    Err(e) => {
                        // The card stayed in progress for the run that was to go on: it waits for you now.
                        let text = format!("Run {} ran out of turns and could not continue: {e:#}", m.id);
                        let block = cards::Block::new(cards::BlockReason::Failed, text.clone(), Some(m.id.clone()));
                        let _ = cards::update(
                            &r,
                            m.card,
                            CardPatch {
                                state: Some(CardState::Blocked),
                                blocked: Some(block),
                                ..Default::default()
                            },
                        );
                        say(text);
                    }
                }
            }
            RunState::Done
                if m.handoff
                    .as_ref()
                    .and_then(|h| h.get("status"))
                    .and_then(Value::as_str)
                    != Some("blocked") =>
            {
                let c = match cards::get(&r, m.card) {
                    Ok(c) if c.state == CardState::Review => c,
                    _ => return,
                };
                let proofs_failed = m.proofs_ok == Some(false);
                let changes = m.verdict == Some(pit_core::verify::Verdict::Changes);
                if (proofs_failed || changes) && m.round < r.flow.verify.max_rounds {
                    let what = fix_request(&self.dir(id));
                    let first = format!(
                        "Pitlane checked your work and it needs another pass:\n\n{what}\n\nFix it, commit, and end with the handoff."
                    );
                    match self.start(
                        m.repo.clone(),
                        m.card,
                        Some(m.id.clone()),
                        StartOpts {
                            message: Some(first),
                            round: m.round + 1,
                            extensions: 0,
                            ..next
                        },
                    ) {
                        Ok(n) => self.emit(
                            id,
                            RunEvent::Note {
                                text: format!(
                                    "Round {} starts on its own: run {}.",
                                    m.round + 1,
                                    n.id
                                ),
                            },
                        ),
                        Err(e) => say(format!(
                            "The fix round for run {} did not start: {e:#}",
                            m.id
                        )),
                    }
                    return;
                }
                if proofs_failed {
                    let _ = cards::update(
                        &r,
                        m.card,
                        CardPatch {
                            state: Some(CardState::Blocked),
                            blocked: Some(cards::Block::new(cards::BlockReason::Checks, format!("The checks still fail after {} rounds.", m.round + 1), Some(m.id.clone()))),
                            ..Default::default()
                        },
                    );
                    say(format!(
                        "The checks still fail after {} rounds, so this card did not land. The rest of the batch went on.",
                        m.round + 1
                    ));
                    return;
                }
                if changes {
                    say(format!(
                        "Landed after {} rounds; the judge still asked for changes. See its findings in the run.",
                        m.round + 1
                    ));
                }
                if c.direct() {
                    // Worked in its target, the work is already there. Done, unless the agent
                    // said more is needed: then the card is back in the backlog for the rest.
                    if unfinished(&m) {
                        self.reopen_unfinished(&m.repo, m.card);
                        return;
                    }
                    let _ = cards::update(
                        &r,
                        m.card,
                        CardPatch {
                            state: Some(CardState::Done),
                            ..Default::default()
                        },
                    );
                    return;
                }
                if let Err(e) = self.land_card(&m.repo, &c, &m.related) {
                    say(format!(
                        "Could not put this card's work in line to land: {e:#}"
                    ));
                }
            }
            _ => {}
        }
    }

    /// A Run all card whose work landed, but whose agent said more is needed (a partial handoff):
    /// it is not done. It goes back to the queue with what is left on it, where the batch (or
    /// the next one) works it again, instead of closing with part of its spec undone.
    pub(super) fn reopen_unfinished(&self, repo: &Path, card: u32) {
        let Ok(r) = Repo::open(repo, &home_dir(), &self.cfg.home) else {
            return;
        };
        let Some(last) = self.latest_run(&r.root, card) else {
            return;
        };
        if !unfinished(&last) {
            return;
        }
        let _ = cards::update(
            &r,
            card,
            CardPatch {
                state: Some(CardState::Queued),
                ..Default::default()
            },
        );
        let _ = cards::note(
            &r,
            card,
            "pitlane",
            &format!(
                "Run {} landed what it did and said more is needed, so this card is not done: it is back in the queue for what is left (see the note above).",
                last.id
            ),
        );
    }

    /// A Run all card whose lane conflicts with its Into branch: its agent brings the branch in and
    /// resolves the conflicts itself, then the card lands as usual. Past the last round it waits.
    pub(super) fn conflict_round(self: &Arc<Self>, job: &pit_core::queue::ShipJob, files: &[String]) {
        let Some(card) = job.card else { return };
        let board_repo = job.card_repo.clone().unwrap_or_else(|| job.repo.clone());
        let Ok(r) = Repo::open(&board_repo, &home_dir(), &self.cfg.home) else {
            return;
        };
        let Some(last) = self.latest_run(&r.root, card) else {
            return;
        };
        if !last.autopilot {
            return;
        }
        if last.round >= r.flow.verify.max_rounds {
            let _ = cards::note(
                &r,
                card,
                "pitlane",
                &format!(
                    "{} conflicts with {} in {}; out of rounds, so it waits for you.",
                    job.branch,
                    job.target,
                    files.join(", ")
                ),
            );
            let _ = cards::update(
                &r,
                card,
                CardPatch {
                    state: Some(CardState::Blocked),
                    blocked: Some(cards::Block::new(cards::BlockReason::Conflict, format!("{} conflicts with {}.", job.branch, job.target), Some(last.id.clone()))),
                    ..Default::default()
                },
            );
            return;
        }
        // In a related repository the merge happens in that repository's worktree.
        let place = match &job.card_repo {
            Some(_) => Repo::open(&job.repo, &home_dir(), &self.cfg.home)
                .and_then(|rr| actions::ensure_worktree(&rr, &job.branch))
                .map(|w| format!(" (in `{}`, use `git -C` there)", w.display()))
                .unwrap_or_default(),
            None => String::new(),
        };
        let msg = format!(
            "Your work could not land: {} changed meanwhile and conflicts with it in {}. Pitlane asks you to \
             bring it in now{place}: run `git merge {}`, resolve every conflict keeping both sides' intent, run the narrowest \
             checks for what you touched, commit the merge, and end with the handoff.",
            job.target,
            files.join(", "),
            job.target
        );
        let opts = StartOpts {
            autopilot: true,
            round: last.round + 1,
            message: Some(msg),
            ..Default::default()
        };
        if let Err(e) = self.start(board_repo.clone(), card, Some(last.id.clone()), opts) {
            let _ = cards::note(
                &r,
                card,
                "pitlane",
                &format!("The conflict round did not start: {e:#}"),
            );
        }
    }
}

/// A Run all run that ended well but handed off as partial: the card still has work left.
fn unfinished(m: &RunMeta) -> bool {
    m.autopilot
        && m.state == RunState::Done
        && m.handoff
            .as_ref()
            .and_then(|h| h.get("status"))
            .and_then(Value::as_str)
            == Some("partial")
}
