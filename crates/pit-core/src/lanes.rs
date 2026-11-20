//! The yard: every lane (local branch) with where it has really landed.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::flow::Cleanup;
use crate::git::Git;
use crate::Repo;

/// How we know a lane is already in a target branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Landed {
    /// The tip is an ancestor: a normal merge or fast-forward.
    Ancestor,
    /// Every commit has a patch-equivalent in the target: rebase or cherry-pick.
    PatchId,
    /// Merging the lane would not change the target's tree: squash, or any other way.
    MergeTree,
    /// GitHub says a PR from this exact tip was merged into the target.
    PullRequest,
}

impl Landed {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ancestor => "merged",
            Self::PatchId => "rebased",
            Self::MergeTree => "squashed",
            Self::PullRequest => "PR",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// Carries commits that exist only in the integration branch.
    Contaminated,
    /// In production, worktree clean: safe to delete.
    CanDelete,
    /// In production, but the worktree has uncommitted changes.
    ShippedDirty,
    /// In production and `cleanup = "never"`.
    Shipped,
    /// In integration, not yet in production.
    NeedsProduction,
    /// Not in integration yet.
    InProgress,
    /// No commits of its own yet.
    New,
    /// Clean, not landed anywhere, no commit for `stale_days`.
    Stale,
}

