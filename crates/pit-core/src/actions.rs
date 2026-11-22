//! What `pit` does to a repository: init, new, ship, clean, undo.
//! Each risky action is split into a `plan_*` (read-only) and an `execute_*`, so the
//! caller can show the plan and ask before anything is pushed or deleted.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::flow::{FLOW_PATH, TEMPLATE};
use crate::lanes::{self, Landed, Status, Target, Yard, dirty_count, landed, now_secs};
use crate::trash::{self, Entry};
use crate::{Repo, Worktree};

// ---------------------------------------------------------------- init

#[derive(Debug, Serialize)]
pub struct InitReport {
    /// Path written, or `None` if a flow file already existed.
    pub flow_written: Option<PathBuf>,
    pub rerere_was_on: bool,
}

pub fn init(repo: &Repo, force: bool) -> Result<InitReport> {
    let path = repo.cwd_root.join(FLOW_PATH);
    let flow_written = if path.exists() && !force {
        None
    } else {
        fs::create_dir_all(path.parent().expect("flow path has a parent"))?;
        fs::write(&path, TEMPLATE).with_context(|| format!("could not write {}", path.display()))?;
        Some(path)
    };
    let rerere_was_on = repo.git.config_get("rerere.enabled")?.as_deref() == Some("true");
    repo.git.config_set("rerere.enabled", "true")?;
    Ok(InitReport { flow_written, rerere_was_on })
}

// ---------------------------------------------------------------- new

#[derive(Debug, Clone, Copy)]
pub struct NewOptions {
    pub fetch: bool,
    pub setup: bool,
}

#[derive(Debug, Serialize)]
pub struct NewReport {
    pub branch: String,
    pub path: PathBuf,
    pub base: String,
    pub base_sha: String,
    pub copied: Vec<String>,
    pub missing: Vec<String>,
    pub cloned: Vec<String>,
    /// `Some(false)` if the setup command failed.
    pub setup_ok: Option<bool>,
}

pub fn new_lane(repo: &Repo, branch: &str, opts: NewOptions) -> Result<NewReport> {
    new_lane_from(repo, branch, opts, None)
}

/// A new lane that starts from `from` when that is a lane (Run all into a feature branch: its
/// newest local work). A trunk or nothing means the flow's base, freshly fetched.
pub fn new_lane_from(repo: &Repo, branch: &str, opts: NewOptions, from: Option<&str>) -> Result<NewReport> {
    let flow = &repo.flow.flow;
    let from = from.filter(|f| !flow.is_trunk(f));
    if repo.git.raw(&["check-ref-format", "--branch", branch])?.0 != 0 {
        bail!("{branch:?} is not a valid branch name");
    }
    if flow.is_trunk(branch) {
        bail!("{branch} is a trunk branch of this flow, not a lane");
    }
    if repo.git.rev_parse(&format!("refs/heads/{branch}"))?.is_some() {
        bail!("branch {branch} already exists");
    }
    if opts.fetch {
        repo.fetch()?;
    }
    // A branch that already exists on the remote is checked out tracking it, never restarted from base.
    let existing = repo.remote_sha(branch)?;
    if existing.is_none() && !flow.name_allowed(branch) {
        bail!("{branch} does not match the naming rule {:?} in {FLOW_PATH}", flow.naming);
    }
    let path = repo.flow.worktree.path_for(&repo.home, &repo.name, branch);
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let path_s = path.to_string_lossy();
    let (base_name, base_sha) = match existing {
        Some(sha) => {
            let remote_branch = format!("{}/{branch}", flow.remote);
            repo.git
                .run(&["worktree", "add", "--quiet", "--track", "-b", branch, &path_s, &remote_branch])?;
            (remote_branch, sha)
        }
        None if from.is_some() => {
            let from = from.unwrap_or_default();
            let sha = match repo.git.rev_parse(&format!("refs/heads/{from}"))? {
                Some(s) => s,
                None => repo.git.rev_parse(&repo.remote_ref(from))?.with_context(|| format!("branch {from} not found"))?,
            };
            repo.git.run(&["worktree", "add", "--quiet", "--no-track", "-b", branch, &path_s, &sha])?;
            repo.git.config_set(&format!("branch.{branch}.pitBase"), &sha)?;
            repo.git.config_set(&format!("branch.{branch}.pitFrom"), from)?;
            (from.to_owned(), sha)
        }
        None => {
            let Some(base) = Target::resolve(repo, &flow.base)? else {
                bail!("base branch {} not found", flow.base);
            };
            repo.git
                .run(&["worktree", "add", "--quiet", "--no-track", "-b", branch, &path_s, &base.sha])?;
            repo.git.config_set(&format!("branch.{branch}.pitBase"), &base.sha)?;
            (format!("{}/{}", flow.remote, flow.base), base.sha)
        }
    };

    let mut copied = Vec::new();
    let mut missing = Vec::new();
    for rel in &repo.flow.worktree.copy {
        let src = repo.root.join(rel);
        if !src.exists() {
            missing.push(rel.clone());
            continue;
        }
        let dst = path.join(rel);
        if dst.exists() {
            continue;
        }
        copy_tree(&src, &dst, false)?;
        copied.push(rel.clone());
    }

    let mut cloned = Vec::new();
    for pattern in &repo.flow.worktree.clone_deps {
        for rel in expand(&repo.root, pattern)? {
            let dst = path.join(&rel);
            if dst.exists() {
                continue;
            }
            copy_tree(&repo.root.join(&rel), &dst, true)?;
            cloned.push(rel.to_string_lossy().into_owned());
        }
    }

    let setup_ok = match (&repo.flow.worktree.setup, opts.setup) {
        (Some(cmd), true) => Some(shell(cmd, &path)?),
        _ => None,
    };

    Ok(NewReport {
        branch: branch.to_owned(),
        path,
        base: base_name,
        base_sha,
        copied,
        missing,
        cloned,
        setup_ok,
    })
}

