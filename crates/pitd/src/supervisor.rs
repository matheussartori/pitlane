//! Owns the agent processes: start a card's run in its worktree, read its events, let the
//! person talk to it, stop it, and record everything under `~/.pitlane/runs/<id>/`.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use pit_core::Repo;
use pit_core::accounts::{self, Choice, Pick, Routing, Situation, Usage};
use pit_core::actions::{self, NewOptions};
use pit_core::cards::{self, CardPatch, CardState, NewQuestion};
use pit_core::flow;
use pit_core::flow::RUN_BASE_TOOLS;
use pit_core::lanes::now_secs;
use pit_core::runs::{self, Approval, Decision, RunEvent, RunMeta, RunState, SessionKind};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{broadcast, oneshot};

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
}

/// What a person decides about a card in review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewAction {
    Approve,
    Changes,
    Discard,
}

/// What the daemon holds for a run while its process lives.
struct Live {
    meta: RunMeta,
    stdin: Option<ChildStdin>,
    /// User messages written to stdin, and results read back: stdin closes when they match,
    /// which is how a stream-json session ends.
    sent: usize,
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
}

/// What a side session needs: who it is for, where it runs, and Claude Code's arguments
/// (without the stream-json ones, which `side_session` adds).
struct Side {
    kind: SessionKind,
    repo: PathBuf,
    cwd: PathBuf,
    card: u32,
    card_title: String,
    parent: Option<String>,
    account: String,
    config_dir: Option<PathBuf>,
    model: Option<String>,
    args: Vec<String>,
    prompt: String,
    limit: Duration,
    /// Give it Pitlane's board tools too (`pit mcp`: add_card, list_cards, update_card, add_note).
    board: bool,
    /// Images sent with the prompt (the card's screenshots, for the analysis).
    images: Vec<(&'static str, Vec<u8>)>,
}

pub struct Subscription {
    pub replay: Vec<(usize, RunEvent)>,
    /// `None` when the run is not live: the replay is all there is.
    pub rx: Option<broadcast::Receiver<(usize, RunEvent)>>,
}

impl Supervisor {
    pub fn new(cfg: Config) -> Arc<Self> {
        let home = cfg.home.clone();
        let queue = pit_core::queue::load(&cfg.home);
        Arc::new(Self {
            cfg,
            live: Arc::new(Mutex::new(HashMap::new())),
            quota: Mutex::new(HashMap::new()),
            queue: Mutex::new(queue),
            queue_wake: tokio::sync::Notify::new(),
            epoch: AtomicU64::new(0),
            starting: Mutex::new(Default::default()),
            analysis_gate: Arc::new(tokio::sync::Semaphore::new(3)),
            analysing: Mutex::new(Default::default()),
            perf: crate::perf::Perf::new(&home),
        })
    }

    fn dir(&self, id: &str) -> PathBuf {
        runs::runs_dir(&self.cfg.home).join(id)
    }

    /// Runs left open by a daemon that went away are marked interrupted.
    pub fn recover(&self) -> Result<usize> {
        let mut n = 0;
        for mut m in self.list_disk()? {
            if m.state.is_live() {
                // An agent the last daemon left behind (it crashed or was killed) must not keep
                // editing the worktree with nobody watching: end its group, if it is still ours.
                if let Some(pid) = m.pid
                    && still_agent(pid)
                {
                    signal(pid, libc::SIGTERM);
                    std::thread::sleep(Duration::from_millis(300));
                    signal(pid, libc::SIGKILL);
                }
                if m.kind == SessionKind::Run {
                    let _ = set_card_state(&m, CardState::Ready, &self.cfg.home);
                }
                m.pid = None;
                m.state = RunState::Interrupted;
                m.ended_at.get_or_insert(now_secs());
                m.error
                    .get_or_insert_with(|| "the daemon stopped while this run was open".into());
                self.write_meta(&m)?;
                n += 1;
            }
        }
        Ok(n)
    }

    fn list_disk(&self) -> Result<Vec<RunMeta>> {
        let dir = runs::runs_dir(&self.cfg.home);
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(&dir) else {
            return Ok(out);
        };
        for e in rd.flatten() {
            if let Ok(t) = fs::read_to_string(e.path().join("meta.json"))
                && let Ok(m) = serde_json::from_str::<RunMeta>(&t)
            {
                out.push(m);
            }
        }
        Ok(out)
    }

    /// A repository's runs, newest first. Side sessions (analysis, judge) are left out.
    pub fn list(&self, repo: Option<&Path>) -> Result<Vec<RunMeta>> {
        Ok(self
            .sessions(repo)?
            .into_iter()
            .filter(|m| m.kind == SessionKind::Run)
            .collect())
    }

    /// Every session, runs and side sessions, newest first.
    pub fn sessions(&self, repo: Option<&Path>) -> Result<Vec<RunMeta>> {
        let live = self.live.lock().unwrap();
        let mut all: Vec<RunMeta> = self
            .list_disk()?
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

    fn write_meta(&self, m: &RunMeta) -> Result<()> {
        let dir = self.dir(&m.id);
        fs::create_dir_all(&dir)?;
        let tmp = dir.join("meta.json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(m)?)?;
        fs::rename(&tmp, dir.join("meta.json"))?;
        Ok(())
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut RunMeta)) {
        let snapshot = {
            let mut live = self.live.lock().unwrap();
            let Some(l) = live.get_mut(id) else { return };
            f(&mut l.meta);
            l.meta.clone()
        };
        let _ = self.write_meta(&snapshot);
    }

    /// Appends an event to the run's timeline and tells everyone following it.
    fn emit(&self, id: &str, ev: RunEvent) {
        let mut live = self.live.lock().unwrap();
        let Some(l) = live.get_mut(id) else { return };
        let i = l.events;
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir(id).join("timeline.jsonl"))
            && let Ok(line) = serde_json::to_string(&ev)
        {
            let _ = writeln!(f, "{line}");
        }
        l.events += 1;
        let _ = l.tx.send((i, ev));
    }

    pub fn read_timeline(&self, id: &str, from: usize) -> Vec<(usize, RunEvent)> {
        fs::read_to_string(self.dir(id).join("timeline.jsonl"))
            .unwrap_or_default()
            .lines()
            .enumerate()
            .skip(from)
            .filter_map(|(i, l)| serde_json::from_str(l).ok().map(|e| (i, e)))
            .collect()
    }

    /// The replay from `from`, then live events. Taken under one lock, so nothing falls between.
    pub fn subscribe(&self, id: &str, from: usize) -> Result<Subscription> {
        let live = self.live.lock().unwrap();
        if let Some(l) = live.get(id) {
            let rx = l.tx.subscribe();
            return Ok(Subscription {
                replay: self.read_timeline(id, from),
                rx: Some(rx),
            });
        }
        drop(live);
        self.get(id)?;
        Ok(Subscription {
            replay: self.read_timeline(id, from),
            rx: None,
        })
    }

    /// Usage per account, in percent, from the last events seen.
    fn usage(&self) -> HashMap<String, Usage> {
        self.quota
            .lock()
            .unwrap()
            .iter()
            .map(|(acct, evs)| {
                let mut u = Usage::default();
                for e in evs {
                    if let RunEvent::Quota { window, used, .. } = e {
                        match window.as_str() {
                            "five_hour" => u.five_hour = Some(used * 100.0),
                            "seven_day" => u.seven_day = Some(used * 100.0),
                            _ => {}
                        }
                    }
                }
                (acct.clone(), u)
            })
            .collect()
    }

    /// The route's choice for a new run of this repository. A resumed run keeps its account and
    /// model (the session lives there); a continuation skips the accounts that hit their limit.
    fn choose(
        &self,
        r: &Repo,
        resume: Option<&RunMeta>,
        opts: &StartOpts,
    ) -> Result<(Choice, String, Vec<String>)> {
        let routing = Routing::load(&self.cfg.home)?;
        if let Some(prev) = resume {
            let a = routing
                .account(&prev.account)
                .with_context(|| format!("account {} is gone from routing.toml", prev.account))?;
            let choice = Choice {
                account: a.id.clone(),
                kind: a.kind,
                model: prev.model.clone(),
                config_dir: accounts::profile_dir(&a.id, &self.cfg.home),
                api_key_helper: a.api_key_helper.clone(),
            };
            return Ok((
                choice,
                format!("resumes {} in the same session", prev.id),
                prev.exhausted.clone(),
            ));
        }
        let exhausted = match &opts.handoff_from {
            Some(prev) => {
                let p = self.get(prev)?;
                let mut e = p.exhausted.clone();
                e.push(p.account.clone());
                e
            }
            None => Vec::new(),
        };
        let choices = routing.choices(&r.root, &home_dir(), &self.cfg.home)?;
        let running: HashMap<String, usize> =
            self.live
                .lock()
                .unwrap()
                .values()
                .fold(HashMap::new(), |mut m, l| {
                    *m.entry(l.meta.account.clone()).or_default() += 1;
                    m
                });
        let max_parallel: HashMap<String, u32> = routing
            .accounts
            .iter()
            .filter_map(|a| a.max_parallel.map(|n| (a.id.clone(), n)))
            .collect();
        let usage = self.usage();
        let claude = self.cfg.claude_bin.clone();
        let home = self.cfg.home.clone();
        let routing_ref = &routing;
        // Signed out only when the official CLI says so; when it cannot tell, the run will say.
        let logged_in = move |c: &Choice| {
            routing_ref.account(&c.account).is_none_or(|a| {
                let s = accounts::status(a, &home, &claude);
                s.logged_in || s.problem.is_some()
            })
        };
        let pick = accounts::pick(
            &choices,
            &routing.route.run.switch_at,
            &Situation {
                usage: &usage,
                running: &running,
                max_parallel: &max_parallel,
                logged_in: &logged_in,
                allow_api_key: opts.allow_api_key,
                skip: &exhausted,
            },
        );
        match pick {
            Pick::Run { choice, reason } => Ok((choice, reason, exhausted)),
            Pick::NeedsApiKey { reason, .. } => bail!("{NEEDS_API_KEY}{reason}"),
            Pick::Nothing { reason } => bail!("{reason}"),
        }
    }

    /// Starts a run of `card` in `repo`. Returns at once; preparing the worktree and the agent's
    /// work show up as events.
    pub fn start(
        self: &Arc<Self>,
        repo: PathBuf,
        card: u32,
        resume: Option<String>,
        opts: StartOpts,
    ) -> Result<RunMeta> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        let c = cards::get(&r, card)?;
        if c.open_questions() > 0 {
            bail!(
                "card #{card} has {} open question(s); answer them in the Inbox first",
                c.open_questions()
            );
        }
        {
            let live = self.live.lock().unwrap();
            if let Some(l) = live.values().find(|l| {
                l.meta.kind == SessionKind::Run && l.meta.repo == r.root && l.meta.card == card
            }) {
                bail!("card #{card} is already running ({})", l.meta.id);
            }
            // Claimed under the same lock as the check, released once the run is in `live`.
            if !self.starting.lock().unwrap().insert((r.root.clone(), card)) {
                bail!("card #{card} is already starting");
            }
        }
        let claim = (r.root.clone(), card);
        let _claim = Release(&self.starting, claim);
        let prev = resume.as_deref().map(|p| self.get(p)).transpose()?;
        let (choice, reason, exhausted) = self.choose(&r, prev.as_ref(), &opts)?;
        let (session_id, resumed_from) = match &prev {
            Some(p) => (p.session_id.clone(), Some(p.id.clone())),
            None => (uuid::Uuid::new_v4().to_string(), None),
        };
        let now = now_secs();
        let id = format!(
            "{card}-{now}-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..4]
        );
        let meta = RunMeta {
            id: id.clone(),
            repo: r.root.clone(),
            card,
            card_title: c.title.clone(),
            branch: c.lane.clone().unwrap_or_default(),
            worktree: PathBuf::new(),
            agent: "claude-code".into(),
            model: choice.model.clone().or_else(|| r.flow.run.model.clone()),
            effort: None,
            session_id,
            pid: None,
            state: RunState::Starting,
            started_at: now,
            ended_at: None,
            cost_usd: None,
            duration_ms: None,
            turns: None,
            tokens: Default::default(),
            handoff: None,
            error: None,
            denied: Vec::new(),
            resumed_from,
            waiting: Vec::new(),
            account: choice.account.clone(),
            reason: Some(reason),
            handoff_from: opts.handoff_from.clone(),
            exhausted,
            next: None,
            resets_at: None,
            round: opts.round,
            verdict: None,
            proofs_ok: None,
            base_sha: None,
            brief: None,
            kind: SessionKind::Run,
            parent: None,
            autopilot: opts.autopilot,
            extensions: opts.extensions,
            related: Vec::new(),
        };
        fs::create_dir_all(self.dir(&id))?;
        self.write_meta(&meta)?;
        let (tx, _) = broadcast::channel(1024);
        self.live.lock().unwrap().insert(
            id.clone(),
            Live {
                meta: meta.clone(),
                stdin: None,
                sent: 0,
                results: 0,
                background: 0,
                stop_requested: false,
                events: 0,
                tx,
                rules: Vec::new(),
                limited: false,
                pending: HashMap::new(),
            },
        );
        let me = Arc::clone(self);
        let run = id.clone();
        tokio::spawn(async move {
            if let Err(e) = me
                .clone()
                .drive(run.clone(), repo, card, resume.is_some(), choice, opts)
                .await
            {
                me.fail(&run, &format!("{e:#}"));
            }
        });
        Ok(meta)
    }

    fn fail(&self, id: &str, why: &str) {
        self.emit(
            id,
            RunEvent::Note {
                text: why.to_owned(),
            },
        );
        self.update(id, |m| {
            m.state = RunState::Failed;
            m.error = Some(why.to_owned());
            m.ended_at = Some(now_secs());
        });
        self.emit(
            id,
            RunEvent::Exited {
                code: None,
                state: RunState::Failed,
            },
        );
        if let Some(m) = self.live.lock().unwrap().remove(id).map(|l| l.meta) {
            let _ = set_card_state(&m, CardState::Blocked, &self.cfg.home);
        }
    }