impl Status {
    /// Whether the lane is waiting on the user.
    pub fn needs_you(self) -> bool {
        matches!(self, Self::Contaminated | Self::ShippedDirty | Self::NeedsProduction)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LaneWorktree {
    pub path: PathBuf,
    /// Changed or untracked paths.
    pub dirty: usize,
    pub is_main: bool,
    /// The worktree `pit` was run from.
    pub is_current: bool,
}

/// The lane against its branch of the same name on the remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Sync {
    /// Never pushed: no branch of this name on the remote.
    LocalOnly,
    /// It was pushed (has an upstream) but the remote branch was deleted.
    Gone,
    Synced,
    /// Local commits to push.
    Ahead { ahead: usize },
    /// Remote commits to pull.
    Behind { behind: usize },
    Diverged { ahead: usize, behind: usize },
    /// On the remote only; no local branch.
    RemoteOnly,
}

#[derive(Debug, Clone, Serialize)]
pub struct Lane {
    pub number: usize,
    pub branch: String,
    /// Tip: the local branch, or the remote one for remote-only lanes.
    pub sha: String,
    pub subject: String,
    /// There is a local branch.
    pub local: bool,
    /// `<remote>/<branch>` tip, if it exists.
    pub remote_sha: Option<String>,
    pub sync: Sync,
    /// When this machine last pushed the branch (unix seconds), from the remote-tracking reflog.
    pub last_push: Option<u64>,
    pub upstream: Option<String>,
    /// The upstream was deleted on the remote.
    pub upstream_gone: bool,
    /// The upstream is a branch with another name: `git pull` brings that branch's work.
    pub tracks_other: Option<String>,
    /// Commits on no remote branch at all: lost if the machine is.
    pub unpushed: usize,
    /// Commits not in production.
    pub ahead: usize,
    pub worktree: Option<LaneWorktree>,
    pub in_integration: Option<Landed>,
    pub in_production: Option<Landed>,
    /// Integration-only commits this lane carries (short shas).
    pub integration_commits: Vec<String>,
    pub idle_days: u64,
    pub status: Status,
}

#[derive(Debug, Clone, Serialize)]
pub struct Commit {
    pub sha: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    pub days_ago: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Yard {
    pub repo: String,
    pub production: String,
    pub integration: Option<String>,
    pub fetched: bool,
    pub github: bool,
    pub lanes: Vec<Lane>,
    /// Direct commits on integration that no branch brings and production lacks.
    /// The flow will never take them to production.
    pub only_in_integration: Vec<Commit>,
}

#[derive(Debug, Clone, Copy)]
pub struct YardOptions {
    pub fetch: bool,
    /// Ask GitHub (`gh`) about merged PRs.
    pub github: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergedPr {
    pub head_ref_name: String,
    pub head_ref_oid: String,
    pub base_ref_name: String,
}

/// A target branch resolved to its ref, preferring the remote-tracking one.
#[derive(Debug, Clone)]
pub struct Target {
    pub name: String,
    pub refname: String,
    pub sha: String,
    pub tree: String,
}

impl Target {
    pub fn resolve(repo: &Repo, name: &str) -> Result<Option<Self>> {
        let remote = repo.remote_ref(name);
        let local = format!("refs/heads/{name}");
        for refname in [remote, local] {
            if let Some(sha) = repo.git.rev_parse(&refname)? {
                let tree = repo.git.run(&["rev-parse", &format!("{sha}^{{tree}}")])?;
                return Ok(Some(Self { name: name.to_owned(), refname, sha, tree }));
            }
        }
        Ok(None)
    }
}

pub fn yard(repo: &Repo, opts: YardOptions) -> Result<Yard> {
    if opts.fetch {
        repo.fetch()?;
    }
    let flow = &repo.flow.flow;
    let Some(prod) = Target::resolve(repo, &flow.production)? else {
        bail!("production branch {} not found (neither {}/{} nor local)", flow.production, flow.remote, flow.production);
    };
    let integ = match flow.integration() {
        Some(name) => Target::resolve(repo, name)?,
        None => None,
    };
    let prs = if opts.github { merged_prs(repo) } else { None };
    let git = &repo.git;

    // The integration branch's own line (its merge commits and direct commits), minus production.
    let integ_line: HashSet<String> = match &integ {
        Some(i) => git
            .lines(&["rev-list", "--first-parent", &i.sha, &format!("^{}", prod.sha)])?
            .into_iter()
            .collect(),
        None => HashSet::new(),
    };

    let worktrees = repo.worktrees()?;
    let now = now_secs();
    let remote = flow.remote.as_str();
    let remote_prefix = format!("refs/remotes/{remote}/");

    struct Input {
        branch: String,
        sha: String,
        subject: String,
        date: u64,
        local: bool,
        upstream: Option<String>,
        upstream_gone: bool,
    }

    // Remote branches by name, so every local lane finds its counterpart without another call.
    let mut remote_refs: BTreeMap<String, Input> = BTreeMap::new();
    for line in git.lines(&[
        "for-each-ref",
        "--format=%(refname)%00%(objectname)%00%(committerdate:unix)%00%(contents:subject)",
        &format!("refs/remotes/{remote}"),
    ])? {
        let f: Vec<&str> = line.splitn(4, '\0').collect();
        let (Some(name), true) = (f[0].strip_prefix(&remote_prefix), f.len() == 4) else { continue };
        if name == "HEAD" || flow.is_trunk(name) {
            continue;
        }
        remote_refs.insert(
            name.to_owned(),
            Input {
                branch: name.to_owned(),
                sha: f[1].to_owned(),
                subject: f[3].to_owned(),
                date: f[2].parse().unwrap_or(now),
                local: false,
                upstream: None,
                upstream_gone: false,
            },
        );
    }

    let mut inputs: Vec<Input> = Vec::new();
    for line in git.lines(&[
        "for-each-ref",
        "--format=%(refname:short)%00%(objectname)%00%(upstream:short)%00%(upstream:track,nobracket)%00%(committerdate:unix)%00%(contents:subject)",
        "refs/heads",
    ])? {
        let f: Vec<&str> = line.splitn(6, '\0').collect();
        if f.len() < 6 || flow.is_trunk(f[0]) {
            continue;
        }
        inputs.push(Input {
            branch: f[0].to_owned(),
            sha: f[1].to_owned(),
            subject: f[5].to_owned(),
            date: f[4].parse().unwrap_or(now),
            local: true,
            upstream: (!f[2].is_empty()).then(|| f[2].to_owned()),
            upstream_gone: f[3] == "gone",
        });
    }
    let local_names: HashSet<String> = inputs.iter().map(|i| i.branch.clone()).collect();
    let remote_shas: BTreeMap<String, String> =
        remote_refs.iter().map(|(n, r)| (n.clone(), r.sha.clone())).collect();
    inputs.extend(remote_refs.into_values().filter(|r| !local_names.contains(&r.branch)));
    inputs.sort_by(|a, b| a.branch.cmp(&b.branch));

    let mut lanes = Vec::new();
    for inp in inputs {
        let Input { branch, sha, subject, date, local, upstream, upstream_gone } = inp;
        let idle_days = now.saturating_sub(date) / 86_400;
        let same_name = format!("{remote}/{branch}");
        let remote_sha = remote_shas.get(&branch).cloned();
        let tracks_other = upstream.clone().filter(|u| *u != same_name);

        let sync = if !local {
            Sync::RemoteOnly
        } else {
            match &remote_sha {
                Some(rs) => {
                    let ahead = count(git, &[&format!("{rs}..{sha}")])?;
                    let behind = count(git, &[&format!("{sha}..{rs}")])?;
                    match (ahead, behind) {
                        (0, 0) => Sync::Synced,
                        (a, 0) => Sync::Ahead { ahead: a },
                        (0, b) => Sync::Behind { behind: b },
                        (a, b) => Sync::Diverged { ahead: a, behind: b },
                    }
                }
                None if upstream_gone || upstream.as_deref() == Some(same_name.as_str()) => Sync::Gone,
                None => Sync::LocalOnly,
            }
        };
        let last_push = if local && remote_sha.is_some() { last_push(git, &same_name)? } else { None };
        let unpushed = if local { count(git, &[&sha, "--not", "--remotes"])? } else { 0 };
        let ahead = count(git, &[&sha, &format!("^{}", prod.sha)])?;

        let worktree = worktrees
            .iter()
            .filter(|_| local)
            .find(|w| w.branch.as_deref() == Some(branch.as_str()) && !w.prunable)
            .map(|w| -> Result<LaneWorktree> {
                Ok(LaneWorktree {
                    dirty: dirty_count(&git.at(&w.path))?,
                    path: w.path.clone(),
                    is_main: w.is_main,
                    is_current: w.path == repo.cwd_root,
                })
            })
            .transpose()?;

        let is_new = local && ahead == 0 && never_moved(repo, &branch, &sha)?;
        let (in_integration, in_production) = if is_new {
            (None, None)
        } else {
            let i = match &integ {
                Some(t) => landed(git, &branch, &sha, t, prs.as_deref())?,
                None => None,
            };
            (i, landed(git, &branch, &sha, &prod, prs.as_deref())?)
        };

        let integration_commits = if integ_line.is_empty() || in_production.is_some() {
            Vec::new()
        } else {
            contamination(git, &sha, &prod.sha, &integ_line)?
        };

        let dirty = worktree.as_ref().map_or(0, |w| w.dirty);
        let status = if !integration_commits.is_empty() {
            Status::Contaminated
        } else if is_new {
            if dirty > 0 { Status::InProgress } else { Status::New }
        } else if in_production.is_some() {
            if dirty > 0 {
                Status::ShippedDirty
            } else if flow.cleanup == Cleanup::Never {
                Status::Shipped
            } else {
                Status::CanDelete
            }
        } else if in_integration.is_some() {
            Status::NeedsProduction
        } else if dirty == 0 && idle_days >= u64::from(flow.stale_days) {
            Status::Stale
        } else {
            Status::InProgress
        };

        lanes.push(Lane {
            number: lanes.len() + 1,
            branch,
            sha,
            subject,
            local,
            remote_sha,
            sync,
            last_push,
            upstream,
            upstream_gone,
            tracks_other,
            unpushed,
            ahead,
            worktree,
            in_integration,
            in_production,
            integration_commits,
            idle_days,
            status,
        });
    }

    let only_in_integration = match &integ {
        Some(i) => only_in(repo, i, &prod)?,
        None => Vec::new(),
    };

    Ok(Yard {
        repo: repo.name.clone(),
        production: prod.name,
        integration: integ.map(|i| i.name),
        fetched: opts.fetch,
        github: prs.is_some(),
        lanes,
        only_in_integration,
    })
}

/// Is `sha` (the tip of `branch`) already in `target`? Cheapest proof first.
pub fn landed(
    git: &Git,
    branch: &str,
    sha: &str,
    target: &Target,
    prs: Option<&[MergedPr]>,
) -> Result<Option<Landed>> {
    if git.is_ancestor(sha, &target.sha)? {
        return Ok(Some(Landed::Ancestor));
    }
    let cherry = git.lines(&["cherry", &target.sha, sha])?;
    if !cherry.is_empty() && cherry.iter().all(|l| l.starts_with('-')) {
        return Ok(Some(Landed::PatchId));
    }
    if let Some(prs) = prs
        && prs.iter().any(|p| {
            p.head_ref_name == branch && p.head_ref_oid == sha && p.base_ref_name == target.name
        })
    {
        return Ok(Some(Landed::PullRequest));
    }
    let (code, out, _) = git.raw(&["merge-tree", "--write-tree", &target.sha, sha])?;
    if code == 0 && out.lines().next() == Some(target.tree.as_str()) {
        return Ok(Some(Landed::MergeTree));
    }
    Ok(None)
}

/// Integration-only commits reachable from the lane. A lane's own non-merge commits are
/// excluded, so a lane fast-forwarded into integration is not flagged.
pub fn contamination(
    git: &Git,
    sha: &str,
    prod_sha: &str,
    integ_line: &HashSet<String>,
) -> Result<Vec<String>> {
    let not_prod = format!("^{prod_sha}");
    let own: HashSet<String> = git
        .lines(&["rev-list", "--first-parent", "--no-merges", sha, &not_prod])?
        .into_iter()
        .collect();
    Ok(git
        .lines(&["rev-list", sha, &not_prod])?
        .into_iter()
        .filter(|c| integ_line.contains(c) && !own.contains(c))
        .map(|c| c[..c.len().min(8)].to_owned())
        .collect())
}

fn only_in(repo: &Repo, integ: &Target, prod: &Target) -> Result<Vec<Commit>> {
    let git = &repo.git;
    let remote = &repo.flow.flow.remote;
    let exclude_local = format!("--exclude={}", integ.name);
    let exclude_remote = format!("--exclude={remote}/{}", integ.name);
    let candidates = git.lines(&[
        "rev-list",
        "--first-parent",
        "--no-merges",
        &integ.sha,
        &format!("^{}", prod.sha),
        "--not",
        &exclude_local,
        "--branches",
        &exclude_remote,
        "--remotes",
    ])?;
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let unmatched: HashSet<String> = git
        .lines(&[
            "log",
            "--cherry-pick",
            "--right-only",
            "--no-merges",
            "--format=%H",
            &format!("{}...{}", prod.sha, integ.sha),
        ])?
        .into_iter()
        .collect();
    let keep: Vec<&str> = candidates
        .iter()
        .filter(|c| unmatched.contains(*c))
        .map(String::as_str)
        .collect();
    if keep.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["show", "-s", "--no-walk=unsorted", "--format=%H%x00%h%x00%s%x00%an%x00%ct"];
    args.extend(keep);
    let now = now_secs();
    Ok(git
        .lines(&args)?
        .into_iter()
        .filter_map(|l| {
            let f: Vec<&str> = l.splitn(5, '\0').collect();
            (f.len() == 5).then(|| Commit {
                sha: f[0].to_owned(),
                short: f[1].to_owned(),
                subject: f[2].to_owned(),
                author: f[3].to_owned(),
                days_ago: now.saturating_sub(f[4].parse().unwrap_or(now)) / 86_400,
            })
        })
        .collect())
}

/// A branch with no commits beyond production is either brand new or fast-forwarded in.
/// It is new if Pitlane recorded this exact tip as its base, or its reflog never moved.
fn never_moved(repo: &Repo, branch: &str, sha: &str) -> Result<bool> {
    if repo.git.config_get(&format!("branch.{branch}.pitBase"))?.as_deref() == Some(sha) {
        return Ok(true);
    }
    let (code, out, _) = repo.git.raw(&["reflog", "show", "--format=%H", &format!("refs/heads/{branch}")])?;
    Ok(code == 0 && out.lines().filter(|l| !l.is_empty()).count() <= 1)
}

/// Last time this machine pushed `remote/branch`: the newest "update by push" entry in the
/// remote-tracking ref's reflog. `None` if it was only ever fetched here.
fn last_push(git: &Git, remote_branch: &str) -> Result<Option<u64>> {
    let (code, out, _) = git.raw(&[
        "reflog",
        "show",
        "--date=unix",
        "--format=%gd%x00%gs",
        &format!("refs/remotes/{remote_branch}"),
    ])?;
    if code != 0 {
        return Ok(None);
    }
    Ok(out.lines().find_map(|l| {
        let (selector, subject) = l.split_once('\0')?;
        if !subject.starts_with("update by push") {
            return None;
        }
        let ts = selector.rsplit_once("@{")?.1.strip_suffix('}')?;
        ts.parse().ok()
    }))
}

fn count(git: &Git, revs: &[&str]) -> Result<usize> {
    let mut args = vec!["rev-list", "--count"];
    args.extend_from_slice(revs);
    git.run(&args)?.trim().parse().context("rev-list --count")
}

pub fn dirty_count(git: &Git) -> Result<usize> {
    Ok(git.lines(&["status", "--porcelain=v1", "--untracked-files=normal"])?.len())
}

/// Merged PRs from GitHub, or `None` if `gh` is missing, signed out or offline.
fn merged_prs(repo: &Repo) -> Option<Vec<MergedPr>> {
    let out = Command::new("gh")
        .args(["pr", "list", "--state", "merged", "--limit", "300", "--json", "headRefName,headRefOid,baseRefName"])
        .current_dir(&repo.root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}
