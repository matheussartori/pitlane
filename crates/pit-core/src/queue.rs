//! The merge queue: lanes ship into integration or production one at a time, each against a freshly
//! fetched target, so two merges never race and a conflict stops only its own lane.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::Repo;
use crate::actions;
use crate::cards::{self, CardPatch, CardState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobState {
    Queued,
    Shipping,
    Done,
    /// Merging would conflict; the lane waits for a person.
    Conflict,
    Failed,
    Removed,
}

impl JobState {
    pub fn is_open(self) -> bool {
        matches!(self, Self::Queued | Self::Shipping)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShipJob {
    pub id: String,
    pub repo: PathBuf,
    pub branch: String,
    /// `dev` or `main` (the flow's integration or production branch).
    pub target: String,
    pub state: JobState,
    pub queued_at: u64,
    #[serde(default)]
    pub finished_at: Option<u64>,
    #[serde(default)]
    pub merge_sha: Option<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
    /// The card worked on this lane, moved along when the lane lands.
    #[serde(default)]
    pub card: Option<u32>,
    /// For a related repository's lane: the repository whose board holds `card`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_repo: Option<PathBuf>,
}

/// What shipping one job came to.
pub enum Outcome {
    Shipped { merge_sha: Option<String> },
    Conflict { files: Vec<String> },
}

/// Ships one lane: fetch, plan against the fresh target, merge and push. A push the remote
/// refuses (someone got there first) is planned again once against the new tip.
pub fn ship(repo_path: &Path, home: &Path, pitlane_home: &Path, branch: &str, target: &str) -> Result<Outcome> {
    let repo = Repo::open(repo_path, home, pitlane_home)?;
    // Into a lane (Run all into a feature branch): a local merge in its worktree, no push.
    if !repo.flow.flow.is_trunk(target) {
        return Ok(match actions::land_local(&repo, branch, target)? {
            Ok(sha) => Outcome::Shipped { merge_sha: Some(sha) },
            Err(files) => Outcome::Conflict { files },
        });
    }
    let mut attempt = 0;
    loop {
        attempt += 1;
        let plan = actions::plan_ship(&repo, branch, Some(target), true)?;
        if plan.already.is_some() {
            return Ok(Outcome::Shipped { merge_sha: None });
        }
        if !plan.conflicts.is_empty() {
            return Ok(Outcome::Conflict { files: plan.conflicts });
        }
        match actions::execute_ship(&repo, &plan) {
            Ok(r) => return Ok(Outcome::Shipped { merge_sha: Some(r.merge_sha) }),
            Err(e) if attempt == 1 && e.downcast_ref::<actions::PushRejected>().is_some() => continue,
            Err(e) => return Err(e),
        }
    }
}

/// The card on this lane follows it: into integration → in-dev, into production → done.
pub fn move_card(repo_path: &Path, home: &Path, pitlane_home: &Path, branch: &str, target: &str) -> Result<Option<u32>> {
    let repo = Repo::open(repo_path, home, pitlane_home)?;
    let (all, _) = cards::list(&repo)?;
    // Cards worked directly in this branch already landed; they do not move with it.
    let Some(card) = all.into_iter().find(|c| c.lane.as_deref() == Some(branch) && !c.direct()) else { return Ok(None) };
    let flow = &repo.flow.flow;
    let state = if target == flow.production || !flow.is_trunk(target) { CardState::Done } else { CardState::InDev };
    cards::update(&repo, card.id, CardPatch { state: Some(state), ..Default::default() })?;
    Ok(Some(card.id))
}

/// The queue pauses when Pitlane closes with work in it; nothing ships until a person resumes it.
pub fn paused(pitlane_home: &Path) -> bool {
    pitlane_home.join("queue.paused").exists()
}

pub fn set_paused(pitlane_home: &Path, on: bool) -> std::io::Result<()> {
    let f = pitlane_home.join("queue.paused");
    if on { std::fs::write(f, b"") } else { std::fs::remove_file(f).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }) }
}

/// `~/.pitlane/queue.json`: the queue survives a daemon restart (a job left shipping goes back to queued).
/// No file is an empty queue; one that cannot be read is an error, so nobody saves over it.
pub fn load(pitlane_home: &Path) -> Result<Vec<ShipJob>> {
    let path = pitlane_home.join("queue.json");
    let Some(text) = crate::fsutil::read_optional(&path)? else { return Ok(Vec::new()) };
    let jobs = serde_json::from_str::<Vec<ShipJob>>(&text).with_context(|| format!("{} is not a queue", path.display()))?;
    Ok(jobs
        .into_iter()
        .map(|mut j| {
            if j.state == JobState::Shipping {
                j.state = JobState::Queued;
            }
            j
        })
        .collect())
}

/// The daemon's start: the saved queue, or, when it cannot be read, an empty one with the old
/// file kept aside (`queue.json.unreadable-<secs>`) so the next save does not lose it.
pub fn load_or_set_aside(pitlane_home: &Path) -> (Vec<ShipJob>, Option<String>) {
    match load(pitlane_home) {
        Ok(jobs) => (jobs, None),
        Err(e) => {
            let path = pitlane_home.join("queue.json");
            let aside = pitlane_home.join(format!("queue.json.unreadable-{}", crate::time::now_secs()));
            let moved = std::fs::rename(&path, &aside);
            let note = match moved {
                Ok(()) => format!("{e:#}; kept it as {} and started an empty queue", aside.display()),
                Err(m) => format!("{e:#}; could not set it aside ({m}), started an empty queue"),
            };
            (Vec::new(), Some(note))
        }
    }
}

pub fn save(pitlane_home: &Path, jobs: &[ShipJob]) -> Result<()> {
    crate::fsutil::write_atomic(&pitlane_home.join("queue.json"), serde_json::to_string_pretty(jobs)?).context("could not save the queue")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreadable_queue_is_set_aside_not_lost() {
        let d = tempfile::tempdir().unwrap();
        assert!(load(d.path()).unwrap().is_empty(), "no file is an empty queue");
        std::fs::write(d.path().join("queue.json"), "[{ half").unwrap();
        assert!(load(d.path()).is_err());
        let (jobs, note) = load_or_set_aside(d.path());
        assert!(jobs.is_empty() && note.is_some());
        let aside: Vec<_> = std::fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert_eq!(aside.len(), 1);
        assert!(aside[0].starts_with("queue.json.unreadable-"));
        assert_eq!(std::fs::read_to_string(d.path().join(&aside[0])).unwrap(), "[{ half");
    }
}