/// The worktree of an existing local branch, creating one under the flow's root when it has none.
/// Lands a lane in a branch that is not a trunk, on this machine: a merge commit in that
/// branch's worktree, nothing pushed (the branch ships later like any lane). A conflict is
/// undone and reported. The worktree must be clean, so nobody's edits are mixed in.
pub fn land_local(repo: &Repo, branch: &str, into: &str) -> Result<Result<String, Vec<String>>> {
    let wt = ensure_worktree(repo, into)?;
    let g = crate::git::Git::new(&wt);
    if !g.run(&["status", "--porcelain", "--untracked-files=no"])?.trim().is_empty() {
        bail!("{into} has changes that are not committed in {}; commit or stash them, then retry", wt.display());
    }
    let (code, _, _) = g.raw(&["merge", "--no-ff", "--no-edit", "-m", &format!("Merge {branch} into {into}"), branch])?;
    if code != 0 {
        let files: Vec<String> = g.run(&["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default().lines().map(str::to_owned).collect();
        let _ = g.raw(&["merge", "--abort"]);
        if files.is_empty() {
            bail!("git could not merge {branch} into {into}");
        }
        return Ok(Err(files));
    }
    Ok(Ok(g.run(&["rev-parse", "HEAD"])?.trim().to_owned()))
}

pub fn ensure_worktree(repo: &Repo, branch: &str) -> Result<PathBuf> {
    if let Some(w) = repo.worktrees()?.into_iter().find(|w| w.branch.as_deref() == Some(branch) && !w.prunable) {
        return Ok(w.path);
    }
    if repo.git.rev_parse(&format!("refs/heads/{branch}"))?.is_none() {
        bail!("no local branch {branch}");
    }
    let path = repo.flow.worktree.path_for(&repo.home, &repo.name, branch);
    if path.exists() {
        bail!("{} already exists but is not a worktree of {branch}", path.display());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    repo.git.run(&["worktree", "add", "--quiet", &path.to_string_lossy(), branch])?;
    Ok(path)
}

/// Copies a file or directory. `clone` asks for copy-on-write (APFS clonefile, or reflink).
fn copy_tree(src: &Path, dst: &Path, clone: bool) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut cmd = Command::new("cp");
    if cfg!(target_os = "macos") {
        cmd.arg(if clone { "-cRp" } else { "-Rp" });
    } else {
        cmd.arg("-Rp");
        if clone {
            cmd.arg("--reflink=auto");
        }
    }
    let status = cmd.arg(src).arg(dst).status().context("could not run cp")?;
    if !status.success() {
        bail!("cp {} → {} failed", src.display(), dst.display());
    }
    Ok(())
}

/// Expands a relative path whose segments may be `*` (one directory level). Only existing paths.
fn expand(root: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let mut found = vec![PathBuf::new()];
    for seg in pattern.split('/').filter(|s| !s.is_empty()) {
        let mut next = Vec::new();
        for base in &found {
            if seg.contains('*') {
                let Ok(rd) = fs::read_dir(root.join(base)) else { continue };
                let mut names: Vec<String> = rd
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| crate::flow::glob_match(seg, n))
                    .collect();
                names.sort();
                next.extend(names.into_iter().map(|n| base.join(n)));
            } else if root.join(base).join(seg).exists() {
                next.push(base.join(seg));
            }
        }
        found = next;
    }
    Ok(found.into_iter().filter(|p| !p.as_os_str().is_empty()).collect())
}

/// Runs a user command from flow.toml in `dir`, output straight to the terminal.
fn shell(cmd: &str, dir: &Path) -> Result<bool> {
    Ok(Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .status()
        .with_context(|| format!("could not run {cmd:?}"))?
        .success())
}

// ---------------------------------------------------------------- ship

#[derive(Debug, Serialize)]
pub struct ShipPlan {
    pub branch: String,
    pub sha: String,
    pub target: String,
    pub target_sha: String,
    /// Already there: nothing to do.
    pub already: Option<Landed>,
    /// Tree of the merge, when it is clean.
    pub tree: Option<String>,
    pub conflicts: Vec<String>,
    /// Command and worktree for the pre-production check.
    pub check: Option<(String, PathBuf)>,
    /// Uncommitted changes in the lane's worktree, which will not ship.
    pub dirty: usize,
    /// Commits on `<remote>/<branch>` that the local branch lacks; they will not ship.
    pub remote_behind: usize,
    /// Local commits not on `<remote>/<branch>`; they ship inside the merge.
    pub remote_ahead: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "detail")]
