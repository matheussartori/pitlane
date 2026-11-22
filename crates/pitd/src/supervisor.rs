//! Owns the agent processes: start a card's run in its worktree, read its events, let the
//! person talk to it, stop it, and record everything under `~/.pitlane/runs/<id>/`.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use pit_core::Repo;
use pit_core::actions::{self, NewOptions};
use pit_core::cards::{self, CardPatch, CardState, NewQuestion};
use pit_core::flow::RUN_BASE_TOOLS;
use pit_core::lanes::now_secs;
use pit_core::accounts::{self, Choice, Pick, Routing, Situation, Usage};
use pit_core::flow;
use pit_core::runs::{self, Approval, Decision, RunEvent, RunMeta, RunState};
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
    live: Mutex<HashMap<String, Live>>,
    /// Last usage events seen, per account.
    quota: Mutex<HashMap<String, Vec<RunEvent>>>,
    /// The merge queue, shipped one job at a time by `run_queue`.
    queue: Mutex<Vec<pit_core::queue::ShipJob>>,
    queue_wake: tokio::sync::Notify,
}

pub struct Subscription {
    pub replay: Vec<(usize, RunEvent)>,
    /// `None` when the run is not live: the replay is all there is.
    pub rx: Option<broadcast::Receiver<(usize, RunEvent)>>,
}

impl Supervisor {
    pub fn new(cfg: Config) -> Arc<Self> {
        let queue = pit_core::queue::load(&cfg.home);
        Arc::new(Self {
            cfg,
            live: Mutex::new(HashMap::new()),
            quota: Mutex::new(HashMap::new()),
            queue: Mutex::new(queue),
            queue_wake: tokio::sync::Notify::new(),
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
                m.state = RunState::Interrupted;
                m.ended_at.get_or_insert(now_secs());
                m.error.get_or_insert_with(|| "the daemon stopped while this run was open".into());
                self.write_meta(&m)?;
                n += 1;
            }
        }
        Ok(n)
    }

    fn list_disk(&self) -> Result<Vec<RunMeta>> {
        let dir = runs::runs_dir(&self.cfg.home);
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(&dir) else { return Ok(out) };
        for e in rd.flatten() {
            if let Ok(t) = fs::read_to_string(e.path().join("meta.json"))
                && let Ok(m) = serde_json::from_str::<RunMeta>(&t)
            {
                out.push(m);
            }
        }
        Ok(out)
    }

    pub fn list(&self, repo: Option<&Path>) -> Result<Vec<RunMeta>> {
        let live = self.live.lock().unwrap();
        let mut all: Vec<RunMeta> = self
            .list_disk()?
            .into_iter()
            .map(|m| live.get(&m.id).map(|l| l.meta.clone()).unwrap_or(m))
            .filter(|m| repo.is_none_or(|r| m.repo == r))
            .collect();
        all.sort_by(|a, b| b.started_at.cmp(&a.started_at).then_with(|| b.id.cmp(&a.id)));
        Ok(all)
    }

    pub fn get(&self, id: &str) -> Result<RunMeta> {
        if let Some(l) = self.live.lock().unwrap().get(id) {
            return Ok(l.meta.clone());
        }
        let t = fs::read_to_string(self.dir(id).join("meta.json")).with_context(|| format!("no run {id}"))?;
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
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(self.dir(id).join("timeline.jsonl"))
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
            return Ok(Subscription { replay: self.read_timeline(id, from), rx: Some(rx) });
        }
        drop(live);
        self.get(id)?;
        Ok(Subscription { replay: self.read_timeline(id, from), rx: None })
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
    fn choose(&self, r: &Repo, resume: Option<&RunMeta>, opts: &StartOpts) -> Result<(Choice, String, Vec<String>)> {
        let routing = Routing::load(&self.cfg.home)?;
        if let Some(prev) = resume {
            let a = routing.account(&prev.account).with_context(|| format!("account {} is gone from routing.toml", prev.account))?;
            let choice = Choice {
                account: a.id.clone(),
                kind: a.kind,
                model: prev.model.clone(),
                config_dir: accounts::profile_dir(&a.id, &self.cfg.home),
                api_key_helper: a.api_key_helper.clone(),
            };
            return Ok((choice, format!("resumes {} in the same session", prev.id), prev.exhausted.clone()));
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
        let running: HashMap<String, usize> = self.live.lock().unwrap().values().fold(HashMap::new(), |mut m, l| {
            *m.entry(l.meta.account.clone()).or_default() += 1;
            m
        });
        let max_parallel: HashMap<String, u32> =
            routing.accounts.iter().filter_map(|a| a.max_parallel.map(|n| (a.id.clone(), n))).collect();
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
    pub fn start(self: &Arc<Self>, repo: PathBuf, card: u32, resume: Option<String>, opts: StartOpts) -> Result<RunMeta> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        let c = cards::get(&r, card)?;
        if c.open_questions() > 0 {
            bail!("card #{card} has {} open question(s); answer them in the Inbox first", c.open_questions());
        }
        {
            let live = self.live.lock().unwrap();
            if let Some(l) = live.values().find(|l| l.meta.repo == r.root && l.meta.card == card) {
                bail!("card #{card} is already running ({})", l.meta.id);
            }
        }
        let prev = resume.as_deref().map(|p| self.get(p)).transpose()?;
        let (choice, reason, exhausted) = self.choose(&r, prev.as_ref(), &opts)?;
        let (session_id, resumed_from) = match &prev {
            Some(p) => (p.session_id.clone(), Some(p.id.clone())),
            None => (uuid::Uuid::new_v4().to_string(), None),
        };
        let now = now_secs();
        let id = format!("{card}-{now}-{}", &uuid::Uuid::new_v4().simple().to_string()[..4]);
        let meta = RunMeta {
            id: id.clone(),
            repo: r.root.clone(),
            card,
            card_title: c.title.clone(),
            branch: c.lane.clone().unwrap_or_default(),
            worktree: PathBuf::new(),
            agent: "claude-code".into(),
            model: choice.model.clone().or_else(|| r.flow.run.model.clone()),
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
            if let Err(e) = me.clone().drive(run.clone(), repo, card, resume.is_some(), choice, opts).await {
                me.fail(&run, &format!("{e:#}"));
            }
        });
        Ok(meta)
    }

