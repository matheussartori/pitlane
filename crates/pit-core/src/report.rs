//! The report of a Run all: what each card came to (state, lane, where it landed, what the agent
//! said, the judge's verdict, files changed, cost) and what the closing step did. Saved as JSON in
//! `~/.pitlane/reports/`, never in the repository; the app shows it and exports it as a PDF.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::Repo;
use crate::cards;
use crate::lanes::now_secs;
use crate::runs::{self, RunMeta, SessionKind};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub added: u64,
    pub removed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardReport {
    pub id: u32,
    pub title: String,
    pub state: String,
    pub lane: Option<String>,
    pub target: Option<String>,
    /// The last thing the working agent said it did (its handoff summary).
    pub summary: Option<String>,
    pub verdict: Option<String>,
    pub runs: usize,
    pub cost_usd: f64,
    pub duration_ms: u64,
    pub files: Vec<FileChange>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Finish {
    pub run: Option<String>,
    pub ok: bool,
    pub summary: Option<String>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub id: String,
    pub repo: PathBuf,
    pub repo_name: String,
    pub started_at: u64,
    pub ended_at: u64,
    pub into: Option<String>,
    pub direct: bool,
    pub instructions: String,
    pub cards: Vec<CardReport>,
    /// Every session of the batch: analyses, runs, judges and the closing step.
    pub sessions: usize,
    pub cost_usd: f64,
    pub finish: Option<Finish>,
}

pub fn dir(pitlane_home: &Path) -> PathBuf {
    pitlane_home.join("reports")
}

fn all_sessions(pitlane_home: &Path, repo: &Path, since: u64) -> Vec<RunMeta> {
    std::fs::read_dir(runs::runs_dir(pitlane_home))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("meta.json")).ok())
        .filter_map(|t| serde_json::from_str::<RunMeta>(&t).ok())
        .filter(|m| m.repo == repo && m.started_at >= since)
        .collect()
}

fn files_of(repo: &Repo, lane: &str, base: Option<&str>) -> Vec<FileChange> {
    let base = base
        .map(str::to_owned)
        .or_else(|| repo.git.run(&["config", "--get", &format!("branch.{lane}.pitBase")]).ok().map(|s| s.trim().to_owned()))
        .filter(|b| !b.is_empty());
    let Some(base) = base else { return Vec::new() };
    repo.git
        .run(&["diff", "--numstat", &format!("{base}..{lane}")])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, '\t');
            let (a, r, path) = (p.next()?, p.next()?, p.next()?);
            Some(FileChange { path: path.to_owned(), added: a.parse().unwrap_or(0), removed: r.parse().unwrap_or(0) })
        })
        .collect()
}

/// Builds and saves the report of a batch that started at `since` (seconds).
#[allow(clippy::too_many_arguments)]
pub fn create(repo: &Repo, pitlane_home: &Path, ids: &[u32], since: u64, into: Option<String>, direct: bool, instructions: &str, finish: Option<Finish>) -> Result<Report> {
    let sessions = all_sessions(pitlane_home, &repo.root, since);
    let mut out = Vec::new();
    for &id in ids {
        let Ok(c) = cards::get(repo, id) else { continue };
        let mut runs: Vec<&RunMeta> = sessions.iter().filter(|m| m.card == id && m.kind == SessionKind::Run).collect();
        runs.sort_by_key(|m| m.started_at);
        let summary = runs.iter().rev().find_map(|m| m.handoff.as_ref()?.get("summary")?.as_str().map(str::to_owned));
        let verdict = runs.iter().rev().find_map(|m| m.verdict).map(|v| format!("{v:?}").to_lowercase());
        let base = runs.first().and_then(|m| m.base_sha.as_deref());
        let files = match &c.lane {
            // Worked directly: what its runs added on top of where the first one started.
            Some(l) if c.direct() => files_of(repo, l, base),
            Some(l) => files_of(repo, l, None),
            None => Vec::new(),
        };
        out.push(CardReport {
            id,
            title: c.title.clone(),
            state: c.state.as_str().to_owned(),
            lane: c.lane.clone(),
            target: c.target.clone(),
            summary,
            verdict,
            runs: runs.len(),
            cost_usd: runs.iter().filter_map(|m| m.cost_usd).sum(),
            duration_ms: runs.iter().filter_map(|m| m.duration_ms).sum(),
            files,
        });
    }
    let now = now_secs();
    let r = Report {
        id: format!("{now}"),
        repo: repo.root.clone(),
        repo_name: repo.name.clone(),
        started_at: since,
        ended_at: now,
        into,
        direct,
        instructions: instructions.trim().to_owned(),
        cards: out,
        sessions: sessions.len(),
        cost_usd: sessions.iter().filter_map(|m| m.cost_usd).sum(),
        finish,
    };
    let d = dir(pitlane_home);
    std::fs::create_dir_all(&d)?;
    std::fs::write(d.join(format!("{}.json", r.id)), serde_json::to_string_pretty(&r)?)?;
    Ok(r)
}

/// A repository's reports, newest first.
pub fn list(pitlane_home: &Path, repo: &Path) -> Vec<Report> {
    let mut all: Vec<Report> = std::fs::read_dir(dir(pitlane_home))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str::<Report>(&t).ok())
        .filter(|r| r.repo == repo)
        .collect();
    all.sort_by_key(|r| std::cmp::Reverse(r.ended_at));
    all
}

/// The prompt of the closing step: what the person asked for, and what the batch did.
pub fn finish_prompt(repo: &Repo, ids: &[u32], instructions: &str, into: Option<&str>) -> String {
    let mut p = String::from("Every card of this batch has been worked. Now do what the person asked for when it is all done.\n\n## What they asked\n\n");
    p.push_str(instructions.trim());
    p.push_str("\n\n## The batch\n\n");
    if let Some(t) = into {
        p.push_str(&format!("The work was aimed at the branch {t}.\n\n"));
    }
    for &id in ids {
        if let Ok(c) = cards::get(repo, id) {
            p.push_str(&format!("- #{} {} [{}]{}\n", c.id, c.title, c.state.as_str(), c.lane.as_deref().map(|l| format!(", lane {l}")).unwrap_or_default()));
        }
    }
    p.push_str("\nDo only what they asked. If something cannot be done, say why. End with a short summary of what you did.\n");
    p
}
