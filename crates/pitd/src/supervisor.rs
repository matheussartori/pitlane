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
    pending: HashMap<String, oneshot::Sender<Answer>>,
}

pub struct Supervisor {
    pub cfg: Config,
    live: Mutex<HashMap<String, Live>>,
    quota: Mutex<Vec<RunEvent>>,
}

pub struct Subscription {
    pub replay: Vec<(usize, RunEvent)>,
    /// `None` when the run is not live: the replay is all there is.
    pub rx: Option<broadcast::Receiver<(usize, RunEvent)>>,
}

impl Supervisor {
    pub fn new(cfg: Config) -> Arc<Self> {
        Arc::new(Self { cfg, live: Mutex::new(HashMap::new()), quota: Mutex::new(Vec::new()) })
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

    pub fn quota(&self) -> Vec<RunEvent> {
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

    fn read_timeline(&self, id: &str, from: usize) -> Vec<(usize, RunEvent)> {
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

    /// Starts a run of `card` in `repo`. Returns at once; preparing the worktree and the agent's
    /// work show up as events.
    pub fn start(self: &Arc<Self>, repo: PathBuf, card: u32, resume: Option<String>) -> Result<RunMeta> {
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
        let (session_id, resumed_from) = match &resume {
            Some(prev) => (self.get(prev)?.session_id, Some(prev.clone())),
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
            model: r.flow.run.model.clone(),
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
                pending: HashMap::new(),
            },
        );
        let me = Arc::clone(self);
        let run = id.clone();
        tokio::spawn(async move {
            if let Err(e) = me.clone().drive(run.clone(), repo, card, resume.is_some()).await {
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

    async fn drive(self: Arc<Self>, id: String, repo: PathBuf, card: u32, resuming: bool) -> Result<()> {
        // Worktree and branch: blocking git work, off the async threads.
        let home = self.cfg.home.clone();
        let me = Arc::clone(&self);
        let rid = id.clone();
        let prep = tokio::task::spawn_blocking(move || prepare(&repo, card, &home, |t| me.emit(&rid, RunEvent::Note { text: t })))
            .await??;
        self.update(&id, |m| {
            m.branch = prep.branch.clone();
            m.worktree = prep.worktree.clone();
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
        if let Some(m) = &prep.model {
            cmd.args(["--model", m]);
        }
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
            "Continue where you stopped. End with the handoff.".to_owned()
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
                let mut q = self.quota.lock().unwrap();
                q.retain(|e| !matches!(e, RunEvent::Quota { window: w, .. } if w == window));
                q.push(ev.clone());
            }
            RunEvent::Finished { ok, cost_usd, duration_ms, turns, tokens, handoff, denied } => {
                *finished_ok = Some(*ok);
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
    tools: Vec<String>,
    max_turns: u32,
    model: Option<String>,
}

/// The card's lane and worktree, made when missing, and what the agent is launched with.
fn prepare(repo: &Path, card: u32, home: &Path, note: impl Fn(String)) -> Result<Prepared> {
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
    tools.extend(r.flow.run.allow.iter().cloned());
    Ok(Prepared {
        branch,
        worktree,
        briefing: runs::briefing(&r, &cards::get(&r, card)?),
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
