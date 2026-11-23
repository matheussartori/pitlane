//! Reviews and the merge queue: lands approved work one job at a time.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use pit_core::Repo;
use pit_core::actions;
use pit_core::cards::{self, CardPatch, CardState};
use pit_core::time::now_secs;

use super::{ReviewAction, StartOpts, Supervisor, home_dir};

impl Supervisor {
    /// A person's decision on a card in review. Approve ships the lane one stage (into
    /// integration); changes resumes the run's session with the comments; discard returns the
    /// card to ready with its lane intact.
    pub async fn review(
        self: &Arc<Self>,
        repo: PathBuf,
        card: u32,
        action: ReviewAction,
        message: Option<String>,
    ) -> Result<serde_json::Value> {
        let home = self.cfg.home.clone();
        let r = Repo::open(&repo, &home_dir(), &home)?;
        let c = cards::get(&r, card)?;
        if c.state != CardState::Review {
            bail!("card #{card} is not in review (it is {})", c.state.as_str());
        }
        let last = self.latest_run(&r.root, card);
        match action {
            ReviewAction::Discard => {
                cards::update(
                    &r,
                    card,
                    CardPatch {
                        state: Some(CardState::Ready),
                        ..Default::default()
                    },
                )?;
                Ok(serde_json::json!({ "card": card, "state": "ready" }))
            }
            ReviewAction::Changes => {
                let text = message
                    .filter(|m| !m.trim().is_empty())
                    .context("say what to change")?;
                let prev = last.context("this card has no run to continue")?;
                let round = prev.round + 1;
                if round > r.flow.verify.max_rounds {
                    bail!(
                        "card #{card} already went through {} rounds of changes; take it from here yourself or discard it",
                        r.flow.verify.max_rounds
                    );
                }
                let first = format!(
                    "A person reviewed your work and asks for changes:\n\n{}\n\nMake them, commit, and end with the handoff.",
                    text.trim()
                );
                let next = self.start(
                    repo,
                    card,
                    Some(prev.id),
                    StartOpts {
                        message: Some(first),
                        round,
                        ..Default::default()
                    },
                )?;
                Ok(serde_json::json!({ "run": next }))
            }
            ReviewAction::Approve => {
                // Approved work joins the merge queue; the card moves when its lane lands.
                let branch = c.lane.clone().context("this card has no lane to ship")?;
                // Worked directly in its target: the work is already there.
                if c.direct() {
                    let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
                    cards::update(
                        &r,
                        card,
                        CardPatch {
                            state: Some(CardState::Done),
                            ..Default::default()
                        },
                    )?;
                    return Ok(serde_json::json!({ "card": card, "landed_in": branch }));
                }
                // The related repositories' lanes go first; the card moves when this one lands.
                let related = last.as_ref().map(|l| l.related.clone()).unwrap_or_default();
                self.land_card(&repo, &c, &related)?;
                let job = self
                    .queue_list(Some(&r.root))
                    .into_iter()
                    .find(|j| j.repo == r.root && j.branch == branch && j.state.is_open());
                let to = job.as_ref().map(|j| j.target.clone()).or_else(|| c.target.clone());
                Ok(serde_json::json!({ "card": card, "job": job, "shipped_to": to }))
            }
        }
    }

    // ------------------------------------------------------------ merge queue

    /// Puts a lane in line to ship into `to` (default: its next stage). One open job per lane.
    pub fn enqueue(
        &self,
        repo: &Path,
        branch: &str,
        to: Option<&str>,
    ) -> Result<pit_core::queue::ShipJob> {
        let r = Repo::open(repo, &home_dir(), &self.cfg.home)?;
        if !r.flow.flow.is_trunk(branch) {
            // The same lane in the project's related repositories goes with it, ahead of it, so a
            // stage (dev, main) always gets the whole change and never half of it.
            let target = match to {
                Some(t) => t.to_owned(),
                None => actions::plan_ship(&r, branch, None, false)?.target,
            };
            let mut with: Vec<Repo> = Vec::new();
            for root in pit_core::related::roots(&r) {
                let Ok(rr) = pit_core::related::open(&r, &root) else { continue };
                if rr.git.rev_parse(&format!("refs/heads/{branch}"))?.is_none() || self.in_line(&rr.root, branch) {
                    continue;
                }
                let into = if rr.remote_sha(&target)?.is_some() {
                    rr.remote_ref(&target)
                } else if rr.git.rev_parse(&format!("refs/heads/{target}"))?.is_some() {
                    target.clone()
                } else {
                    bail!("{} has {branch} but no {target}: create {target} there first, so both repositories ship together", rr.name);
                };
                let ahead = rr.git.run(&["rev-list", "--count", &format!("{into}..{branch}")])?;
                if ahead.trim() != "0" {
                    with.push(rr);
                }
            }
            for rr in with {
                self.enqueue_for(&rr.root, branch, Some(&target), None)?;
            }
        }
        self.enqueue_for(repo, branch, to, None)
    }

