//! A card's run: starting it, driving its Claude Code session, and reading what the agent says.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use pit_core::Repo;
use pit_core::accounts::{self, Choice};
use pit_core::cards::{self, CardPatch, CardState};
use pit_core::lanes::now_secs;
use pit_core::runs::{self, RunEvent, RunMeta, RunState, SessionKind};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::card::{add_to_briefing, last_words, prepare, resume_message, set_card_state, settle_card, block_card};
use super::control::{Release, end_group, signal};
use super::launch::{claude_dir, session_saved};
use super::{StartOpts, Supervisor, home_dir};

/// How long what an agent printed is still read once it is gone (a child it left can hold the
/// pipe open).
pub(super) const AFTER_EXIT_READ: Duration = Duration::from_secs(3);
/// How long the end of an agent's stderr is waited for once it is gone.
const STDERR_READ: Duration = Duration::from_secs(1);

impl Supervisor {
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
        let mut opts = opts;
        let mut resume = resume;
        // A card stopped mid-work picks its session up where it was left: the agent keeps all it
        // did and said. Unless the person starts over, or the session is gone.
        let paused = c
            .paused
            .as_deref()
            .filter(|_| resume.is_none() && opts.handoff_from.is_none() && !opts.fresh)
            .and_then(|p| self.get(p).ok())
            .filter(|p| self.resumable(p, &r.root, card));
        if let Some(p) = &paused {
            resume = Some(p.id.clone());
            if opts.message.is_none() {
                opts.message = Some(resume_message(&c, p));
            }
        }
        let mut prev = resume.as_deref().map(|p| self.get(p)).transpose()?;
        let (choice, reason, exhausted) = match self.choose(&r, prev.as_ref(), &opts) {
            // Its account is gone: a new session, which starts from the card.
            Err(_) if paused.is_some() => {
                (prev, resume, opts.message) = (None, None, None);
                self.choose(&r, None, &opts)?
            }
            x => x?,
        };
        if c.paused.is_some() {
            let _ = cards::update(
                &r,
                card,
                CardPatch {
                    paused: Some(None),
                    ..Default::default()
                },
            );
        }
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
            asking: Vec::new(),
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
            goes_on: false,
            related: Vec::new(),
        };
        self.register(meta.clone())?;
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

    /// A stopped run of this card whose Claude Code session can be picked up: its worktree is
    /// there and Claude Code saved the session in its account's directory.
    fn resumable(&self, p: &RunMeta, repo: &Path, card: u32) -> bool {
        p.kind == SessionKind::Run
            && p.repo == repo
            && p.card == card
            && matches!(p.state, RunState::Stopped | RunState::Interrupted)
            && !self.live.lock().unwrap().contains_key(&p.id)
            && p.worktree.is_dir()
            && session_saved(
                &accounts::profile_dir(&p.account, &self.cfg.home).unwrap_or_else(claude_dir),
                &p.session_id,
            )
    }

    fn fail(&self, id: &str, why: &str) {
        // The card first, as when a run ends: whoever sees the run failed finds the card blocked.
        // Written from a copy, without holding the list.
        if let Ok(m) = self.get(id) {
            let _ = block_card(&m, pit_core::cards::BlockReason::Failed, why, &self.cfg.home);
        }
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
        self.live.lock().unwrap().remove(id);
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
            add_to_briefing(&mut prep.briefing, &format!("\n## What the person asked for this whole batch\n\nFollow what applies while you work this card (what is for the very end is done later, not by you):\n\n{i}\n"));
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

        if self.stopped_before_start(&id).await? {
            return Ok(());
        }
        self.wait_room(&id).await;
        if self.stopped_before_start(&id).await? {
            return Ok(());
        }
        let mut cmd = self.run_command(&id, &repo, resuming, &prep, &choice)?;
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
            format!(
                "{}{}",
                prep.briefing,
                pit_core::language::reminder(&pit_core::language::agent_language(&self.cfg.home))
            )
        };
        let line = if resuming {
            runs::claude_user_line(&first)
        } else {
            runs::claude_user_line_with(&first, &prep.images)
        };
        stdin.write_all(format!("{line}\n").as_bytes()).await?;
        stdin.flush().await?;
        let stopped_meanwhile = {
            let mut live = self.live.lock().unwrap();
            live.get_mut(&id).is_some_and(|l| {
                l.stdin = Some(stdin);
                l.turn = true;
                l.meta.pid = pid;
                l.meta.state = RunState::Running;
                l.stop_requested
            })
        };
        self.update(&id, |_| {});
        // A stop that came while the agent was starting found no process to signal: send it now.
        if stopped_meanwhile {
            let _ = self.stop(&id);
        }
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
                    me.keep_shots(&id, &v);
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
        if tokio::time::timeout(AFTER_EXIT_READ, &mut reader)
            .await
            .is_err()
        {
            reader.abort();
        }
        let tail = match tokio::time::timeout(STDERR_READ, err_task).await {
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
        // Out of turns under Run all goes on in the same session: said on the run before it ends,
        // so nobody is told it stopped.
        let goes_on = Repo::open(&settled.repo, &home_dir(), &home)
            .is_ok_and(|r| super::autopilot::goes_on(&settled, r.flow.run.max_turns));
        let card_result = tokio::task::spawn_blocking(move || settle_card(&settled, &home, true)).await?;
        // Said while the run is still live: once it leaves the list, nothing more is recorded.
        match &card_result {
            Err(e) => self.emit(&id, RunEvent::Note { text: format!("The card could not be moved: {e:#}") }),
            Ok(()) => {
                if let Ok(m) = self.get(&id) {
                    self.after_settle(&m.repo, m.card);
                }
            }
        }
        self.update(&id, |m| {
            m.state = state;
            m.ended_at = Some(now_secs());
            m.pid = None;
            m.goes_on = goes_on;
            m.waiting.clear();
            m.asking.clear();
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

    /// Keeps the run record in step with what the agent reports.
    pub(super) async fn absorb(&self, id: &str, ev: &RunEvent, finished_ok: &mut Option<bool>) {
        // The agent is at work: a turn somebody asked for, or one it started by itself (a
        // background task reported back after the last result). Either ends with a result.
        if matches!(
            ev,
            RunEvent::Started { .. }
                | RunEvent::Text { .. }
                | RunEvent::Tool { .. }
                | RunEvent::ToolDone { .. }
        ) && let Some(l) = self.live.lock().unwrap().get_mut(id)
        {
            l.turn = true;
        }
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
                let handed_off = handoff.is_some();
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
                let said = match self.live.lock().unwrap().get_mut(id) {
                    Some(l) => {
                        l.results += 1;
                        l.turn = false;
                        std::mem::take(&mut l.said)
                    }
                    None => false,
                };
                self.close_when_idle(id, handed_off, said);
            }
            _ => {}
        }
    }

    /// Tracks the background tasks Claude Code reports, and lets the session end once the last
    /// one is done and nothing else is pending.
    pub(super) fn note_background(&self, id: &str, v: &Value) {
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
            self.close_when_idle(id, false, false);
        }
    }

    /// Ends the session once its work is over: closes stdin, which is how a stream-json session
    /// ends, when no turn is in flight and none starts for a moment. Claude Code can start a turn
    /// by itself right after a result (a task it waited on reported back, or a message that came
    /// too late to be folded into the turn); closing at once would leave that turn unable to hear
    /// the person.
    ///
    /// A background task the agent left running keeps the session open until it reports back,
    /// for at most a quarter of an hour. A card's handoff is the end of its work, though: what
    /// still runs then (a push finishing, a server nobody stopped) gets two minutes, not a
    /// quarter of an hour of a card that looks at work.
    ///
    /// Closing stdin is a request. An agent still there half a minute later, with no turn in
    /// flight, is ended: a session whose work is in never stays behind as a run that looks alive.
    fn close_when_idle(&self, id: &str, handed_off: bool, said: bool) {
        const GRACE: Duration = Duration::from_secs(2);
        const SAID_GRACE: Duration = Duration::from_secs(6);
        const BACKGROUND_WAIT: Duration = Duration::from_secs(15 * 60);
        let leave = Duration::from_millis(self.leave_ms.load(Ordering::Relaxed));
        let left_running = Duration::from_millis(self.left_running_ms.load(Ordering::Relaxed));
        let (live, id) = (Arc::clone(&self.live), id.to_owned());
        tokio::spawn(async move {
            let since = tokio::time::Instant::now();
            if said {
                tokio::time::sleep(SAID_GRACE - GRACE).await;
            }
            loop {
                tokio::time::sleep(GRACE).await;
                let (stdin, pid) = {
                    let mut live = live.lock().unwrap();
                    let Some(l) = live.get_mut(&id) else { return };
                    // A session the person opened (the Board's assistant) is theirs to end:
                    // it waits for the next message until they stop it.
                    if l.meta.kind == SessionKind::Assist {
                        return;
                    }
                    if l.results == 0 || l.turn {
                        return; // working again: its next result checks once more
                    }
                    let wait = if handed_off && l.meta.kind == SessionKind::Run {
                        left_running
                    } else {
                        BACKGROUND_WAIT
                    };
                    if l.background > 0 && since.elapsed() < wait {
                        continue;
                    }
                    (l.stdin.take(), l.meta.pid)
                };
                if let Some(mut s) = stdin {
                    let _ = s.shutdown().await;
                }
                // Paused by Performance, it would not hear the end.
                if let Some(pid) = pid {
                    signal(pid, libc::SIGCONT);
                }
                tokio::time::sleep(leave).await;
                let lingering = live
                    .lock()
                    .unwrap()
                    .get(&id)
                    .filter(|l| !l.turn)
                    .and_then(|l| l.meta.pid);
                if let Some(pid) = lingering {
                    end_group(pid);
                }
                return;
            }
        });
    }

    /// A run whose stop came before its agent started: it ends Stopped, and its card is settled
    /// as if it never ran. Asked before and after waiting for room, so a stop while waiting
    /// never starts the agent.
    async fn stopped_before_start(&self, id: &str) -> Result<bool> {
        if !self.stop_requested(id) {
            return Ok(false);
        }
        self.emit(
            id,
            RunEvent::Note {
                text: "Stopped before the agent started".into(),
            },
        );
        let mut settled = self.get(id)?;
        settled.state = RunState::Stopped;
        let home = self.cfg.home.clone();
        let _ = tokio::task::spawn_blocking(move || settle_card(&settled, &home, false)).await;
        self.update(id, |m| {
            m.state = RunState::Stopped;
            m.ended_at = Some(now_secs());
        });
        self.emit(
            id,
            RunEvent::Exited {
                code: None,
                state: RunState::Stopped,
            },
        );
        self.live.lock().unwrap().remove(id);
        Ok(true)
    }
}