    fn fail(&self, id: &str, why: &str) {
        self.emit(id, RunEvent::Note { text: why.to_owned() });
        self.update(id, |m| {
            m.state = RunState::Failed;
            m.error = Some(why.to_owned());
            m.ended_at = Some(now_secs());
        });
        self.emit(id, RunEvent::Exited { code: None, state: RunState::Failed });
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
        let StartOpts { handoff_from, message, no_brief, .. } = opts;
        // Worktree and branch: blocking git work, off the async threads.
        let home = self.cfg.home.clone();
        let me = Arc::clone(&self);
        let rid = id.clone();
        let prev = handoff_from.as_deref().map(|p| self.get(p)).transpose()?;
        let prev_texts = handoff_from.as_deref().map(|p| last_words(&self.read_timeline(p, 0))).unwrap_or_default();
        let repo_for_prep = repo.clone();
        let prep = tokio::task::spawn_blocking(move || {
            prepare(&repo_for_prep, card, &home, prev.as_ref().map(|p| (p, prev_texts.as_str())), !no_brief, |t| me.emit(&rid, RunEvent::Note { text: t }))
        })
        .await??;
        let base_sha = pit_core::git::Git::new(&prep.worktree).run(&["rev-parse", "HEAD"]).ok();
        self.update(&id, |m| {
            m.branch = prep.branch.clone();
            m.worktree = prep.worktree.clone();
            m.base_sha = base_sha;
            m.brief = prep.brief.clone();
        });

        let session = self.get(&id)?.session_id;
        let mut cmd = Command::new(&self.cfg.claude_bin);
        cmd.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"]);
        if resuming {
            cmd.args(["--resume", &session]);
        } else {
            cmd.args(["--session-id", &session]);
        }
        cmd.args(["--setting-sources", "project,local", "--permission-mode", "acceptEdits"])
            .arg("--allowedTools")
            .arg(prep.tools.join(","))
            .args(["--max-turns", &prep.max_turns.to_string()])
            .arg("--append-system-prompt")
            .arg(runs::RULES)
            .arg("--json-schema")
            .arg(runs::handoff_schema().to_string());
        if let Some(m) = choice.model.as_ref().or(prep.model.as_ref()) {
            cmd.args(["--model", m]);
        }
        // The account: its own Claude Code directory, where you signed in. Pitlane only points at it.
        if let Some(dir) = &choice.config_dir {
            fs::create_dir_all(dir)?;
            cmd.env("CLAUDE_CONFIG_DIR", dir);
        }
        // An API key account: Claude Code runs your helper command; the key never passes here.
        if let Some(helper) = &choice.api_key_helper {
            cmd.arg("--settings").arg(serde_json::json!({ "apiKeyHelper": helper }).to_string());
        }
        self.emit(&id, RunEvent::Note { text: format!("Route: {}", self.get(&id)?.reason.unwrap_or_default()) });
        if let Some(pit) = &self.cfg.pit_bin {
            // Anything no rule allows is asked to a person, through Pitlane, and waits for them.
            let mcp = serde_json::json!({ "mcpServers": { "pitlane_run": {
                "command": pit, "args": ["mcp", "--approve", &id] } } });
            cmd.args(["--mcp-config", &mcp.to_string()])
                .args(["--permission-prompts", "host", "--permission-prompt-tool", "mcp__pitlane_run__approve"])
                .env("MCP_TOOL_TIMEOUT", "86400000");
        }
        cmd.current_dir(&prep.worktree)
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(false);
        let mut child = cmd.spawn().with_context(|| format!("could not start {}", self.cfg.claude_bin))?;
        let pid = child.id();
        let mut stdin = child.stdin.take().context("no stdin")?;
        let first = if resuming {
            message.clone().unwrap_or_else(|| "Continue where you stopped. End with the handoff.".to_owned())
        } else {
            fs::write(self.dir(&id).join("briefing.md"), &prep.briefing)?;
            prep.briefing.clone()
        };
        stdin.write_all(format!("{}\n", runs::claude_user_line(&first)).as_bytes()).await?;
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
            let mut f = OpenOptions::new().create(true).append(true).open(err_path).ok();
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
        let mut raw = OpenOptions::new().create(true).append(true).open(raw_path)?;
        let finished = Arc::new(Mutex::new(None::<bool>));
        let reader = {
            let me = Arc::clone(&self);
            let id = id.clone();
            let finished = Arc::clone(&finished);
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = writeln!(raw, "{line}");
                    let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
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
        // A tool's child process can keep stdout open after the agent is gone: read what is
        // left for a moment, then let go.
        let mut reader = reader;
        if tokio::time::timeout(Duration::from_secs(3), &mut reader).await.is_err() {
            reader.abort();
        }
        let tail = match tokio::time::timeout(Duration::from_secs(1), err_task).await {
            Ok(Ok(t)) => t,
            _ => Vec::new(),
        };
        let finished_ok = *finished.lock().unwrap();
        let stopped = self.live.lock().unwrap().get(&id).is_some_and(|l| l.stop_requested);
        let limited = !stopped && self.live.lock().unwrap().get(&id).is_some_and(|l| l.limited);
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
            if tail.is_empty() { format!("the agent exited with {status}") } else { tail.join("\n") }
        });
        // Finished work is checked before anyone reviews it: the repository's proofs, then the judge.
        let blocked = self.get(&id)?.handoff.as_ref().and_then(|h| h.get("status")).and_then(|s| s.as_str()) == Some("blocked");
        if state == RunState::Done && !blocked {
            self.verify(&id, card).await;
        }
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
        self.emit(&id, RunEvent::Exited { code: status.code(), state });
        self.live.lock().unwrap().remove(&id);
        card_result?;
        Ok(())
    }

    /// Keeps the run record in step with what the agent reports.
    async fn absorb(&self, id: &str, ev: &RunEvent, finished_ok: &mut Option<bool>) {
        match ev {
            RunEvent::Started { model, session } => {
                let (model, session) = (model.clone(), session.clone());
                self.update(id, move |m| {
                    m.model = Some(model);
                    if !session.is_empty() {
                        m.session_id = session;
                    }
                });
            }
            RunEvent::Quota { window, .. } => {
                let account = self.live.lock().unwrap().get(id).map(|l| l.meta.account.clone()).unwrap_or_default();
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
            RunEvent::Finished { ok, cost_usd, duration_ms, turns, tokens, handoff, denied, limited } => {
                *finished_ok = Some(*ok);
                if *limited && let Some(l) = self.live.lock().unwrap().get_mut(id) {
                    l.limited = true;
                }
                let (ok, cost, dur, turns, tokens, handoff, denied) =
                    (*ok, *cost_usd, *duration_ms, *turns, tokens.clone(), handoff.clone(), denied.clone());
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
                        m.error.get_or_insert_with(|| "the agent stopped with an error".into());
                    }
                });
                if let Some(h) = self.get(id).ok().and_then(|m| m.handoff) {
                    let _ = fs::write(self.dir(id).join("handoff.json"), serde_json::to_string_pretty(&h).unwrap_or_default());
                }
                // Every message answered: close stdin so the session ends.
                let stdin = {
                    let mut live = self.live.lock().unwrap();
                    live.get_mut(id).and_then(|l| {
                        l.results += 1;
                        (l.results >= l.sent).then(|| l.stdin.take()).flatten()
                    })
                };
                if let Some(mut s) = stdin {
                    let _ = s.shutdown().await;
                }
            }
            _ => {}
        }
    }

    /// The agent asks to use a tool no rule allows. Answers at once when a rule of this run or of
    /// the repository allows it; otherwise waits for a person. Returns Claude Code's answer:
    /// `{"behavior":"allow","updatedInput":…}` or `{"behavior":"deny","message":…}`.
    pub async fn ask(&self, run: &str, tool: &str, input: Value, tool_use_id: Option<String>) -> Value {
        let allow = |input: &Value| serde_json::json!({ "behavior": "allow", "updatedInput": input });
        let deny = |msg: &str| serde_json::json!({ "behavior": "deny", "message": msg });
        let (repo, run_rules) = {
            let live = self.live.lock().unwrap();
            let Some(l) = live.get(run) else { return deny("This run is not live any more.") };
            (l.meta.repo.clone(), l.rules.clone())
        };
        let repo_rules = Repo::open(&repo, &home_dir(), &self.cfg.home).map(|r| r.flow.run.allow).unwrap_or_default();
        if run_rules.iter().chain(repo_rules.iter()).any(|r| runs::rule_allows(r, tool, &input)) {
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
            let Some(l) = live.get_mut(run) else { return deny("This run is not live any more.") };
            l.pending.insert(approval.id.clone(), tx);
            l.meta.waiting.push(approval.clone());
        }
        self.update(run, |_| {});
        self.emit(run, RunEvent::Approval { approval: approval.clone() });

        let answer = rx.await;
        self.update(run, |m| m.waiting.retain(|a| a.id != approval.id));
        let Ok((decision, message)) = answer else { return deny("The run ended before anyone answered.") };
        match decision {
            Decision::Run => {
                if let Some(l) = self.live.lock().unwrap().get_mut(run) {
                    l.rules.push(approval.rule.clone());
                }
            }
            Decision::Always => {
                if let Err(e) = flow::add_run_allow(&repo, &approval.rule) {
                    self.emit(run, RunEvent::Note { text: format!("Could not save {} to flow.toml: {e:#}", approval.rule) });
                }
                if let Some(l) = self.live.lock().unwrap().get_mut(run) {
                    l.rules.push(approval.rule.clone());
                }
            }
            Decision::Once | Decision::Deny => {}
        }
        self.emit(run, RunEvent::Decided { id: approval.id.clone(), decision, message: message.clone() });
        if decision == Decision::Deny {
            deny(message.as_deref().filter(|m| !m.trim().is_empty()).unwrap_or("The person declined this. Find another way or report it in the handoff."))
        } else {
            allow(&input)
        }
    }

    /// Answers a waiting approval: the one named, or the oldest of the run.
    pub fn decide(&self, run: &str, approval: Option<&str>, decision: Decision, message: Option<String>) -> Result<String> {
        let mut live = self.live.lock().unwrap();
        let l = live.get_mut(run).with_context(|| format!("run {run} is not live"))?;
        let id = match approval {
            Some(a) => a.to_owned(),
            None => l.meta.waiting.first().map(|a| a.id.clone()).context("nothing is waiting in this run")?,
        };
        let tx = l.pending.remove(&id).with_context(|| format!("no approval {id} waiting in run {run}"))?;
        tx.send((decision, message)).map_err(|_| anyhow::anyhow!("the agent stopped waiting"))?;
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
    async fn hand_off(self: Arc<Self>, id: String, repo: PathBuf, card: u32, code: Option<i32>) -> Result<()> {
        let meta = self.get(&id)?;
        let worktree = meta.worktree.clone();
        let label = meta.card;
        let checkpoint = tokio::task::spawn_blocking(move || checkpoint(&worktree, label)).await?;
        if let Ok(Some(sha)) = &checkpoint {
            self.emit(&id, RunEvent::Note { text: format!("Committed the work so far as {sha} before handing off.") });
        }
        let why = format!("Hit the usage limit on {}.", meta.account);
        self.update(&id, |m| {
            m.state = RunState::Failed;
            m.ended_at = Some(now_secs());
            m.pid = None;
            m.waiting.clear();
            m.error = Some(why.clone());
        });
        self.emit(&id, RunEvent::Exited { code, state: RunState::Failed });
        self.live.lock().unwrap().remove(&id);

        match self.start(repo, card, None, StartOpts { allow_api_key: false, handoff_from: Some(id.clone()), round: meta.round, ..Default::default() }) {
            Ok(next) => {
                let text = format!("{why} Continues as run {} with {}.", next.id, next.reason.clone().unwrap_or_default());
                self.update_disk(&id, |m| m.error = Some(text.clone()));
                Ok(())
            }
            Err(e) => {
                let e = format!("{e:#}");
                let (next, text) = match e.strip_prefix(NEEDS_API_KEY) {
                    // The reason reads "<option>: <why the others were passed over>".
                    Some(reason) => {
                        let option = reason.split_once(':').map_or(reason, |(o, _)| o).trim().to_owned();
                        (Some(option.clone()), format!("{why} The next option, {option}, is paid per token and waits for your yes."))
                    }
                    None => (None, format!("{why} Nothing else in the route can take it now: {e}")),
                };
                // The card first: whoever sees the run's next option finds the card already waiting.
                let home = self.cfg.home.clone();
                tokio::task::spawn_blocking(move || set_card_state(&meta, CardState::Blocked, &home)).await??;
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
        let Ok(r) = Repo::open(&meta.repo, &home_dir(), &home) else { return };
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
            self.emit(id, RunEvent::Note { text: format!("Checking: {cmd}") });
            let (wt, c) = (meta.worktree.clone(), cmd.clone());
            let proof = tokio::task::spawn_blocking(move || pit_core::verify::run_proof(&wt, &c)).await.unwrap_or_else(|e| pit_core::verify::Proof {
                command: cmd.clone(),
                ok: false,
                code: None,
                duration_ms: 0,
                tail: e.to_string(),
            });
            self.emit(id, RunEvent::Proof { command: proof.command.clone(), ok: proof.ok, duration_ms: proof.duration_ms });
            v.proofs.push(proof);
        }
        if !v.proofs.is_empty() {
            let ok = v.proofs_ok();
            self.update(id, move |m| m.proofs_ok = Some(ok));
        }

        if cfg.judge {
            let model = cfg.judge_model.clone();
            self.emit(id, RunEvent::Note { text: format!("The judge reads the diff ({}), in a session of its own.", model.as_deref().unwrap_or("default model")) });
            let judge = self.judge(&meta, &r, card, &v.proofs, model.clone()).await;
            if let Some(j) = &judge.judgement {
                let (verdict, summary, n) = (j.verdict, j.summary.clone(), j.findings.len());
                self.update(id, move |m| m.verdict = Some(verdict));
                self.emit(id, RunEvent::Judged { verdict, summary, findings: n });
            } else if let Some(e) = &judge.error {
                self.emit(id, RunEvent::Note { text: format!("The judge could not finish: {e}") });
            }
            v.judge = Some(judge);
        }
        v.ended_at = Some(now_secs());
        let _ = fs::write(self.dir(id).join("verify.json"), serde_json::to_string_pretty(&v).unwrap_or_default());
    }

    /// The judge: a fresh Claude Code session, read-only, on the same account as the run. It gets
    /// the spec, the decisions and the diff; nothing the working agent said.
    async fn judge(&self, meta: &RunMeta, r: &Repo, card: u32, proofs: &[pit_core::verify::Proof], model: Option<String>) -> pit_core::verify::JudgeRun {
        let mut out = pit_core::verify::JudgeRun { model: model.clone(), ..Default::default() };
        let prompt = (|| -> Result<String> {
            let c = cards::get(r, card)?;
            let diff = pit_core::verify::lane_diff(r, &meta.branch, 200_000, None)?;
            let decisions = fs::read_to_string(cards::decisions_path(r)).unwrap_or_default();
            Ok(pit_core::verify::judge_prompt(&c, &decisions, &diff, proofs))
        })();
        let prompt = match prompt {
            Ok(p) => p,
            Err(e) => {
                out.error = Some(format!("{e:#}"));
                return out;
            }
        };
        let routing = Routing::load(&self.cfg.home).ok();
        let account = routing.as_ref().and_then(|rt| rt.account(&meta.account).cloned());
        let mut cmd = Command::new(&self.cfg.claude_bin);
        cmd.args(["-p", "--output-format", "json", "--setting-sources", "project,local", "--permission-mode", "default"])
            .args(["--permission-prompts", "none", "--allowedTools", "Read,Grep,Glob,Bash(git diff:*),Bash(git log:*),Bash(git show:*)"])
            .args(["--max-turns", "30"])
            .arg("--append-system-prompt")
            .arg(pit_core::verify::JUDGE_RULES)
            .arg("--json-schema")
            .arg(pit_core::verify::verdict_schema().to_string());
        if let Some(m) = &model {
            cmd.args(["--model", m]);
        }
        if let Some(dir) = accounts::profile_dir(&meta.account, &self.cfg.home) {
            cmd.env("CLAUDE_CONFIG_DIR", dir);
        }
        if let Some(helper) = account.and_then(|a| a.api_key_helper) {
            cmd.arg("--settings").arg(serde_json::json!({ "apiKeyHelper": helper }).to_string());
        }
        cmd.current_dir(&meta.worktree)
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let run = async {
            let mut child = cmd.spawn()?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(prompt.as_bytes()).await?;
                stdin.shutdown().await?;
            }
            let o = child.wait_with_output().await?;
            anyhow::Ok(String::from_utf8_lossy(&o.stdout).into_owned())
        };
        match tokio::time::timeout(Duration::from_secs(20 * 60), run).await {
            Ok(Ok(stdout)) => match pit_core::verify::read_judge(&stdout) {
                Ok((j, cost, dur)) => {
                    out.judgement = Some(j);
                    out.cost_usd = cost;
                    out.duration_ms = dur;
                }
                Err(e) => out.error = Some(format!("{e:#}")),
            },
            Ok(Err(e)) => out.error = Some(format!("{e:#}")),
            Err(_) => out.error = Some("the judge took longer than 20 minutes".into()),
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
        let model = r.flow.verify.judge_model.clone();
        let todo: Vec<cards::Card> = ids
            .into_iter()
            .filter_map(|id| cards::get(&r, id).ok())
            .filter(|c| c.analyzed.is_none() && c.state != CardState::Done)
            .collect();
        let gate = Arc::new(tokio::sync::Semaphore::new(3));
        let mut tasks = Vec::new();
        for card in todo {
            let (gate, me, root, choice, model) = (gate.clone(), self.clone(), r.root.clone(), choice.clone(), model.clone());
            let prompt = pit_core::analyze::prompt(&r, &card);
            tasks.push(tokio::spawn(async move {
                let _slot = gate.acquire().await;
                let out = me.analyst(&root, &prompt, choice.as_ref(), model.as_deref()).await;
                (card.id, out)
            }));
        }
        let mut results = Vec::new();
        for t in tasks {
            let Ok((id, out)) = t.await else { continue };
            results.push(match out.and_then(|(a, cost)| {
                let (q, n) = (a.questions.len(), a.notes.len());
                pit_core::analyze::record(&r, id, a)?;
                Ok((q, n, cost))
            }) {
                Ok((q, n, cost)) => serde_json::json!({ "card": id, "questions": q, "notes": n, "cost_usd": cost }),
                Err(e) => serde_json::json!({ "card": id, "error": format!("{e:#}") }),
            });
        }
        Ok(serde_json::json!({ "analyzed": results }))
    }

    async fn analyst(&self, root: &Path, prompt: &str, choice: Option<&Choice>, model: Option<&str>) -> Result<(pit_core::analyze::Analysis, Option<f64>)> {
        let mut cmd = Command::new(&self.cfg.claude_bin);
        cmd.args(["-p", "--output-format", "json", "--setting-sources", "project,local", "--permission-mode", "default"])
            .args(["--permission-prompts", "none", "--allowedTools", "Read,Grep,Glob,Bash(git log:*),Bash(git show:*),Bash(ls:*)"])
            .args(["--max-turns", "25"])
            .arg("--append-system-prompt")
            .arg(pit_core::analyze::RULES)
            .arg("--json-schema")
            .arg(pit_core::analyze::schema().to_string());
        if let Some(m) = model {
            cmd.args(["--model", m]);
        }
        if let Some(dir) = choice.and_then(|c| c.config_dir.as_ref()) {
            cmd.env("CLAUDE_CONFIG_DIR", dir);
        }
        cmd.current_dir(root)
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let run = async {
            let mut child = cmd.spawn()?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(prompt.as_bytes()).await?;
                stdin.shutdown().await?;
            }
            let o = child.wait_with_output().await?;
            anyhow::Ok(String::from_utf8_lossy(&o.stdout).into_owned())
        };
        let stdout = tokio::time::timeout(Duration::from_secs(10 * 60), run).await.map_err(|_| anyhow::anyhow!("the analysis took longer than 10 minutes"))??;
        pit_core::analyze::read(&stdout)
    }

    /// A person's decision on a card in review. Approve ships the lane one stage (into
    /// integration); changes resumes the run's session with the comments; discard returns the
    /// card to ready with its lane intact.
    pub async fn review(self: &Arc<Self>, repo: PathBuf, card: u32, action: ReviewAction, message: Option<String>) -> Result<serde_json::Value> {
        let home = self.cfg.home.clone();
        let r = Repo::open(&repo, &home_dir(), &home)?;
        let c = cards::get(&r, card)?;
        if c.state != CardState::Review {
            bail!("card #{card} is not in review (it is {})", c.state.as_str());
        }
        // The card's latest run: the highest review round, then the newest (ids can share a second).
        let last = self.list(Some(&r.root))?.into_iter().filter(|m| m.card == card).max_by_key(|m| (m.round, m.started_at));
        match action {
            ReviewAction::Discard => {
                cards::update(&r, card, CardPatch { state: Some(CardState::Ready), ..Default::default() })?;
                Ok(serde_json::json!({ "card": card, "state": "ready" }))
            }
            ReviewAction::Changes => {
                let text = message.filter(|m| !m.trim().is_empty()).context("say what to change")?;
                let prev = last.context("this card has no run to continue")?;
                let round = prev.round + 1;
                if round > r.flow.verify.max_rounds {
                    bail!("card #{card} already went through {} rounds of changes; take it from here yourself or discard it", r.flow.verify.max_rounds);
                }
                let first = format!("A person reviewed your work and asks for changes:\n\n{}\n\nMake them, commit, and end with the handoff.", text.trim());
                let next = self.start(repo, card, Some(prev.id), StartOpts { message: Some(first), round, ..Default::default() })?;
                Ok(serde_json::json!({ "run": next }))
            }
            ReviewAction::Approve => {
                // Approved work joins the merge queue; the card moves when its lane lands.
                let branch = c.lane.clone().context("this card has no lane to ship")?;
                let job = self.enqueue(&repo, &branch, None)?;
                Ok(serde_json::json!({ "card": card, "job": job, "shipped_to": job.target }))
            }
        }
    }

    // ------------------------------------------------------------ merge queue

    /// Puts a lane in line to ship into `to` (default: its next stage). One open job per lane.
    pub fn enqueue(&self, repo: &Path, branch: &str, to: Option<&str>) -> Result<pit_core::queue::ShipJob> {
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
        let card = cards::list(&r).ok().and_then(|(all, _)| all.into_iter().find(|c| c.lane.as_deref() == Some(branch)).map(|c| c.id));
        let job = {
            let mut q = self.queue.lock().unwrap();
            if let Some(open) = q.iter().find(|j| j.repo == r.root && j.branch == branch && j.state.is_open()) {
                bail!("{branch} is already in line for {} ({})", open.target, open.id);
            }
            let job = ShipJob {
                id: format!("q{}-{}", now_secs(), &uuid::Uuid::new_v4().simple().to_string()[..4]),
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
        let mine: Vec<_> = q.iter().filter(|j| repo.is_none_or(|r| j.repo == r)).cloned().collect();
        let mut open: Vec<_> = mine.iter().filter(|j| j.state.is_open()).cloned().collect();
        open.sort_by_key(|j| j.queued_at);
        let mut done: Vec<_> = mine.into_iter().filter(|j| !j.state.is_open()).collect();
        done.sort_by_key(|j| std::cmp::Reverse(j.finished_at.unwrap_or(j.queued_at)));
        open.extend(done.into_iter().take(20));
        open
    }

    /// Takes a job out of line (not one that is shipping right now).
    pub fn dequeue(&self, id: &str) -> Result<()> {
        use pit_core::queue::JobState;
        let mut q = self.queue.lock().unwrap();
        let j = q.iter_mut().find(|j| j.id == id).with_context(|| format!("no job {id}"))?;
        if j.state == JobState::Shipping {
            bail!("{} is shipping right now", j.branch);
        }
        j.state = JobState::Removed;
        j.finished_at = Some(now_secs());
        pit_core::queue::save(&self.cfg.home, &q)
    }

    pub fn start_queue(self: &Arc<Self>) {
        let me = Arc::clone(self);
        tokio::spawn(async move { me.run_queue().await });
    }

    /// Ships queued jobs one at a time, oldest first, forever.
    async fn run_queue(self: Arc<Self>) {
        use pit_core::queue::{JobState, Outcome};
        loop {
            let next = {
                let mut q = self.queue.lock().unwrap();
                let pick = q.iter_mut().filter(|j| j.state == JobState::Queued).min_by_key(|j| j.queued_at);
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
            let result = tokio::task::spawn_blocking(move || -> Result<(pit_core::queue::Outcome, Option<u32>)> {
                let out = pit_core::queue::ship(&repo, &home, &ph, &branch, &target)?;
                let card = match &out {
                    Outcome::Shipped { .. } => pit_core::queue::move_card(&repo, &home, &ph, &branch, &target)?,
                    Outcome::Conflict { .. } => None,
                };
                Ok((out, card))
            })
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r);
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
        let next = self.start(prev.repo.clone(), prev.card, None, StartOpts { allow_api_key, handoff_from: Some(prev.id.clone()), round: prev.round, ..Default::default() })?;
        self.update_disk(id, |m| m.next = None);
        Ok(next)
    }

    /// A message to the agent while it works: it reads it after its current step.
    pub async fn say(&self, id: &str, text: &str) -> Result<()> {
        let mut stdin = {
            let mut live = self.live.lock().unwrap();
            let l = live.get_mut(id).with_context(|| format!("run {id} is not running; resume it to continue"))?;
            let s = l.stdin.take().context("the agent already finished this session; resume it to continue")?;
            l.sent += 1;
            s
        };
        let res = async {
            stdin.write_all(format!("{}\n", runs::claude_user_line(text)).as_bytes()).await?;
            stdin.flush().await
        }
        .await;
        if let Some(l) = self.live.lock().unwrap().get_mut(id) {
            l.stdin = Some(stdin);
        }
        res?;
        self.emit(id, RunEvent::Said { text: text.to_owned() });
        Ok(())
    }

    /// SIGINT ends the current turn cleanly; if the agent is still there after 10 s, SIGTERM.
    pub fn stop(self: &Arc<Self>, id: &str) -> Result<()> {
        let pid = {
            let mut live = self.live.lock().unwrap();
            let l = live.get_mut(id).with_context(|| format!("run {id} is not running"))?;
            l.stop_requested = true;
            l.meta.pid
        };
        let Some(pid) = pid else {
            bail!("run {id} is still preparing its worktree; stop it once the agent starts");
        };
        signal(pid, libc::SIGINT);
        let me = Arc::clone(self);
        let run = id.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            if me.live.lock().unwrap().contains_key(&run) {
                signal(pid, libc::SIGTERM);
            }
        });
        Ok(())
    }
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
}

/// The card's lane and worktree, made when missing, and what the agent is launched with.
/// Commits whatever is uncommitted in the worktree, so the next agent starts from it.
fn checkpoint(worktree: &Path, card: u32) -> Result<Option<String>> {
    let g = pit_core::git::Git::new(worktree);
    if g.lines(&["status", "--porcelain"])?.is_empty() {
        return Ok(None);
    }
    g.run(&["add", "-A"])?;
    g.run(&["commit", "-q", "-m", &format!("pitlane: checkpoint of card #{card} at a usage limit")])?;
    Ok(Some(g.run(&["rev-parse", "--short", "HEAD"])?))
}

/// The agent's last words in a run: its handoff summary if any, else its last two messages.
fn last_words(timeline: &[(usize, RunEvent)]) -> String {
    if let Some(s) = timeline.iter().rev().find_map(|(_, e)| match e {
        RunEvent::Finished { handoff: Some(h), .. } => h.get("summary").and_then(|s| s.as_str()).map(str::to_owned),
        _ => None,
    }) {
        return s;
    }
    let texts: Vec<&str> = timeline
        .iter()
        .filter_map(|(_, e)| match e {
            RunEvent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    texts.iter().rev().take(2).rev().cloned().collect::<Vec<_>>().join("\n\n")
}

fn prepare(repo: &Path, card: u32, home: &Path, continues: Option<(&RunMeta, &str)>, brief: bool, note: impl Fn(String)) -> Result<Prepared> {
    let r = Repo::open(repo, &home_dir(), home)?;
    let c = cards::get(&r, card)?;
    let branch = match &c.lane {
        Some(b) => b.clone(),
        None => {
            let branch = format!("{}{}-{}", r.flow.run.branch_prefix, c.id, cards::slug(&c.title));
            note(format!("Creating lane {branch} from {}/{}", r.flow.flow.remote, r.flow.flow.base));
            actions::new_lane(&r, &branch, NewOptions { fetch: true, setup: true })?;
            cards::update(&r, card, CardPatch { lane: Some(Some(branch.clone())), ..Default::default() })?;
            branch
        }
    };
    let worktree = if r.git.rev_parse(&format!("refs/heads/{branch}"))?.is_some() {
        actions::ensure_worktree(&r, &branch)?
    } else {
        note(format!("Checking out {branch} from {}", r.flow.flow.remote));
        actions::new_lane(&r, &branch, NewOptions { fetch: true, setup: true })?.path
    };
    note(format!("Working in {}", worktree.display()));
    let mut tools: Vec<String> = RUN_BASE_TOOLS.iter().map(|s| s.to_string()).collect();
    tools.push("mcp__pitlane_run__note".into());
    tools.extend(r.flow.run.allow.iter().cloned());
    let the_card = cards::get(&r, card)?;
    // Where to look, from the index. A failure here never stops the run; it runs without the map.
    let map = if brief && r.flow.run.brief {
        match pit_core::brief::compile(&r, home, &the_card, pit_core::brief::DEFAULT_BUDGET) {
            Ok(b) if !b.text.is_empty() => {
                note(format!("Briefed with {} from the index (~{} tokens)", plural(b.stats.files, "file"), b.stats.chars / 4));
                Some(b)
            }
            Ok(_) => {
                note("Nothing in the index matches this card; running without a map".into());
                None
            }
            Err(e) => {
                note(format!("Could not compile the map ({e:#}); running without it"));
                None
            }
        }
    } else {
        None
    };
    Ok(Prepared {
        branch,
        worktree: worktree.clone(),
        brief: map.as_ref().map(|b| b.stats.clone()),
        briefing: {
            let mut b = runs::briefing(&r, &the_card);
            if let Some(m) = &map {
                let end = "\nWhen you finish, end with the handoff.\n";
                let at = b.rfind(end).unwrap_or(b.len());
                b.insert_str(at, &format!("\n{}", m.text));
            }
            if let Some((prev, words)) = continues {
                let g = pit_core::git::Git::new(&worktree);
                // Production as the yard resolves it: the remote branch, or the local one without a remote.
                let base = pit_core::lanes::Target::resolve(&r, &r.flow.flow.production).ok().flatten().map(|t| t.sha);
                let log = match base {
                    Some(b) => g.run(&["log", "--oneline", "-20", &format!("{b}..HEAD")]).unwrap_or_default(),
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
    cards::update(&r, m.card, CardPatch { state: Some(state), ..Default::default() })?;
    Ok(())
}

/// Where the card goes when its run ends.
fn settle_card(m: &RunMeta, home: &Path) -> Result<()> {
    let r = Repo::open(&m.repo, &home_dir(), home)?;
    let status = m.handoff.as_ref().and_then(|h| h.get("status")).and_then(Value::as_str);
    // The handoff stays on the card as a note: the next session starts from it.
    if let Some(h) = &m.handoff
        && let Some(summary) = h.get("summary").and_then(Value::as_str).filter(|s| !s.trim().is_empty())
    {
        let mut text = format!("Run {} ended {}: {}", m.id, status.unwrap_or("done"), summary.trim());
        let next: Vec<&str> = h.get("next_steps").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        if !next.is_empty() {
            text.push_str("\n\nLeft to do:\n");
            for n in next {
                text.push_str(&format!("- {n}\n"));
            }
        }
        let _ = cards::note(&r, m.card, "claude", &text);
    }
    if let Some(qs) = m.handoff.as_ref().and_then(|h| h.get("questions")).and_then(Value::as_array) {
        let qs: Vec<NewQuestion> = qs.iter().filter_map(|q| serde_json::from_value(q.clone()).ok()).collect();
        if !qs.is_empty() {
            cards::ask(&r, m.card, qs)?;
        }
    }
    let next = match (m.state, status) {
        (RunState::Done, Some("blocked")) => CardState::Blocked,
        (RunState::Done, _) => CardState::Review,
        (RunState::Stopped, _) => CardState::Ready,
        _ => CardState::Blocked,
    };
    cards::update(&r, m.card, CardPatch { state: Some(next), ..Default::default() })?;
    Ok(())
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}