    /// A card's work, here and in its related repositories, in line to land in its Into branch:
    /// the related lanes first (those with commits of their own), this repository's last, so the
    /// card moves once all of it is in.
    pub(super) fn land_card(
        &self,
        repo: &Path,
        c: &cards::Card,
        related: &[pit_core::related::RelatedLane],
    ) -> Result<()> {
        let lane = c.lane.clone().context("this card has no lane to ship")?;
        let mut seen: Vec<PathBuf> = Vec::new();
        for l in related {
            let Ok(rr) = Repo::open(&l.root, &home_dir(), &self.cfg.home) else {
                continue;
            };
            // A repository listed twice (a worktree of it next to it) lands once.
            if seen.contains(&rr.root) {
                continue;
            }
            seen.push(rr.root.clone());
            let target = c
                .target
                .clone()
                .unwrap_or_else(|| rr.flow.flow.integration.clone());
            let ahead = rr
                .git
                .run(&["rev-list", "--count", &format!("{target}..{}", l.branch)])
                .ok()
                .and_then(|n| n.trim().parse::<u32>().ok());
            if ahead == Some(0) || self.in_line(&rr.root, &l.branch) {
                continue;
            }
            self.enqueue_for(
                &l.root,
                &l.branch,
                c.target.as_deref(),
                Some((c.id, repo.to_path_buf())),
            )?;
        }
        // This repository's lane last, even with no commits of its own: its landing moves the card.
        let r = Repo::open(repo, &home_dir(), &self.cfg.home)?;
        if !self.in_line(&r.root, &lane) {
            self.enqueue(repo, &lane, c.target.as_deref())?;
        }
        Ok(())
    }

    /// Whether `branch` of `root` already has an open job in the merge queue.
    fn in_line(&self, root: &Path, branch: &str) -> bool {
        self.queue
            .lock()
            .unwrap()
            .iter()
            .any(|j| j.repo == root && j.branch == branch && j.state.is_open())
    }

    /// `card`: the card and the repository whose board holds it, for a related repository's lane.
    pub fn enqueue_for(
        &self,
        repo: &Path,
        branch: &str,
        to: Option<&str>,
        card: Option<(u32, PathBuf)>,
    ) -> Result<pit_core::queue::ShipJob> {
        use pit_core::queue::{JobState, ShipJob};
        let r = Repo::open(repo, &home_dir(), &self.cfg.home)?;
        if r.flow.flow.is_trunk(branch) {
            bail!("{branch} is a trunk branch; only lanes ship");
        }
        // The target is decided now, against what is known locally; the worker fetches before merging.
        let target = match to {
            Some(t) => t.to_owned(),
            None => actions::plan_ship(&r, branch, None, false)?.target,
        };
        let (card, card_repo) = match card {
            Some((id, board)) => (Some(id), Some(board)),
            None => (
                cards::list(&r).ok().and_then(|(all, _)| {
                    all.into_iter()
                        .find(|c| c.lane.as_deref() == Some(branch))
                        .map(|c| c.id)
                }),
                None,
            ),
        };
        let job = {
            let mut q = self.queue.lock().unwrap();
            if let Some(open) = q
                .iter()
                .find(|j| j.repo == r.root && j.branch == branch && j.state.is_open())
            {
                bail!(
                    "{branch} is already in line for {} ({})",
                    open.target,
                    open.id
                );
            }
            let job = ShipJob {
                id: format!(
                    "q{}-{}",
                    now_secs(),
                    &uuid::Uuid::new_v4().simple().to_string()[..4]
                ),
                repo: r.root.clone(),
                branch: branch.to_owned(),
                target,
                state: JobState::Queued,
                queued_at: now_secs(),
                finished_at: None,
                merge_sha: None,
                conflicts: Vec::new(),
                error: None,
                card,
                card_repo,
            };
            q.push(job.clone());
            pit_core::queue::save(&self.cfg.home, &q)?;
            job
        };
        self.queue_wake.notify_one();
        Ok(job)
    }

