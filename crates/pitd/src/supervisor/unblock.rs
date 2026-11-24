//! Nothing stays blocked for good: each reason has a way out. Questions are answered, then the
//! card is read again (side.rs); an agent that got stuck without asking has what stopped it turned
//! into questions; a failed run and a card with no account free are tried again by themselves; an
//! API key's cost, a conflict and checks that still fail wait for one action from you, which
//! `unblock` carries out the way that fits.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use pit_core::Repo;
use pit_core::cards::{self, BlockReason, CardPatch, CardState, Unblock};
use pit_core::time::now_secs;
use pit_core::runs::RunMeta;

use super::verify::fix_request;
use super::{StartOpts, Supervisor, home_dir};

/// How often blocked cards due for another try are looked for.
const TICK: Duration = Duration::from_secs(60);
/// After a try of its own that could not start, how long before the next.
const STALLED_WAIT_SECS: u64 = 600;

impl Supervisor {
    /// Carries out what you chose for a blocked card (or what Pitlane chose, `auto`): the run it
    /// starts, if it starts one.
    pub fn unblock(self: &Arc<Self>, repo: PathBuf, id: u32, action: Unblock, auto: bool) -> Result<Option<RunMeta>> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        let card = cards::get(&r, id)?;
        let Some(block) = card.blocked.clone().filter(|_| card.state == CardState::Blocked) else {
            bail!("card #{id} is not blocked");
        };
        if block.checking.is_some() {
            bail!("card #{id} is being read again; wait for it, or send it back to the Backlog");
        }
        match action {
            Unblock::Backlog => {
                cards::update(&r, id, CardPatch { state: Some(CardState::Ready), retries: Some(0), ..Default::default() })?;
                return Ok(None);
            }
            // What it did is worth a look as it is: checks that fail, a conflict, an agent that
            // got stuck partway. Not a run that failed.
            Unblock::Review => {
                if !matches!(block.reason, BlockReason::Checks | BlockReason::Conflict | BlockReason::Stuck) {
                    bail!("card #{id} has nothing to review: its run did not get through");
                }
                if card.lane.is_none() {
                    bail!("card #{id} has no lane to review");
                }
                cards::update(&r, id, CardPatch { state: Some(CardState::Review), ..Default::default() })?;
                return Ok(None);
            }
            Unblock::Retry => {}
        }
        if matches!(block.reason, BlockReason::Questions | BlockReason::Check) {
            if card.open_questions() > 0 {
                bail!("card #{id} has {} open question(s); answer them first", card.open_questions());
            }
            self.recheck(repo, id)?;
            return Ok(None);
        }
        // A try of Pitlane's own counts toward its few; one of yours starts the count again.
        let retries = if auto { card.retries + 1 } else { 0 };
        cards::update(&r, id, CardPatch { retries: Some(retries), ..Default::default() })?;
        let last = self.latest_run(&r.root, id);
        let autopilot = last.as_ref().is_some_and(|m| m.autopilot);
        let plain = StartOpts { autopilot, ..Default::default() };
        let started = match (block.reason, &last) {
            (BlockReason::ApiKey, _) => self.start(repo, id, None, StartOpts { allow_api_key: true, ..plain }),
            // Another fix round, on what the checks and the judge said, in the same session.
            (BlockReason::Checks, Some(m)) => self.start(
                repo,
                id,
                Some(m.id.clone()),
                StartOpts { message: Some(format!("Pitlane checked your work and it needs another pass:\n\n{}\n\nFix it, commit, and end with the handoff.", fix_request(&self.dir(&m.id)))), round: m.round + 1, ..plain },
            ),
            // Bring the branch it lands in into the lane, in the same session.
            (BlockReason::Conflict, Some(m)) => {
                let target = card.target.clone().unwrap_or_else(|| r.flow.flow.base.clone());
                let msg = format!(
                    "Your work could not land: {target} changed meanwhile and conflicts with it. Bring it in now: \
                     run `git merge {target}` (fetch it first when it lives on the remote), resolve every conflict keeping both \
                     sides' intent, run the narrowest checks for what you touched, commit the merge, and end with the handoff."
                );
                self.start(repo, id, Some(m.id.clone()), StartOpts { message: Some(msg), round: m.round + 1, ..plain })
            }
            // A failed run, no account free, a stuck agent, or a block by hand: run it again. A
            // paused session picks up where it was; the card's notes say what happened.
            _ => self.start(repo, id, None, plain),
        };
        match started {
            Ok(m) => Ok(Some(m)),
            Err(e) => {
                // Pitlane's own try: not again every minute, and the card says why.
                if auto {
                    let why = format!("{e:#}");
                    let _ = cards::edit_block(&r, id, |b| {
                        b.retry_at = Some(now_secs() + STALLED_WAIT_SECS);
                        b.detail = super::card::short(&why);
                        Ok(())
                    });
                }
                Err(e)
            }
        }
    }

    /// Tries blocked cards again when their time comes, every minute, in every repository the
    /// daemon has run cards in.
    pub fn start_unblocker(self: &Arc<Self>) {
        let me = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(TICK).await;
                let me = Arc::clone(&me);
                let _ = tokio::task::spawn_blocking(move || me.retry_due()).await;
            }
        });
    }

    fn retry_due(self: &Arc<Self>) {
        let Ok(runs) = self.sessions(None) else { return };
        let mut repos: Vec<PathBuf> = runs.into_iter().map(|m| m.repo).collect();
        repos.sort();
        repos.dedup();
        let now = now_secs();
        for repo in repos {
            let Ok(r) = Repo::open(&repo, &home_dir(), &self.cfg.home) else { continue };
            for c in cards::due(&r, now).unwrap_or_default() {
                let reason = c.blocked.as_ref().map(|b| b.reason);
                match self.unblock(r.root.clone(), c.id, Unblock::Retry, true) {
                    Ok(Some(m)) => {
                        let _ = cards::note(&r, c.id, "pitlane", &format!("Tried again by itself ({}): run {}.", why_word(reason), m.id));
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("pitd: card #{} in {}: {e:#}", c.id, repo.display()),
                }
            }
        }
    }

    /// After a run's card is settled: an agent that got stuck without asking has what stopped it
    /// turned into questions.
    pub(super) fn after_settle(self: &Arc<Self>, repo: &Path, card: u32) {
        if let Err(e) = self.triage(repo, card) {
            eprintln!("pitd: card #{card}: {e:#}");
        }
    }
}

fn why_word(reason: Option<BlockReason>) -> &'static str {
    match reason {
        Some(BlockReason::Failed) => "the last run failed",
        Some(BlockReason::NoRoute) => "an account is free again",
        Some(BlockReason::Stuck) => "the agent got stuck",
        _ => "it was due",
    }
}
