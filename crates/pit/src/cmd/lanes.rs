//! Lanes: init, the yard, new lanes, shipping them and cleaning up the landed ones.

use std::process::ExitCode;

use anyhow::{Result, bail};
use pit_core::Repo;
use pit_core::actions::{self, NewOptions};
use pit_core::cards::{self, CardPatch};
use pit_core::lanes::{self, YardOptions};
use pit_core::queue::JobState;
use pit_core::trash;

use super::{daemon, pitlane_home, print_json};
use super::yard::print_yard;
use crate::term::{ACCENT, DONE, FAIL, MUTED, confirm, paint, plain, shell_quote, short};

pub fn init(repo: &Repo, force: bool) -> Result<ExitCode> {
    let r = actions::init(repo, force)?;
    match &r.flow_written {
        Some(p) => println!("wrote {}", p.display()),
        None => println!("{} already exists (--force to overwrite)", pit_core::flow::FLOW_PATH),
    }
    println!("git rerere {}", if r.rerere_was_on { "was already on" } else { "turned on" });
    Ok(ExitCode::SUCCESS)
}

pub fn lanes(repo: &Repo, no_fetch: bool, json: bool) -> Result<ExitCode> {
    let y = lanes::yard(repo, YardOptions { fetch: !no_fetch, github: !no_fetch })?;
    if json {
        return print_json(&y);
    }
    print_yard(&y);
    Ok(ExitCode::SUCCESS)
}

pub fn new(repo: &Repo, branch: String, no_fetch: bool, no_setup: bool, card: Option<u32>, json: bool) -> Result<ExitCode> {
    if let Some(id) = card {
        cards::get(repo, id)?; // fail before creating anything
    }
    let r = actions::new_lane(repo, &branch, NewOptions { fetch: !no_fetch, setup: !no_setup })?;
    if let Some(id) = card {
        cards::update(repo, id, CardPatch { lane: Some(Some(r.branch.clone())), ..Default::default() })?;
    }
    if json {
        // The report says it too (`setup_ok`); the exit code must not call a failed setup a success.
        let printed = print_json(&r)?;
        return Ok(if r.setup_ok == Some(false) { ExitCode::FAILURE } else { printed });
    }
    println!("{} {} from {} {}", paint(DONE, "✓"), r.branch, r.base, short(&r.base_sha));
    println!("  {}", r.path.display());
    if !r.copied.is_empty() {
        println!("  copied {}", r.copied.join(", "));
    }
    if !r.cloned.is_empty() {
        println!("  cloned {}", r.cloned.join(", "));
    }
    for m in &r.missing {
        println!("  {} {m} not found in the main worktree, not copied", paint(ACCENT, "!"));
    }
    if r.setup_ok == Some(false) {
        println!("  {} setup command failed", paint(FAIL, "✕"));
        return Ok(ExitCode::FAILURE);
    }
    println!("  cd {}", shell_quote(&r.path.to_string_lossy()));
    Ok(ExitCode::SUCCESS)
}