    /// The queue of a repository (or all): open jobs, then the last 20 finished.
    pub fn queue_list(&self, repo: Option<&Path>) -> Vec<pit_core::queue::ShipJob> {
        let q = self.queue.lock().unwrap();
        let mine: Vec<_> = q
            .iter()
            // A card's lanes in its related repositories count as this repository's.
            .filter(|j| repo.is_none_or(|r| j.repo == r || j.card_repo.as_deref() == Some(r)))
            .cloned()
            .collect();
        let mut open: Vec<_> = mine.iter().filter(|j| j.state.is_open()).cloned().collect();
        open.sort_by_key(|j| j.queued_at);
        let mut done: Vec<_> = mine.into_iter().filter(|j| !j.state.is_open()).collect();
        done.sort_by_key(|j| std::cmp::Reverse(j.finished_at.unwrap_or(j.queued_at)));
        open.extend(done.into_iter().take(20));
        open
    }

    /// Takes a job out of line (not one that is shipping right now).
    pub fn resume_queue(&self) -> Result<()> {
        pit_core::queue::set_paused(&self.cfg.home, false)?;
        self.queue_wake.notify_one();
        Ok(())
    }

    pub fn dequeue(&self, id: &str) -> Result<()> {
        use pit_core::queue::JobState;
        let mut q = self.queue.lock().unwrap();
        let j = q
            .iter_mut()
            .find(|j| j.id == id)
            .with_context(|| format!("no job {id}"))?;
        if j.state == JobState::Shipping {
            bail!("{} is shipping right now", j.branch);
        }
        j.state = JobState::Removed;
        j.finished_at = Some(now_secs());
        pit_core::queue::save(&self.cfg.home, &q)
    }

    pub fn start_queue(self: &Arc<Self>) {
        let me = Arc::clone(self);
        tokio::spawn(async move { me.run_queue().await });
    }

    /// Ships queued jobs one at a time, oldest first, forever.
    async fn run_queue(self: Arc<Self>) {
        use pit_core::queue::{JobState, Outcome};
        loop {
            if pit_core::queue::paused(&self.cfg.home) {
                self.queue_wake.notified().await;
                continue;
            }
            let next = {
                let mut q = self.queue.lock().unwrap();
                let pick = q
                    .iter_mut()
                    .filter(|j| j.state == JobState::Queued)
                    .min_by_key(|j| j.queued_at);
                pick.map(|j| {
                    j.state = JobState::Shipping;
                    j.clone()
                })
            };
            let Some(job) = next else {
                self.queue_wake.notified().await;
                continue;
            };
            let _ = pit_core::queue::save(&self.cfg.home, &self.queue.lock().unwrap());
            let (home, ph) = (home_dir(), self.cfg.home.clone());
            let (repo, branch, target) = (job.repo.clone(), job.branch.clone(), job.target.clone());
            let result = tokio::task::spawn_blocking(
                move || -> Result<(pit_core::queue::Outcome, Option<u32>)> {
                    let out = pit_core::queue::ship(&repo, &home, &ph, &branch, &target)?;
                    let card = match &out {
                        Outcome::Shipped { .. } => {
                            pit_core::queue::move_card(&repo, &home, &ph, &branch, &target)?
                        }
                        Outcome::Conflict { .. } => None,
                    };
                    Ok((out, card))
                },
            )
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r);
            if let Ok((Outcome::Conflict { files }, _)) = &result {
                self.conflict_round(&job, files);
            }
            if let Ok((Outcome::Shipped { .. }, Some(card))) = &result {
                self.reopen_unfinished(&job.repo, *card);
            }
            let mut q = self.queue.lock().unwrap();
            if let Some(j) = q.iter_mut().find(|j| j.id == job.id) {
                j.finished_at = Some(now_secs());
                match result {
                    Ok((Outcome::Shipped { merge_sha }, _)) => {
                        j.state = JobState::Done;
                        j.merge_sha = merge_sha;
                    }
                    Ok((Outcome::Conflict { files }, _)) => {
                        j.state = JobState::Conflict;
                        j.conflicts = files;
                    }
                    Err(e) => {
                        j.state = JobState::Failed;
                        j.error = Some(format!("{e:#}"));
                    }
                }
            }
            let _ = pit_core::queue::save(&self.cfg.home, &q);
        }
    }
}
