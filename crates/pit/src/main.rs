//! `pit`: the Pitlane CLI. Parses arguments, calls `pit-core`, prints.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use pit_core::Repo;
use pit_core::actions::{self, LocalUpdate, NewOptions};
use pit_core::lanes::{self, Lane, Status, Sync, Yard, YardOptions};
use pit_core::cards::{self, CardOut, CardPatch, CardState, NewCard, NewQuestion};
use pit_core::runs::{Client, RunEvent, RunMeta, RunState};
use pit_core::trash;

mod mcp;

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
        /// Link the new lane to this card.
        #[arg(long, value_name = "ID")]
        card: Option<u32>,
        #[arg(long)]
        json: bool,
    },
    /// Cards: the board's work items, kept in .pitlane/cards.
    #[command(subcommand)]
    Card(CardCmd),
    /// Serve the cards over MCP on stdio: claude mcp add pitlane -- pit mcp
    Mcp {
        /// Internal: answer a run's permission prompts (started by the daemon for each run).
        #[arg(long, value_name = "RUN", hide = true)]
        approve: Option<String>,
    },
    /// Tools agents are waiting on you to allow, across runs.
    Approvals,
    /// Allow what a run is waiting on: once, for the rest of the run, or always in this repo.
    Allow {
        run: String,
        /// Which approval (default: the oldest).
        #[arg(long)]
        id: Option<String>,
        /// The same command until the run ends.
        #[arg(long, conflicts_with = "always")]
        run_wide: bool,
        /// The same command from now on, saved to [run] allow in .pitlane/flow.toml.
        #[arg(long)]
        always: bool,
    },
    /// Deny what a run is waiting on; the agent reads the reason.
    Deny {
        run: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Run the daemon that owns agent runs (started on demand by pit and the app).
    Daemon,
    /// Have an agent work a card in the card's worktree, and follow it.
    Run {
        card: u32,
        /// Start it and return; follow later with pit log.
        #[arg(short, long)]
        detach: bool,
        /// Yes to an API key account if the route reaches one (paid per token).
        #[arg(long)]
        allow_api_key: bool,
    },
    /// Continue a run that hit its usage limit with the route's next option.
    Continue {
        run: String,
        #[arg(long)]
        allow_api_key: bool,
        #[arg(short, long)]
        detach: bool,
    },
    /// What the checks found on a card in review: proofs, the judge, the diff.
    Review { card: u32 },
    /// Approve a card in review: its lane ships into the integration branch.
    Approve { card: u32 },
    /// Ask for changes on a card in review: its run continues with your comments.
    Changes {
        card: u32,
        #[arg(short, long)]
        message: String,
    },
    /// Put a card in review back to ready; its lane stays.
    Discard { card: u32 },
    /// The merge queue: lanes shipping into dev or main, one at a time.
    Queue,
    /// Put a lane in the merge queue (default: its next stage).
    Enqueue {
        branch: String,
        #[arg(long, value_name = "BRANCH")]
        to: Option<String>,
    },
    /// Take a job out of the merge queue.
    Dequeue { id: String },
    /// Your accounts: signed in or not, their last usage, and this repository's.
    Accounts,
    /// Add an account, sign in to it, or choose this repository's.
    #[command(subcommand)]
    Account(AccountCmd),
    /// Runs of this repository, newest first.
    Runs {
        #[arg(long)]
        json: bool,
    },
    /// A run's events; -f keeps following until it ends.
    Log {
        run: String,
        #[arg(short, long)]
        follow: bool,
    },
    /// Tell a running agent something without stopping it.
    Say { run: String, text: String },
    /// Stop a run: the agent ends its current step cleanly.
    Stop { run: String },
    /// Continue a finished or stopped run in the same session.
    Resume {
        run: String,
        #[arg(short, long)]
        detach: bool,
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

#[derive(Subcommand)]
enum AccountCmd {
    /// Add an account. You sign in yourself with the official CLI: pit account login <id>.
    Add {
        id: String,
        /// subscription, team or api-key.
        #[arg(long, default_value = "subscription")]
        kind: String,
        /// For api-key accounts: a command of yours that prints the key (Claude Code runs it).
        #[arg(long, value_name = "COMMAND")]
        api_key_helper: Option<String>,
    },
    /// Open the official Claude Code sign-in for this account's profile.
    Login { id: String },
    /// Use this account for this repository's next runs (your choice, never automatic).
    Use { id: String },
}

#[derive(Subcommand)]
enum CardCmd {
    /// New card. The body is the spec, in markdown.
    Add {
        title: String,
        #[arg(long)]
        body: Option<String>,
        /// Read the body from a file (- for stdin).
        #[arg(long, value_name = "PATH", conflicts_with = "body")]
        body_file: Option<PathBuf>,
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
    List {
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Show {
        id: u32,
        #[arg(long)]
        json: bool,
    },
    /// Set the state: inbox, ready, running, review, in-dev, done, blocked.
    Move { id: u32, state: String },
    /// Ask a question about a card, with options; mark the one you recommend.
    Ask {
        id: u32,
        question: String,
        #[arg(long = "option")]
        options: Vec<String>,
        /// The recommended option, counting from 1.
        #[arg(long, value_name = "N")]
        recommend: Option<usize>,
    },
    /// Answer a question: an option number (from 1), its text, or free text.
    Answer { id: u32, question: u32, answer: String },
    /// Link a card to a lane (branch), or unlink it.
    Link {
        id: u32,
        branch: Option<String>,
        #[arg(long, conflicts_with = "branch")]
        unlink: bool,
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
    if let Cmd::Daemon = cli.cmd {
        let (_, home) = pit_core::default_homes();
        let claude_bin = std::env::var("PITLANE_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
        let pit_bin = std::env::current_exe().ok();
        tokio::runtime::Runtime::new()?.block_on(pitd::serve(pitd::Config { home, claude_bin, pit_bin }))?;
        return Ok(ExitCode::SUCCESS);
    }
    if let Cmd::Mcp { approve: Some(run) } = cli.cmd {
        mcp::serve_approver(run)?;
        return Ok(ExitCode::SUCCESS);
    }
    if let Cmd::Mcp { .. } = cli.cmd {
        // The agent may start it anywhere; each tool call opens the repository it runs in.
        let dir = match cli.dir {
            Some(d) => d,
            None => std::env::current_dir()?,
        };
        mcp::serve(dir)?;
        return Ok(ExitCode::SUCCESS);
    }
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
        Cmd::Mcp { .. } | Cmd::Daemon => unreachable!("handled above"),
        Cmd::Approvals => {
            let v = daemon()?.call(serde_json::json!({ "op": "approvals" }))?;
            let list = v["approvals"].as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                println!("nothing is waiting on you");
            }
            for a in list {
                println!(
                    "{}  #{} {}\n  {} {}  {}",
                    paint(ACCENT, a["run"].as_str().unwrap_or_default()),
                    a["card"],
                    a["card_title"].as_str().unwrap_or_default(),
                    a["approval"]["tool"].as_str().unwrap_or_default(),
                    a["approval"]["detail"].as_str().unwrap_or_default(),
                    paint(MUTED, &format!("pit allow {} | pit deny {}", a["run"].as_str().unwrap_or_default(), a["run"].as_str().unwrap_or_default()))
                );
            }
        }
        Cmd::Allow { run, id, run_wide, always } => {
            let decision = if always { "always" } else if run_wide { "run" } else { "once" };
            let v = daemon()?.call(serde_json::json!({ "op": "decide", "run": run, "approval": id, "decision": decision }))?;
            println!("{} allowed {} ({decision})", paint(DONE, "✓"), v["approval"].as_str().unwrap_or_default());
        }
        Cmd::Deny { run, id, message } => {
            let v = daemon()?.call(serde_json::json!({ "op": "decide", "run": run, "approval": id, "decision": "deny", "message": message }))?;
            println!("{} denied {}", paint(DONE, "✓"), v["approval"].as_str().unwrap_or_default());
        }
        Cmd::Run { card, detach, allow_api_key } => {
            let mut c = daemon()?;
            let v = c
                .call(serde_json::json!({ "op": "start", "repo": repo.root, "card": card, "allow_api_key": allow_api_key }))
                .map_err(api_key_hint)?;
            let m: RunMeta = serde_json::from_value(v["run"].clone())?;
            println!("{} run {} for #{} {}", paint(DONE, "✓"), m.id, m.card, m.card_title);
            println!("  {}", paint(MUTED, m.reason.as_deref().unwrap_or_default()));
            return follow_or_detach(&m.id, detach);
        }
        Cmd::Continue { run, allow_api_key, detach } => {
            let v = daemon()?
                .call(serde_json::json!({ "op": "continue", "run": run, "allow_api_key": allow_api_key }))
                .map_err(api_key_hint)?;
            let m: RunMeta = serde_json::from_value(v["run"].clone())?;
            println!("{} run {} continues {run}", paint(DONE, "✓"), m.id);
            println!("  {}", paint(MUTED, m.reason.as_deref().unwrap_or_default()));
            return follow_or_detach(&m.id, detach);
        }
        Cmd::Review { card } => {
            let v = daemon()?.call(serde_json::json!({ "op": "list", "repo": repo.root }))?;
            let runs: Vec<RunMeta> = serde_json::from_value(v["runs"].clone())?;
            let Some(m) = runs.iter().filter(|m| m.card == card).max_by_key(|m| (m.round, m.started_at)) else {
                bail!("card #{card} has no runs");
            };
            println!("#{} {}  {}  {}", m.card, m.card_title, state_word(m.state), paint(MUTED, &m.branch));
            if let Some(s) = m.handoff.as_ref().and_then(|h| h.get("summary")).and_then(|s| s.as_str()) {
                println!("\n{s}");
            }
            let (_, home) = pit_core::default_homes();
            match std::fs::read_to_string(pit_core::runs::runs_dir(&home).join(&m.id).join("verify.json")) {
                Ok(t) => {
                    let v: pit_core::verify::Verification = serde_json::from_str(&t)?;
                    println!();
                    if v.no_commands {
                        println!("{}", paint(MUTED, "no checks configured ([verify] commands in .pitlane/flow.toml)"));
                    }
                    for p in &v.proofs {
                        println!("{} {}  {}", if p.ok { paint(DONE, "✓") } else { paint(FAIL, "✕") }, p.command, paint(MUTED, &format!("{:.1}s", p.duration_ms as f64 / 1000.0)));
                        if !p.ok {
                            for l in p.tail.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev() {
                                println!("    {}", paint(MUTED, l));
                            }
                        }
                    }
                    if let Some(j) = v.judge.as_ref().and_then(|j| j.judgement.as_ref()) {
                        let word = match j.verdict {
                            pit_core::verify::Verdict::Approve => paint(DONE, "approves"),
                            pit_core::verify::Verdict::Changes => paint(ACCENT, "asks for changes"),
                        };
                        println!("\njudge {word}: {}", j.summary);
                        for f in &j.findings {
                            let at = match (&f.file, f.line) {
                                (Some(file), Some(l)) => format!("{file}:{l} "),
                                (Some(file), None) => format!("{file} "),
                                _ => String::new(),
                            };
                            println!("  {} {at}{}", paint(MUTED, &f.severity), f.text);
                        }
                    } else if let Some(e) = v.judge.as_ref().and_then(|j| j.error.as_ref()) {
                        println!("\njudge could not finish: {e}");
                    }
                }
                Err(_) => println!("\n{}", paint(MUTED, "no checks ran for this run")),
            }
            println!("\n{}", paint(MUTED, &format!("pit approve {card} | pit changes {card} -m \"…\" | pit discard {card}")));
        }
        Cmd::Approve { card } => {
            let v = daemon()?.call(serde_json::json!({ "op": "review", "repo": repo.root, "card": card, "action": "approve" }))?;
            println!("{} #{card} is in the merge queue for {} (pit queue)", paint(DONE, "✓"), v["shipped_to"].as_str().unwrap_or("its next stage"));
        }
        Cmd::Changes { card, message } => {
            let v = daemon()?.call(serde_json::json!({ "op": "review", "repo": repo.root, "card": card, "action": "changes", "message": message }))?;
            let run = v["run"]["id"].as_str().unwrap_or_default().to_owned();
            println!("{} #{card} goes back to its agent as run {run}", paint(DONE, "✓"));
            return follow_or_detach(&run, false);
        }
        Cmd::Discard { card } => {
            daemon()?.call(serde_json::json!({ "op": "review", "repo": repo.root, "card": card, "action": "discard" }))?;
            println!("{} #{card} is ready again; its lane stays", paint(DONE, "✓"));
        }
        Cmd::Queue => {
            let v = daemon()?.call(serde_json::json!({ "op": "queue", "repo": repo.root }))?;
            let jobs: Vec<pit_core::queue::ShipJob> = serde_json::from_value(v["jobs"].clone())?;
            if jobs.is_empty() {
                println!("the queue is empty. Add a lane: pit enqueue <branch>");
            }
            for j in jobs {
                let state = match j.state {
                    pit_core::queue::JobState::Queued => paint(MUTED, "queued  "),
                    pit_core::queue::JobState::Shipping => paint(RUN, "shipping"),
                    pit_core::queue::JobState::Done => paint(DONE, "done    "),
                    pit_core::queue::JobState::Conflict => paint(FAIL, "conflict"),
                    pit_core::queue::JobState::Failed => paint(FAIL, "failed  "),
                    pit_core::queue::JobState::Removed => paint(MUTED, "removed "),
                };
                let extra = if !j.conflicts.is_empty() {
                    format!("  {}", j.conflicts.join(", "))
                } else {
                    j.error.clone().map(|e| format!("  {}", e.lines().next().unwrap_or_default())).unwrap_or_default()
                };
                println!("{state}  {} → {}  {}{}", j.branch, j.target, paint(MUTED, &j.id), paint(MUTED, &extra));
            }
        }
        Cmd::Enqueue { branch, to } => {
            let v = daemon()?.call(serde_json::json!({ "op": "enqueue", "repo": repo.root, "branch": branch, "to": to }))?;
            println!("{} {branch} is in line for {} ({})", paint(DONE, "✓"), v["job"]["target"].as_str().unwrap_or_default(), v["job"]["id"].as_str().unwrap_or_default());
        }
        Cmd::Dequeue { id } => {
            daemon()?.call(serde_json::json!({ "op": "dequeue", "id": id }))?;
            println!("{} {id} left the queue", paint(DONE, "✓"));
        }
        Cmd::Accounts => {
            let (home, pitlane_home) = pit_core::default_homes();
            let routing = pit_core::accounts::Routing::load(&pitlane_home)?;
            let mine = routing.repo_account(&repo.root, &home);
            let quota = daemon_quota();
            let claude = std::env::var("PITLANE_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
            for a in &routing.accounts {
                let st = pit_core::accounts::status(a, &pitlane_home, &claude);
                let signed = if st.logged_in {
                    paint(DONE, "signed in")
                } else if st.problem.is_some() {
                    paint(MUTED, "unknown")
                } else {
                    paint(ACCENT, &format!("not signed in: pit account login {}", a.id))
                };
                let usage = quota
                    .get(&a.id)
                    .map(|u| {
                        let f = |v: Option<f64>| v.map(|p| format!("{p:.0}%")).unwrap_or_else(|| "?".into());
                        format!("5h {} 7d {}", f(u.five_hour), f(u.seven_day))
                    })
                    .unwrap_or_default();
                let here = if a.id == mine { paint(ACCENT, "this repo") } else { String::new() };
                println!("{:<14} {:<13} {signed}  {}  {here}", a.id, format!("{:?}", a.kind).to_lowercase(), paint(MUTED, &usage));
            }
            println!("{}", paint(MUTED, &format!("route: {}", routing.route.run.prefer.join(" → "))));
        }
        Cmd::Account(AccountCmd::Add { id, kind, api_key_helper }) => {
            let (_, pitlane_home) = pit_core::default_homes();
            let kind = match kind.as_str() {
                "subscription" => pit_core::accounts::Kind::Subscription,
                "team" => pit_core::accounts::Kind::Team,
                "api-key" => pit_core::accounts::Kind::ApiKey,
                k => bail!("unknown kind {k}; subscription, team or api-key"),
            };
            let dir = pit_core::accounts::add_account(&pitlane_home, &id, kind, api_key_helper.as_deref())?;
            println!("{} account {id} added to {}", paint(DONE, "✓"), pit_core::accounts::Routing::path(&pitlane_home).display());
            if let Some(d) = dir
                && kind != pit_core::accounts::Kind::ApiKey
            {
                println!("  its Claude Code profile is {}", d.display());
                println!("  sign in with: pit account login {id}");
            }
        }
        Cmd::Account(AccountCmd::Login { id }) => {
            let (_, pitlane_home) = pit_core::default_homes();
            let routing = pit_core::accounts::Routing::load(&pitlane_home)?;
            let a = routing.account(&id).with_context(|| format!("no account {id}; add it with pit account add {id}"))?;
            if a.kind.is_api_key() {
                bail!("{id} is an api-key account: it signs in through its api_key_helper");
            }
            let mut cmd = std::process::Command::new("claude");
            cmd.args(["auth", "login"]).env_remove("CLAUDECODE");
            if let Some(dir) = pit_core::accounts::profile_dir(&id, &pitlane_home) {
                std::fs::create_dir_all(&dir)?;
                cmd.env("CLAUDE_CONFIG_DIR", dir);
            }
            // The official CLI signs you in; Pitlane never sees the result.
            let st = cmd.status().context("could not run claude auth login")?;
            return Ok(if st.success() { ExitCode::SUCCESS } else { ExitCode::FAILURE });
        }
        Cmd::Account(AccountCmd::Use { id }) => {
            let (_, pitlane_home) = pit_core::default_homes();
            pit_core::accounts::set_repo_account(&pitlane_home, &repo.root, &id)?;
            println!("{} {} uses {id} from its next run", paint(DONE, "✓"), repo.name);
        }
        Cmd::Resume { run, detach } => {
            let v = daemon()?.call(serde_json::json!({ "op": "resume", "run": run }))?;
            let m: RunMeta = serde_json::from_value(v["run"].clone())?;
            println!("{} run {} resumes {}", paint(DONE, "✓"), m.id, run);
            return follow_or_detach(&m.id, detach);
        }
        Cmd::Runs { json } => {
            let v = daemon()?.call(serde_json::json!({ "op": "list", "repo": repo.root }))?;
            let runs: Vec<RunMeta> = serde_json::from_value(v["runs"].clone())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&runs)?);
                return Ok(ExitCode::SUCCESS);
            }
            if runs.is_empty() {
                println!("no runs yet. Start one: pit run <card>");
            }
            for m in &runs {
                println!(
                    "{}  {}  #{} {}  {}  {}",
                    paint(MUTED, &m.id),
                    state_word(m.state),
                    m.card,
                    m.card_title,
                    paint(MUTED, &m.branch),
                    paint(MUTED, &run_figures(m))
                );
            }
        }
        Cmd::Log { run, follow } => {
            let mut c = daemon()?;
            if follow {
                return follow_run(&mut c, &run);
            }
            let end = c.follow(&run, 0, |_, e| {
                print_event(&e);
                true
            })?;
            if let Some(m) = end {
                print_end(&m);
            }
        }
        Cmd::Say { run, text } => {
            daemon()?.call(serde_json::json!({ "op": "say", "run": run, "text": text }))?;
            println!("{} sent; the agent reads it after its current step", paint(DONE, "✓"));
        }
        Cmd::Stop { run } => {
            daemon()?.call(serde_json::json!({ "op": "stop", "run": run }))?;
            println!("{} stopping {run}; the agent ends its current step", paint(DONE, "✓"));
        }
        Cmd::Card(c) => return card(&repo, c),
        Cmd::New { branch, no_fetch, no_setup, card, json } => {
            if let Some(id) = card {
                cards::get(&repo, id)?; // fail before creating anything
            }
            let r = actions::new_lane(&repo, &branch, NewOptions { fetch: !no_fetch, setup: !no_setup })?;
            if let Some(id) = card {
                cards::update(&repo, id, CardPatch { lane: Some(Some(r.branch.clone())), ..Default::default() })?;
            }
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

/// A start refused because the next option is an API key: say how to say yes.
fn api_key_hint(e: anyhow::Error) -> anyhow::Error {
    let s = format!("{e:#}");
    match s.strip_prefix(pitd::supervisor::NEEDS_API_KEY) {
        Some(reason) => anyhow::anyhow!("the route reached an API key ({reason}). It is paid per token; run again with --allow-api-key if you agree"),
        None => e,
    }
}

/// Usage per account from the daemon, if it is running.
fn daemon_quota() -> std::collections::HashMap<String, pit_core::accounts::Usage> {
    let (_, home) = pit_core::default_homes();
    let Ok(mut c) = Client::connect(&home, None) else { return Default::default() };
    let Ok(v) = c.call(serde_json::json!({ "op": "quota" })) else { return Default::default() };
    let all: std::collections::HashMap<String, Vec<RunEvent>> = serde_json::from_value(v["quota"].clone()).unwrap_or_default();
    all.into_iter()
        .map(|(acct, evs)| {
            let mut u = pit_core::accounts::Usage::default();
            for e in evs {
                if let RunEvent::Quota { window, used, .. } = e {
                    match window.as_str() {
                        "five_hour" => u.five_hour = Some(used * 100.0),
                        "seven_day" => u.seven_day = Some(used * 100.0),
                        _ => {}
                    }
                }
            }
            (acct, u)
        })
        .collect()
}

fn daemon() -> Result<Client> {
    let (_, home) = pit_core::default_homes();
    Client::connect(&home, std::env::current_exe().ok().as_deref())
}

fn follow_or_detach(run: &str, detach: bool) -> Result<ExitCode> {
    if detach {
        println!("  follow with: pit log {run} -f");
        return Ok(ExitCode::SUCCESS);
    }
    println!("{}", paint(MUTED, &format!("  following. Ctrl-C leaves it running; pit stop {run} stops it")));
    follow_run(&mut daemon()?, run)
}

fn follow_run(c: &mut Client, run: &str) -> Result<ExitCode> {
    let end = c.follow(run, 0, |_, e| {
        print_event(&e);
        true
    })?;
    Ok(match end {
        Some(m) => {
            print_end(&m);
            if m.state == RunState::Done { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        None => ExitCode::SUCCESS,
    })
}

fn state_word(s: RunState) -> String {
    match s {
        RunState::Starting => paint(RUN, "starting"),
        RunState::Running => paint(RUN, "running "),
        RunState::Done => paint(DONE, "done    "),
        RunState::Failed => paint(FAIL, "failed  "),
        RunState::Stopped => paint(MUTED, "stopped "),
        RunState::Interrupted => paint(ACCENT, "interrupted"),
        RunState::Verifying => paint(RUN, "checking"),
    }
}

fn run_figures(m: &RunMeta) -> String {
    let mut parts = Vec::new();
    if let Some(ms) = m.duration_ms {
        parts.push(format!("{}m{:02}s", ms / 60_000, ms / 1000 % 60));
    }
    if let Some(t) = m.turns {
        parts.push(format!("{t} turns"));
    }
    if let Some(c) = m.cost_usd {
        parts.push(format!("${c:.2}"));
    }
    parts.join(", ")
}

fn print_event(e: &RunEvent) {
    match e {
        RunEvent::Started { model, .. } => println!("{} agent started ({model})", paint(RUN, "▸")),
        RunEvent::Text { text } => println!("{}", text.trim_end()),
        RunEvent::Tool { name, detail, .. } => println!("  {} {}", paint(MUTED, name), paint(MUTED, detail)),
        RunEvent::ToolDone { ok: false, detail, .. } => println!("    {} {}", paint(FAIL, "✕"), paint(MUTED, detail)),
        RunEvent::ToolDone { .. } | RunEvent::Quota { .. } => {}
        RunEvent::Said { text } => println!("{} {text}", paint(ACCENT, "you:")),
        RunEvent::Note { text } => println!("{}", paint(MUTED, text)),
        RunEvent::Proof { command, ok, duration_ms } => println!(
            "{} {command}  {}",
            if *ok { paint(DONE, "✓") } else { paint(FAIL, "✕") },
            paint(MUTED, &format!("{:.1}s", *duration_ms as f64 / 1000.0))
        ),
        RunEvent::Judged { verdict, summary, findings } => println!(
            "{} judge {}: {summary} ({findings} finding(s))",
            paint(ACCENT, "?"),
            if *verdict == pit_core::verify::Verdict::Approve { "approves" } else { "asks for changes" }
        ),
        RunEvent::Limited { window, .. } => println!("{} hit the {} usage limit", paint(ACCENT, "!"), window.replace('_', "-")),
        RunEvent::Approval { approval } => println!(
            "{} waiting for you: {} {}  {}",
            paint(ACCENT, "?"),
            approval.tool,
            approval.detail,
            paint(MUTED, "pit allow <run> [--run-wide|--always] | pit deny <run>")
        ),
        RunEvent::Decided { decision, message, .. } => println!(
            "  {} {}",
            paint(MUTED, &format!("{decision:?}").to_lowercase()),
            paint(MUTED, message.as_deref().unwrap_or_default())
        ),
        RunEvent::Finished { ok, handoff, denied, .. } => {
            if let Some(s) = handoff.as_ref().and_then(|h| h.get("summary")).and_then(|s| s.as_str()) {
                println!("\n{} {s}", if *ok { paint(DONE, "✓") } else { paint(FAIL, "✕") });
            }
            for d in denied {
                println!("  {} not allowed: {d}", paint(ACCENT, "!"));
            }
        }
        RunEvent::Exited { .. } => {}
    }
}

fn print_end(m: &RunMeta) {
    println!("{} {}  {}", state_word(m.state), m.id, paint(MUTED, &run_figures(m)));
    if let Some(e) = &m.error {
        println!("  {}", paint(FAIL, e.lines().next().unwrap_or_default()));
    }
}

fn card(repo: &Repo, cmd: CardCmd) -> Result<ExitCode> {
    match cmd {
        CardCmd::Add { title, body, body_file, tags } => {
            let body = match body_file {
                Some(p) if p.as_os_str() == "-" => std::io::read_to_string(std::io::stdin())?,
                Some(p) => std::fs::read_to_string(&p)?,
                None => body.unwrap_or_default(),
            };
            let c = cards::add(repo, NewCard { title, body, tags, questions: Vec::new() })?;
            println!("{} #{} {}", paint(DONE, "✓"), c.id, c.title);
            println!("  {}", c.path.display());
        }
        CardCmd::List { state, json } => {
            let state = state.map(|s| CardState::parse(&s)).transpose()?;
            let (all, problems) = cards::list(repo)?;
            let shown: Vec<_> = all.into_iter().filter(|c| state.is_none_or(|s| c.state == s)).collect();
            if json {
                let out: Vec<CardOut> = shown.into_iter().map(CardOut::from).collect();
                println!("{}", serde_json::to_string_pretty(&out)?);
                return Ok(ExitCode::SUCCESS);
            }
            if shown.is_empty() {
                println!("no cards. Add one: pit card add \"<title>\"");
            }
            for c in &shown {
                let open = c.open_questions();
                let asks = if open > 0 { paint(ACCENT, &format!("  {open} to answer")) } else { String::new() };
                let lane = c.lane.as_deref().map(|l| paint(MUTED, &format!("  {l}"))).unwrap_or_default();
                println!("{}  {:<8}  {}{asks}{lane}", paint(MUTED, &format!("#{:04}", c.id)), c.state.as_str(), c.title);
            }
            for p in problems {
                eprintln!("{} {p}", paint(FAIL, "skipped:"));
            }
        }
        CardCmd::Show { id, json } => {
            let c = cards::get(repo, id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&CardOut::from(c))?);
                return Ok(ExitCode::SUCCESS);
            }
            println!("#{} {}  ({}{})", c.id, c.title, c.state.as_str(), c.lane.as_deref().map(|l| format!(", lane {l}")).unwrap_or_default());
            if !c.body.is_empty() {
                println!("\n{}", c.body);
            }
            for q in &c.questions {
                println!("\n{} {}", paint(if q.is_open() { ACCENT } else { MUTED }, &format!("Q{}", q.id)), q.text);
                for (i, o) in q.options.iter().enumerate() {
                    let rec = if q.recommended == Some(i) { "  (recommended)" } else { "" };
                    println!("   {}. {o}{rec}", i + 1);
                }
                if let Some(a) = &q.answer {
                    println!("   {} {a}  {}", paint(DONE, "→"), paint(MUTED, q.decision.as_deref().unwrap_or("")));
                }
            }
        }
        CardCmd::Move { id, state } => {
            let c = cards::update(repo, id, CardPatch { state: Some(CardState::parse(&state)?), ..Default::default() })?;
            println!("{} #{} is {}", paint(DONE, "✓"), c.id, c.state.as_str());
        }
        CardCmd::Ask { id, question, options, recommend } => {
            let recommended = match recommend {
                Some(0) => bail!("--recommend counts from 1"),
                Some(n) => Some(n - 1),
                None => None,
            };
            let c = cards::ask(repo, id, vec![NewQuestion { text: question, options, recommended }])?;
            println!("{} #{} has {} open question(s)", paint(DONE, "✓"), c.id, c.open_questions());
        }
        CardCmd::Answer { id, question, answer } => {
            let a = cards::answer(repo, id, question, &answer)?;
            println!("{} recorded as {} in {}", paint(DONE, "✓"), a.decision, cards::DECISIONS);
        }
        CardCmd::Link { id, branch, unlink } => {
            if branch.is_none() && !unlink {
                bail!("name a branch, or pass --unlink");
            }
            let c = cards::update(repo, id, CardPatch { lane: Some(branch), ..Default::default() })?;
            match &c.lane {
                Some(l) => println!("{} #{} is worked on {l}", paint(DONE, "✓"), c.id),
                None => println!("{} #{} has no lane", paint(DONE, "✓"), c.id),
            }
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