pub fn ship(repo: &Repo, branch: Option<String>, to: Option<&str>, yes: bool, fetch: bool) -> Result<ExitCode> {
    let branch = match branch {
        Some(b) => b,
        None => {
            let b = repo.git.at(&repo.cwd_root).run(&["branch", "--show-current"])?;
            if b.is_empty() {
                bail!("not on a branch; name the lane: pit ship <branch>");
            }
            b
        }
    };
    let plan = actions::plan_ship(repo, &branch, to, fetch)?;
    let remote = &repo.flow.flow.remote;
    if let Some(how) = plan.already {
        println!("{} {} is already in {} ({})", paint(DONE, "✓"), branch, plan.target, how.label());
        return Ok(ExitCode::SUCCESS);
    }
    if !plan.conflicts.is_empty() {
        println!("{} {} conflicts with {remote}/{}:", paint(FAIL, "✕"), branch, plan.target);
        for c in &plan.conflicts {
            println!("    {c}");
        }
        println!("  nothing changed. Resolve it by hand where {} is checked out:", plan.target);
        println!("    git switch {t} && git pull && git merge {branch} && git push", t = plan.target);
        println!("  rerere records the resolution, so the same conflict resolves itself on the next stage.");
        return Ok(ExitCode::FAILURE);
    }
    println!(
        "merge {} {} into {remote}/{} {}, then push {remote} {}",
        branch,
        short(&plan.sha),
        plan.target,
        short(&plan.target_sha),
        plan.target
    );
    if let Some((cmd, dir)) = &plan.check {
        println!("  check first: {cmd} (in {})", dir.display());
    }
    if plan.dirty > 0 {
        println!("  {} {} uncommitted change(s) in the worktree will not ship", paint(ACCENT, "!"), plan.dirty);
    }
    if !yes && !confirm("ship?")? {
        println!("nothing shipped");
        return Ok(ExitCode::FAILURE);
    }
    // Through the merge queue, like every other landing: one merge at a time per repository, by
    // plumbing, with the card moved along. What was shown above is the plan; the queue plans
    // again when the lane's turn comes.
    let mut d = daemon()?;
    let job = d.enqueue(&repo.root, branch.clone(), to.map(str::to_owned))?;
    if pit_core::queue::paused(&pitlane_home()) {
        println!("{} the queue is paused: {branch} is in line for {} ({}) and ships when it resumes", paint(ACCENT, "!"), job.target, job.id);
        return Ok(ExitCode::FAILURE);
    }
    let mut said_shipping = false;
    loop {
        let Some(j) = d.queue(Some(&repo.root))?.into_iter().find(|j| j.id == job.id) else {
            bail!("{} left the queue before it shipped", job.id);
        };
        match j.state {
            JobState::Queued => {}
            JobState::Shipping if !said_shipping => {
                said_shipping = true;
                println!("  shipping…");
            }
            JobState::Shipping => {}
            JobState::Done => {
                println!("{} {} is in {} ({})", paint(DONE, "✓"), branch, j.target, short(j.merge_sha.as_deref().unwrap_or_default()));
                return Ok(ExitCode::SUCCESS);
            }
            JobState::Conflict => {
                println!("{} {} conflicts with {remote}/{}: {}", paint(FAIL, "✕"), branch, j.target, plain(&j.conflicts.join(", ")));
                println!("  nothing changed.");
                return Ok(ExitCode::FAILURE);
            }
            JobState::Failed => {
                println!("{} {} did not ship: {}", paint(FAIL, "✕"), branch, plain(j.error.as_deref().unwrap_or("no reason given")));
                return Ok(ExitCode::FAILURE);
            }
            JobState::Removed => {
                println!("{} {} was taken out of the queue; nothing shipped", paint(ACCENT, "!"), branch);
                return Ok(ExitCode::FAILURE);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
}

/// Bring back a branch a previous clean deleted.
pub fn undo(repo: &Repo, b: &str) -> Result<ExitCode> {
    let r = actions::undo_clean(repo, b)?;
    println!("{} {} restored at {}", paint(DONE, "✓"), r.branch, short(&r.sha));
    if r.remote_restored {
        println!("  remote branch pushed back");
    }
    if let Some(w) = r.worktree {
        println!("  worktree not recreated: git worktree add {} {}", shell_quote(&w.to_string_lossy()), r.branch);
    }
    Ok(ExitCode::SUCCESS)
}

/// List what a previous clean deleted and can still be undone.
pub fn list_trash(repo: &Repo) -> Result<ExitCode> {
    let entries = trash::list(repo)?;
    if entries.is_empty() {
        println!("nothing to undo");
    }
    let now = pit_core::time::now_secs();
    for e in entries {
        let left = trash::KEEP_DAYS.saturating_sub(now.saturating_sub(e.deleted_at) / 86_400);
        println!("{}  {}  {}d left  pit clean --undo {}", short(&e.sha), e.branch, left, e.branch);
    }
    Ok(ExitCode::SUCCESS)
}

pub fn clean(repo: &Repo, yes: bool, dry_run: bool, fetch: bool) -> Result<ExitCode> {
    let y = lanes::yard(repo, YardOptions { fetch, github: fetch })?;
    let plan = actions::plan_clean(repo, &y)?;
    for s in &plan.skipped {
        println!("  {} {}: {}", paint(MUTED, "skip"), s.branch, s.reason);
    }
    if plan.items.is_empty() {
        println!("nothing to clean");
        return Ok(ExitCode::SUCCESS);
    }
    println!("in {}, safe to delete:", y.production);
    for i in &plan.items {
        let mut what = vec!["branch"];
        if i.worktree.is_some() {
            what.push("worktree");
        }
        if i.delete_remote {
            what.push("remote");
        }
        let how = i.how.map_or("", |h| h.label());
        println!("  {}  {}  ({how})  {}", short(&i.sha), i.branch, paint(MUTED, &what.join(" + ")));
    }
    if dry_run {
        return Ok(ExitCode::SUCCESS);
    }
    if !yes && !confirm(&format!("delete {}? (undo for {} days)", plan.items.len(), trash::KEEP_DAYS))? {
        println!("nothing deleted");
        return Ok(ExitCode::FAILURE);
    }
    let mut failed = false;
    for o in actions::execute_clean(repo, &plan)? {
        match o.error {
            None => println!("{} {}", paint(DONE, "✓"), o.branch),
            Some(e) => {
                failed = true;
                println!("{} {}: {e}", paint(FAIL, "✕"), o.branch);
            }
        }
    }
    println!("undo with: pit clean --undo <branch>");
    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}
