//! What `pit clean` deleted, kept 30 days so it can be undone. The log lives in
//! `~/.pitlane/trash.jsonl`; a ref under `refs/pitlane/trash/` keeps each commit safe from gc.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::Repo;
use crate::time::now_secs;

pub const KEEP_DAYS: u64 = 30;
const KEEP_SECS: u64 = KEEP_DAYS * 86_400;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Main worktree of the repository.
    pub repo: PathBuf,
    pub branch: String,
    pub sha: String,
    /// Set when the remote branch was deleted too.
    pub remote: Option<String>,
    pub remote_sha: Option<String>,
    pub worktree: Option<PathBuf>,
    /// A local branch was deleted (false for lanes that lived only on the remote).
    #[serde(default = "yes")]
    pub had_local: bool,
    pub deleted_at: u64,
    /// Refs that keep `sha` and `remote_sha` alive until the entry expires.
    pub keep_refs: Vec<String>,
}

fn yes() -> bool {
    true
}

impl Entry {
    pub fn expired(&self, now: u64) -> bool {
        now.saturating_sub(self.deleted_at) > KEEP_SECS
    }
}

fn log_path(pitlane_home: &Path) -> PathBuf {
    pitlane_home.join("trash.jsonl")
}

/// Held while the log is appended to or rewritten, across processes: a prune that read the log
/// before an append would otherwise write it back without that entry.
fn lock(pitlane_home: &Path) -> Result<crate::fsutil::FileLock> {
    crate::fsutil::FileLock::take(&pitlane_home.join(".trash.lock"), std::time::Duration::from_secs(10))
}

pub fn load(pitlane_home: &Path) -> Result<Vec<Entry>> {
    let text = crate::fsutil::read_optional(&log_path(pitlane_home))?.unwrap_or_default();
    // A damaged line is skipped rather than blocking every clean.
    Ok(text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
}

/// Pins the commits with refs and appends the entry. Call before deleting anything.
pub fn record(repo: &Repo, mut entry: Entry) -> Result<Entry> {
    let ts = entry.deleted_at;
    let pin = |kind: &str, sha: &str| -> Result<String> {
        let name = format!("refs/pitlane/{kind}/{ts}/{}", entry.branch);
        repo.git.run(&["update-ref", &name, sha])?;
        Ok(name)
    };
    let mut refs = vec![pin("trash", &entry.sha)?];
    if let Some(rs) = entry.remote_sha.as_deref().filter(|rs| *rs != entry.sha) {
        refs.push(pin("trash-remote", rs)?);
    }
    entry.keep_refs = refs;

    let _lock = lock(&repo.pitlane_home)?;
    let path = log_path(&repo.pitlane_home);
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("could not open {}", path.display()))?;
    writeln!(f, "{}", serde_json::to_string(&entry)?)?;
    Ok(entry)
}

/// Latest unexpired entry for this branch in this repo.
pub fn latest(repo: &Repo, branch: &str) -> Result<Option<Entry>> {
    let now = now_secs();
    Ok(load(&repo.pitlane_home)?
        .into_iter()
        .filter(|e| e.repo == repo.root && e.branch == branch && !e.expired(now))
        .max_by_key(|e| e.deleted_at))
}

/// Unexpired entries for this repo, newest first.
pub fn list(repo: &Repo) -> Result<Vec<Entry>> {
    let now = now_secs();
    let mut v: Vec<Entry> = load(&repo.pitlane_home)?
        .into_iter()
        .filter(|e| e.repo == repo.root && !e.expired(now))
        .collect();
    v.sort_by_key(|e| std::cmp::Reverse(e.deleted_at));
    Ok(v)
}

/// Drops expired entries of this repo and their pinning refs. Lines it cannot read (damaged, or
/// written by a newer pit) are kept as they are.
pub fn prune(repo: &Repo) -> Result<()> {
    let now = now_secs();
    let expired = |e: &Entry| e.repo == repo.root && e.expired(now);
    if !load(&repo.pitlane_home)?.iter().any(expired) {
        return Ok(());
    }
    let _lock = lock(&repo.pitlane_home)?;
    let path = log_path(&repo.pitlane_home);
    let text = crate::fsutil::read_optional(&path)?.unwrap_or_default();
    let mut keep = String::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<Entry>(line) {
            Ok(e) if expired(&e) => {
                for r in &e.keep_refs {
                    let _ = repo.git.run(&["update-ref", "-d", r]);
                }
            }
            _ => {
                keep.push_str(line);
                keep.push('\n');
            }
        }
    }
    crate::fsutil::write_atomic(&path, keep)
}
