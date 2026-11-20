//! `pit`: the Pitlane CLI. Parses arguments, calls `pit-core`, prints.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use pit_core::Repo;
use pit_core::actions::{self, LocalUpdate, NewOptions};
use pit_core::lanes::{self, Lane, Status, Sync, Yard, YardOptions};
use pit_core::trash;

#[derive(Parser)]
#[command(name = "pit", version, about = "Pitlane: every lane in view, only the ones that need you light up")]
struct Cli {
    /// Run as if started in this directory.
    #[arg(short = 'C', global = true, value_name = "PATH")]
    dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write .pitlane/flow.toml and turn on git rerere.
    Init {
        /// Overwrite an existing flow.toml.
        #[arg(long)]
        force: bool,
    },
    /// The yard: every lane and where it has really landed.
    Lanes {
        /// Use the remote-tracking refs as they are (no fetch, no GitHub).
        #[arg(long)]
        no_fetch: bool,
        #[arg(long)]
        json: bool,
    },
    /// New lane: branch from a freshly fetched origin/<base> in its own worktree.
    New {
        branch: String,
        #[arg(long)]
        no_fetch: bool,
        /// Skip the [worktree] setup command.
        #[arg(long)]
        no_setup: bool,
        #[arg(long)]
        json: bool,
    },
    /// Move a lane one stage forward: into integration, then into production.
    Ship {
        /// Defaults to the branch checked out here.
        branch: Option<String>,
        /// Force the target stage (e.g. dev or main).
        #[arg(long, value_name = "BRANCH")]
        to: Option<String>,
        /// Do not ask.
        #[arg(short, long)]
        yes: bool,
        #[arg(long)]
        no_fetch: bool,
    },
    /// Delete lanes already in production (worktree, local and remote branch). Undo for 30 days.
    Clean {
        #[arg(short, long)]
        yes: bool,
        /// Show what would go, delete nothing.
        #[arg(long)]
        dry_run: bool,
        /// Bring back a branch a previous clean deleted.
        #[arg(long, value_name = "BRANCH")]
        undo: Option<String>,
        /// List what can still be undone.
        #[arg(long)]
        trash: bool,
        #[arg(long)]
        no_fetch: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{} {e:#}", paint(FAIL, "pit:"));
            ExitCode::FAILURE
        }
    }
}

fn open(dir: Option<PathBuf>) -> Result<Repo> {
    let dir = match dir {
        Some(d) => d,
        None => std::env::current_dir()?,
    };
    Repo::open_default(&dir)
}