pub enum LocalUpdate {
    /// The local target branch was fast-forwarded.
    Updated,
    /// Checked out in a worktree with changes; left behind.
    LeftBehind(PathBuf),
    /// The local branch has commits the remote lacks; left alone.
    Diverged,
    NoLocalBranch,
}

#[derive(Debug, Serialize)]
pub struct ShipReport {
    pub merge_sha: String,
    pub target: String,
    pub local: LocalUpdate,
}

/// `to`: `None` moves the lane one stage forward (integration, then production).
pub fn plan_ship(repo: &Repo, branch: &str, to: Option<&str>, fetch: bool) -> Result<ShipPlan> {
    let flow = &repo.flow.flow;
    if flow.is_trunk(branch) {
        bail!("{branch} is a trunk branch; only lanes ship");
    }
    let Some(sha) = repo.git.rev_parse(&format!("refs/heads/{branch}"))? else {
        bail!("no local branch {branch}");
    };
    if fetch {
        repo.fetch()?;
    }
    let remote_target = |name: &str| -> Result<Target> {
        let Some(t) = Target::resolve(repo, name)? else { bail!("{name} not found") };
        if t.refname != repo.remote_ref(name) {
            bail!("{}/{name} not found; shipping pushes to the remote", flow.remote);
        }
        Ok(t)
    };
    let prod = remote_target(&flow.production)?;
    let integ = flow.integration().map(remote_target).transpose()?;

    let target = match to {
        Some(name) if name == prod.name => prod.clone(),
        Some(name) => match &integ {
            Some(i) if i.name == name => i.clone(),
            _ => bail!("{name} is not a stage of this flow ({} or {})", flow.integration, flow.production),
        },
        None => match &integ {
            Some(i) if landed(&repo.git, branch, &sha, i, None)?.is_none() => i.clone(),
            _ => prod.clone(),
        },
    };
    if flow.forbids(branch, &target.name) {
        bail!("{FLOW_PATH} forbids merging {branch} into {}", target.name);
    }

    let worktree = repo.worktrees()?.into_iter().find(|w| w.branch.as_deref() == Some(branch));
    let dirty = match &worktree {
        Some(w) if !w.prunable => dirty_count(&repo.git.at(&w.path))?,
        _ => 0,
    };

    let (remote_ahead, remote_behind) = match repo.remote_sha(branch)? {
        Some(rs) => (
            repo.git.run(&["rev-list", "--count", &format!("{rs}..{sha}")])?.parse()?,
            repo.git.run(&["rev-list", "--count", &format!("{sha}..{rs}")])?.parse()?,
        ),
        None => (0, 0),
    };

    let already = landed(&repo.git, branch, &sha, &target, None)?;
    let mut plan = ShipPlan {
        remote_behind,
        remote_ahead,
        branch: branch.to_owned(),
        sha: sha.clone(),
        target: target.name.clone(),
        target_sha: target.sha.clone(),
        already,
        tree: None,
        conflicts: Vec::new(),
        check: None,
        dirty,
    };
    if already.is_some() {
        return Ok(plan);
    }

    if target.name == prod.name
        && let Some(i) = &integ
    {
        let line: HashSet<String> = repo
            .git
            .lines(&["rev-list", "--first-parent", &i.sha, &format!("^{}", prod.sha)])?
            .into_iter()
            .collect();
        let carried = lanes::contamination(&repo.git, &sha, &prod.sha, &line)?;
        if !carried.is_empty() {
            bail!(
                "{branch} carries {} commit(s) that exist only in {} ({}). Shipping it would take other work to {}. \
                 Rebuild the lane from {}/{} with only its own commits.",
                carried.len(),
                i.name,
                carried.join(", "),
                prod.name,
                flow.remote,
                flow.base
            );
        }
        if let Some(cmd) = &repo.flow.ship.check {
            let Some(w) = worktree.as_ref().filter(|w| !w.prunable) else {
                bail!("[ship] check needs {branch} checked out in a worktree");
            };
            plan.check = Some((cmd.clone(), w.path.clone()));
        }
    }

    let (code, out, err) =
        repo.git.raw(&["merge-tree", "--write-tree", "--name-only", &target.sha, &sha])?;
    let mut lines = out.lines();
    match code {
        0 => plan.tree = lines.next().map(str::to_owned),
        1 => {
            lines.next();
            plan.conflicts = lines.take_while(|l| !l.is_empty()).map(str::to_owned).collect();
        }
        _ => bail!("git merge-tree failed: {}", err.trim()),
    }
    Ok(plan)
}