    async fn drive(
        self: Arc<Self>,
        id: String,
        repo: PathBuf,
        card: u32,
        resuming: bool,
        choice: Choice,
        opts: StartOpts,
    ) -> Result<()> {
        let StartOpts {
            handoff_from,
            message,
            no_brief,
            into,
            direct,
            instructions,
            ..
        } = opts;
        // Worktree and branch: blocking git work, off the async threads.
        let home = self.cfg.home.clone();
        let me = Arc::clone(&self);
        let rid = id.clone();
        let prev = handoff_from.as_deref().map(|p| self.get(p)).transpose()?;
        let prev_texts = handoff_from
            .as_deref()
            .map(|p| last_words(&self.read_timeline(p, 0)))
            .unwrap_or_default();
        let repo_for_prep = repo.clone();
        let prep = tokio::task::spawn_blocking(move || {
            prepare(
                &repo_for_prep,
                card,
                &home,
                prev.as_ref().map(|p| (p, prev_texts.as_str())),
                !no_brief,
                into.as_deref(),
                direct,
                |t| me.emit(&rid, RunEvent::Note { text: t }),
            )
        })
        .await??;
        // One agent per worktree: two in the same checkout would edit over each other.
        if let Some(other) = self.live.lock().unwrap().values().find(|l| {
            l.meta.id != id && l.meta.kind == SessionKind::Run && l.meta.worktree == prep.worktree
        }) {
            bail!(
                "#{} is already working in {}; this card starts when it ends",
                other.meta.card,
                prep.worktree.display()
            );
        }
        let mut prep = prep;
        if let Some(i) = instructions
            .as_deref()
            .map(str::trim)
            .filter(|i| !i.is_empty())
        {
            let end = "\nWhen you finish, end with the handoff.\n";
            let at = prep.briefing.rfind(end).unwrap_or(prep.briefing.len());
            prep.briefing.insert_str(at, &format!("\n## What the person asked for this whole batch\n\nFollow what applies while you work this card (what is for the very end is done later, not by you):\n\n{i}\n"));
        }
        let base_sha = pit_core::git::Git::new(&prep.worktree)
            .run(&["rev-parse", "HEAD"])
            .ok();
        self.update(&id, |m| {
            m.branch = prep.branch.clone();
            m.worktree = prep.worktree.clone();
            m.base_sha = base_sha;
            m.brief = prep.brief.clone();
            m.related = prep.related.clone();
        });

        if self
            .live
            .lock()
            .unwrap()
            .get(&id)
            .is_some_and(|l| l.stop_requested)
        {
            self.emit(
                &id,
                RunEvent::Note {
                    text: "Stopped before the agent started".into(),
                },
            );
            let mut settled = self.get(&id)?;
            settled.state = RunState::Stopped;
            let home = self.cfg.home.clone();
            let _ = tokio::task::spawn_blocking(move || settle_card(&settled, &home)).await;
            self.update(&id, |m| {
                m.state = RunState::Stopped;
                m.ended_at = Some(now_secs());
            });
            self.emit(
                &id,
                RunEvent::Exited {
                    code: None,
                    state: RunState::Stopped,
                },
            );
            self.live.lock().unwrap().remove(&id);
            return Ok(());
        }
        self.wait_room(&id).await;
        let session = self.get(&id)?.session_id;
        let mut cmd = Command::new(&self.cfg.claude_bin);
        cmd.args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
        ]);
        if resuming {
            cmd.args(["--resume", &session]);
        } else {
            cmd.args(["--session-id", &session]);
        }
        // Run all asks nothing: its runs never stop for a permission (the person chose so).
        let autopilot = self.get(&id).is_ok_and(|m| m.autopilot);
        let mode = if autopilot {
            "bypassPermissions".to_owned()
        } else {
            mode_for(&repo)
        };
        cmd.args([
            "--setting-sources",
            "project,local",
            "--permission-mode",
            &mode,
        ])
        .arg("--allowedTools")
        .arg(prep.tools.join(","))
        // Nobody is watching a run: it decides, notes the choice, or ends blocked.
        .args(["--disallowedTools", "AskUserQuestion"])
        .args(prep.related.iter().flat_map(|l| {
            [
                "--add-dir".to_owned(),
                l.worktree.to_string_lossy().into_owned(),
            ]
        }))
        .args(["--max-turns", &prep.max_turns.to_string()])
        .arg("--append-system-prompt")
        .arg(format!(
            "{}{}",
            runs::RULES,
            pit_core::language::rule(&pit_core::language::agent_language(&self.cfg.home))
        ))
        .arg("--json-schema")
        .arg(runs::handoff_schema().to_string());
        // The model: the route's, the repository's, or else the one you use in Claude Code.
        let picked = choice.model.clone().or(prep.model.clone());
        let (your_model, your_effort) =
            your_claude(choice.config_dir.as_deref(), picked.as_deref());
        if let Some(m) = picked.as_ref().or(your_model.as_ref()) {
            cmd.args(["--model", m]);
        }
        if let Some(e) = &your_effort {
            cmd.args(["--effort", e]);
            let e = e.clone();
            self.update(&id, move |m| m.effort = Some(e));
        }
        // The account: its own Claude Code directory, where you signed in. Pitlane only points at it.
        if let Some(dir) = &choice.config_dir {
            fs::create_dir_all(dir)?;
            cmd.env("CLAUDE_CONFIG_DIR", dir);
        }
        // An API key account: Claude Code runs your helper command; the key never passes here.
        if let Some(helper) = &choice.api_key_helper {
            cmd.arg("--settings")
                .arg(serde_json::json!({ "apiKeyHelper": helper }).to_string());
        }
        self.emit(
            &id,
            RunEvent::Note {
                text: format!("Route: {}", self.get(&id)?.reason.unwrap_or_default()),
            },
        );
        if let Some(pit) = &self.cfg.pit_bin {
            // Anything no rule allows is asked to a person, through Pitlane, and waits for them.
            let mcp = serde_json::json!({ "mcpServers": { "pitlane_run": {
                "command": pit, "args": ["mcp", "--approve", &id] } } });
            cmd.args(["--mcp-config", &mcp.to_string()])
                .args([
                    "--permission-prompts",
                    "host",
                    "--permission-prompt-tool",
                    "mcp__pitlane_run__approve",
                ])
                .env("MCP_TOOL_TIMEOUT", "86400000");
        }
        cmd.current_dir(&prep.worktree)
            .env_remove("CLAUDECODE")
            // Performance may pause a session for a while: a request that went quiet then is
            // retried by Claude Code, not dropped.
            .env("CLAUDE_STREAM_IDLE_TIMEOUT_MS", crate::perf::STREAM_IDLE_MS)
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(false);
        let mut child = cmd
            .spawn()
            .with_context(|| format!("could not start {}", self.cfg.claude_bin))?;
        let pid = child.id();
        let mut stdin = child.stdin.take().context("no stdin")?;
        let first = if resuming {
            message
                .clone()
                .unwrap_or_else(|| "Continue where you stopped. End with the handoff.".to_owned())
        } else {
            fs::write(self.dir(&id).join("briefing.md"), &prep.briefing)?;
            prep.briefing.clone()
        };
        let line = if resuming {
            runs::claude_user_line(&first)
        } else {
            runs::claude_user_line_with(&first, &prep.images)
        };
        stdin.write_all(format!("{line}\n").as_bytes()).await?;
        stdin.flush().await?;
        {
            let mut live = self.live.lock().unwrap();
            if let Some(l) = live.get_mut(&id) {
                l.stdin = Some(stdin);
                l.sent = 1;
                l.meta.pid = pid;
                l.meta.state = RunState::Running;
            }
        }
        self.update(&id, |_| {});
        let _ = set_card_state(&self.get(&id)?, CardState::Running, &self.cfg.home);

        let stdout = child.stdout.take().context("no stdout")?;
        let stderr = child.stderr.take().context("no stderr")?;
        let err_path = self.dir(&id).join("stderr.log");
        let err_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut tail: Vec<String> = Vec::new();
            let mut f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(err_path)
                .ok();
            while let Ok(Some(l)) = lines.next_line().await {
                if let Some(f) = f.as_mut() {
                    let _ = writeln!(f, "{l}");
                }
                tail.push(l);
                if tail.len() > 8 {
                    tail.remove(0);
                }
            }
            tail
        });

        let raw_path = self.dir(&id).join("events.jsonl");
        let mut raw = OpenOptions::new()
            .create(true)
            .append(true)
            .open(raw_path)?;
        let finished = Arc::new(Mutex::new(None::<bool>));
        let reader = {
            let me = Arc::clone(&self);
            let id = id.clone();
            let finished = Arc::clone(&finished);
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = writeln!(raw, "{line}");
                    let Ok(v) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    me.note_background(&id, &v);
                    for ev in runs::read_claude_line(&v) {
                        let mut ok = None;
                        me.absorb(&id, &ev, &mut ok).await;
                        if ok.is_some() {
                            *finished.lock().unwrap() = ok;
                        }
                        me.emit(&id, ev);
                    }
                }
            })
        };

        let status = child.wait().await?;
        // The agent is gone: its pid may be reused, and what it left in its group (a server, a
        // watcher) must not outlive it.
        if let Some(l) = self.live.lock().unwrap().get_mut(&id) {
            l.meta.pid = None;
        }
        if let Some(pid) = pid {
            end_group(pid);
        }
        // A tool's child process can keep stdout open after the agent is gone: read what is
        // left for a moment, then let go.
        let mut reader = reader;
        if tokio::time::timeout(Duration::from_secs(3), &mut reader)
            .await
            .is_err()
        {
            reader.abort();
        }
        let tail = match tokio::time::timeout(Duration::from_secs(1), err_task).await {
            Ok(Ok(t)) => t,
            _ => Vec::new(),
        };
        let finished_ok = *finished.lock().unwrap();
        let stopped = self.stop_requested(&id);
        let limited = !stopped
            && self
                .live
                .lock()
                .unwrap()
                .get(&id)
                .is_some_and(|l| l.limited);
        if limited {
            return self.hand_off(id, repo, card, status.code()).await;
        }
        let state = if stopped {
            RunState::Stopped
        } else if finished_ok == Some(true) {
            RunState::Done
        } else {
            RunState::Failed
        };
        let error = (state == RunState::Failed).then(|| {
            if tail.is_empty() {
                format!("the agent exited with {status}")
            } else {
                tail.join("\n")
            }
        });
        // Finished work is checked before anyone reviews it: the repository's proofs, then the judge.
        let blocked = self
            .get(&id)?
            .handoff
            .as_ref()
            .and_then(|h| h.get("status"))
            .and_then(|s| s.as_str())
            == Some("blocked");
        if state == RunState::Done && !blocked {
            self.verify(&id, card).await;
        }
        // Stopped while it was being checked: back to ready, not to review.
        let state = if self.stop_requested(&id) {
            RunState::Stopped
        } else {
            state
        };
        // The card follows the run first (a handoff moves it to review, its questions go to the
        // Inbox), so anyone who sees the run ended also sees where the card went.
        let mut settled = self.get(&id)?;
        settled.state = state;
        let home = self.cfg.home.clone();
        let card_result = tokio::task::spawn_blocking(move || settle_card(&settled, &home)).await?;
        self.update(&id, |m| {
            m.state = state;
            m.ended_at = Some(now_secs());
            m.pid = None;
            m.waiting.clear();
            if m.error.is_none() {
                m.error = error;
            }
        });
        self.emit(
            &id,
            RunEvent::Exited {
                code: status.code(),
                state,
            },
        );
        self.live.lock().unwrap().remove(&id);
        card_result?;
        if self.get(&id).is_ok_and(|m| m.autopilot) {
            self.autopilot(&id).await;
        }
        Ok(())
    }

    /// Run all's next step for a finished run, with nobody watching: more turns for a run that ran
    /// out of them, a fix round for what the proofs or the judge found, else the work lands in its
    /// Into branch. Only a card that cannot go on (proofs still failing, a blocked handoff, an
    /// error) stays behind; the rest of the batch never waits on it.
    async fn autopilot(self: &Arc<Self>, id: &str) {
        let Ok(m) = self.get(id) else { return };
        let home = self.cfg.home.clone();
        let Ok(r) = Repo::open(&m.repo, &home_dir(), &home) else {
            return;
        };
        let next = StartOpts {
            autopilot: true,
            round: m.round,
            extensions: m.extensions,
            ..Default::default()
        };
        let say = |text: String| {
            let _ = cards::note(&r, m.card, "pitlane", &text);
        };
        match m.state {
            // Out of turns: the same session goes on, twice at most.
            RunState::Failed
                if m.handoff.is_none()
                    && m.turns.is_some_and(|t| t + 1 >= r.flow.run.max_turns) =>
            {
                if m.extensions >= 2 {
                    say(format!(
                        "Run {} ran out of turns a third time; the card waits for you.",
                        m.id
                    ));
                    return;
                }
                let msg = "You ran out of turns before finishing. Carry on from where you stopped: commit what works, \
                           finish the card, and end with the handoff. Skip anything you already did.";
                match self.start(
                    m.repo.clone(),
                    m.card,
                    Some(m.id.clone()),
                    StartOpts {
                        message: Some(msg.into()),
                        extensions: m.extensions + 1,
                        ..next
                    },
                ) {
                    Ok(n) => self.emit(
                        id,
                        RunEvent::Note {
                            text: format!("Out of turns: continues as run {}.", n.id),
                        },
                    ),
                    Err(e) => say(format!(
                        "Run {} ran out of turns and could not continue: {e:#}",
                        m.id
                    )),
                }
            }
            RunState::Done
                if m.handoff
                    .as_ref()
                    .and_then(|h| h.get("status"))
                    .and_then(Value::as_str)
                    != Some("blocked") =>
            {
                let c = match cards::get(&r, m.card) {
                    Ok(c) if c.state == CardState::Review => c,
                    _ => return,
                };
                let proofs_failed = m.proofs_ok == Some(false);
                let changes = m.verdict == Some(pit_core::verify::Verdict::Changes);
                if (proofs_failed || changes) && m.round < r.flow.verify.max_rounds {
                    let what = fix_request(&self.dir(id));
                    let first = format!(
                        "Pitlane checked your work and it needs another pass:\n\n{what}\n\nFix it, commit, and end with the handoff."
                    );
                    match self.start(
                        m.repo.clone(),
                        m.card,
                        Some(m.id.clone()),
                        StartOpts {
                            message: Some(first),
                            round: m.round + 1,
                            extensions: 0,
                            ..next
                        },
                    ) {
                        Ok(n) => self.emit(
                            id,
                            RunEvent::Note {
                                text: format!(
                                    "Round {} starts on its own: run {}.",
                                    m.round + 1,
                                    n.id
                                ),
                            },
                        ),
                        Err(e) => say(format!(
                            "The fix round for run {} did not start: {e:#}",
                            m.id
                        )),
                    }
                    return;
                }
                if proofs_failed {
                    let _ = cards::update(
                        &r,
                        m.card,
                        CardPatch {
                            state: Some(CardState::Blocked),
                            ..Default::default()
                        },
                    );
                    say(format!(
                        "The checks still fail after {} rounds, so this card did not land. The rest of the batch went on.",
                        m.round + 1
                    ));
                    return;
                }
                if changes {
                    say(format!(
                        "Landed after {} rounds; the judge still asked for changes. See its findings in the run.",
                        m.round + 1
                    ));
                }
                if c.direct() {
                    let _ = cards::update(
                        &r,
                        m.card,
                        CardPatch {
                            state: Some(CardState::Done),
                            ..Default::default()
                        },
                    );
                    return;
                }
                if let Err(e) = self.land_card(&m.repo, &c, &m.related) {
                    say(format!(
                        "Could not put this card's work in line to land: {e:#}"
                    ));
                }
            }
            _ => {}
        }
    }

    /// Keeps the run record in step with what the agent reports.
    async fn absorb(&self, id: &str, ev: &RunEvent, finished_ok: &mut Option<bool>) {
        match ev {
            RunEvent::Started { model, session } => {
                // A turn nobody asked for (a background task reported back after the last
                // result): it ends with a result like a message would, so count it as one.
                if let Some(l) = self.live.lock().unwrap().get_mut(id)
                    && l.results > 0
                    && l.results >= l.sent
                {
                    l.sent += 1;
                }
                let (model, session) = (model.clone(), session.clone());
                self.update(id, move |m| {
                    m.model = Some(model);
                    if !session.is_empty() {
                        m.session_id = session;
                    }
                });
            }
            RunEvent::Tuned { model, effort } => {
                let (model, effort) = (model.clone(), effort.clone());
                self.update(id, move |m| {
                    if model.is_some() {
                        m.model = model;
                    }
                    if effort.is_some() {
                        m.effort = effort;
                    }
                });
            }
            RunEvent::Quota { window, .. } => {
                let account = self
                    .live
                    .lock()
                    .unwrap()
                    .get(id)
                    .map(|l| l.meta.account.clone())
                    .unwrap_or_default();
                let mut all = self.quota.lock().unwrap();
                let q = all.entry(account).or_default();
                q.retain(|e| !matches!(e, RunEvent::Quota { window: w, .. } if w == window));
                q.push(ev.clone());
            }
            RunEvent::Limited { resets_at, .. } => {
                if let Some(l) = self.live.lock().unwrap().get_mut(id) {
                    l.limited = true;
                }
                let at = (*resets_at > 0).then_some(*resets_at);
                self.update(id, move |m| m.resets_at = at);
            }
            RunEvent::Finished {
                ok,
                cost_usd,
                duration_ms,
                turns,
                tokens,
                handoff,
                denied,
                limited,
            } => {
                *finished_ok = Some(*ok);
                if *limited && let Some(l) = self.live.lock().unwrap().get_mut(id) {
                    l.limited = true;
                }
                let (ok, cost, dur, turns, tokens, handoff, denied) = (
                    *ok,
                    *cost_usd,
                    *duration_ms,
                    *turns,
                    tokens.clone(),
                    handoff.clone(),
                    denied.clone(),
                );
                self.update(id, move |m| {
                    m.cost_usd = Some(m.cost_usd.unwrap_or(0.0) + cost);
                    m.duration_ms = Some(m.duration_ms.unwrap_or(0) + dur);
                    m.turns = Some(m.turns.unwrap_or(0) + turns);
                    m.tokens.input += tokens.input;
                    m.tokens.output += tokens.output;
                    m.tokens.cache_read += tokens.cache_read;
                    m.tokens.cache_write += tokens.cache_write;
                    if handoff.is_some() {
                        m.handoff = handoff;
                    }
                    m.denied.extend(denied);
                    if !ok {
                        m.error
                            .get_or_insert_with(|| "the agent stopped with an error".into());
                    }
                });
                if let Some(h) = self.get(id).ok().and_then(|m| m.handoff) {
                    let _ = fs::write(
                        self.dir(id).join("handoff.json"),
                        serde_json::to_string_pretty(&h).unwrap_or_default(),
                    );
                }
                if let Some(l) = self.live.lock().unwrap().get_mut(id) {
                    l.results += 1;
                }
                self.close_when_idle(id);
            }
            _ => {}
        }
    }

    /// Tracks the background tasks Claude Code reports, and lets the session end once the last
    /// one is done and nothing else is pending.
    fn note_background(&self, id: &str, v: &Value) {
        if v.get("type").and_then(Value::as_str) != Some("system")
            || v.get("subtype").and_then(Value::as_str) != Some("background_tasks_changed")
        {
            return;
        }
        let n = v.get("tasks").and_then(Value::as_array).map_or(0, Vec::len);
        if let Some(l) = self.live.lock().unwrap().get_mut(id) {
            l.background = n;
        }
        if n == 0 {
            self.close_when_idle(id);
        }
    }

    /// Closes stdin, which is how a stream-json session ends, once the session is idle: every
    /// message answered, no background task left, and no new turn for a moment. Claude Code can
    /// start a turn by itself right after a result (a task it waited on reported back); closing
    /// at once would leave that turn unable to hear the person. A background task that never
    /// ends (a dev server) keeps it open for at most a quarter of an hour.
    fn close_when_idle(&self, id: &str) {
        const GRACE: Duration = Duration::from_secs(2);
        const BACKGROUND_WAIT: Duration = Duration::from_secs(15 * 60);
        let (live, id) = (Arc::clone(&self.live), id.to_owned());
        tokio::spawn(async move {
            let since = tokio::time::Instant::now();
            loop {
                tokio::time::sleep(GRACE).await;
                let stdin = {
                    let mut live = live.lock().unwrap();
                    let Some(l) = live.get_mut(&id) else { return };
                    if l.results == 0 || l.results < l.sent {
                        return; // working again: its next result checks once more
                    }
                    if l.background > 0 && since.elapsed() < BACKGROUND_WAIT {
                        continue;
                    }
                    l.stdin.take()
                };
                if let Some(mut s) = stdin {
                    let _ = s.shutdown().await;
                }
                return;
            }
        });
    }

    /// The agent asks to use a tool no rule allows. Answers at once when a rule of this run or of
    /// the repository allows it; otherwise waits for a person. Returns Claude Code's answer:
    /// `{"behavior":"allow","updatedInput":…}` or `{"behavior":"deny","message":…}`.
    pub async fn ask(
        &self,
        run: &str,
        tool: &str,
        input: Value,
        tool_use_id: Option<String>,
    ) -> Value {
        let allow =
            |input: &Value| serde_json::json!({ "behavior": "allow", "updatedInput": input });
        let deny = |msg: &str| serde_json::json!({ "behavior": "deny", "message": msg });
        let (repo, run_rules, autopilot) = {
            let live = self.live.lock().unwrap();
            let Some(l) = live.get(run) else {
                return deny("This run is not live any more.");
            };
            (l.meta.repo.clone(), l.rules.clone(), l.meta.autopilot)
        };
        // Run all: allowed at once, and written down so the review can see what ran.
        if autopilot {
            self.emit(
                run,
                RunEvent::Note {
                    text: format!("Allowed {tool} without asking (Run all)"),
                },
            );
            return allow(&input);
        }
        let repo_rules = Repo::open(&repo, &home_dir(), &self.cfg.home)
            .map(|r| r.flow.run.allow)
            .unwrap_or_default();
        if run_rules
            .iter()
            .chain(repo_rules.iter())
            .any(|r| runs::rule_allows(r, tool, &input))
        {
            return allow(&input);
        }
        let approval = Approval {
            id: tool_use_id.unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string()),
            tool: tool.to_owned(),
            detail: runs::read_claude_line(&serde_json::json!({
                "type": "assistant", "message": { "content": [{ "type": "tool_use", "id": "", "name": tool, "input": input }] } }))
            .into_iter()
            .find_map(|e| match e {
                RunEvent::Tool { detail, .. } => Some(detail),
                _ => None,
            })
            .unwrap_or_default(),
            rule: runs::rule_for(tool, &input),
            input: input.clone(),
            asked_at: now_secs(),
        };
        let (tx, rx) = oneshot::channel();
        {
            let mut live = self.live.lock().unwrap();
            let Some(l) = live.get_mut(run) else {
                return deny("This run is not live any more.");
            };
            l.pending.insert(approval.id.clone(), tx);
            l.meta.waiting.push(approval.clone());
        }
        self.update(run, |_| {});
        self.emit(
            run,
            RunEvent::Approval {
                approval: approval.clone(),
            },
        );

        let answer = rx.await;
        self.update(run, |m| m.waiting.retain(|a| a.id != approval.id));
        let Ok((decision, message)) = answer else {
            return deny("The run ended before anyone answered.");
        };
        match decision {
            Decision::Run => {
                if let Some(l) = self.live.lock().unwrap().get_mut(run) {
                    l.rules.push(approval.rule.clone());
                }
            }
            Decision::Always => {
                if let Err(e) = flow::add_run_allow(&repo, &approval.rule) {
                    self.emit(
                        run,
                        RunEvent::Note {
                            text: format!("Could not save {} to flow.toml: {e:#}", approval.rule),
                        },
                    );
                }
                if let Some(l) = self.live.lock().unwrap().get_mut(run) {
                    l.rules.push(approval.rule.clone());
                }
            }
            Decision::Once | Decision::Deny => {}
        }
        self.emit(
            run,
            RunEvent::Decided {
                id: approval.id.clone(),
                decision,
                message: message.clone(),
            },
        );
        if decision == Decision::Deny {
            deny(
                message
                    .as_deref()
                    .filter(|m| !m.trim().is_empty())
                    .unwrap_or(
                        "The person declined this. Find another way or report it in the handoff.",
                    ),
            )
        } else {
            allow(&input)
        }
    }

    /// Answers a waiting approval: the one named, or the oldest of the run.
    pub fn decide(
        &self,
        run: &str,
        approval: Option<&str>,
        decision: Decision,
        message: Option<String>,
    ) -> Result<String> {
        let mut live = self.live.lock().unwrap();
        let l = live
            .get_mut(run)
            .with_context(|| format!("run {run} is not live"))?;
        let id = match approval {
            Some(a) => a.to_owned(),
            None => l
                .meta
                .waiting
                .first()
                .map(|a| a.id.clone())
                .context("nothing is waiting in this run")?,
        };
        let tx = l
            .pending
            .remove(&id)
            .with_context(|| format!("no approval {id} waiting in run {run}"))?;
        tx.send((decision, message))
            .map_err(|_| anyhow::anyhow!("the agent stopped waiting"))?;
        Ok(id)
    }

    /// Everything waiting on a person, across live runs.
    pub fn approvals(&self) -> Vec<(RunMeta, Approval)> {
        self.live
            .lock()
            .unwrap()
            .values()
            .flat_map(|l| l.meta.waiting.iter().map(|a| (l.meta.clone(), a.clone())))
            .collect()
    }

    /// The run hit a usage limit: commit what it did, close it, and continue the card with the
    /// route's next option. When that option is an API key, it waits for the person's yes.
    async fn hand_off(
        self: Arc<Self>,
        id: String,
        repo: PathBuf,
        card: u32,
        code: Option<i32>,
    ) -> Result<()> {
        let meta = self.get(&id)?;
        let worktree = meta.worktree.clone();
        let label = meta.card;
        let also: Vec<PathBuf> = meta.related.iter().map(|l| l.worktree.clone()).collect();
        let checkpoint = tokio::task::spawn_blocking(move || {
            for w in &also {
                let _ = checkpoint(w, label);
            }
            checkpoint(&worktree, label)
        })
        .await?;
        if let Ok(Some(sha)) = &checkpoint {
            self.emit(
                &id,
                RunEvent::Note {
                    text: format!("Committed the work so far as {sha} before handing off."),
                },
            );
        }
        let why = format!("Hit the usage limit on {}.", meta.account);
        self.update(&id, |m| {
            m.state = RunState::Failed;
            m.ended_at = Some(now_secs());
            m.pid = None;
            m.waiting.clear();
            m.error = Some(why.clone());
        });
        self.emit(
            &id,
            RunEvent::Exited {
                code,
                state: RunState::Failed,
            },
        );
        self.live.lock().unwrap().remove(&id);

        match self.start(
            repo,
            card,
            None,
            StartOpts {
                allow_api_key: false,
                handoff_from: Some(id.clone()),
                round: meta.round,
                autopilot: meta.autopilot,
                extensions: meta.extensions,
                ..Default::default()
            },
        ) {
            Ok(next) => {
                let text = format!(
                    "{why} Continues as run {} with {}.",
                    next.id,
                    next.reason.clone().unwrap_or_default()
                );
                self.update_disk(&id, |m| m.error = Some(text.clone()));
                Ok(())
            }
            Err(e) => {
                let e = format!("{e:#}");
                let (next, text) = match e.strip_prefix(NEEDS_API_KEY) {
                    // The reason reads "<option>: <why the others were passed over>".
                    Some(reason) => {
                        let option = reason
                            .split_once(':')
                            .map_or(reason, |(o, _)| o)
                            .trim()
                            .to_owned();
                        (
                            Some(option.clone()),
                            format!(
                                "{why} The next option, {option}, is paid per token and waits for your yes."
                            ),
                        )
                    }
                    None => (
                        None,
                        format!("{why} Nothing else in the route can take it now: {e}"),
                    ),
                };
                // The card first: whoever sees the run's next option finds the card already waiting.
                let home = self.cfg.home.clone();
                tokio::task::spawn_blocking(move || {
                    set_card_state(&meta, CardState::Blocked, &home)
                })
                .await??;
                self.update_disk(&id, |m| {
                    m.error = Some(text.clone());
                    m.next = next.clone();
                });
                Ok(())
            }
        }
    }

    /// Proofs, then the judge, on the run's lane. The run stays live (state `verifying`) meanwhile,
    /// so everyone following it sees each step. Failures here are recorded, never fatal.
    async fn verify(self: &Arc<Self>, id: &str, card: u32) {
        let meta = match self.get(id) {
            Ok(m) => m,
            Err(_) => return,
        };
        let home = self.cfg.home.clone();
        let Ok(r) = Repo::open(&meta.repo, &home_dir(), &home) else {
            return;
        };
        let cfg = r.flow.verify.clone();
        if cfg.commands.is_empty() && !cfg.judge {
            return;
        }
        self.update(id, |m| m.state = RunState::Verifying);
        let _ = set_card_state(&meta, CardState::Verifying, &home);
        let mut v = pit_core::verify::Verification {
            run: id.to_owned(),
            started_at: now_secs(),
            no_commands: cfg.commands.is_empty(),
            max_rounds: cfg.max_rounds,
            ..Default::default()
        };

        for cmd in &cfg.commands {
            self.emit(
                id,
                RunEvent::Note {
                    text: format!("Checking: {cmd}"),
                },
            );
            if self.stop_requested(id) {
                break;
            }
            let (wt, c, me, rid) = (
                meta.worktree.clone(),
                cmd.clone(),
                Arc::clone(self),
                id.to_owned(),
            );
            let proof = tokio::task::spawn_blocking(move || {
                pit_core::verify::run_proof(&wt, &c, &|| me.stop_requested(&rid))
            })
            .await
            .unwrap_or_else(|e| pit_core::verify::Proof {
                command: cmd.clone(),
                ok: false,
                code: None,
                duration_ms: 0,
                tail: e.to_string(),
            });
            self.emit(
                id,
                RunEvent::Proof {
                    command: proof.command.clone(),
                    ok: proof.ok,
                    duration_ms: proof.duration_ms,
                },
            );
            v.proofs.push(proof);
        }
        if !v.proofs.is_empty() {
            let ok = v.proofs_ok();
            self.update(id, move |m| m.proofs_ok = Some(ok));
        }

        if cfg.judge && !self.stop_requested(id) {
            let model = cfg.judge_model.clone();
            self.emit(
                id,
                RunEvent::Note {
                    text: format!(
                        "The judge reads the diff ({}), in a session of its own.",
                        model.as_deref().unwrap_or("default model")
                    ),
                },
            );
            let judge = self.judge(&meta, &r, card, &v.proofs, model.clone()).await;
            if let Some(j) = &judge.judgement {
                let (verdict, summary, n) = (j.verdict, j.summary.clone(), j.findings.len());
                self.update(id, move |m| m.verdict = Some(verdict));
                self.emit(
                    id,
                    RunEvent::Judged {
                        verdict,
                        summary,
                        findings: n,
                    },
                );
            } else if let Some(e) = &judge.error {
                self.emit(
                    id,
                    RunEvent::Note {
                        text: format!("The judge could not finish: {e}"),
                    },
                );
            }
            v.judge = Some(judge);
        }
        v.ended_at = Some(now_secs());
        let _ = fs::write(
            self.dir(id).join("verify.json"),
            serde_json::to_string_pretty(&v).unwrap_or_default(),
        );
    }

    /// The judge: a fresh Claude Code session, read-only, on the same account as the run. It gets
    /// the spec, the decisions and the diff; nothing the working agent said.
    async fn judge(
        &self,
        meta: &RunMeta,
        r: &Repo,
        card: u32,
        proofs: &[pit_core::verify::Proof],
        model: Option<String>,
    ) -> pit_core::verify::JudgeRun {
        let mut out = pit_core::verify::JudgeRun {
            model: model.clone(),
            ..Default::default()
        };
        let prompt = (|| -> Result<String> {
            let c = cards::get(r, card)?;
            let diff = pit_core::verify::project_diff(r, &meta.branch, &meta.related, 200_000)?;
            let decisions = fs::read_to_string(cards::decisions_path(r)).unwrap_or_default();
            Ok(pit_core::verify::judge_prompt(
                &c, &decisions, &diff, proofs,
            ))
        })();
        let prompt = match prompt {
            Ok(p) => p,
            Err(e) => {
                out.error = Some(format!("{e:#}"));
                return out;
            }
        };
        let routing = Routing::load(&self.cfg.home).ok();
        let account = routing
            .as_ref()
            .and_then(|rt| rt.account(&meta.account).cloned());
        let mut args: Vec<String> = read_only_args(&mode_for(&meta.repo));
        args.extend([
            "--allowedTools".into(),
            "Read,Grep,Glob,Bash(git diff:*),Bash(git log:*),Bash(git show:*)".into(),
            "--max-turns".into(),
            "30".into(),
        ]);
        for l in &meta.related {
            args.extend([
                "--add-dir".into(),
                l.worktree.to_string_lossy().into_owned(),
            ]);
        }
        args.extend([
            "--append-system-prompt".into(),
            pit_core::verify::JUDGE_RULES.into(),
            "--json-schema".into(),
            pit_core::verify::verdict_schema().to_string(),
        ]);
        if let Some(m) = &model {
            args.extend(["--model".into(), m.clone()]);
        }
        if let Some(helper) = account.and_then(|a| a.api_key_helper) {
            args.extend([
                "--settings".into(),
                serde_json::json!({ "apiKeyHelper": helper }).to_string(),
            ]);
        }
        let side = Side {
            kind: SessionKind::Judge,
            repo: meta.repo.clone(),
            cwd: meta.worktree.clone(),
            card,
            card_title: meta.card_title.clone(),
            parent: Some(meta.id.clone()),
            account: meta.account.clone(),
            config_dir: accounts::profile_dir(&meta.account, &self.cfg.home),
            model: model.clone(),
            args,
            prompt,
            limit: Duration::from_secs(20 * 60),
            board: false,
            images: Vec::new(),
        };
        match self.side_session(side).await {
            Ok(stdout) => match pit_core::verify::read_judge(&stdout) {
                Ok((j, cost, dur)) => {
                    out.judgement = Some(j);
                    out.cost_usd = cost;
                    out.duration_ms = dur;
                }
                Err(e) => out.error = Some(format!("{e:#}")),
            },
            Err(e) => out.error = Some(format!("the judge {e:#}")),
        }
        out
    }

    /// Run all's analysis: for each card not analysed yet, a read-only Claude Code session in the
    /// repository returns questions and notes, written on the card. Three at a time.
    pub async fn analyze(self: &Arc<Self>, repo: PathBuf, ids: Vec<u32>) -> Result<Value> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        let routing = Routing::load(&self.cfg.home)?;
        // Never the API key: analysis is not worth a paid token without a yes.
        let choice = routing
            .choices(&r.root, &home_dir(), &self.cfg.home)?
            .into_iter()
            .find(|c| c.kind != accounts::Kind::ApiKey);
        // The repository's model for agents, else yours (the judge's model is for judging).
        let model = r.flow.run.model.clone();
        let batch: Vec<cards::Card> = ids
            .into_iter()
            .filter_map(|id| cards::get(&r, id).ok())
            .collect();
        let todo: Vec<cards::Card> = batch
            .iter()
            .filter(|c| c.analyzed.is_none() && c.state != CardState::Done)
            .cloned()
            .collect();
        let gate = Arc::clone(&self.analysis_gate);
        let epoch = self.epoch.load(Ordering::SeqCst);
        // A card already being analysed (another Run all) is not analysed twice.
        let todo: Vec<cards::Card> = {
            let mut busy = self.analysing.lock().unwrap();
            todo.into_iter()
                .filter(|c| busy.insert((r.root.clone(), c.id)))
                .collect()
        };
        let mut tasks = Vec::new();
        for card in todo {
            let (gate, me, root, choice, model) = (
                gate.clone(),
                self.clone(),
                r.root.clone(),
                choice.clone(),
                model.clone(),
            );
            let prompt = pit_core::analyze::prompt(&r, &card, &batch);
            tasks.push(tokio::spawn(async move {
                let _busy = Release(&me.analysing, (root.clone(), card.id));
                let _slot = gate.acquire().await;
                if me.epoch.load(Ordering::SeqCst) != epoch {
                    return (card.id, Err(anyhow::anyhow!("stopped before it started")));
                }
                let out = me
                    .analyst(&root, &card, &prompt, choice.as_ref(), model.as_deref())
                    .await;
                (card.id, out)
            }));
        }
        let mut results = Vec::new();
        // Sibling repositories the cards' code lives in: what the analysts named, and what their
        // notes mention, that exist next to this one and are not configured yet.
        let mut found: Vec<String> = Vec::new();
        for t in tasks {
            let Ok((id, out)) = t.await else { continue };
            results.push(match out.and_then(|(a, cost)| {
                let (q, n) = (a.questions.len(), a.notes.len());
                let text = format!("{}\n{}", a.repos.join(" "), a.notes.join("\n"));
                for rel in pit_core::related::mentioned(&r, &text) {
                    if !found.contains(&rel) {
                        found.push(rel);
                    }
                }
                pit_core::analyze::record(&r, id, a)?;
                Ok((q, n, cost))
            }) {
                Ok((q, n, cost)) => serde_json::json!({ "card": id, "questions": q, "notes": n, "cost_usd": cost }),
                Err(e) => serde_json::json!({ "card": id, "error": format!("{e:#}") }),
            });
        }
        // Added now, shown on the Run sheet, where the person can take any of them off.
        let repos_error = if found.is_empty() {
            None
        } else {
            pit_core::related::add(&r, &found)
                .err()
                .map(|e| format!("{e:#}"))
        };
        Ok(
            serde_json::json!({ "analyzed": results, "repos_added": found, "repos_error": repos_error }),
        )
    }

    async fn analyst(
        &self,
        repo: &Path,
        card: &cards::Card,
        prompt: &str,
        choice: Option<&Choice>,
        model: Option<&str>,
    ) -> Result<(pit_core::analyze::Analysis, Option<f64>)> {
        let mut args: Vec<String> = read_only_args(&mode_for(repo));
        args.extend([
            "--allowedTools".into(),
            "Read,Grep,Glob,Bash(git log:*),Bash(git show:*),Bash(ls:*)".into(),
            "--max-turns".into(),
            "25".into(),
        ]);
        // Read-only: it may look at sibling repositories, where a project's code often lives.
        if let Some(parent) = repo.parent() {
            args.extend(["--add-dir".into(), parent.to_string_lossy().into_owned()]);
        }
        args.extend([
            "--append-system-prompt".into(),
            pit_core::analyze::RULES.into(),
            "--json-schema".into(),
            pit_core::analyze::schema().to_string(),
        ]);
        if let Some(m) = model {
            args.extend(["--model".into(), m.into()]);
        }
        let side = Side {
            kind: SessionKind::Analysis,
            repo: repo.to_owned(),
            cwd: repo.to_owned(),
            card: card.id,
            card_title: card.title.clone(),
            parent: None,
            account: choice
                .map(|c| c.account.clone())
                .unwrap_or_else(|| accounts::DEFAULT_ACCOUNT.into()),
            config_dir: choice.and_then(|c| c.config_dir.clone()),
            model: model.map(str::to_owned),
            args,
            prompt: prompt.to_owned(),
            limit: Duration::from_secs(20 * 60),
            board: false,
            images: Repo::open(repo, &home_dir(), &self.cfg.home)
                .map(|r| cards::image_bytes(&r, card))
                .unwrap_or_default(),
        };
        let stdout = self
            .side_session(side)
            .await
            .map_err(|e| anyhow::anyhow!("the analysis {e:#}"))?;
        pit_core::analyze::read(&stdout)
    }

    /// Talks one question over: what the person said goes on the question, a read-only Claude
    /// Code session answers it with the options as they stand now, and that answer goes on the
    /// question too (its options become the question's). Without a message it answers the last
    /// one again (after a failure). Waits for the answer; returns the card.
    pub async fn talk(self: &Arc<Self>, repo: PathBuf, id: u32, question: u32, message: Option<String>) -> Result<Value> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        let card = match message.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
            Some(m) => cards::say(&r, id, question, "you", m, None)?,
            None => cards::get(&r, id)?,
        };
        let last = card.questions.iter().find(|q| q.id == question).and_then(|q| q.talk.last());
        if last.is_none_or(|t| t.by != "you") {
            anyhow::bail!("there is nothing to answer on question {question} of card #{id}");
        }
        let prompt = pit_core::analyze::talk_prompt(&r, &card, question)?;
        let routing = Routing::load(&self.cfg.home)?;
        // Like the analysis: never the API key without a yes.
        let choice = routing
            .choices(&r.root, &home_dir(), &self.cfg.home)?
            .into_iter()
            .find(|c| c.kind != accounts::Kind::ApiKey);
        let model = r.flow.run.model.clone();
        let mut args: Vec<String> = read_only_args(&mode_for(&r.root));
        args.extend([
            "--allowedTools".into(),
            "Read,Grep,Glob,Bash(git log:*),Bash(git show:*),Bash(ls:*)".into(),
            "--max-turns".into(),
            "15".into(),
        ]);
        if let Some(parent) = r.root.parent() {
            args.extend(["--add-dir".into(), parent.to_string_lossy().into_owned()]);
        }
        args.extend([
            "--append-system-prompt".into(),
            pit_core::analyze::TALK_RULES.into(),
            "--json-schema".into(),
            pit_core::analyze::talk_schema().to_string(),
        ]);
        if let Some(m) = &model {
            args.extend(["--model".into(), m.clone()]);
        }
        let side = Side {
            // An analysis to everything that counts sessions: it reads, it is no run.
            kind: SessionKind::Analysis,
            repo: r.root.clone(),
            cwd: r.root.clone(),
            card: id,
            card_title: card.title.clone(),
            parent: None,
            account: choice
                .as_ref()
                .map(|c| c.account.clone())
                .unwrap_or_else(|| accounts::DEFAULT_ACCOUNT.into()),
            config_dir: choice.and_then(|c| c.config_dir),
            model,
            args,
            prompt,
            limit: Duration::from_secs(10 * 60),
            board: false,
            images: cards::image_bytes(&r, &card),
        };
        let stdout = self
            .side_session(side)
            .await
            .map_err(|e| anyhow::anyhow!("the answer {e:#}"))?;
        let (reply, cost) = pit_core::analyze::read_talk(&stdout)?;
        let offer = cards::NewQuestion {
            text: String::new(),
            options: reply.options,
            recommended: reply.recommended,
            context: reply.context,
        };
        let card = cards::say(&r, id, question, "claude", &reply.reply, Some(offer))?;
        Ok(serde_json::json!({ "card": cards::CardOut::from(card), "cost_usd": cost }))
    }

    /// Stops the conversation going on a card's questions (it is an analysis session of that
    /// card), and nothing else: Run all's analyses waiting for a slot keep their place.
    pub fn stop_talk(self: &Arc<Self>, repo: &Path, id: u32) -> usize {
        self.stop_matching(|m| m.kind == SessionKind::Analysis && m.card == id && m.repo == repo)
    }

    /// Run all's closing step: one Claude Code session in the repository (or the target branch's
    /// worktree) that does what the person asked for when everything is done. It edits files and
    /// asks through Pitlane for anything no rule allows. Waits for it; returns its summary.
    pub async fn finish(
        self: &Arc<Self>,
        repo: PathBuf,
        ids: Vec<u32>,
        instructions: String,
        into: Option<String>,
    ) -> Result<Value> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        let cwd = match into.as_deref() {
            Some(t) if !r.flow.flow.is_trunk(t) => {
                actions::ensure_worktree(&r, t).unwrap_or_else(|_| r.root.clone())
            }
            _ => r.root.clone(),
        };
        let routing = Routing::load(&self.cfg.home)?;
        let choice = routing
            .choices(&r.root, &home_dir(), &self.cfg.home)?
            .into_iter()
            .find(|c| c.kind != accounts::Kind::ApiKey);
        let mut tools: Vec<String> = RUN_BASE_TOOLS.iter().map(|s| s.to_string()).collect();
        tools.extend(r.flow.run.allow.iter().cloned());
        let args: Vec<String> = vec![
            "--setting-sources".into(),
            "project,local".into(),
            "--permission-mode".into(),
            mode_for(&r.root),
            "--allowedTools".into(),
            tools.join(","),
            "--disallowedTools".into(),
            "AskUserQuestion".into(),
            "--max-turns".into(),
            "80".into(),
        ];
        let side = Side {
            kind: SessionKind::Finish,
            repo: r.root.clone(),
            cwd,
            card: 0,
            card_title: "When it is all done".into(),
            parent: None,
            account: choice
                .as_ref()
                .map(|c| c.account.clone())
                .unwrap_or_else(|| accounts::DEFAULT_ACCOUNT.into()),
            config_dir: choice.as_ref().and_then(|c| c.config_dir.clone()),
            model: None,
            args,
            prompt: pit_core::report::finish_prompt(&r, &ids, &instructions, into.as_deref()),
            limit: Duration::from_secs(90 * 60),
            board: false,
            images: Vec::new(),
        };
        let id_hint = now_secs();
        let out = self.side_session(side).await;
        // The session's id: the newest finish session of this repository.
        let run = self
            .sessions(Some(&r.root))?
            .into_iter()
            .find(|m| m.kind == SessionKind::Finish && m.started_at >= id_hint)
            .map(|m| m.id);
        Ok(match out {
            Ok(line) => {
                let v: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                serde_json::json!({
                    "run": run,
                    "ok": !v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                    "summary": v.get("result").and_then(Value::as_str),
                    "cost_usd": v.get("total_cost_usd").and_then(Value::as_f64),
                })
            }
            Err(e) => serde_json::json!({ "run": run, "ok": false, "summary": format!("{e:#}") }),
        })
    }

    /// The Board's assistant: what the person typed or said ("make cards from this list") goes to
    /// a Claude Code session in the repository that reads the code and writes cards with
    /// Pitlane's board tools. It runs in the background, in view in the Agents dock; people can
    /// keep talking to it while it works.
    pub fn assist(self: &Arc<Self>, repo: PathBuf, text: String) -> Result<()> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        if self.cfg.pit_bin.is_none() {
            bail!("the board assistant needs the pit executable for its board tools");
        }
        let routing = Routing::load(&self.cfg.home)?;
        let choice = routing
            .choices(&r.root, &home_dir(), &self.cfg.home)?
            .into_iter()
            .find(|c| c.kind != accounts::Kind::ApiKey);
        let tools = "Read,Grep,Glob,Bash(git log:*),Bash(git show:*),Bash(ls:*),mcp__pitlane__add_card,mcp__pitlane__list_cards,mcp__pitlane__get_card,mcp__pitlane__update_card,mcp__pitlane__delete_card,mcp__pitlane__add_note,mcp__pitlane__brief";
        let args: Vec<String> = vec![
            "--setting-sources".into(),
            "project,local".into(),
            "--permission-mode".into(),
            mode_for(&r.root),
            "--disallowedTools".into(),
            "Edit,Write,NotebookEdit,AskUserQuestion".into(),
            "--allowedTools".into(),
            tools.into(),
            "--max-turns".into(),
            "60".into(),
            "--append-system-prompt".into(),
            ASSIST_RULES.into(),
        ];
        let side = Side {
            kind: SessionKind::Assist,
            repo: r.root.clone(),
            cwd: r.root.clone(),
            card: 0,
            card_title: "Board assistant".into(),
            parent: None,
            account: choice
                .as_ref()
                .map(|c| c.account.clone())
                .unwrap_or_else(|| accounts::DEFAULT_ACCOUNT.into()),
            config_dir: choice.as_ref().and_then(|c| c.config_dir.clone()),
            model: None,
            args,
            prompt: text,
            limit: Duration::from_secs(30 * 60),
            board: true,
            images: Vec::new(),
        };
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let _ = me.side_session(side).await;
        });
        Ok(())
    }

    /// A read-only Claude Code session beside the runs (analysis, judge), recorded and followed
    /// like a run, so the app shows it live and people can write to it while it works. Returns
    /// the last `result` line it printed.
    async fn side_session(&self, side: Side) -> Result<String> {
        let now = now_secs();
        let tag = match side.kind {
            SessionKind::Analysis => "analysis",
            SessionKind::Judge => "judge",
            SessionKind::Run => "run",
            SessionKind::Finish => "finish",
            SessionKind::Assist => "assist",
        };
        let id = format!(
            "{}-{tag}-{now}-{}",
            side.card,
            &uuid::Uuid::new_v4().simple().to_string()[..4]
        );
        let meta = RunMeta {
            id: id.clone(),
            repo: side.repo.clone(),
            card: side.card,
            card_title: side.card_title.clone(),
            branch: String::new(),
            worktree: side.cwd.clone(),
            agent: "claude-code".into(),
            model: side.model.clone(),
            effort: None,
            session_id: uuid::Uuid::new_v4().to_string(),
            pid: None,
            state: RunState::Running,
            started_at: now,
            ended_at: None,
            cost_usd: None,
            duration_ms: None,
            turns: None,
            tokens: Default::default(),
            handoff: None,
            error: None,
            denied: Vec::new(),
            resumed_from: None,
            waiting: Vec::new(),
            account: side.account.clone(),
            reason: None,
            handoff_from: None,
            exhausted: Vec::new(),
            next: None,
            resets_at: None,
            round: 0,
            verdict: None,
            proofs_ok: None,
            base_sha: None,
            brief: None,
            kind: side.kind,
            parent: side.parent.clone(),
            autopilot: false,
            extensions: 0,
            related: Vec::new(),
        };
        fs::create_dir_all(self.dir(&id))?;
        self.write_meta(&meta)?;
        let (tx, _) = broadcast::channel(1024);
        self.live.lock().unwrap().insert(
            id.clone(),
            Live {
                meta,
                stdin: None,
                sent: 0,
                results: 0,
                background: 0,
                stop_requested: false,
                events: 0,
                tx,
                rules: Vec::new(),
                limited: false,
                pending: HashMap::new(),
            },
        );

        let mut cmd = Command::new(&self.cfg.claude_bin);
        cmd.args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
        ])
        .args(["--session-id", &self.get(&id)?.session_id])
        .args(with_language(&side.args, &self.cfg.home));
        // Whatever the session does not set comes from your Claude Code: model and effort.
        let set = side
            .args
            .iter()
            .position(|a| a == "--model")
            .and_then(|i| side.args.get(i + 1))
            .cloned();
        let (your_model, your_effort) = your_claude(side.config_dir.as_deref(), set.as_deref());
        if !side.args.iter().any(|a| a == "--model")
            && let Some(m) = your_model
        {
            cmd.args(["--model", &m]);
        }
        let effort = match side.args.iter().position(|a| a == "--effort") {
            Some(i) => side.args.get(i + 1).cloned(),
            None => {
                if let Some(e) = &your_effort {
                    cmd.args(["--effort", e]);
                }
                your_effort
            }
        };
        if let Some(e) = effort {
            self.update(&id, move |m| m.effort = Some(e));
        }
        // Like a run: what no rule allows (a read outside the repository, say) is asked to a
        // person in Pitlane, instead of being denied with nobody to ask.
        if let Some(pit) = &self.cfg.pit_bin {
            let mut mcp = serde_json::json!({ "mcpServers": { "pitlane_run": { "command": pit, "args": ["mcp", "--approve", &id] } } });
            if side.board {
                mcp["mcpServers"]["pitlane"] =
                    serde_json::json!({ "command": pit, "args": ["mcp"] });
            }
            cmd.args(["--mcp-config", &mcp.to_string()])
                .args([
                    "--permission-prompts",
                    "host",
                    "--permission-prompt-tool",
                    "mcp__pitlane_run__approve",
                ])
                .env("MCP_TOOL_TIMEOUT", "86400000");
        } else {
            cmd.args(["--permission-prompts", "none"]);
        }
        if let Some(dir) = &side.config_dir {
            cmd.env("CLAUDE_CONFIG_DIR", dir);
        }
        cmd.current_dir(&side.cwd)
            .env_remove("CLAUDECODE")
            // Performance may pause a session for a while: a request that went quiet then is
            // retried by Claude Code, not dropped.
            .env("CLAUDE_STREAM_IDLE_TIMEOUT_MS", crate::perf::STREAM_IDLE_MS)
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        let work = async {
            let mut child = cmd
                .spawn()
                .with_context(|| format!("could not start {}", self.cfg.claude_bin))?;
            let mut stdin = child.stdin.take().context("no stdin")?;
            stdin
                .write_all(
                    format!(
                        "{}\n",
                        runs::claude_user_line_with(&side.prompt, &side.images)
                    )
                    .as_bytes(),
                )
                .await?;
            stdin.flush().await?;
            {
                let mut live = self.live.lock().unwrap();
                if let Some(l) = live.get_mut(&id) {
                    l.stdin = Some(stdin);
                    l.sent = 1;
                    l.meta.pid = child.id();
                }
            }
            self.update(&id, |_| {});
            let mut err = child.stderr.take().context("no stderr")?;
            let err_task = tokio::spawn(async move {
                let mut s = String::new();
                let _ = tokio::io::AsyncReadExt::read_to_string(&mut err, &mut s).await;
                s
            });
            let mut raw = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.dir(&id).join("events.jsonl"))?;
            let mut lines = BufReader::new(child.stdout.take().context("no stdout")?).lines();
            let mut last = None::<String>;
            let mut ok = None::<bool>;
            let pid = child.id();
            // Read until the agent exits (not until the pipe closes: a child it left could hold
            // that open), then what is left for a moment.
            let mut status = None;
            loop {
                let line = tokio::select! {
                    l = lines.next_line() => match l { Ok(Some(l)) => l, _ => break },
                    st = child.wait(), if status.is_none() => { status = Some(st?); continue; }
                    _ = tokio::time::sleep(Duration::from_secs(3)), if status.is_some() => break,
                };
                let _ = writeln!(raw, "{line}");
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                self.note_background(&id, &v);
                if v.get("type").and_then(Value::as_str) == Some("result") {
                    last = Some(line.clone());
                }
                for ev in runs::read_claude_line(&v) {
                    let mut done = None;
                    self.absorb(&id, &ev, &mut done).await;
                    ok = done.or(ok);
                    self.emit(&id, ev);
                }
            }
            let status = match status {
                Some(s) => s,
                None => child.wait().await?,
            };
            if let Some(l) = self.live.lock().unwrap().get_mut(&id) {
                l.meta.pid = None;
            }
            if let Some(pid) = pid {
                end_group(pid);
            }
            let stderr = err_task.await.unwrap_or_default();
            if !stderr.trim().is_empty() {
                let _ = fs::write(self.dir(&id).join("stderr.log"), &stderr);
            }
            anyhow::Ok((last, ok, status, stderr))
        };
        let outcome = tokio::time::timeout(side.limit, work).await;
        let stopped = self
            .live
            .lock()
            .unwrap()
            .get(&id)
            .is_some_and(|l| l.stop_requested);
        let (state, code, result) = match outcome {
            Ok(Ok((Some(line), Some(true), status, _))) => {
                (RunState::Done, status.code(), Ok(line))
            }
            Ok(Ok((last, _, status, stderr))) => {
                let why = if stopped {
                    "was stopped".to_owned()
                } else {
                    let tail: Vec<&str> = stderr.lines().rev().take(4).collect();
                    match last {
                        Some(_) => "ended with an error".to_owned(),
                        None if tail.is_empty() => format!("exited with {status}"),
                        None => tail.into_iter().rev().collect::<Vec<_>>().join("\n"),
                    }
                };
                (
                    if stopped {
                        RunState::Stopped
                    } else {
                        RunState::Failed
                    },
                    status.code(),
                    last.ok_or_else(|| anyhow::anyhow!(why)),
                )
            }
            Ok(Err(e)) => (RunState::Failed, None, Err(e)),
            Err(_) => {
                if let Some(pid) = self.get(&id).ok().and_then(|m| m.pid) {
                    end_group(pid);
                }
                (
                    RunState::Failed,
                    None,
                    Err(anyhow::anyhow!(
                        "took longer than {} minutes",
                        side.limit.as_secs() / 60
                    )),
                )
            }
        };
        let error = result.as_ref().err().map(|e| format!("{e:#}"));
        self.update(&id, |m| {
            m.state = state;
            m.ended_at = Some(now_secs());
            m.pid = None;
            if m.error.is_none() {
                m.error = error;
            }
        });
        self.emit(&id, RunEvent::Exited { code, state });
        self.live.lock().unwrap().remove(&id);
        result
    }

    /// A person's decision on a card in review. Approve ships the lane one stage (into
    /// integration); changes resumes the run's session with the comments; discard returns the
    /// card to ready with its lane intact.
    pub async fn review(
        self: &Arc<Self>,
        repo: PathBuf,
        card: u32,
        action: ReviewAction,
        message: Option<String>,
    ) -> Result<serde_json::Value> {
        let home = self.cfg.home.clone();
        let r = Repo::open(&repo, &home_dir(), &home)?;
        let c = cards::get(&r, card)?;
        if c.state != CardState::Review {
            bail!("card #{card} is not in review (it is {})", c.state.as_str());
        }
        // The card's latest run: the highest review round, then the newest (ids can share a second).
        let last = self
            .list(Some(&r.root))?
            .into_iter()
            .filter(|m| m.card == card)
            .max_by_key(|m| (m.round, m.started_at));
        match action {
            ReviewAction::Discard => {
                cards::update(
                    &r,
                    card,
                    CardPatch {
                        state: Some(CardState::Ready),
                        ..Default::default()
                    },
                )?;
                Ok(serde_json::json!({ "card": card, "state": "ready" }))
            }
            ReviewAction::Changes => {
                let text = message
                    .filter(|m| !m.trim().is_empty())
                    .context("say what to change")?;
                let prev = last.context("this card has no run to continue")?;
                let round = prev.round + 1;
                if round > r.flow.verify.max_rounds {
                    bail!(
                        "card #{card} already went through {} rounds of changes; take it from here yourself or discard it",
                        r.flow.verify.max_rounds
                    );
                }
                let first = format!(
                    "A person reviewed your work and asks for changes:\n\n{}\n\nMake them, commit, and end with the handoff.",
                    text.trim()
                );
                let next = self.start(
                    repo,
                    card,
                    Some(prev.id),
                    StartOpts {
                        message: Some(first),
                        round,
                        ..Default::default()
                    },
                )?;
                Ok(serde_json::json!({ "run": next }))
            }
            ReviewAction::Approve => {
                // Approved work joins the merge queue; the card moves when its lane lands.
                let branch = c.lane.clone().context("this card has no lane to ship")?;
                // Worked directly in its target: the work is already there.
                if c.direct() {
                    let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
                    cards::update(
                        &r,
                        card,
                        CardPatch {
                            state: Some(CardState::Done),
                            ..Default::default()
                        },
                    )?;
                    return Ok(serde_json::json!({ "card": card, "landed_in": branch }));
                }
                // The related repositories' lanes go first; the card moves when this one lands.
                if let Some(l) = &last
                    && !l.related.is_empty()
                {
                    for rl in &l.related {
                        let ahead = Repo::open(&rl.root, &home_dir(), &self.cfg.home)
                            .ok()
                            .and_then(|rr| {
                                let t = c
                                    .target
                                    .clone()
                                    .unwrap_or_else(|| rr.flow.flow.integration.clone());
                                rr.git
                                    .run(&["rev-list", "--count", &format!("{t}..{}", rl.branch)])
                                    .ok()?
                                    .trim()
                                    .parse::<u32>()
                                    .ok()
                            });
                        if ahead != Some(0) {
                            self.enqueue_for(
                                &rl.root,
                                &rl.branch,
                                c.target.as_deref(),
                                Some((card, repo.clone())),
                            )?;
                        }
                    }
                }
                let job = self.enqueue(&repo, &branch, c.target.as_deref())?;
                Ok(serde_json::json!({ "card": card, "job": job, "shipped_to": job.target }))
            }
        }
    }

    // ------------------------------------------------------------ merge queue

    /// Puts a lane in line to ship into `to` (default: its next stage). One open job per lane.
    pub fn enqueue(
        &self,
        repo: &Path,
        branch: &str,
        to: Option<&str>,
    ) -> Result<pit_core::queue::ShipJob> {
        self.enqueue_for(repo, branch, to, None)
    }

    /// A card's work, here and in its related repositories, in line to land in its Into branch:
    /// the related lanes first (those with commits of their own), this repository's last, so the
    /// card moves once all of it is in.
    fn land_card(
        &self,
        repo: &Path,
        c: &cards::Card,
        related: &[pit_core::related::RelatedLane],
    ) -> Result<()> {
        let lane = c.lane.clone().context("this card has no lane to ship")?;
        for l in related {
            let Ok(rr) = Repo::open(&l.root, &home_dir(), &self.cfg.home) else {
                continue;
            };
            let target = c
                .target
                .clone()
                .unwrap_or_else(|| rr.flow.flow.integration.clone());
            let ahead = rr
                .git
                .run(&["rev-list", "--count", &format!("{target}..{}", l.branch)])
                .ok()
                .and_then(|n| n.trim().parse::<u32>().ok());
            if ahead == Some(0) {
                continue;
            }
            self.enqueue_for(
                &l.root,
                &l.branch,
                c.target.as_deref(),
                Some((c.id, repo.to_path_buf())),
            )?;
        }
        self.enqueue(repo, &lane, c.target.as_deref())?;
        Ok(())
    }

    /// `card`: the card and the repository whose board holds it, for a related repository's lane.
    pub fn enqueue_for(
        &self,
        repo: &Path,
        branch: &str,
        to: Option<&str>,
        card: Option<(u32, PathBuf)>,
    ) -> Result<pit_core::queue::ShipJob> {
        use pit_core::queue::{JobState, ShipJob};
        let r = Repo::open(repo, &home_dir(), &self.cfg.home)?;
        if r.flow.flow.is_trunk(branch) {
            bail!("{branch} is a trunk branch; only lanes ship");
        }
        // The target is decided now, against what is known locally; the worker fetches before merging.
        let target = match to {
            Some(t) => t.to_owned(),
            None => actions::plan_ship(&r, branch, None, false)?.target,
        };
        let (card, card_repo) = match card {
            Some((id, board)) => (Some(id), Some(board)),
            None => (
                cards::list(&r).ok().and_then(|(all, _)| {
                    all.into_iter()
                        .find(|c| c.lane.as_deref() == Some(branch))
                        .map(|c| c.id)
                }),
                None,
            ),
        };
        let job = {
            let mut q = self.queue.lock().unwrap();
            if let Some(open) = q
                .iter()
                .find(|j| j.repo == r.root && j.branch == branch && j.state.is_open())
            {
                bail!(
                    "{branch} is already in line for {} ({})",
                    open.target,
                    open.id
                );
            }
            let job = ShipJob {
                id: format!(
                    "q{}-{}",
                    now_secs(),
                    &uuid::Uuid::new_v4().simple().to_string()[..4]
                ),
                repo: r.root.clone(),
                branch: branch.to_owned(),
                target,
                state: JobState::Queued,
                queued_at: now_secs(),
                finished_at: None,
                merge_sha: None,
                conflicts: Vec::new(),
                error: None,
                card,
                card_repo,
            };
            q.push(job.clone());
            pit_core::queue::save(&self.cfg.home, &q)?;
            job
        };
        self.queue_wake.notify_one();
        Ok(job)
    }

    /// The queue of a repository (or all): open jobs, then the last 20 finished.
    pub fn queue_list(&self, repo: Option<&Path>) -> Vec<pit_core::queue::ShipJob> {
        let q = self.queue.lock().unwrap();
        let mine: Vec<_> = q
            .iter()
            .filter(|j| repo.is_none_or(|r| j.repo == r))
            .cloned()
            .collect();
        let mut open: Vec<_> = mine.iter().filter(|j| j.state.is_open()).cloned().collect();
        open.sort_by_key(|j| j.queued_at);
        let mut done: Vec<_> = mine.into_iter().filter(|j| !j.state.is_open()).collect();
        done.sort_by_key(|j| std::cmp::Reverse(j.finished_at.unwrap_or(j.queued_at)));
        open.extend(done.into_iter().take(20));
        open
    }

    /// Takes a job out of line (not one that is shipping right now).
    pub fn resume_queue(&self) -> Result<()> {
        pit_core::queue::set_paused(&self.cfg.home, false)?;
        self.queue_wake.notify_one();
        Ok(())
    }

    pub fn dequeue(&self, id: &str) -> Result<()> {
        use pit_core::queue::JobState;
        let mut q = self.queue.lock().unwrap();
        let j = q
            .iter_mut()
            .find(|j| j.id == id)
            .with_context(|| format!("no job {id}"))?;
        if j.state == JobState::Shipping {
            bail!("{} is shipping right now", j.branch);
        }
        j.state = JobState::Removed;
        j.finished_at = Some(now_secs());
        pit_core::queue::save(&self.cfg.home, &q)
    }

    /// The Performance governor: samples the Mac every 2 s and pauses, resumes and throttles
    /// the agents to fit the mode (see `perf`).
    pub fn start_perf(self: &Arc<Self>) {
        self.perf.enable();
        let me = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                let sessions: Vec<crate::perf::LiveSession> = me
                    .live
                    .lock()
                    .unwrap()
                    .values()
                    .map(|l| crate::perf::LiveSession {
                        meta: l.meta.clone(),
                    })
                    .collect();
                let m = Arc::clone(&me);
                let notes = tokio::task::spawn_blocking(move || m.perf.tick(&sessions))
                    .await
                    .unwrap_or_default();
                for (id, text) in notes {
                    me.emit(&id, RunEvent::Note { text });
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    /// Pauses (`hold`) or resumes a session by hand, from the Performance screen.
    pub fn perf_hold(&self, id: &str, hold: bool) -> Result<bool> {
        let (pid, card, title) = {
            let live = self.live.lock().unwrap();
            let m = &live
                .get(id)
                .with_context(|| format!("run {id} is not running"))?
                .meta;
            (m.pid, m.card, m.card_title.clone())
        };
        if hold {
            let pid = pid.context("its agent has not started yet")?;
            let done = self.perf.hold(id, pid);
            if done {
                self.perf.log_manual("paused_you", card, &title);
                self.emit(id, RunEvent::Note { text: "Paused by you from Performance. Nothing is lost; Resume continues where it was.".into() });
            }
            Ok(done)
        } else {
            let done = self.perf.release(id, pid);
            if done {
                self.perf.log_manual("resumed_you", card, &title);
                self.emit(
                    id,
                    RunEvent::Note {
                        text: "Resumed by you from Performance.".into(),
                    },
                );
            }
            Ok(done)
        }
    }

    /// A card's run waits here, before its agent starts, while the mode or the memory has no
    /// room for one more. Stop ends the wait.
    async fn wait_room(&self, id: &str) {
        if !self.perf.enabled() {
            return;
        }
        let mut said = false;
        loop {
            let working = self
                .live
                .lock()
                .unwrap()
                .values()
                .filter(|l| {
                    l.meta.id != id
                        && l.meta.kind == SessionKind::Run
                        && l.meta.pid.is_some()
                        && !self.perf.is_held(&l.meta.id)
                })
                .count() as u32;
            let allowed = self.perf.allowed();
            if working < allowed || self.stop_requested(id) {
                break;
            }
            if !said {
                said = true;
                self.perf.set_waiting(id, true);
                self.emit(id, RunEvent::Note { text: format!("Waiting for room: Performance lets {allowed} card(s) work at once right now. It starts as soon as one ends or there is room.") });
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        self.perf.set_waiting(id, false);
    }

    pub fn start_queue(self: &Arc<Self>) {
        let me = Arc::clone(self);
        tokio::spawn(async move { me.run_queue().await });
    }

    /// Ships queued jobs one at a time, oldest first, forever.
    async fn run_queue(self: Arc<Self>) {
        use pit_core::queue::{JobState, Outcome};
        loop {
            if pit_core::queue::paused(&self.cfg.home) {
                self.queue_wake.notified().await;
                continue;
            }
            let next = {
                let mut q = self.queue.lock().unwrap();
                let pick = q
                    .iter_mut()
                    .filter(|j| j.state == JobState::Queued)
                    .min_by_key(|j| j.queued_at);
                pick.map(|j| {
                    j.state = JobState::Shipping;
                    j.clone()
                })
            };
            let Some(job) = next else {
                self.queue_wake.notified().await;
                continue;
            };
            let _ = pit_core::queue::save(&self.cfg.home, &self.queue.lock().unwrap());
            let (home, ph) = (home_dir(), self.cfg.home.clone());
            let (repo, branch, target) = (job.repo.clone(), job.branch.clone(), job.target.clone());
            let result = tokio::task::spawn_blocking(
                move || -> Result<(pit_core::queue::Outcome, Option<u32>)> {
                    let out = pit_core::queue::ship(&repo, &home, &ph, &branch, &target)?;
                    let card = match &out {
                        Outcome::Shipped { .. } => {
                            pit_core::queue::move_card(&repo, &home, &ph, &branch, &target)?
                        }
                        Outcome::Conflict { .. } => None,
                    };
                    Ok((out, card))
                },
            )
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r);
            if let Ok((Outcome::Conflict { files }, _)) = &result {
                self.conflict_round(&job, files);
            }
            let mut q = self.queue.lock().unwrap();
            if let Some(j) = q.iter_mut().find(|j| j.id == job.id) {
                j.finished_at = Some(now_secs());
                match result {
                    Ok((Outcome::Shipped { merge_sha }, _)) => {
                        j.state = JobState::Done;
                        j.merge_sha = merge_sha;
                    }
                    Ok((Outcome::Conflict { files }, _)) => {
                        j.state = JobState::Conflict;
                        j.conflicts = files;
                    }
                    Err(e) => {
                        j.state = JobState::Failed;
                        j.error = Some(format!("{e:#}"));
                    }
                }
            }
            let _ = pit_core::queue::save(&self.cfg.home, &q);
        }
    }

    /// A Run all card whose lane conflicts with its Into branch: its agent brings the branch in and
    /// resolves the conflicts itself, then the card lands as usual. Past the last round it waits.
    fn conflict_round(self: &Arc<Self>, job: &pit_core::queue::ShipJob, files: &[String]) {
        let Some(card) = job.card else { return };
        let board_repo = job.card_repo.clone().unwrap_or_else(|| job.repo.clone());
        let Ok(r) = Repo::open(&board_repo, &home_dir(), &self.cfg.home) else {
            return;
        };
        let Some(last) = self.list(Some(&r.root)).ok().and_then(|l| {
            l.into_iter()
                .filter(|m| m.card == card && m.kind == SessionKind::Run)
                .max_by_key(|m| (m.round, m.started_at))
        }) else {
            return;
        };
        if !last.autopilot {
            return;
        }
        if last.round >= r.flow.verify.max_rounds {
            let _ = cards::note(
                &r,
                card,
                "pitlane",
                &format!(
                    "{} conflicts with {} in {}; out of rounds, so it waits for you.",
                    job.branch,
                    job.target,
                    files.join(", ")
                ),
            );
            let _ = cards::update(
                &r,
                card,
                CardPatch {
                    state: Some(CardState::Blocked),
                    ..Default::default()
                },
            );
            return;
        }
        // In a related repository the merge happens in that repository's worktree.
        let place = match &job.card_repo {
            Some(_) => Repo::open(&job.repo, &home_dir(), &self.cfg.home)
                .and_then(|rr| actions::ensure_worktree(&rr, &job.branch))
                .map(|w| format!(" (in `{}`, use `git -C` there)", w.display()))
                .unwrap_or_default(),
            None => String::new(),
        };
        let msg = format!(
            "Your work could not land: {} changed meanwhile and conflicts with it in {}. Pitlane asks you to \
             bring it in now{place}: run `git merge {}`, resolve every conflict keeping both sides' intent, run the narrowest \
             checks for what you touched, commit the merge, and end with the handoff.",
            job.target,
            files.join(", "),
            job.target
        );
        let opts = StartOpts {
            autopilot: true,
            round: last.round + 1,
            message: Some(msg),
            ..Default::default()
        };
        if let Err(e) = self.start(board_repo.clone(), card, Some(last.id.clone()), opts) {
            let _ = cards::note(
                &r,
                card,
                "pitlane",
                &format!("The conflict round did not start: {e:#}"),
            );
        }
    }

    /// Changes a run that is no longer live, on disk.
    fn update_disk(&self, id: &str, f: impl FnOnce(&mut RunMeta)) {
        if let Ok(mut m) = self.get(id) {
            f(&mut m);
            let _ = self.write_meta(&m);
        }
    }

    /// Continues a run that hit its limit; `allow_api_key` is the person's yes to the paid option.
    pub fn continue_run(self: &Arc<Self>, id: &str, allow_api_key: bool) -> Result<RunMeta> {
        let prev = self.get(id)?;
        if prev.state.is_live() {
            bail!("run {id} is still running");
        }
        let next = self.start(
            prev.repo.clone(),
            prev.card,
            None,
            StartOpts {
                allow_api_key,
                handoff_from: Some(prev.id.clone()),
                round: prev.round,
                ..Default::default()
            },
        )?;
        self.update_disk(id, |m| m.next = None);
        Ok(next)
    }

    /// A message to the agent while it works: it reads it after its current step.
    pub async fn say(&self, id: &str, text: &str) -> Result<()> {
        let mut stdin = {
            let mut live = self.live.lock().unwrap();
            let l = live
                .get_mut(id)
                .with_context(|| format!("run {id} is not running; resume it to continue"))?;
            let s = l
                .stdin
                .take()
                .context("the agent already finished this session; resume it to continue")?;
            l.sent += 1;
            s
        };
        let res = async {
            stdin
                .write_all(format!("{}\n", runs::claude_user_line(text)).as_bytes())
                .await?;
            stdin.flush().await
        }
        .await;
        if let Some(l) = self.live.lock().unwrap().get_mut(id) {
            l.stdin = Some(stdin);
        }
        res?;
        self.emit(
            id,
            RunEvent::Said {
                text: text.to_owned(),
            },
        );
        Ok(())
    }

    /// `/model` and `/effort` while the session lives: Claude Code's own control requests on
    /// stdin (`set_model`, `apply_flag_settings`), as its SDK sends them. Not a message: the
    /// turn count that closes stdin does not move.
    pub async fn tune(&self, id: &str, model: Option<&str>, effort: Option<&str>) -> Result<()> {
        if let Some(e) = effort
            && !EFFORTS.contains(&e)
        {
            bail!("effort is one of {}", EFFORTS.join(", "));
        }
        if let Some(m) = model
            && (m.is_empty() || m.chars().any(|c| !(c.is_ascii_alphanumeric() || "-._[]".contains(c))))
        {
            bail!("{m:?} is not a model name");
        }
        let mut lines = String::new();
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        if let Some(m) = model {
            let req = serde_json::json!({ "type": "control_request", "request_id": format!("pit-model-{id}-{n}"),
                "request": { "subtype": "set_model", "model": m } });
            lines.push_str(&format!("{req}\n"));
        }
        if let Some(e) = effort {
            let req = serde_json::json!({ "type": "control_request", "request_id": format!("pit-effort-{id}-{n}"),
                "request": { "subtype": "apply_flag_settings", "settings": { "effortLevel": e } } });
            lines.push_str(&format!("{req}\n"));
        }
        if lines.is_empty() {
            return Ok(());
        }
        let mut stdin = {
            let mut live = self.live.lock().unwrap();
            let l = live.get_mut(id).with_context(|| format!("run {id} is not running"))?;
            l.stdin.take().context("the agent already finished this session")?
        };
        let res = async {
            stdin.write_all(lines.as_bytes()).await?;
            stdin.flush().await
        }
        .await;
        if let Some(l) = self.live.lock().unwrap().get_mut(id) {
            l.stdin = Some(stdin);
        }
        res?;
        // The model shows once Claude Code answers with the one it resolved (`opus` is
        // claude-opus-5-5); the effort has nothing to resolve, so it shows now.
        if let Some(e) = effort.map(str::to_owned) {
            self.emit(id, RunEvent::Tuned { model: None, effort: Some(e.clone()) });
            self.update(id, move |meta| meta.effort = Some(e));
        }
        Ok(())
    }

    fn stop_requested(&self, id: &str) -> bool {
        self.live
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|l| l.stop_requested)
    }

    /// Stops every session (runs, analyses, judges) and every analysis still waiting for a slot.
    /// SIGINT first, SIGTERM after 4 s, SIGKILL after 8 s, always to the whole process group.
    pub fn stop_all(self: &Arc<Self>) -> usize {
        self.stop_where(|_| true)
    }

    /// Stops the sessions that match (and the analyses still waiting for a slot).
    pub fn stop_where(self: &Arc<Self>, which: impl Fn(&RunMeta) -> bool) -> usize {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.stop_matching(which)
    }

    /// Stops the live sessions that match; what waits for a slot keeps waiting.
    fn stop_matching(self: &Arc<Self>, which: impl Fn(&RunMeta) -> bool) -> usize {
        let pids: Vec<(String, u32)> = {
            let mut live = self.live.lock().unwrap();
            live.iter_mut()
                .filter(|(_, l)| which(&l.meta))
                .filter_map(|(id, l)| {
                    l.stop_requested = true;
                    l.meta.pid.map(|p| (id.clone(), p))
                })
                .collect()
        };
        let n = pids.len();
        for (id, pid) in &pids {
            // A paused session hears nothing until it runs again.
            self.perf.release(id, Some(*pid));
            signal(*pid, libc::SIGINT);
        }
        let me = Arc::clone(self);
        tokio::spawn(async move {
            for (after, sig) in [(4, libc::SIGTERM), (4, libc::SIGKILL)] {
                tokio::time::sleep(Duration::from_secs(after)).await;
                let live: Vec<String> = me.live.lock().unwrap().keys().cloned().collect();
                for (id, pid) in &pids {
                    if live.contains(id) {
                        signal(*pid, sig);
                    }
                }
            }
        });
        n
    }

    /// Stop all, wait for the sessions to go (at most 9 s), then end the daemon. Nothing it
    /// started outlives it.
    pub async fn shutdown(self: &Arc<Self>) {
        // Work left in the merge queue waits for a person to resume it.
        if self.queue.lock().unwrap().iter().any(|j| j.state.is_open()) {
            let _ = pit_core::queue::set_paused(&self.cfg.home, true);
        }
        self.stop_all();
        for _ in 0..90 {
            if self.live.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        for l in self.live.lock().unwrap().values() {
            if let Some(pid) = l.meta.pid {
                signal(pid, libc::SIGKILL);
            }
        }
        self.perf.release_all();
        let _ = fs::remove_file(runs::socket_path(&self.cfg.home));
        std::process::exit(0)
    }

    /// SIGINT ends the current turn cleanly; if the agent is still there after 10 s, SIGTERM.
    pub fn stop(self: &Arc<Self>, id: &str) -> Result<()> {
        let pid = {
            let mut live = self.live.lock().unwrap();
            let l = live
                .get_mut(id)
                .with_context(|| format!("run {id} is not running"))?;
            l.stop_requested = true;
            l.meta.pid
        };
        // Sessions it opened (its judge) stop with it.
        let children: Vec<String> = self
            .live
            .lock()
            .unwrap()
            .values()
            .filter(|l| l.meta.parent.as_deref() == Some(id))
            .map(|l| l.meta.id.clone())
            .collect();
        for c in children {
            let _ = self.stop(&c);
        }
        // Still preparing its worktree, or being checked: `drive` and `verify` see the request.
        let Some(pid) = pid else { return Ok(()) };
        self.perf.release(id, Some(pid));
        signal(pid, libc::SIGINT);
        let me = Arc::clone(self);
        let run = id.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let same = me
                .live
                .lock()
                .unwrap()
                .get(&run)
                .is_some_and(|l| l.meta.pid == Some(pid));
            if same {
                end_group(pid);
            }
        });
        Ok(())
    }
}