fn run(cli: Cli) -> Result<ExitCode> {
    let repo = open(cli.dir)?;
    match cli.cmd {
        Cmd::Init { force } => {
            let r = actions::init(&repo, force)?;
            match &r.flow_written {
                Some(p) => println!("wrote {}", p.display()),
                None => println!("{} already exists (--force to overwrite)", pit_core::flow::FLOW_PATH),
            }
            println!("git rerere {}", if r.rerere_was_on { "was already on" } else { "turned on" });
        }
        Cmd::Lanes { no_fetch, json } => {
            let y = lanes::yard(&repo, YardOptions { fetch: !no_fetch, github: !no_fetch })?;
            if json {
                println!("{}", serde_json::to_string_pretty(&y)?);
            } else {
                print_yard(&y);
            }
        }
        Cmd::New { branch, no_fetch, no_setup, json } => {
            let r = actions::new_lane(&repo, &branch, NewOptions { fetch: !no_fetch, setup: !no_setup })?;
            if json {
                println!("{}", serde_json::to_string_pretty(&r)?);
                return Ok(ExitCode::SUCCESS);
            }
            println!("{} {} from {} {}", paint(DONE, "✓"), r.branch, r.base, &r.base_sha[..8]);
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
        }
        Cmd::Ship { branch, to, yes, no_fetch } => return ship(&repo, branch, to.as_deref(), yes, !no_fetch),
        Cmd::Clean { yes, dry_run, undo, trash: list_trash, no_fetch } => {
            if let Some(b) = undo {
                let r = actions::undo_clean(&repo, &b)?;
                println!("{} {} restored at {}", paint(DONE, "✓"), r.branch, &r.sha[..8]);
                if r.remote_restored {
                    println!("  remote branch pushed back");
                }
                if let Some(w) = r.worktree {
                    println!("  worktree not recreated: git worktree add {} {}", shell_quote(&w.to_string_lossy()), r.branch);
                }
                return Ok(ExitCode::SUCCESS);
            }
            if list_trash {
                let entries = trash::list(&repo)?;
                if entries.is_empty() {
                    println!("nothing to undo");
                }
                let now = lanes_now();
                for e in entries {
                    let left = trash::KEEP_DAYS.saturating_sub(now.saturating_sub(e.deleted_at) / 86_400);
                    println!("{}  {}  {}d left  pit clean --undo {}", &e.sha[..8], e.branch, left, e.branch);
                }
                return Ok(ExitCode::SUCCESS);
            }
            return clean(&repo, yes, dry_run, !no_fetch);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn ship(repo: &Repo, branch: Option<String>, to: Option<&str>, yes: bool, fetch: bool) -> Result<ExitCode> {
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
        &plan.sha[..8],
        plan.target,
        &plan.target_sha[..8],
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
    let r = actions::execute_ship(repo, &plan)?;
    println!("{} {} is in {} ({})", paint(DONE, "✓"), branch, r.target, &r.merge_sha[..8]);
    match r.local {
        LocalUpdate::Updated => println!("  local {} updated", r.target),
        LocalUpdate::LeftBehind(p) => println!("  local {} has changes in {}; pull there when ready", r.target, p.display()),
        LocalUpdate::Diverged => println!("  {} local {} has commits {remote} lacks; left as is", paint(ACCENT, "!"), r.target),
        LocalUpdate::NoLocalBranch => {}
    }
    Ok(ExitCode::SUCCESS)
}

fn clean(repo: &Repo, yes: bool, dry_run: bool, fetch: bool) -> Result<ExitCode> {
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
        println!("  {}  {}  ({how})  {}", &i.sha[..8], i.branch, paint(MUTED, &what.join(" + ")));
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

// ---------------------------------------------------------------- yard table

fn print_yard(y: &Yard) {
    let stage = match &y.integration {
        Some(i) => format!("{} → {}", i, y.production),
        None => y.production.clone(),
    };
    let mut head = vec![y.repo.clone(), stage];
    head.push(if y.fetched { "fetched".into() } else { "not fetched".into() });
    if y.github {
        head.push("GitHub PRs".into());
    }
    println!("{}\n", paint(MUTED, &head.join("  ·  ")));

    if y.lanes.is_empty() {
        println!("no lanes. Start one: pit new feat/<thing>");
    } else {
        let integ = y.integration.as_deref();
        let mut rows: Vec<[Cell; 9]> = vec![[
            Cell::plain("#"),
            Cell::plain("LANE"),
            Cell::plain("WORKTREE"),
            Cell::plain(&integ.unwrap_or("").to_uppercase()),
            Cell::plain(&y.production.to_uppercase()),
            Cell::plain("REMOTE"),
            Cell::plain("IDLE"),
            Cell::plain("STATUS"),
            Cell::plain("NEXT"),
        ]];
        let header_len = rows.len();
        let build = |compact: bool| -> Vec<[Cell; 9]> { y.lanes.iter().map(|l| row(l, integ, compact)).collect() };
        rows.extend(build(false));
        let measure = |rows: &[[Cell; 9]]| -> Vec<usize> {
            (0..9).map(|c| rows.iter().map(|r| r[c].text.chars().count()).max().unwrap_or(0)).collect()
        };
        let mut shown = [true; 9];
        shown[3] = integ.is_some();
        let total = |w: &[usize], shown: &[bool; 9]| -> usize {
            let cols: Vec<usize> = (0..9).filter(|&c| shown[c]).map(|c| w[c]).collect();
            cols.iter().sum::<usize>() + 2 * cols.len().saturating_sub(1)
        };
        let mut widths = measure(&rows);
        // Narrow terminal: drop IDLE, then UNPUSHED, then shrink WORKTREE to its change count.
        if let Some(max) = term_width() {
            for col in [6, 5] {
                if total(&widths, &shown) > max {
                    shown[col] = false;
                }
            }
            if total(&widths, &shown) > max {
                rows.truncate(header_len);
                rows[0][2] = Cell::plain("WT");
                rows.extend(build(true));
                widths = measure(&rows);
            }
        }
        let last = (0..9).rev().find(|&c| shown[c]).unwrap_or(8);
        for (i, r) in rows.iter().enumerate() {
            let mut line = String::new();
            for (c, cell) in r.iter().enumerate() {
                if !shown[c] {
                    continue;
                }
                let pad = widths[c] - cell.text.chars().count();
                let text = if i == 0 { paint(MUTED, &cell.text) } else { cell.render() };
                if c == 0 {
                    line.push_str(&" ".repeat(pad));
                    line.push_str(&text);
                } else {
                    line.push_str(&text);
                    if c < last {
                        line.push_str(&" ".repeat(pad));
                    }
                }
                if c < last {
                    line.push_str("  ");
                }
            }
            println!("{}", line.trim_end());
        }
    }

    if !y.only_in_integration.is_empty() {
        let integ = y.integration.as_deref().unwrap_or("integration");
        println!(
            "\n{} only in {integ}: {} commit(s) made straight on {integ}. The flow never takes them to {}.",
            paint(ACCENT, "●"),
            y.only_in_integration.len(),
            y.production
        );
        for c in &y.only_in_integration {
            println!("  {}  {}  {}", c.short, c.subject, paint(MUTED, &format!("{} · {}d", c.author, c.days_ago)));
        }
    }
}

/// Where the lane stands against its branch on the remote.
fn remote_cell(l: &Lane) -> Cell {
    let at_risk = l.unpushed > 0 && l.in_production.is_none();
    let mut cell = match l.sync {
        Sync::Synced => Cell::new(DONE, "✓ synced"),
        Sync::Ahead { ahead } => Cell::plain(&format!("↑{ahead}")),
        Sync::Behind { behind } => Cell::new(ACCENT, &format!("↓{behind}")),
        Sync::Diverged { ahead, behind } => Cell::new(FAIL, &format!("↑{ahead} ↓{behind}")),
        Sync::LocalOnly if at_risk => Cell::new(ACCENT, &format!("local only, {}↑", l.unpushed)),
        Sync::LocalOnly => Cell::muted("local only"),
        Sync::Gone if at_risk => Cell::new(ACCENT, &format!("deleted on remote, {}↑", l.unpushed)),
        Sync::Gone => Cell::muted("deleted on remote"),
        Sync::RemoteOnly => Cell::muted("remote only"),
    };
    if let Some(other) = &l.tracks_other {
        cell = Cell::new(ACCENT, &format!("{} · tracks {other}", cell.text));
    }
    cell
}

fn row(l: &Lane, integ: Option<&str>, compact: bool) -> [Cell; 9] {
    let wt = match &l.worktree {
        None => Cell::muted("–"),
        Some(w) if compact => {
            if w.dirty > 0 { Cell::new(ACCENT, &format!("+{}", w.dirty)) } else { Cell::plain("✓") }
        }
        Some(w) => {
            let name = if w.is_main {
                "(main checkout)".to_owned()
            } else {
                w.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            };
            let mark = if w.is_current { "*" } else { "" };
            if w.dirty > 0 {
                Cell::new(ACCENT, &format!("{mark}{name} +{}", w.dirty))
            } else {
                Cell::plain(&format!("{mark}{name}"))
            }
        }
    };
    let landed = |x: Option<lanes::Landed>| match x {
        Some(h) => Cell::new(DONE, h.label()),
        None => Cell::muted("–"),
    };
    let unpushed = remote_cell(l);
    let (status, next) = match l.status {
        Status::Contaminated => (
            Cell::new(FAIL, &format!("✕ carries {} {} commit(s)", l.integration_commits.len(), integ.unwrap_or("integration"))),
            Cell::new(FAIL, "rebuild from base"),
        ),
        Status::CanDelete => (Cell::new(DONE, "✓ can delete"), Cell::plain("pit clean")),
        Status::Shipped => (Cell::new(DONE, "✓ shipped"), Cell::muted("–")),
        Status::ShippedDirty => (Cell::new(ACCENT, "● shipped, uncommitted"), Cell::new(ACCENT, "commit or discard")),
        Status::NeedsProduction => (Cell::new(ACCENT, "● needs ship"), Cell::new(ACCENT, "ship → main")),
        Status::InProgress => (
            Cell::new(RUN, "▸ in progress"),
            match (integ, l.worktree.as_ref().map_or(0, |w| w.dirty)) {
                _ if !l.local => Cell::plain("check out"),
                (Some(i), 0) if l.ahead > 0 => Cell::plain(&format!("ship → {i}")),
                _ => Cell::muted("–"),
            },
        ),
        Status::New => (Cell::muted("○ new"), Cell::muted("–")),
        Status::Stale => (Cell::muted("◌ stale"), Cell::muted("archive?")),
    };
    [
        Cell::muted(&l.number.to_string()),
        if l.status.needs_you() { Cell::bold(&l.branch) } else { Cell::plain(&l.branch) },
        wt,
        if integ.is_some() { landed(l.in_integration) } else { Cell::plain("") },
        landed(l.in_production),
        unpushed,
        Cell::muted(&format!("{}d", l.idle_days)),
        status,
        next,
    ]
}

// ---------------------------------------------------------------- terminal

/// 256-colour approximations of design/tokens.css: accent (hue 58) means "your move" only.
const ACCENT: &str = "38;5;208";
const DONE: &str = "38;5;114";
const FAIL: &str = "38;5;203";
const RUN: &str = "38;5;75";
const MUTED: &str = "38;5;244";
const BOLD: &str = "1";

struct Cell {
    text: String,
    style: Option<&'static str>,
}

impl Cell {
    fn new(style: &'static str, text: &str) -> Self {
        Self { text: text.to_owned(), style: Some(style) }
    }
    fn plain(text: &str) -> Self {
        Self { text: text.to_owned(), style: None }
    }
    fn muted(text: &str) -> Self {
        Self::new(MUTED, text)
    }
    fn bold(text: &str) -> Self {
        Self::new(BOLD, text)
    }
    fn render(&self) -> String {
        match self.style {
            Some(s) => paint(s, &self.text),
            None => self.text.clone(),
        }
    }
}

fn term_width() -> Option<usize> {
    std::io::stdout().is_terminal().then(|| terminal_size::terminal_size().map(|(w, _)| usize::from(w.0)))?
}

fn color_on() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

fn paint(style: &str, text: &str) -> String {
    if color_on() { format!("\x1b[{style}m{text}\x1b[0m") } else { text.to_owned() }
}

fn confirm(question: &str) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("not a terminal; pass --yes to proceed without asking");
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes" | "s" | "sim"))
}

fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-~".contains(c)) {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

fn lanes_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
