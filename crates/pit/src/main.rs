//! `pit`: the Pitlane CLI. Parses arguments, calls `pit-core`, prints.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;
use pit_core::Repo;

use cli::{AccountCmd, Cli, Cmd};
use cmd::{accounts, cards, lanes, review, runs};
use term::{FAIL, claude_bin, paint_err};

mod cli;
mod cmd;
mod mcp;
mod term;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{} {e:#}", paint_err(FAIL, "pit:"));
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
    if std::env::var_os(mcp::RUN_ENV).is_some() && !cli.cmd.open_to_a_run() {
        anyhow::bail!("not available to an agent working a card: answers, moves, ships and approvals come from the person, and Pitlane moves the card when the run ends");
    }
    if let Cmd::Daemon = cli.cmd {
        let home = cmd::pitlane_home();
        let claude_bin = claude_bin();
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
    // Opened only by the commands that work on a repository: answering an agent (`pit allow`),
    // following a run or adding an account works from any folder.
    let dir = cli.dir;
    let repo = || open(dir.clone());
    match cli.cmd {
        Cmd::Init { force } => lanes::init(&repo()?, force),
        Cmd::Lanes { no_fetch, json } => lanes::lanes(&repo()?, no_fetch, json),
        Cmd::Mcp { .. } | Cmd::Daemon => unreachable!("handled above"),
        Cmd::Approvals => runs::approvals(),
        Cmd::Allow { run, id, run_wide, always } => runs::allow(run, id, run_wide, always),
        Cmd::Deny { run, id, message } => runs::deny(run, id, message),
        Cmd::Run { card, detach, allow_api_key, no_brief, fresh } => runs::start(&repo()?, card, detach, allow_api_key, no_brief, fresh),
        Cmd::Continue { run, allow_api_key, detach } => runs::continue_run(run, allow_api_key, detach),
        Cmd::Review { card } => review::review(&repo()?, card),
        Cmd::Approve { card } => review::approve(&repo()?, card),
        Cmd::Changes { card, message } => review::changes(&repo()?, card, message),
        Cmd::Discard { card } => review::discard(&repo()?, card),
        Cmd::Plan { json } => cards::plan(&repo()?, json),
        Cmd::Index => cards::index(&repo()?),
        Cmd::Brief { card, text, json, budget } => cards::brief(&repo()?, card, text, json, budget),
        Cmd::Queue => review::queue(&repo()?),
        Cmd::Enqueue { branch, to } => review::enqueue(&repo()?, branch, to),
        Cmd::Dequeue { id } => review::dequeue(id),
        Cmd::Accounts => accounts::accounts(&repo()?),
        Cmd::Account(AccountCmd::Add { id, kind, api_key_helper }) => accounts::add(id, kind, api_key_helper),
        Cmd::Account(AccountCmd::Login { id }) => accounts::login(id),
        Cmd::Account(AccountCmd::Use { id }) => accounts::use_for_repo(&repo()?, id),
        Cmd::Resume { run, detach } => runs::resume(run, detach),
        Cmd::Runs { json } => runs::runs(&repo()?, json),
        Cmd::Log { run, follow } => runs::log(run, follow),
        Cmd::Say { run, text } => runs::say(run, text),
        Cmd::Stop { run } => runs::stop(run),
        Cmd::Card(c) => cards::card(&repo()?, c),
        Cmd::New { branch, no_fetch, no_setup, card, json } => lanes::new(&repo()?, branch, no_fetch, no_setup, card, json),
        Cmd::Ship { branch, to, yes, no_fetch } => lanes::ship(&repo()?, branch, to.as_deref(), yes, !no_fetch),
        Cmd::Clean { yes, dry_run, undo, trash: list_trash, no_fetch } => match undo {
            Some(b) => lanes::undo(&repo()?, &b),
            None if list_trash => lanes::list_trash(&repo()?),
            None => lanes::clean(&repo()?, yes, dry_run, !no_fetch),
        },
    }
}