const ASSIST_RULES: &str = "You are Pitlane's board assistant. The person tells you, typed or dictated (so expect \
spoken phrasing and transcription slips), what work they want on this repository's board. You turn it into cards with \
the pitlane tools: one card per piece of work someone could finish on its own, a short imperative title, and a spec in \
markdown that says what and why, with acceptance in a line or two. Read the code when it helps you write a precise spec \
(paths, names), but never change files and never do the work itself. Before adding, list the cards to avoid duplicates; \
update an existing card instead when it is the same work. When they ask to change, annotate or delete cards, do that. End with a \
one-line summary per card you added or changed (#id and title).";

/// Claude Code's effort levels (`--effort`, `/effort`).
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// The model and effort you chose in Claude Code (`/model`), from the account's settings.json:
/// Pitlane starts sessions with project settings only, so it passes these on itself. `model` is
/// the one the session will use when Pitlane already picked one; the effort is yours for that
/// model (`modelSettings.<id>.effortLevel`, matched by alias: `opus` finds `claude-opus-5-5`).
/// Only these values are read; nothing about the login.
fn your_claude(config_dir: Option<&Path>, model: Option<&str>) -> (Option<String>, Option<String>) {
    let dir = config_dir
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from))
        .unwrap_or_else(|| home_dir().join(".claude"));
    let Ok(v) = fs::read_to_string(dir.join("settings.json"))
        .map_err(anyhow::Error::from)
        .and_then(|t| Ok(serde_json::from_str::<Value>(&t)?))
    else {
        return (None, None);
    };
    let yours = v
        .get("model")
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty())
        .map(str::to_owned);
    let Some(m) = model.map(str::to_owned).or_else(|| yours.clone()) else {
        return (None, None);
    };
    let effort = v
        .get("modelSettings")
        .and_then(Value::as_object)
        .and_then(|all| {
            let level = |k: &String| all.get(k)?.get("effortLevel")?.as_str().map(str::to_owned);
            // The exact id, else the newest id that contains the alias.
            let exact = all.keys().find(|k| **k == m).cloned();
            let by_alias = || {
                let mut ks: Vec<&String> = all.keys().filter(|k| k.contains(m.as_str())).collect();
                ks.sort();
                ks.last().map(|k| (*k).clone())
            };
            exact.or_else(by_alias).and_then(|k| level(&k))
        });
    (yours, effort)
}

