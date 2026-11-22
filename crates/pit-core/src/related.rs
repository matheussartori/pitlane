//! Related repositories: a project whose cards also change code in sibling repositories
//! (`[project] repos` in flow.toml). A run gets the same branch in each, made the same way as
//! in the main repository, and lands there the same way.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::actions::{self, NewOptions};
use crate::Repo;

/// One related repository's side of a run: its branch and the worktree the agent works in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelatedLane {
    /// The repository's folder name (`amity-games`).
    pub name: String,
    /// Its main worktree.
    pub root: PathBuf,
    pub branch: String,
    pub worktree: PathBuf,
}

/// The related repositories that exist, as main-worktree roots, in the order flow.toml lists them.
pub fn roots(r: &Repo) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for rel in &r.flow.project.repos {
        let p = r.root.join(rel);
        let Ok(p) = p.canonicalize() else { continue };
        if p == r.root || !p.join(".git").exists() || out.contains(&p) {
            continue;
        }
        out.push(p);
    }
    out
}

/// Opens a related repository with the same homes as `r`.
pub fn open(r: &Repo, root: &Path) -> Result<Repo> {
    Repo::open(root, &r.home, &r.pitlane_home)
}

/// Makes sure `branch` exists in `rr` with a worktree: the remote's if it has one, else from
/// `from` (when that branch exists there) or the flow's base, freshly fetched.
fn ensure_branch(rr: &Repo, branch: &str, from: Option<&str>) -> Result<PathBuf> {
    if rr.git.rev_parse(&format!("refs/heads/{branch}"))?.is_some() {
        return actions::ensure_worktree(rr, branch);
    }
    let from = from.filter(|f| rr.git.rev_parse(&format!("refs/heads/{f}")).ok().flatten().is_some());
    Ok(actions::new_lane_from(rr, branch, NewOptions { fetch: true, setup: true }, from)?.path)
}

/// The run's branch in every related repository. `into` is Run all's Into branch: it is made in
/// each repository too (from a fresh base) when missing, so the lane can start from and land in
/// it. `direct` works in `into` itself. A repository that fails is reported and skipped.
pub fn prepare(r: &Repo, branch: &str, into: Option<&str>, direct: bool, note: &dyn Fn(String)) -> Vec<RelatedLane> {
    let mut out = Vec::new();
    for root in roots(r) {
        let made = (|| -> Result<RelatedLane> {
            let rr = open(r, &root)?;
            if let Some(t) = into.filter(|t| !rr.flow.flow.is_trunk(t))
                && rr.git.rev_parse(&format!("refs/heads/{t}"))?.is_none()
            {
                note(format!("Creating {t} in {} from {}/{}", rr.name, rr.flow.flow.remote, rr.flow.flow.base));
                ensure_branch(&rr, t, None)?;
            }
            let worktree = if direct { ensure_branch(&rr, branch, None)? } else { ensure_branch(&rr, branch, into)? };
            Ok(RelatedLane { name: rr.name.clone(), root: rr.root.clone(), branch: branch.to_owned(), worktree })
        })();
        match made {
            Ok(l) => {
                note(format!("Also working in {} ({})", l.worktree.display(), l.name));
                out.push(l);
            }
            Err(e) => note(format!("Could not prepare {}: {e:#}", root.display())),
        }
    }
    out
}

/// What the briefing says about the related repositories.
pub fn briefing(lanes: &[RelatedLane]) -> String {
    if lanes.is_empty() {
        return String::new();
    }
    let mut b = String::from(
        "\n## Related repositories\n\nThis project's code also lives in these repositories. Each has a worktree on this card's \
         branch, made for this run: edit, test and commit there exactly as here (`git -C <path> …`). Never make worktrees \
         or branches of your own, in any repository.\n\n",
    );
    for l in lanes {
        b.push_str(&format!("- {}: `{}` (branch {})\n", l.name, l.worktree.display(), l.branch));
    }
    b
}

/// Sibling git repositories a text mentions (`../amity-games`, or a sibling folder's name), that
/// are not configured yet. Paths come back relative to `r`'s root, as flow.toml keeps them.
pub fn mentioned(r: &Repo, text: &str) -> Vec<String> {
    let Some(parent) = r.root.parent() else { return Vec::new() };
    let known = roots(r);
    let Ok(entries) = std::fs::read_dir(parent) else { return Vec::new() };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p == r.root || !p.join(".git").exists() || known.iter().any(|k| k == &p) {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let named = text.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '.')).any(|w| w == name);
        if named {
            out.push(format!("../{name}"));
        }
    }
    out.sort();
    out
}

/// Takes a related repository off flow.toml.
pub fn remove(r: &Repo, rel: &str) -> Result<()> {
    let at = if r.cwd_root.join(crate::flow::FLOW_PATH).exists() { &r.cwd_root } else { &r.root };
    crate::flow::remove_project_repo(at, rel)
}

/// Records related repositories in flow.toml.
pub fn add(r: &Repo, rels: &[String]) -> Result<()> {
    // Where this repository's flow.toml is read from: the worktree's own, else the main one's.
    let at = if r.cwd_root.join(crate::flow::FLOW_PATH).exists() { &r.cwd_root } else { &r.root };
    for rel in rels {
        crate::flow::add_project_repo(at, rel).with_context(|| format!("could not add {rel}"))?;
    }
    Ok(())
}
