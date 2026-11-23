//! Owns the agent processes: start a card's run in its worktree, read its events, let the
//! person talk to it, stop it, and record everything under `~/.pitlane/runs/<id>/`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use pit_core::runs::{Decision, RunEvent, RunMeta, SessionKind};
use tokio::process::ChildStdin;
use tokio::sync::{broadcast, oneshot};

mod approvals;
mod autopilot;
mod card;
mod consult;
mod control;
mod launch;
mod moves;
mod queue;
mod record;
mod route;
mod run;
mod side;
mod unblock;
mod verify;

pub use launch::EFFORTS;

pub struct Config {
    /// `~/.pitlane` (or `$PITLANE_HOME`).
    pub home: PathBuf,
    /// The Claude Code executable: `claude`, or a stand-in for tests.
    pub claude_bin: String,
    /// The `pit` executable the agent starts as its approval MCP server (`pit mcp --approve <run>`).
    /// `None`: no approvals; anything outside the allowed tools is denied.
    pub pit_bin: Option<PathBuf>,
}

/// What a person answered to an approval.
pub type Answer = (Decision, Option<String>);

/// An error that starts with this means: the route's next usable option is an API key and needs
/// the person's yes (`allow_api_key`). The rest of the message says why.
pub const NEEDS_API_KEY: &str = "needs-api-key: ";

#[derive(Debug, Clone, Default)]
pub struct StartOpts {
    /// The person said yes to the API key for this run.
    pub allow_api_key: bool,
    /// This run continues that one, which hit a usage limit.
    pub handoff_from: Option<String>,
    /// For a resumed run: the first message instead of "continue" (a person's review comments).
    pub message: Option<String>,
    /// Review round this run belongs to.
    pub round: u32,
    /// Leave Pitlane's map out of the first message (to compare runs with and without it).
    pub no_brief: bool,
    /// Run all's "Into": the branch a card without a lane lands in. Its lane starts from it
    /// (when it is a lane) and merges back into it.
    pub into: Option<String>,
    /// Work directly in `into`'s worktree, without a lane of its own.
    pub direct: bool,
    /// What the person said for the whole batch (Run all's "when it is all done"), passed on.
    pub instructions: Option<String>,
    /// Run all: take the next step without a person (see `RunMeta::autopilot`).
    pub autopilot: bool,
    /// Continuations after running out of turns so far.
    pub extensions: u32,
    /// Start over: do not resume the session of the card's stopped run.
    pub fresh: bool,
}

/// What a person decides about a card in review.
pub use pit_core::protocol::ReviewAction;

/// What the daemon holds for a run while its process lives.
struct Live {
    meta: RunMeta,
    stdin: Option<ChildStdin>,
    /// A turn is in flight: a message was written, or the agent started one by itself, and its
    /// result has not been read yet. Stdin closes once no turn is, which is how a stream-json
    /// session ends. Messages are not counted against results: Claude Code folds a message that
    /// arrives mid-turn into that turn, so it never gets a result of its own, and a session
    /// waiting for one would stay open (and count as work) for ever.
    turn: bool,
    /// A message was written while the turn ran: if Claude Code did not fold it in, it answers
    /// it in a turn of its own right after the result, so the session waits a little longer.
    said: bool,
    results: usize,
    /// Background tasks (a test suite, a server) the agent left running: when one reports back,
    /// Claude Code starts a turn by itself, so stdin stays open until they are done.
    background: usize,
    stop_requested: bool,
    events: usize,
    tx: broadcast::Sender<(usize, RunEvent)>,
    /// Rules allowed "for this run".
    rules: Vec<String>,
    /// It hit a usage limit: when it ends, the route continues the card elsewhere.
    limited: bool,
    pending: HashMap<String, oneshot::Sender<Answer>>,
}

pub struct Supervisor {
    pub cfg: Config,
    live: Arc<Mutex<HashMap<String, Live>>>,
    /// Last usage events seen, per account.
    quota: Mutex<HashMap<String, Vec<RunEvent>>>,
    /// The merge queue, shipped one job at a time by `run_queue`.
    queue: Mutex<Vec<pit_core::queue::ShipJob>>,
    queue_wake: tokio::sync::Notify,
    /// Bumped by Stop all: work queued before it (analyses waiting for a slot) never starts.
    epoch: AtomicU64,
    /// Cards being started (routing can take a while): a second start of the same card waits out.
    starting: Mutex<std::collections::HashSet<(PathBuf, u32)>>,
    /// Analyses across every request: three at a time, one per card.
    analysis_gate: Arc<tokio::sync::Semaphore>,
    analysing: Mutex<std::collections::HashSet<(PathBuf, u32)>>,
    /// Performance: Full or Light, and the governor that keeps the agents to it.
    pub perf: crate::perf::Perf,
    /// Presence: whether runs ask you their calls (Here) or go with their recommendation (Away).
    pub presence: crate::presence::Presence,
    /// How often a run waiting on your answer looks at the card again, in milliseconds.
    consult_poll_ms: AtomicU64,
    /// How long an agent may take to leave once its session was closed, in milliseconds.
    leave_ms: AtomicU64,
    /// How long what a card's agent left running (a push, a server) holds its session after the
    /// handoff, in milliseconds.
    left_running_ms: AtomicU64,
    /// Held while a run's record is snapshotted and written, so records land in the order they
    /// were taken: an older snapshot never overwrites a newer one.
    meta_writes: Mutex<()>,
    /// Process groups of the one-shot calls in flight (a card's description, a report's digest):
    /// they are not sessions, but shutting down ends them too.
    one_shots: Mutex<std::collections::HashSet<u32>>,
}