/// A session's arguments with the language you chose for agents added to its system prompt.
fn with_language(args: &[String], home: &Path) -> Vec<String> {
    let rule = pit_core::language::rule(&pit_core::language::agent_language(home));
    let mut out = args.to_vec();
    match out.iter().position(|a| a == "--append-system-prompt") {
        Some(i) if i + 1 < out.len() => out[i + 1].push_str(&rule),
        _ => out.extend([
            "--append-system-prompt".into(),
            rule.trim_start().to_owned(),
        ]),
    }
    out
}

/// The repository's permission mode for Claude Code (`[run] permission_mode`, `auto` by default).
fn mode_for(repo: &Path) -> String {
    Repo::open(repo, &home_dir(), &pit_core::default_homes().1)
        .map(|r| r.flow.run.permission_mode)
        .unwrap_or_else(|_| "auto".into())
}

/// A session that reads and never writes (analysis, judge): the repository's mode, without the
/// editing tools, and without asking the person in the middle (its questions go in its answer).
fn read_only_args(mode: &str) -> Vec<String> {
    [
        "--setting-sources",
        "project,local",
        "--permission-mode",
        mode,
        "--disallowedTools",
        "Edit,Write,NotebookEdit,AskUserQuestion",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Removes its key from the set when dropped.
struct Release<'a>(
    &'a Mutex<std::collections::HashSet<(PathBuf, u32)>>,
    (PathBuf, u32),
);
impl Drop for Release<'_> {
    fn drop(&mut self) {
        self.0.lock().unwrap().remove(&self.1);
    }
}

