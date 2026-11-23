//! A run asking you a call while it works (Presence). Here, the question goes on the card (so the
//! Inbox, `pit` and the board's tools answer it like any other) and the run waits for the answer;
//! Away, or with no answer in time, it goes with its recommendation, written on the card as taken
//! for you. A call costly to undo is never taken for you: the run finishes the rest and stops on it.

use std::time::{Duration, Instant};

use pit_core::Repo;
use pit_core::cards::{self, NewQuestion};
use pit_core::runs::RunEvent;
use std::sync::atomic::Ordering;

use super::{Supervisor, home_dir};
use crate::presence::Mode;

const COSTLY: &str = "Do not take this call yourself: finish everything that does not depend on it, then end with \
status \"blocked\".";

impl Supervisor {
    /// What the run's `ask` tool answers: the person's answer, or what to do without one.
    pub async fn consult(&self, run: &str, mut q: NewQuestion, costly: bool) -> String {
        let (root, card) = {
            let live = self.live.lock().unwrap();
            match live.get(run) {
                Some(l) => (l.meta.repo.clone(), l.meta.card),
                None => return "This run is not live any more.".into(),
            }
        };
        let repo = match Repo::open(&root, &home_dir(), &self.cfg.home) {
            Ok(r) => r,
            Err(e) => return format!("Pitlane could not open the repository: {e:#}. Go with your recommendation and note it."),
        };
        q.options.retain(|o| !o.trim().is_empty());
        if q.options.len() < 2 {
            return "Give two to four options and the one you recommend.".into();
        }
        let rec = q.recommended.filter(|r| *r < q.options.len()).unwrap_or(0);
        q.recommended = Some(rec);
        let recommended = q.options[rec].trim().to_owned();
        let cfg = self.presence.config();
        let asked = cards::get(&repo, card)
            .map(|c| c.questions.iter().filter(|x| x.run.as_deref() == Some(run)).count() as u32)
            .unwrap_or(0);
        let here = self.presence.effective(self.perf.config().away_minutes) == Mode::Here;

        if !here || asked >= cfg.per_run {
            let why = if here { format!("You already asked the person {asked} times in this run") } else { "The person is away".into() };
            if costly {
                self.emit(run, RunEvent::Note { text: format!("Left for you: {} ({})", q.text.trim(), why.to_lowercase()) });
                return format!("{why}. {COSTLY} Put this question in the handoff, with its options and your recommendation.");
            }
            return match self.take_for_you(&repo, card, q, run, &recommended) {
                Ok(()) => format!("{why}: go with your recommendation, \"{recommended}\". It is written on the card as taken for them; no note needed."),
                Err(e) => format!("{why}: go with your recommendation, \"{recommended}\", and note it (Pitlane could not write it: {e:#})."),
            };
        }

        let text = q.text.trim().to_owned();
        let qid = match cards::ask_live(&repo, card, q, run) {
            Ok(id) => id,
            Err(e) => return format!("Pitlane could not ask: {e:#}. Go with your recommendation, \"{recommended}\", and note it."),
        };
        self.update(run, |m| m.asking.push(qid));
        self.emit(run, RunEvent::Note { text: format!("Asked you: {text}") });
        let deadline = Instant::now() + Duration::from_secs(u64::from(cfg.wait_minutes) * 60);
        let poll = Duration::from_millis(self.consult_poll_ms.load(Ordering::Relaxed));
        let reply = loop {
            let answered = cards::get(&repo, card).ok().and_then(|c| c.questions.into_iter().find(|x| x.id == qid)).and_then(|x| x.answer.map(|a| (a, x.auto)));
            if let Some((a, auto)) = answered {
                if auto {
                    break format!("Nobody answered: go with your recommendation, \"{a}\".");
                }
                self.emit(run, RunEvent::Note { text: format!("You answered: {a}") });
                break format!("The person answered: \"{a}\". Go on with that.");
            }
            if !self.live.lock().unwrap().contains_key(run) || self.stop_requested(run) {
                break "The run is stopping.".into();
            }
            let gone = self.presence.effective(self.perf.config().away_minutes) == Mode::Away;
            if gone || Instant::now() >= deadline {
                let why = if gone { "The person went away" } else { "No answer in time" };
                if costly {
                    self.emit(run, RunEvent::Note { text: format!("{why}; the question stays open for you: {text}") });
                    break format!("{why}. {COSTLY} The question stays open on the card for the person; do not repeat it in the handoff.");
                }
                match cards::answer_as(&repo, card, qid, &recommended, true) {
                    Ok(_) => {
                        self.emit(run, RunEvent::Note { text: format!("{why}; went with the recommendation: {recommended}") });
                        break format!("{why}: go with your recommendation, \"{recommended}\". It is written on the card as taken for them; no note needed.");
                    }
                    // Answered meanwhile: the next look reads it.
                    Err(_) => continue,
                }
            }
            tokio::time::sleep(poll).await;
        };
        self.update(run, |m| m.asking.retain(|x| *x != qid));
        reply
    }

    /// A question answered with the recommendation at once: on the card, for the review.
    fn take_for_you(&self, repo: &Repo, card: u32, q: NewQuestion, run: &str, recommended: &str) -> anyhow::Result<()> {
        let text = q.text.trim().to_owned();
        let qid = cards::ask_live(repo, card, q, run)?;
        cards::answer_as(repo, card, qid, recommended, true)?;
        self.emit(run, RunEvent::Note { text: format!("Took the recommendation for you: {text}: {recommended}") });
        Ok(())
    }

    /// Tests look at the card more often than every two seconds.
    pub fn set_consult_poll(&self, every: Duration) {
        self.consult_poll_ms.store(every.as_millis() as u64, Ordering::Relaxed);
    }
}