pub struct Subscription {
    pub replay: Vec<(usize, RunEvent)>,
    /// `None` when the run is not live: the replay is all there is.
    pub rx: Option<broadcast::Receiver<(usize, RunEvent)>>,
}

/// Analyses that may run at once, across every request.
const ANALYSES_AT_ONCE: usize = 3;
/// How long an agent may take to leave once its session was closed, unless tests shorten it.
const LEAVE_MS: u64 = 30_000;
/// How long what a card's agent left running holds its session after the handoff, unless tests
/// shorten it.
const LEFT_RUNNING_MS: u64 = 120_000;

impl Supervisor {
    pub fn new(cfg: Config) -> Arc<Self> {
        let home = cfg.home.clone();
        let (queue, unreadable) = pit_core::queue::load_or_set_aside(&cfg.home);
        if let Some(why) = unreadable {
            eprintln!("pitd: merge queue: {why}");
        }
        Arc::new(Self {
            cfg,
            live: Arc::new(Mutex::new(HashMap::new())),
            quota: Mutex::new(HashMap::new()),
            queue: Mutex::new(queue),
            queue_wake: tokio::sync::Notify::new(),
            epoch: AtomicU64::new(0),
            starting: Mutex::new(Default::default()),
            analysis_gate: Arc::new(tokio::sync::Semaphore::new(ANALYSES_AT_ONCE)),
            analysing: Mutex::new(Default::default()),
            perf: crate::perf::Perf::new(&home),
            presence: crate::presence::Presence::new(&home),
            consult_poll_ms: AtomicU64::new(2_000),
            leave_ms: AtomicU64::new(LEAVE_MS),
            left_running_ms: AtomicU64::new(LEFT_RUNNING_MS),
            meta_writes: Mutex::new(()),
            one_shots: Mutex::default(),
        })
    }

    /// How long a finished session is waited for (tests shorten it): `leave` for the agent to go
    /// once its session was closed, `left_running` for what a card's agent left running after
    /// its handoff.
    pub fn set_patience(&self, leave: Duration, left_running: Duration) {
        self.leave_ms
            .store(leave.as_millis() as u64, Ordering::Relaxed);
        self.left_running_ms
            .store(left_running.as_millis() as u64, Ordering::Relaxed);
    }

    /// A repository's runs, newest first. Side sessions (analysis, judge) are left out.
    pub fn list(&self, repo: Option<&Path>) -> Result<Vec<RunMeta>> {
        Ok(self
            .sessions(repo)?
            .into_iter()
            .filter(|m| m.kind == SessionKind::Run)
            .collect())
    }

    /// A card's latest run: the newest to start, then the higher round when two started in the
    /// same second. By start first: a card started over after review rounds has a round-0 run
    /// that is newer than its round-2 one.
    fn latest_run(&self, root: &Path, card: u32) -> Option<RunMeta> {
        self.list(Some(root))
            .ok()?
            .into_iter()
            .filter(|m| m.card == card)
            .max_by_key(|m| (m.started_at, m.round))
    }

    /// Every session, runs and side sessions, newest first. The records are read before the
    /// live list is locked: every agent's output waits on that lock.
    pub fn sessions(&self, repo: Option<&Path>) -> Result<Vec<RunMeta>> {
        let disk = self.list_disk()?;
        let live = self.live.lock().unwrap();
        let mut all: Vec<RunMeta> = disk
            .into_iter()
            .map(|m| live.get(&m.id).map(|l| l.meta.clone()).unwrap_or(m))
            .filter(|m| repo.is_none_or(|r| m.repo == r))
            .collect();
        all.sort_by(|a, b| {
            b.started_at
                .cmp(&a.started_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(all)
    }

    pub fn get(&self, id: &str) -> Result<RunMeta> {
        if let Some(l) = self.live.lock().unwrap().get(id) {
            return Ok(l.meta.clone());
        }
        let t = fs::read_to_string(self.dir(id).join("meta.json"))
            .with_context(|| format!("no run {id}"))?;
        Ok(serde_json::from_str(&t)?)
    }

    /// Last usage events seen, per account.
    pub fn quota(&self) -> HashMap<String, Vec<RunEvent>> {
        self.quota.lock().unwrap().clone()
    }

    /// The replay from `from`, then live events. The receiver and the count of events written
    /// so far are taken under one lock, the file is read after it: the replay stops at that count
    /// and the receiver starts there, so nothing falls between or comes twice.
    pub fn subscribe(&self, id: &str, from: usize) -> Result<Subscription> {
        let live = self.live.lock().unwrap();
        if let Some(l) = live.get(id) {
            let (rx, written) = (l.tx.subscribe(), l.events);
            drop(live);
            let mut replay = self.read_timeline(id, from);
            replay.retain(|(i, _)| *i < written);
            return Ok(Subscription { replay, rx: Some(rx) });
        }
        drop(live);
        self.get(id)?;
        Ok(Subscription {
            replay: self.read_timeline(id, from),
            rx: None,
        })
    }
}

fn home_dir() -> PathBuf {
    pit_core::default_homes().0
}