/// Is that pid still a Claude Code process (and not a stranger who got the number)?
fn still_agent(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("claude"))
        .unwrap_or(false)
}

/// SIGTERM to the whole group, SIGKILL a moment later, off the async threads.
fn end_group(pid: u32) {
    signal(pid, libc::SIGTERM);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        signal(pid, libc::SIGKILL);
    });
}

/// Signals the agent's whole process group, like Ctrl-C in a terminal: the agent and the
/// command it is running both hear it.
fn signal(pid: u32, sig: i32) {
    // SAFETY: kill(2) on the group of a process we spawned with its own group; a stale id only
    // yields ESRCH.
    unsafe {
        libc::kill(-(pid as i32), sig);
    }
}

fn home_dir() -> PathBuf {
    pit_core::default_homes().0
}

struct Prepared {
    branch: String,
    worktree: PathBuf,
    briefing: String,
    brief: Option<pit_core::brief::BriefStats>,
    tools: Vec<String>,
    max_turns: u32,
    model: Option<String>,
    /// The card's images, sent with the briefing.
    images: Vec<(&'static str, Vec<u8>)>,
    /// The same branch in the project's related repositories.
    related: Vec<pit_core::related::RelatedLane>,
}

/// The card's lane and worktree, made when missing, and what the agent is launched with.
/// Commits whatever is uncommitted in the worktree, so the next agent starts from it.
fn checkpoint(worktree: &Path, card: u32) -> Result<Option<String>> {
    let g = pit_core::git::Git::new(worktree);
    if g.lines(&["status", "--porcelain"])?.is_empty() {
        return Ok(None);
    }
    g.run(&["add", "-A"])?;
    g.run(&[
        "commit",
        "-q",
        "-m",
        &format!("pitlane: checkpoint of card #{card} at a usage limit"),
    ])?;
    Ok(Some(g.run(&["rev-parse", "--short", "HEAD"])?))
}

/// The agent's last words in a run: its handoff summary if any, else its last two messages.
fn last_words(timeline: &[(usize, RunEvent)]) -> String {
    if let Some(s) = timeline.iter().rev().find_map(|(_, e)| match e {
        RunEvent::Finished {
            handoff: Some(h), ..
        } => h.get("summary").and_then(|s| s.as_str()).map(str::to_owned),
        _ => None,
    }) {
        return s;
    }
    let texts: Vec<&str> = timeline
        .iter()
        .filter_map(|(_, e)| match e {
            RunEvent::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    texts
        .iter()
        .rev()
        .take(2)
        .rev()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[allow(clippy::too_many_arguments)]
fn prepare(
    repo: &Path,
    card: u32,
    home: &Path,
    continues: Option<(&RunMeta, &str)>,
    brief: bool,
    into: Option<&str>,
    direct: bool,
    note: impl Fn(String),
) -> Result<Prepared> {
    let r = Repo::open(repo, &home_dir(), home)?;
    let c = cards::get(&r, card)?;
    let into = into.filter(|b| !b.trim().is_empty());
    let branch = match (&c.lane, into) {
        (Some(b), _) => b.clone(),
        (None, Some(t)) if direct => {
            note(format!("Working directly in {t}"));
            cards::update(
                &r,
                card,
                CardPatch {
                    lane: Some(Some(t.to_owned())),
                    target: Some(Some(t.to_owned())),
                    ..Default::default()
                },
            )?;
            t.to_owned()
        }
        (None, _) => {
            let branch = format!(
                "{}{}-{}",
                r.flow.run.branch_prefix,
                c.id,
                cards::slug(&c.title)
            );
            let from = into.filter(|t| !r.flow.flow.is_trunk(t));
            match from {
                Some(f) => note(format!("Creating lane {branch} from {f}")),
                None => note(format!(
                    "Creating lane {branch} from {}/{}",
                    r.flow.flow.remote, r.flow.flow.base
                )),
            }
            actions::new_lane_from(
                &r,
                &branch,
                NewOptions {
                    fetch: true,
                    setup: true,
                },
                from,
            )?;
            cards::update(
                &r,
                card,
                CardPatch {
                    lane: Some(Some(branch.clone())),
                    target: Some(into.map(str::to_owned)),
                    ..Default::default()
                },
            )?;
            if let Some(t) = into {
                note(format!("It lands in {t} once you approve it"));
            }
            branch
        }
    };
    let worktree = if r.git.rev_parse(&format!("refs/heads/{branch}"))?.is_some() {
        actions::ensure_worktree(&r, &branch)?
    } else {
        note(format!("Checking out {branch} from {}", r.flow.flow.remote));
        actions::new_lane(
            &r,
            &branch,
            NewOptions {
                fetch: true,
                setup: true,
            },
        )?
        .path
    };
    note(format!("Working in {}", worktree.display()));
    let mut tools: Vec<String> = RUN_BASE_TOOLS.iter().map(|s| s.to_string()).collect();
    tools.push("mcp__pitlane_run__note".into());
    tools.extend(r.flow.run.allow.iter().cloned());
    let the_card = cards::get(&r, card)?;
    // The same branch in each related repository, made and landed like this one.
    let related = pit_core::related::prepare(
        &r,
        &branch,
        the_card.target.as_deref(),
        the_card.direct(),
        &note,
    );
    // Where to look, from the index. A failure here never stops the run; it runs without the map.
    let map = if brief && r.flow.run.brief {
        match pit_core::brief::compile(&r, home, &the_card, pit_core::brief::DEFAULT_BUDGET) {
            Ok(b) if !b.text.is_empty() => {
                note(format!(
                    "Briefed with {} from the index (~{} tokens)",
                    plural(b.stats.files, "file"),
                    b.stats.chars / 4
                ));
                Some(b)
            }
            Ok(_) => {
                note("Nothing in the index matches this card; running without a map".into());
                None
            }
            Err(e) => {
                note(format!(
                    "Could not compile the map ({e:#}); running without it"
                ));
                None
            }
        }
    } else {
        None
    };
    Ok(Prepared {
        images: cards::image_bytes(&r, &the_card),
        related: related.clone(),
        branch,
        worktree: worktree.clone(),
        brief: map.as_ref().map(|b| b.stats.clone()),
        briefing: {
            let mut b = runs::briefing(&r, &the_card);
            let also = pit_core::related::briefing(&related);
            if !also.is_empty() {
                let end = "\nWhen you finish, end with the handoff.\n";
                let at = b.rfind(end).unwrap_or(b.len());
                b.insert_str(at, &also);
            }
            if let Some(m) = &map {
                let end = "\nWhen you finish, end with the handoff.\n";
                let at = b.rfind(end).unwrap_or(b.len());
                b.insert_str(at, &format!("\n{}", m.text));
            }
            if let Some((prev, words)) = continues {
                let g = pit_core::git::Git::new(&worktree);
                // Production as the yard resolves it: the remote branch, or the local one without a remote.
                let base = pit_core::lanes::Target::resolve(&r, &r.flow.flow.production)
                    .ok()
                    .flatten()
                    .map(|t| t.sha);
                let log = match base {
                    Some(b) => g
                        .run(&["log", "--oneline", "-20", &format!("{b}..HEAD")])
                        .unwrap_or_default(),
                    None => g.run(&["log", "--oneline", "-5"]).unwrap_or_default(),
                };
                let status = g.run(&["status", "--short"]).unwrap_or_default();
                b.push_str(&format!(
                    "\n## You are continuing run {}\n\nIt stopped at a usage limit on another account or model. Its work is committed on this branch; carry on from there, do not redo it.\n",
                    prev.id
                ));
                if !words.trim().is_empty() {
                    b.push_str(&format!("\nWhere it was:\n\n{}\n", words.trim()));
                }
                if !log.is_empty() {
                    b.push_str(&format!("\nCommits so far:\n\n{log}\n"));
                }
                if !status.is_empty() {
                    b.push_str(&format!("\nUncommitted:\n\n{status}\n"));
                }
            }
            b
        },
        tools,
        max_turns: r.flow.run.max_turns,
        model: r.flow.run.model.clone(),
    })
}

fn set_card_state(m: &RunMeta, state: CardState, home: &Path) -> Result<()> {
    let r = Repo::open(&m.repo, &home_dir(), home)?;
    cards::update(
        &r,
        m.card,
        CardPatch {
            state: Some(state),
            ..Default::default()
        },
    )?;
    Ok(())
}

/// Where the card goes when its run ends.
fn settle_card(m: &RunMeta, home: &Path) -> Result<()> {
    let r = Repo::open(&m.repo, &home_dir(), home)?;
    let status = m
        .handoff
        .as_ref()
        .and_then(|h| h.get("status"))
        .and_then(Value::as_str);
    // The handoff stays on the card as a note: the next session starts from it.
    if let Some(h) = &m.handoff
        && let Some(summary) = h
            .get("summary")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
    {
        let mut text = format!(
            "Run {} ended {}: {}",
            m.id,
            status.unwrap_or("done"),
            summary.trim()
        );
        let next: Vec<&str> = h
            .get("next_steps")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if !next.is_empty() {
            text.push_str("\n\nLeft to do:\n");
            for n in next {
                text.push_str(&format!("- {n}\n"));
            }
        }
        let _ = cards::note(&r, m.card, "claude", &text);
    }
    if let Some(qs) = m
        .handoff
        .as_ref()
        .and_then(|h| h.get("questions"))
        .and_then(Value::as_array)
    {
        let qs: Vec<NewQuestion> = qs
            .iter()
            .filter_map(|q| serde_json::from_value(q.clone()).ok())
            .collect();
        if status == Some("blocked") {
            if !qs.is_empty() {
                cards::ask(&r, m.card, qs)?;
            }
        } else {
            // A run that finished does not get to ask: what it would have asked is on the card, for the review.
            for q in qs {
                let _ = cards::note(&r, m.card, "claude", &unasked(&q));
            }
        }
    }
    let next = match (m.state, status) {
        (RunState::Done, Some("blocked")) => CardState::Blocked,
        (RunState::Done, _) => CardState::Review,
        (RunState::Stopped, _) => CardState::Ready,
        _ => CardState::Blocked,
    };
    cards::update(
        &r,
        m.card,
        CardPatch {
            state: Some(next),
            ..Default::default()
        },
    )?;
    Ok(())
}

/// What a fix round is asked to fix: the failing proofs' tails and the judge's findings.
fn fix_request(dir: &Path) -> String {
    let Ok(v) = fs::read_to_string(dir.join("verify.json"))
        .map_err(anyhow::Error::from)
        .and_then(|t| Ok(serde_json::from_str::<pit_core::verify::Verification>(&t)?))
    else {
        return "The checks or the judge found problems; look at the diff again and fix what is wrong.".into();
    };
    let mut out = String::new();
    for p in v.proofs.iter().filter(|p| !p.ok) {
        out.push_str(&format!(
            "- `{}` fails:\n```\n{}\n```\n",
            p.command,
            p.tail.trim()
        ));
    }
    if let Some(j) = v.judge.as_ref().and_then(|j| j.judgement.as_ref()) {
        if !j.findings.is_empty() || j.verdict == pit_core::verify::Verdict::Changes {
            out.push_str(&format!("The reviewer says: {}\n", j.summary.trim()));
        }
        for f in &j.findings {
            let at = match (&f.file, f.line) {
                (Some(file), Some(l)) => format!(" ({file}:{l})"),
                (Some(file), None) => format!(" ({file})"),
                _ => String::new(),
            };
            out.push_str(&format!("- [{}]{at} {}\n", f.severity, f.text.trim()));
        }
    }
    if out.is_empty() {
        "The checks or the judge found problems; look at the diff again and fix what is wrong."
            .into()
    } else {
        out
    }
}

/// A question a finished run raised anyway, as a note: what it asked and what it went with.
fn unasked(q: &NewQuestion) -> String {
    let went = q.recommended.and_then(|i| q.options.get(i));
    match went {
        Some(o) => format!("Decided without asking: {} Went with: {o}", q.text.trim()),
        None => format!("Left for the review: {}", q.text.trim()),
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod your_claude_tests {
    use super::*;

    #[test]
    fn reads_your_model_and_its_effort() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("settings.json"),
            r#"{"model":"opus","modelSettings":{"claude-opus-5-5":{"effortLevel":"medium"},"claude-fable-5-2":{"effortLevel":"xhigh"}}}"#,
        )
        .unwrap();
        assert_eq!(
            your_claude(Some(d.path()), None),
            (Some("opus".into()), Some("medium".into()))
        );
        // A model Pitlane picked gets your effort for that model.
        assert_eq!(
            your_claude(Some(d.path()), Some("fable")).1,
            Some("xhigh".into())
        );
        assert_eq!(your_claude(Some(d.path()), Some("sonnet")).1, None);
        assert_eq!(
            your_claude(Some(&d.path().join("none")), None),
            (None, None)
        );
    }
}