pub fn execute_ship(repo: &Repo, plan: &ShipPlan) -> Result<ShipReport> {
    if plan.already.is_some() {
        bail!("{} is already in {}", plan.branch, plan.target);
    }
    let Some(tree) = &plan.tree else {
        bail!("{} conflicts with {}: {}", plan.branch, plan.target, plan.conflicts.join(", "));
    };
    if let Some((cmd, dir)) = &plan.check
        && !shell(cmd, dir)?
    {
        bail!("check {cmd:?} failed in {}; nothing shipped", dir.display());
    }
    let msg = format!("Merge {} into {}", plan.branch, plan.target);
    let merge = repo
        .git
        .run(&["commit-tree", tree, "-p", &plan.target_sha, "-p", &plan.sha, "-m", &msg])?;
    let remote = &repo.flow.flow.remote;
    // The merge's first parent is the remote tip we planned against, so this is a
    // fast-forward; if someone pushed meanwhile the remote rejects it and nothing changes.
    repo.git
        .run(&["push", "--quiet", remote, &format!("{merge}:refs/heads/{}", plan.target)])
        .with_context(|| format!("push to {remote}/{} was rejected; run pit ship again", plan.target))?;
    let _ = repo.git.run(&["update-ref", &repo.remote_ref(&plan.target), &merge]);

    let local = fast_forward_local(repo, &plan.target, &merge)?;
    Ok(ShipReport { merge_sha: merge, target: plan.target.clone(), local })
}

/// Brings the local copy of `branch` up to `sha` when that is a clean fast-forward.
fn fast_forward_local(repo: &Repo, branch: &str, sha: &str) -> Result<LocalUpdate> {
    let Some(local) = repo.git.rev_parse(&format!("refs/heads/{branch}"))? else {
        return Ok(LocalUpdate::NoLocalBranch);
    };
    if !repo.git.is_ancestor(&local, sha)? {
        return Ok(LocalUpdate::Diverged);
    }
    let checked_out: Option<Worktree> =
        repo.worktrees()?.into_iter().find(|w| w.branch.as_deref() == Some(branch) && !w.prunable);
    match checked_out {
        Some(w) => {
            let g = repo.git.at(&w.path);
            if dirty_count(&g)? > 0 {
                return Ok(LocalUpdate::LeftBehind(w.path));
            }
            g.run(&["merge", "--ff-only", "--quiet", sha])?;
        }
        None => {
            repo.git.run(&["update-ref", &format!("refs/heads/{branch}"), sha, &local])?;
        }
    }
    Ok(LocalUpdate::Updated)
}

// ---------------------------------------------------------------- clean

