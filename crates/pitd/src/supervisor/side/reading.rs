//! A blocked card read again: are its answers enough to go on, or what would get its agent going?

use super::*;

impl Supervisor {
    /// A card blocked on its questions, all answered now: a read-only Claude Code session reads it
    /// again with the answers. Nothing left open: the card goes back to the Backlog (its next Run
    /// resumes the agent's session with the answers). Something still open: the new questions go
    /// on the card, which keeps waiting for you. Returns at once; the reading goes on behind.
    pub fn recheck(self: &Arc<Self>, repo: PathBuf, id: u32) -> Result<bool> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        if !cards::wants_recheck(&cards::get(&r, id)?) {
            return Ok(false);
        }
        self.reread(r, id, Reading::Recheck)
    }

    /// A card whose agent stopped blocked without asking: a read-only session turns what stopped
    /// it into questions for you, and the card then waits on them like any other. When nothing you
    /// could answer would help, the card is run again once by itself, with what the reading noted.
    pub fn triage(self: &Arc<Self>, repo: &Path, id: u32) -> Result<bool> {
        let r = Repo::open(repo, &home_dir(), &self.cfg.home)?;
        let c = cards::get(&r, id)?;
        if !c.blocked.as_ref().is_some_and(|b| b.reason == cards::BlockReason::Stuck && b.checking.is_none()) {
            return Ok(false);
        }
        self.reread(r, id, Reading::Triage)
    }

    fn reread(self: &Arc<Self>, r: Repo, id: u32, kind: Reading) -> Result<bool> {
        // One reading at a time per card (an answer from the app and one from pit, say).
        if !self.analysing.lock().unwrap().insert((r.root.clone(), id)) {
            return Ok(false);
        }
        let card = match cards::set_checking(&r, id, true) {
            Ok(c) => c,
            Err(e) => {
                self.analysing.lock().unwrap().remove(&(r.root.clone(), id));
                return Err(e);
            }
        };
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let _busy = Release(&me.analysing, (r.root.clone(), id));
            let out = async {
                let choice = me.side_choice(&r.root)?;
                let _slot = me.analysis_gate.acquire().await;
                let (prompt, rules) = match kind {
                    Reading::Recheck => (pit_core::analyze::recheck_prompt(&r, &card), pit_core::analyze::RECHECK_RULES),
                    Reading::Triage => (pit_core::analyze::triage_prompt(&r, &card), pit_core::analyze::TRIAGE_RULES),
                };
                let (a, _) = me.analyst(&r.root, &card, &prompt, rules, choice.as_ref(), r.flow.run.model.as_deref()).await?;
                pit_core::analyze::record_recheck(&r, id, a)
            }
            .await;
            // Moved by hand meanwhile: what it is now stands.
            let still = cards::get(&r, id).is_ok_and(|c| c.blocked.as_ref().is_some_and(|b| b.checking.is_some()));
            if !still {
                return;
            }
            let note = |text: &str| {
                let _ = cards::note(&r, id, "pitlane", text);
            };
            match (kind, out) {
                (Reading::Recheck, Ok(true)) => {
                    // A Run all card goes back to the queue, where the batch takes it again.
                    let batch = me.latest_run(&r.root, id).is_some_and(|m| m.autopilot);
                    note(if batch {
                        "Read again with your answers: nothing else is open, so the card is back in the queue. Its next run picks up the agent's session with the answers."
                    } else {
                        "Read again with your answers: nothing else is open, so the card is back in the Backlog. Its next run picks up the agent's session with the answers."
                    });
                    let _ = cards::update(&r, id, CardPatch { state: Some(if batch { CardState::Queued } else { CardState::Ready }), ..Default::default() });
                }
                // Questions to answer: from here it is a card blocked on questions like any other.
                (_, Ok(false)) => {
                    let _ = cards::edit_block(&r, id, |b| {
                        b.reason = cards::BlockReason::Questions;
                        b.checking = None;
                        Ok(())
                    });
                }
                (Reading::Triage, Ok(true)) => {
                    let again = cards::get(&r, id).is_ok_and(|c| c.retries < STUCK_RETRIES);
                    let _ = cards::edit_block(&r, id, |b| {
                        b.checking = None;
                        b.retry_at = again.then(now_secs);
                        Ok(())
                    });
                    note(if again {
                        "Nothing you could answer would get the agent going, so Pitlane runs the card again, with what the reading noted."
                    } else {
                        "Nothing you could answer would get the agent going, and it already got stuck after a second try: the card waits for you (edit the spec, try again, or send it back to the Backlog)."
                    });
                }
                (_, Err(e)) => {
                    note(&format!("Could not read the card again ({e:#}). Try again, or move it back to the Backlog when it is ready."));
                    let _ = cards::set_checking(&r, id, false);
                }
            }
        });
        Ok(true)
    }
}

/// What a reading of a blocked card is for.
#[derive(Clone, Copy)]
enum Reading {
    /// Its questions are answered: can the work go on?
    Recheck,
    /// Its agent stopped without asking: what would get it going?
    Triage,
}

/// Times a card whose agent got stuck is run again by itself before it waits for you.
const STUCK_RETRIES: u32 = 1;
