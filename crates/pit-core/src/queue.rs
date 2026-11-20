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
            Err(e) if attempt == 1 && format!("{e:#}").contains("rejected") => continue,
            Err(e) => return Err(e),
        }
    }
}

/// The card on this lane follows it: into integration → in-dev, into production → done.
pub fn move_card(repo_path: &Path, home: &Path, pitlane_home: &Path, branch: &str, target: &str) -> Result<Option<u32>> {
    let repo = Repo::open(repo_path, home, pitlane_home)?;
    let (all, _) = cards::list(&repo)?;
    let Some(card) = all.into_iter().find(|c| c.lane.as_deref() == Some(branch)) else { return Ok(None) };
    let state = if target == repo.flow.flow.production { CardState::Done } else { CardState::InDev };
    cards::update(&repo, card.id, CardPatch { state: Some(state), ..Default::default() })?;
    Ok(Some(card.id))
}

/// `~/.pitlane/queue.json`: the queue survives a daemon restart (a job left shipping goes back to queued).
pub fn load(pitlane_home: &Path) -> Vec<ShipJob> {
    std::fs::read_to_string(pitlane_home.join("queue.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<ShipJob>>(&t).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|mut j| {
            if j.state == JobState::Shipping {
                j.state = JobState::Queued;
            }
            j
        })
        .collect()
}

pub fn save(pitlane_home: &Path, jobs: &[ShipJob]) -> Result<()> {
    std::fs::create_dir_all(pitlane_home)?;
    let path = pitlane_home.join("queue.json");
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(jobs)?)?;
    std::fs::rename(&tmp, &path).context("could not save the queue")
}
