//! The command line: every subcommand and flag. The doc comments are `pit --help`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "pit", version, about = "Pitlane: every lane in view, only the ones that need you light up")]
pub struct Cli {
    /// Run as if started in this directory.
    #[arg(short = 'C', global = true, value_name = "PATH")]
    pub dir: Option<PathBuf>,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
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
    /// Cards: the board's work items, kept outside the repository in ~/.pitlane/boards.
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
        /// Leave Pitlane's map of where to look out of the first message.
        #[arg(long)]
        no_brief: bool,
        /// Start over: a card stopped mid-work otherwise resumes its session.
        #[arg(long)]
        fresh: bool,
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
    /// Index the repository's symbols (tree-sitter), for briefings. Incremental.
    Index,
    /// Which open cards can run together, and which would touch the same files.
    Plan {
        #[arg(long)]
        json: bool,
    },
    /// The briefing a run would get: where to look for a card (or for a text with --text).
    Brief {
        card: Option<u32>,
        #[arg(long)]
        text: Option<String>,
        /// Print JSON (files, scores, why).
        #[arg(long)]
        json: bool,
        /// Size in characters; 4000 or less gives the lean map (paths and named symbols).
        #[arg(long, default_value_t = pit_core::brief::DEFAULT_BUDGET)]
        budget: usize,
    },
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
pub enum AccountCmd {
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
pub enum CardCmd {
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