#[derive(Debug, Clone, Serialize)]
pub struct CleanItem {
    pub branch: String,
    pub sha: String,
    /// There is a local branch to delete (false: the lane lives only on the remote).
    pub local: bool,
    pub how: Option<Landed>,
    pub worktree: Option<PathBuf>,
    /// Remote branch tip, if one exists.
    pub remote_sha: Option<String>,
    pub delete_remote: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub branch: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct CleanPlan {
    pub items: Vec<CleanItem>,
    pub skipped: Vec<Skipped>,
}

/// Lanes that are in production with a clean worktree. Never the main or current worktree.
pub fn plan_clean(repo: &Repo, yard: &Yard) -> Result<CleanPlan> {
    let prod = Target::resolve(repo, &repo.flow.flow.production)?
        .context("production branch not found")?;
    let mut items = Vec::new();
    let mut skipped = Vec::new();
    for lane in yard.lanes.iter().filter(|l| l.status == Status::CanDelete) {
        let skip = |reason: &str| Skipped { branch: lane.branch.clone(), reason: reason.into() };
        if let Some(w) = &lane.worktree {
            if w.is_main {
                skipped.push(skip("checked out in the main worktree"));
                continue;
            }
            if w.is_current {
                skipped.push(skip("you are in its worktree"));
                continue;
            }
        }
        let remote_sha = repo.remote_sha(&lane.branch)?;
        let delete_remote = match &remote_sha {
            None => false,
            Some(rs) if *rs == lane.sha => true,
            Some(rs) => {
                if landed(&repo.git, &lane.branch, rs, &prod, None)?.is_none() {
                    skipped.push(skip("the remote branch has commits that are not in production"));
                    continue;
                }
                true
            }
        };
        items.push(CleanItem {
            branch: lane.branch.clone(),
            sha: lane.sha.clone(),
            local: lane.local,
            how: lane.in_production,
            worktree: lane.worktree.as_ref().map(|w| w.path.clone()),
            remote_sha,
            delete_remote,
        });
    }
    Ok(CleanPlan { items, skipped })
}

#[derive(Debug, Serialize)]
pub struct CleanOutcome {
    pub branch: String,
    pub error: Option<String>,
}

pub fn execute_clean(repo: &Repo, plan: &CleanPlan) -> Result<Vec<CleanOutcome>> {
    trash::prune(repo)?;
    let remote = repo.flow.flow.remote.clone();
    let now = now_secs();
    Ok(plan
        .items
        .iter()
        .map(|item| {
            let r = (|| -> Result<()> {
                trash::record(
                    repo,
                    Entry {
                        repo: repo.root.clone(),
                        branch: item.branch.clone(),
                        sha: item.sha.clone(),
                        remote: item.delete_remote.then(|| remote.clone()),
                        remote_sha: item.remote_sha.clone(),
                        worktree: item.worktree.clone(),
                        had_local: item.local,
                        deleted_at: now,
                        keep_refs: Vec::new(),
                    },
                )?;
                if let Some(w) = &item.worktree {
                    repo.git.run(&["worktree", "remove", &w.to_string_lossy()])?;
                }
                if item.local {
                    repo.git.run(&["branch", "-D", "--quiet", &item.branch])?;
                }
                if let (true, Some(rs)) = (item.delete_remote, &item.remote_sha) {
                    let r = format!("refs/heads/{}", item.branch);
                    repo.git.run(&[
                        "push",
                        "--quiet",
                        &format!("--force-with-lease={r}:{rs}"),
                        &remote,
                        &format!(":{r}"),
                    ])?;
                }
                Ok(())
            })();
            CleanOutcome { branch: item.branch.clone(), error: r.err().map(|e| format!("{e:#}")) }
        })
        .collect())
}

#[derive(Debug, Serialize)]
pub struct UndoReport {
    pub branch: String,
    pub sha: String,
    pub local_restored: bool,
    pub remote_restored: bool,
    /// Where the worktree used to be; it is not recreated.
    pub worktree: Option<PathBuf>,
}

/// Brings back a branch `pit clean` deleted (local, and remote if it was deleted too).
pub fn undo_clean(repo: &Repo, branch: &str) -> Result<UndoReport> {
    let Some(e) = trash::latest(repo, branch)? else {
        bail!("nothing to undo for {branch} in the last {} days", trash::KEEP_DAYS);
    };
    let local_exists = repo.git.rev_parse(&format!("refs/heads/{branch}"))?.is_some();
    if e.had_local && local_exists {
        bail!("branch {branch} exists again; nothing to undo");
    }
    if e.had_local {
        repo.git.run(&["branch", branch, &e.sha])?;
    }
    let mut remote_restored = false;
    if let (Some(remote), Some(rs)) = (&e.remote, &e.remote_sha)
        && repo.remote_sha(branch)?.is_none()
    {
        repo.git.run(&["push", "--quiet", remote, &format!("{rs}:refs/heads/{branch}")])?;
        remote_restored = true;
    }
    if !e.had_local && !remote_restored {
        bail!("{branch} is on the remote again; nothing to undo");
    }
    Ok(UndoReport {
        branch: branch.to_owned(),
        sha: e.sha,
        local_restored: e.had_local,
        remote_restored,
        worktree: e.worktree,
    })
}
